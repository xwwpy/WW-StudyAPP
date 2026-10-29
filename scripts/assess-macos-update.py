"""Verify system distribution policy for the actual macOS release artifact."""
import argparse
import contextlib
import hashlib
import json
import os
import plistlib
import re
from pathlib import Path
import subprocess
import sys
import time
import uuid
from macos_runner_auth import runner_credential, unique_authentication, authentication_diagnostics


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


def select_approval(tree, label, *, for_scroll=False):
    def texts(node):
        result = []
        if node.get("role") == "AXStaticText":
            value = node.get("name") or node.get("value")
            if isinstance(value, str) and value.strip():
                result.append(value.strip())
        for child in node.get("children", []):
            result.extend(texts(child))
        return result
    def own_warning(values):
        if not values or len(values) > 8:
            return False
        quoted = [name for value in values for name in re.findall(r'[“"]([^”"]+)[”"]', value)]
        if any(name not in ("WW-StudyApp", "WW-StudyApp.app") for name in quoted):
            return False
        return any(value in ("WW-StudyApp", "WW-StudyApp.app") for value in values) or bool(quoted)
    candidates = []
    def visit(node, path, ancestors):
        if node.get("role") == "AXButton" and node.get("name") == label and node.get("enabled") is True and (node.get("visible") is True or for_scroll):
            for parent, parent_path in reversed(ancestors):
                values = texts(parent)
                if not values:
                    continue
                related = parent.get("role") in ("AXGroup", "AXSheet", "AXDialog") or parent.get("subrole") in ("AXDialog", "AXSystemDialog")
                if related and own_warning(values):
                    candidates.append({"button_path": path, "container_path": parent_path, "container": parent,
                                       "label": label})
                # Never climb out of one warning into a broader window containing another.
                break
        for index, child in enumerate(node.get("children", [])):
            visit(child, [*path, index], [*ancestors, (node, path)])
    visit(tree, [], [])
    return candidates[0] if len(candidates) == 1 else None


@contextlib.contextmanager
def verified_runner_credential(actions):
    with runner_credential() as fixture:
        credential, username = fixture
        actions.append({"command": ["runner-credential-preflight"], "exit_code": 0, "stdout": "authonly and administrator membership verified before candidate launch", "stderr": ""})
        script = Path(__file__).with_name("macos-credential-check.js")
        checked = capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), username, str(credential)], 10)
        verification = json.loads(checked["stdout"]) if checked["exit_code"] == 0 else {"verified": False, "bridgePhase": "process"}
        actions.append({"command": ["directory-record-verification"], "exit_code": 0 if verification.get("verified") is True else 1,
                        "stdout": json.dumps(verification), "stderr": ""})
        if verification.get("verified") is not True:
            raise RuntimeError("Direct directory record verification did not succeed")
        yield fixture


def system_approval(output, fixture=None):
    actions = []
    try:
        if fixture is not None:
            return system_approval_with_credential(output, *fixture, actions)
        with verified_runner_credential(actions) as prepared:
            return system_approval_with_credential(output, *prepared, actions)
    except RuntimeError as error:
        actions.append({"command": ["runner-authentication"], "exit_code": 1, "stdout": "", "stderr": str(error)})
        return actions


def system_approval_with_credential(output, credential, username, actions):
    """Use only visible per-app system controls on the disposable runner."""
    script = Path(__file__).with_name("macos-approval-ui.js")
    actions.append(capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "permissions"], 10))
    def screenshot(stage):
        actions.append(capture(["/usr/sbin/screencapture", "-x", str(output.with_name(output.stem + "-" + stage + ".png"))], 10))
    def click(process, buttons, can_scroll=True):
        script = Path(__file__).with_name("macos-approval-ui.js")
        snapshot = capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "snapshot", process], 20)
        actions.append(snapshot)
        if snapshot["exit_code"] != 0:
            return
        try:
            tree = json.loads(snapshot["stdout"])
        except ValueError:
            return
        for label in buttons:
            selection = select_approval(tree, label)
            if selection is not None:
                pressed = capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "click", process,
                                   json.dumps(selection, separators=(",", ":"))], 20)
                actions.append(pressed)
                return pressed["exit_code"] == 0 and json.loads(pressed["stdout"]).get("clicked") == label
        if can_scroll and process == "System Settings":
            for label in buttons:
                selection = select_approval(tree, label, for_scroll=True)
                if selection is not None:
                    actions.append(capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "scroll", process,
                                            json.dumps(selection, separators=(",", ":"))], 20))
                    time.sleep(1)
                    screenshot("scrolled-warning")
                    return click(process, buttons, can_scroll=False)
    screenshot("blocked")
    click("CoreServicesUIAgent", ["Open", "Done", "Cancel", "OK"])
    actions.append(capture(["/usr/bin/open", "x-apple.systempreferences:com.apple.preference.security?Privacy"], 15))
    time.sleep(3)
    screenshot("privacy-security")
    click("System Settings", ["Open Anyway"])
    time.sleep(2)
    screenshot("allow-confirmation")
    confirmed = click("CoreServicesUIAgent", ["Open Anyway", "Open"])
    if confirmed:
        time.sleep(1)
        inventory = capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "inventory"], 10)
        actions.append(inventory)
        snapshots = []
        if inventory["exit_code"] == 0:
            for owner in json.loads(inventory["stdout"]):
                if not str(owner.get("executable") or "").startswith("/System/"):
                    continue
                process = "pid:" + str(owner["pid"])
                authentication = capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "snapshot", process], 20)
                actions.append(authentication)
                snapshots.append((process, authentication))
            candidate = unique_authentication(snapshots)
            if candidate is not None:
                process, selection = candidate
                try:
                    selection["test_username"] = username
                    submitted_at = time.time()
                    submitted = capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "authenticate", process,
                                         json.dumps(selection, separators=(",", ":")), str(credential)], 20)
                    failure_stage = re.search(r"Authentication failed at (permission|identity|credential-file|username-focus|username-keypost|password-focus|password-keypost|username-delivery|button-owner|button-default|button-event|pointer-observation)\b", submitted["stderr"])
                    outcome = json.loads(submitted["stdout"]) if submitted["exit_code"] == 0 else None
                    actions.append({"command": ["runner-authentication"], "exit_code": 0 if outcome and outcome.get("authenticationSubmitted") is True else 1,
                                    "stdout": json.dumps(outcome) if outcome is not None else "",
                                    "stderr": failure_stage.group(1) if failure_stage else ""})
                    time.sleep(2)
                    actions.append({"command": ["authentication-status-codes"], "exit_code": 0,
                                    "stdout": json.dumps(authentication_diagnostics(submitted_at, credential, username)), "stderr": ""})
                except RuntimeError as error:
                    actions.append({"command": ["runner-authentication"], "exit_code": 1, "stdout": "", "stderr": str(error)})
                time.sleep(2)
    click("System Settings", ["Open"])
    screenshot("allow-result")
    actions.append(capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "snapshot", "universalAccessAuthWarn"], 10))
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
    approval_actions = []
    with verified_runner_credential(approval_actions) as fixture:
        opener = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        time.sleep(3)
        if not (probe / "receipt.json").exists():
            approval_actions.extend(system_approval(args.output, fixture))
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
