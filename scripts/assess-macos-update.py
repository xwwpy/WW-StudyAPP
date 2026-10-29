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


def capture(argv):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=120)
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
    info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
    request = {"nonce": str(uuid.uuid4()), "version": info["CFBundleShortVersionString"],
               "sha256": hashlib.sha256((bundle / "Contents/MacOS/studyapp").read_bytes()).hexdigest(),
               "health": None, "created": int(time.time())}
    (probe / "request.json").write_text(json.dumps(request), encoding="utf-8")
    launch = capture(["/usr/bin/open", "-n", "-g", "-a", str(bundle), "--args", "--update-policy-probe", str(probe)])
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and not (probe / "receipt.json").exists():
        time.sleep(0.1)
    receipt = json.loads((probe / "receipt.json").read_text()) if (probe / "receipt.json").exists() else None
    accepted = launch_accepted(enabled, all(item["exit_code"] == 0 for item in (provenance, integrity)), request, receipt)
    screenshot = capture(["/usr/sbin/screencapture", "-x", str(args.output.with_suffix(".png"))])
    report = {"schema": 1, "source_sha": os.environ.get("GITHUB_SHA"),
              "package": package.name, "package_sha256": hashlib.sha256(package.read_bytes()).hexdigest(),
              "system_policy_enabled": enabled, "distribution_accepted": accepted,
              "quarantine": provenance, "system_policy": policy, "integrity": integrity, "assessment": assessment,
              "launch_services": launch, "launch_request": request, "launch_receipt": receipt,
              "screenshot": screenshot, "normal_user_approval_required": not accepted}
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    if not accepted:
        raise RuntimeError("macOS did not launch the headless candidate under enabled system policy. Inspect assessment and LaunchServices evidence; normal user approval remains required.")


if __name__ == "__main__":
    main()
