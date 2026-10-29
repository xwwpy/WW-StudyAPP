"""Verify system distribution policy for the actual macOS release artifact."""
import argparse
import hashlib
import json
import os
import plistlib
from pathlib import Path
import subprocess
import sys
import time
import uuid


def capture(argv, timeout=120):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        def text(value):
            return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else (value or "")
        return {"command": argv, "exit_code": 124, "timed_out": True,
                "stdout": text(error.stdout), "stderr": text(error.stderr)}
    return {"command": argv, "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr}


def launch_accepted(enabled, intact, request, receipt):
    return (enabled and intact and isinstance(receipt, dict)
            and isinstance(receipt.get("pid"), int) and receipt["pid"] > 1
            and all(receipt.get(field) == request[field] for field in ("nonce", "version", "sha256")))


def system_approval(output):
    """Use only visible per-app system controls on the disposable runner."""
    actions = []
    def screenshot(stage):
        actions.append(capture(["/usr/sbin/screencapture", "-x", str(output.with_name(output.stem + "-" + stage + ".png"))], 10))
    def click(process, buttons):
        # Require this application's name in the actual window before clicking any button.
        script = '''tell application "System Events"
set report to ""
if not (exists process "PROCESS") then return "process absent"
tell process "PROCESS"
repeat with win in windows
set items to entire contents of win
set matchesApp to false
repeat with itemRef in items
try
set labelText to name of itemRef as text
set report to report & labelText & linefeed
if labelText contains "WW-StudyApp" then set matchesApp to true
end try
end repeat
if matchesApp then
repeat with wanted in {BUTTONS}
repeat with itemRef in items
try
if role of itemRef is "AXButton" and name of itemRef is (wanted as text) then
click itemRef
return "clicked " & wanted & linefeed & report
end if
end try
end repeat
end repeat
end if
end repeat
end tell
return report
end tell'''.replace("PROCESS", process).replace("BUTTONS", ", ".join(json.dumps(v) for v in buttons))
        actions.append(capture(["/usr/bin/osascript", "-e", script], 15))
    screenshot("blocked")
    click("CoreServicesUIAgent", ["Open", "Done", "Cancel", "OK"])
    actions.append(capture(["/usr/bin/open", "x-apple.systempreferences:com.apple.preference.security?Privacy"], 15))
    time.sleep(3)
    screenshot("privacy-security")
    click("System Settings", ["Open Anyway"])
    time.sleep(2)
    screenshot("allow-confirmation")
    click("CoreServicesUIAgent", ["Open"])
    click("System Settings", ["Open"])
    screenshot("allow-result")
    return actions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Distribution assessment runs only on GitHub macOS runners")
    bundle = args.bundle.resolve(strict=True)
    package = args.package.resolve(strict=True)
    quarantine = f"0083;{int(time.time()):x};WW-StudyApp;{uuid.uuid4()}"
    provenance = capture(["/usr/bin/xattr", "-w", "com.apple.quarantine", quarantine, str(bundle)])
    policy = capture(["/usr/sbin/spctl", "--status"])
    integrity = capture(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(bundle)])
    assessment = capture(["/usr/sbin/spctl", "--assess", "--type", "execute", "--verbose=4", str(bundle)])
    enabled = "assessments enabled" in policy["stdout"] + policy["stderr"]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    probe = args.output.parent.resolve() / ("policy-probe-" + str(uuid.uuid4()))
    probe.mkdir(mode=0o700)
    candidate = probe / "WW-StudyApp.app"
    copied = capture(["/usr/bin/ditto", str(bundle), str(candidate)])
    if copied["exit_code"] != 0:
        raise RuntimeError("Cannot prepare policy probe copy")
    candidate_integrity = capture(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(candidate)])
    candidate_quarantine = capture(["/usr/bin/xattr", "-p", "com.apple.quarantine", str(candidate)])
    info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
    request = {"nonce": str(uuid.uuid4()), "version": info["CFBundleShortVersionString"],
               "sha256": hashlib.sha256((bundle / "Contents/MacOS/studyapp").read_bytes()).hexdigest(),
               "health": None, "created": int(time.time())}
    (probe / "request.json").write_text(json.dumps(request), encoding="utf-8")
    pointer = Path.home() / "Library/Application Support/io.StudyApp.StudyApp/update-policy-probe.txt"
    pointer.parent.mkdir(parents=True, exist_ok=True)
    pointer.write_text(str(probe), encoding="utf-8")
    command = ["/usr/bin/open", "-n", "-g", "-a", str(candidate), "--args", "--update-policy-probe", str(probe)]
    opener = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(3)
    approval_actions = []
    if not (probe / "receipt.json").exists():
        approval_actions = system_approval(args.output)
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and not (probe / "receipt.json").exists():
        time.sleep(0.1)
    receipt = json.loads((probe / "receipt.json").read_text()) if (probe / "receipt.json").exists() else None
    try:
        stdout, stderr = opener.communicate(timeout=3)
        launch = {"command": command, "exit_code": opener.returncode, "stdout": stdout, "stderr": stderr}
    except subprocess.TimeoutExpired:
        opener.kill()
        stdout, stderr = opener.communicate(timeout=3)
        launch = {"command": command, "exit_code": 124, "timed_out": True, "stdout": stdout, "stderr": stderr}
    policy_after = capture(["/usr/sbin/spctl", "--status"])
    enabled_after = "assessments enabled" in policy_after["stdout"] + policy_after["stderr"]
    accepted = launch_accepted(enabled and enabled_after, all(item["exit_code"] == 0 for item in (provenance, integrity, candidate_integrity, candidate_quarantine)), request, receipt)
    screenshot = capture(["/usr/sbin/screencapture", "-x", str(args.output.with_suffix(".png"))])
    report = {"schema": 1, "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "package": package.name, "package_sha256": hashlib.sha256(package.read_bytes()).hexdigest(),
              "system_policy_enabled": enabled, "distribution_accepted": accepted,
              "quarantine": provenance, "system_policy": policy, "integrity": integrity, "assessment": assessment,
              "launch_services": launch, "launch_request": request, "launch_receipt": receipt,
              "approval_actions": approval_actions,
              "candidate_integrity": candidate_integrity, "candidate_quarantine": candidate_quarantine,
              "system_policy_after": policy_after,
              "screenshot": screenshot, "normal_user_approval_required": not accepted}
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    if accepted:
        (probe / "approved").write_text("approved", encoding="utf-8")
    if not accepted:
        raise RuntimeError("macOS did not launch the headless candidate under enabled system policy. Inspect assessment and LaunchServices evidence; normal user approval remains required.")


if __name__ == "__main__":
    main()
