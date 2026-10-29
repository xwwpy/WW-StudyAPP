"""Verify system distribution policy for the actual macOS release artifact."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid


def capture(argv):
    result = subprocess.run(argv, capture_output=True, text=True, timeout=120)
    return {"command": argv, "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr}


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
    accepted = enabled and all(item["exit_code"] == 0 for item in (provenance, integrity, assessment))
    report = {"schema": 1, "source_sha": os.environ.get("GITHUB_SHA"),
              "package": package.name, "package_sha256": hashlib.sha256(package.read_bytes()).hexdigest(),
              "system_policy_enabled": enabled, "distribution_accepted": accepted,
              "quarantine": provenance, "system_policy": policy, "integrity": integrity, "assessment": assessment}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    if not accepted:
        raise RuntimeError("macOS did not approve this distribution under enabled system policy; inspect assessment evidence. Ad-hoc signature integrity is insufficient for automatic distribution approval.")


if __name__ == "__main__":
    main()
