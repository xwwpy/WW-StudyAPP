"""Real installed DMG/card/system-approval/save/restart transition on macOS CI."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import sys
import time


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


common = load("native_common", "native-update-acceptance.py")
policy = load("native_policy", "assess-macos-update.py")


def mount(package, destination):
    destination.mkdir()
    common.run(["hdiutil", "attach", package, "-readonly", "-nobrowse", "-mountpoint", destination])
    return destination / "WW-StudyApp.app"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--production-binary", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin" or os.environ.get("GITHUB_ACTIONS") != "true" or os.getuid() == 0:
        raise RuntimeError("macOS acceptance requires an ordinary GitHub runner user")
    root = args.work_dir.resolve()
    root.mkdir(mode=0o700)
    source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    archive = subprocess.check_output(["git", "archive", "HEAD"])
    old_version = common.workspace_version(common.ROOT)
    target_version = common.next_patch(old_version)
    build_env = dict(os.environ, CARGO_TARGET_DIR=str(common.ROOT / "target/update-acceptance"))
    packages = {}
    for label, version in [("old", old_version), ("candidate", target_version)]:
        source = root / label
        common.snapshot(source, archive, version)
        common.run(["cargo", "build", "--locked", "-p", "studyapp-desktop", "--bin", "studyapp", "--features", "update-acceptance"], cwd=source, env=build_env)
        output = root / (label + "-packages")
        common.run([sys.executable, source / "scripts/package-unix.py", "dmg", "--binary", common.ROOT / "target/update-acceptance/debug/studyapp", "--output", output], cwd=source)
        packages[label] = next(output.glob("*.dmg"))
    fixture = root / "fixture"
    fixture.mkdir(mode=0o700)
    config = common.sign_fixture(fixture, packages["candidate"], source_sha, target_version, "macos-" + platform.machine())
    verify = root / "verify-production"
    verify.mkdir(mode=0o700)
    shutil.copy2(fixture / "latest.json", verify / "manifest.json")
    shutil.copy2(packages["candidate"], verify / "package")
    if subprocess.run([args.production_binary.resolve(), "--update-verify-package", verify], timeout=20).returncode != 1:
        raise RuntimeError("Production accepted fixture signing key")
    installed = root / "installed/WW-StudyApp.app"
    installed.parent.mkdir(mode=0o700)
    source = mount(packages["old"], root / "old-mount")
    try:
        common.run(["ditto", source, installed])
    finally:
        common.run(["hdiutil", "detach", root / "old-mount"])
    candidate = mount(packages["candidate"], root / "candidate-mount")
    try:
        expected_hash = common.sha(candidate / "Contents/MacOS/studyapp")
    finally:
        common.run(["hdiutil", "detach", root / "candidate-mount"])
    data = root / "library"
    environment = {key: value for key, value in os.environ.items() if key in (
        "PATH", "HOME", "TMPDIR", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "GITHUB_ACTIONS")}
    environment["STUDYAPP_UPDATE_ACCEPTANCE_DIR"] = str(fixture)
    actions = []
    try:
        with (root / "application.log").open("w") as log:
            old = subprocess.Popen([installed / "Contents/MacOS/studyapp", "--data-dir", data], env=environment, stdout=log, stderr=log)
            before = common.wait_file(fixture / "old-observation.json", 120, old)
            pointer = Path.home() / "Library/Application Support/io.StudyApp.StudyApp/update-policy-probe.txt"
            deadline = time.monotonic() + 180
            approved = False
            while old.poll() is None:
                if time.monotonic() > deadline:
                    raise TimeoutError("Old application did not complete approved handoff")
                try:
                    probe = Path(pointer.read_text())
                    request = json.loads((probe / "request.json").read_text())
                    # Never operate on a previous assessment's unrelated probe.
                    if request["version"] == target_version and request["sha256"] == expected_hash and not approved:
                        actions.extend(policy.system_approval(root / "native-macos-approval.json"))
                        receipt = common.wait_file(probe / "receipt.json", 20)
                        status = policy.capture(["/usr/sbin/spctl", "--status"])
                        enabled = "assessments enabled" in status["stdout"] + status["stderr"]
                        if not policy.launch_accepted(enabled, True, request, receipt):
                            raise RuntimeError("System approval did not produce matching candidate receipt")
                        (fixture / "system-approved.json").write_text(json.dumps({"request": request, "receipt": receipt, "policy": status}))
                        approved = True
                except FileNotFoundError:
                    pass
                time.sleep(.25)
            assert old.returncode == 0
            # Restart is owned by the supervisor; only normal system controls may be approved here.
            deadline = time.monotonic() + 120
            next_approval = time.monotonic() + 8
            while not (fixture / "new-observation.json").exists():
                if time.monotonic() > deadline:
                    raise TimeoutError("Installed candidate did not restore its library")
                if time.monotonic() >= next_approval:
                    actions.extend(policy.system_approval(root / "native-macos-restart.json"))
                    next_approval = time.monotonic() + 30
                time.sleep(1)
            after = common.wait_file(fixture / "new-observation.json", 10)
            transaction = Path(after["transaction"])
            tx = json.loads((transaction / "transaction.json").read_text())["transaction"]
            health = common.wait_file(transaction / "health.json", 30)
            deadline = time.monotonic() + 30
            while not list(transaction.glob("status-healthy-*.json")):
                if time.monotonic() > deadline:
                    raise TimeoutError("Supervisor did not confirm installed app health")
                time.sleep(.2)
            executable = installed / "Contents/MacOS/studyapp"
            info = plistlib.loads((installed / "Contents/Info.plist").read_bytes())
            assert info["CFBundleShortVersionString"] == after["version"] == target_version
            assert common.sha(executable) == expected_hash
            assert Path(after["executable"]).resolve() == executable.resolve()
            assert before["pid"] == old.pid == tx["old_pid"] != after["pid"]
            assert int(subprocess.check_output(["ps", "-o", "uid=", "-p", str(after["pid"])], text=True)) == os.getuid()
            assert health == {"id": tx["id"], "version": target_version, "pid": after["pid"]}
            pending = json.loads((fixture / "close-pending.json").read_text())
            ack = json.loads((fixture / "close-ack.json").read_text())
            handoff = json.loads((fixture / "helper-handoff.json").read_text())
            assert pending["id"] == ack["id"] == before["draft_id"]
            assert ack["revision"] >= pending["revision"] and handoff["ack"] == ack
            assert pending["timestamp"] <= ack["timestamp"] <= handoff["timestamp"]
            draft = json.loads((data / "drafts" / f'{before["draft_id"]}.json').read_text())
            assert draft["draft"]["content"] == after["live_text"] == config["draft_marker"]
            quarantine = policy.capture(["xattr", "-p", "com.apple.quarantine", str(installed)])
            assert quarantine["exit_code"] == 0
            (root / "transition-result.json").write_text(json.dumps({"source_sha": source_sha, "old_version": old_version,
                "installed_version": target_version, "installed_sha256": expected_hash, "health": health,
                "uid": os.getuid(), "new_pid": after["pid"], "real_dmg": True, "app_owned_restart": True,
                "draft_preserved": True, "quarantine": quarantine, "production_rejected_fixture_key": True}, indent=2))
    finally:
        (root / "approval-actions.json").write_text(json.dumps(actions, indent=2))
        common.collect_diagnostics(root)


if __name__ == "__main__":
    main()
