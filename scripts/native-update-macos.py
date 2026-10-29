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
from macos_quarantine import read_quarantine as quarantine


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


def observe_relaunch(fixture):
    # Runner-owned capture, before the driver activates or interacts with this window.
    for request in fixture.glob("relaunch-observe-*.json"):
        requested = json.loads(request.read_text())
        pid = requested["pid"]
        acknowledgement = fixture / f"relaunch-observed-{pid}.json"
        if acknowledgement.exists():
            continue
        result = policy.capture(["/usr/bin/osascript", "-l", "JavaScript",
            str(Path(__file__).with_name("macos-approval-ui.js")), "observe-window", str(pid)], 10)
        (fixture / f"relaunch-window-{pid}.json").write_text(json.dumps(result))
        assert result["exit_code"] == 0, "Cannot independently observe relaunched window"
        common.run(["/usr/sbin/screencapture", "-x", fixture / f"relaunch-screen-{pid}.png"])
        observed = json.loads(result["stdout"])
        assert requested["native"]["ready"] is True, "App did not naturally present its window"
        assert observed["pid"] == observed["frontmostPid"] == pid, "Relaunched app is not foreground"
        assert any(window["kCGWindowNumber"] == requested["native"]["window"]
            and window["kCGWindowLayer"] == 0 and window["kCGWindowAlpha"] > 0
            and window["kCGWindowBounds"]["Width"] > 100 and window["kCGWindowBounds"]["Height"] > 100
            for window in observed["windows"]), "No matching visible native application window"
        acknowledgement.write_text(json.dumps({"pid": pid, "observed_without_activation": True}))


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
    refusal = root / "quarantine-refusal"
    refusal.mkdir(mode=0o700)
    transition(refusal, packages, source_sha, old_version, target_version, args.production_binary, "quarantine-refusal")
    transition(root, packages, source_sha, old_version, target_version, args.production_binary, "positive")
    negative = root / "health-failure"
    negative.mkdir(mode=0o700)
    transition(negative, packages, source_sha, old_version, target_version, args.production_binary, "health-failure")


def transition(root, packages, source_sha, old_version, target_version, production_binary, scenario):
    fixture = root / "fixture"
    fixture.mkdir(mode=0o700)
    config = common.sign_fixture(fixture, packages["candidate"], source_sha, target_version, "macos-" + platform.machine())
    config["scenario"] = scenario
    (fixture / "fixture.json").write_text(json.dumps(config), encoding="utf-8")
    verify = root / "verify-production"
    verify.mkdir(mode=0o700)
    shutil.copy2(fixture / "latest.json", verify / "manifest.json")
    shutil.copy2(packages["candidate"], verify / "package")
    if subprocess.run([production_binary.resolve(), "--update-verify-package", verify], timeout=20).returncode != 1:
        raise RuntimeError("Production accepted fixture signing key")
    installed = root / "installed/WW-StudyApp.app"
    installed.parent.mkdir(mode=0o700)
    source = mount(packages["old"], root / "old-mount")
    try:
        common.run(["ditto", source, installed])
    finally:
        common.run(["hdiutil", "detach", root / "old-mount"])
    old_hash = common.sha(installed / "Contents/MacOS/studyapp")
    candidate = mount(packages["candidate"], root / "candidate-mount")
    try:
        expected_hash = common.sha(candidate / "Contents/MacOS/studyapp")
        source_quarantine = quarantine(candidate)
    finally:
        common.run(["hdiutil", "detach", root / "candidate-mount"])
    (root / "installation-context.json").write_text(json.dumps({"old_quarantine":quarantine(installed),"source_bundle_quarantine":source_quarantine,
        "first_browser_install_approval":"not_automatically_verified","network_transport":"fixture bytes through production file writer"},indent=2))
    data = root / "library"
    environment = {key: value for key, value in os.environ.items() if key in (
        "PATH", "HOME", "TMPDIR", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "GITHUB_ACTIONS")}
    environment["STUDYAPP_UPDATE_ACCEPTANCE_DIR"] = str(fixture)
    actions = []
    try:
        with (root / "application.log").open("w") as log:
            old = subprocess.Popen([installed / "Contents/MacOS/studyapp", "--data-dir", data], env=environment, stdout=log, stderr=log)
            before = common.wait_file(fixture / "old-observation.json", 120, old)
            if scenario == "quarantine-refusal":
                pointer = Path.home() / "Library/Application Support/io.StudyApp.StudyApp/update-policy-probe.txt"
                downloaded = common.wait_file(fixture / "download-metadata.json", 120, old)
                expected_probe = (Path(downloaded["path"]).parent / "approval").resolve()
                deadline = time.monotonic() + 60
                while True:
                    if old.poll() is not None:
                        raise RuntimeError("Old app exited before system refusal was observed")
                    try:
                        probe = Path(pointer.read_text())
                        request = json.loads((probe / "request.json").read_text())
                        if probe.resolve() == expected_probe and request["version"] == target_version and request["sha256"] == expected_hash:
                            break
                    except (FileNotFoundError, json.JSONDecodeError):
                        pass
                    if time.monotonic() >= deadline:
                        raise TimeoutError("Current quarantined candidate was not prepared for LaunchServices")
                    time.sleep(.1)
                assert not (probe / "receipt.json").exists()
                image_quarantine = quarantine(Path(downloaded["path"]))
                candidate_quarantine = quarantine(probe / "WW-StudyApp.app")
                assert image_quarantine is not None and candidate_quarantine == image_quarantine
                assessment = policy.capture(["/usr/sbin/spctl", "--assess", "--type", "execute", "--verbose=4", str(probe / "WW-StudyApp.app")])
                assert assessment["exit_code"] != 0 and "rejected" in assessment["stderr"].lower()
                system = policy.capture(["/usr/sbin/spctl", "--status"])
                assert "assessments enabled" in system["stdout"] + system["stderr"]
                script = Path(__file__).with_name("macos-approval-ui.js")
                deadline = time.monotonic() + 30
                while True:
                    dialog = policy.capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "snapshot", "CoreServicesUIAgent"], 5)
                    choice = policy.select_approval(json.loads(dialog["stdout"]), "Done") if dialog["exit_code"] == 0 else None
                    if choice is not None:
                        break
                    if time.monotonic() >= deadline:
                        actions.extend([assessment, system, dialog])
                        raise TimeoutError("Actual own-app blocked system dialog was not observed")
                    time.sleep(.2)
                actions.extend([assessment, system, dialog])
                actions.append(policy.capture(["/usr/sbin/screencapture", "-x", str(root / "system-refusal.png")]))
                # Close only the verified rejection warning; never approve the candidate.
                dismissed = policy.capture(["/usr/bin/osascript", "-l", "JavaScript", str(script), "click", "CoreServicesUIAgent", json.dumps(choice)])
                actions.append(dismissed)
                assert dismissed["exit_code"] == 0 and json.loads(dismissed["stdout"]).get("clicked") == "Done"
                (fixture / "system-refusal-dismissed.json").write_text(json.dumps({"probe":str(probe),"request":request,"rejected":True,"clicked":"Done"}))
                refusal = common.wait_file(fixture / "quarantine-refused.json", 60, old)
                assert not (probe / "receipt.json").exists()
                assert old.poll() is None and refusal["state"]["pid"] == before["pid"] == old.pid
                assert refusal["state"]["version"] == old_version and common.sha(installed / "Contents/MacOS/studyapp") == old_hash
                assert refusal["live_text"] == config["draft_marker"] and refusal["unsaved_draft_preserved"] is True
                assert not (fixture / "helper-handoff.json").exists() and not (fixture / "attempt.json").exists()
                (root / "transition-result.json").write_text(json.dumps({"source_sha":source_sha,"scenario":scenario,"old_pid":old.pid,
                    "old_version":old_version,"installed_sha256":old_hash,"old_gui_alive":True,"no_helper_handoff":True,
                    "unsaved_draft_preserved":True,"launch_receipt":None,"image_quarantine":image_quarantine,
                    "candidate_quarantine":candidate_quarantine,"system_rejected":True,"policy_enabled":True},indent=2))
                os.kill(old.pid,15)
                old.wait(timeout=15)
                return
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
                    downloaded = json.loads((fixture / "download-metadata.json").read_text())
                    current_probe = (Path(downloaded["path"]).parent / "approval").resolve()
                    if probe.resolve() == current_probe and request["version"] == target_version and request["sha256"] == expected_hash and not approved:
                        receipt = common.wait_file(probe / "receipt.json", 20)
                        status = policy.capture(["/usr/sbin/spctl", "--status"])
                        enabled = "assessments enabled" in status["stdout"] + status["stderr"]
                        if not policy.launch_accepted(enabled, True, request, receipt):
                            raise RuntimeError("System approval did not produce matching candidate receipt")
                        (fixture / "system-approved.json").write_text(json.dumps({"request": request, "receipt": receipt, "policy": status, "candidate_quarantine":quarantine(probe / "WW-StudyApp.app")}))
                        approved = True
                except FileNotFoundError:
                    pass
                time.sleep(.25)
            assert old.returncode == 0
            # Restart belongs to the supervisor; the harness never launches or approves it.
            deadline = time.monotonic() + (180 if scenario == "health-failure" else 120)
            observed_unhealthy = None
            while not (fixture / "new-observation.json").exists():
                observe_relaunch(fixture)
                failures = [*fixture.glob("panic-*.json"), *fixture.glob("startup-error-*.json")]
                if failures:
                    raise RuntimeError("Native candidate failure: " + failures[0].read_text(encoding="utf-8"))
                if time.monotonic() > deadline:
                    raise TimeoutError("Installed candidate did not restore its library")
                if scenario == "health-failure" and observed_unhealthy is None and (fixture / "unhealthy-candidate.json").exists():
                    observed_unhealthy = common.wait_file(fixture / "unhealthy-candidate.json", 5)
                    pid = observed_unhealthy["pid"]
                    assert observed_unhealthy["version"] == target_version
                    assert Path(observed_unhealthy["executable"]).resolve() == (installed / "Contents/MacOS/studyapp").resolve()
                    assert int(subprocess.check_output(["ps", "-o", "uid=", "-p", str(pid)], text=True)) == os.getuid()
                    assert common.sha(installed / "Contents/MacOS/studyapp") == expected_hash
                    assert not (Path(observed_unhealthy["transaction"]) / "health.json").exists()
                    (root / "unhealthy-process-alive.json").write_text(json.dumps({"pid": pid, "uid": os.getuid(),
                        "time": time.time(), "installed_sha256": expected_hash, "harness_sent_signal": False}))
                time.sleep(1)
            after = common.wait_file(fixture / "new-observation.json", 10)
            common.run(["/usr/sbin/screencapture", "-x", fixture / "restored-runner.png"])
            transaction = Path(common.wait_file(fixture / "attempt.json", 5)["directory"])
            tx = json.loads((transaction / "transaction.json").read_text())["transaction"]
            health = None
            if scenario == "positive":
                health = common.wait_file(transaction / "health.json", 30)
                deadline = time.monotonic() + 30
                while not list(transaction.glob("status-healthy-*.json")):
                    if time.monotonic() > deadline:
                        raise TimeoutError("Supervisor did not confirm installed app health")
                    time.sleep(.2)
            else:
                assert observed_unhealthy is not None
                assert subprocess.run(["ps", "-p", str(observed_unhealthy["pid"])], capture_output=True).returncode == 1
                assert list(transaction.glob("status-recovering-*.json")) and list(transaction.glob("status-recovered-*.json"))
                recovering = [json.loads(path.read_text()) for path in transaction.glob("status-recovering-*.json")]
                assert any(value["message"] == "Updated application startup timeout" for value in recovering), "A premature candidate exit cannot prove supervisor stop"
                assert list(transaction.glob("status-failed-*.json")) and not (transaction / "health.json").exists()
                retained = list(installed.parent.glob(".WW-StudyApp-failed-*.app"))
                assert len(retained) == 1 and common.sha(retained[0] / "Contents/MacOS/studyapp") == expected_hash
                retained_quarantine = quarantine(retained[0])
            executable = installed / "Contents/MacOS/studyapp"
            info = plistlib.loads((installed / "Contents/Info.plist").read_bytes())
            expected_version = target_version if scenario == "positive" else old_version
            installed_hash = expected_hash if scenario == "positive" else old_hash
            assert info["CFBundleShortVersionString"] == after["version"] == expected_version
            assert common.sha(executable) == installed_hash
            assert Path(after["executable"]).resolve() == executable.resolve()
            assert before["pid"] == old.pid == tx["old_pid"] != after["pid"]
            assert int(subprocess.check_output(["ps", "-o", "uid=", "-p", str(after["pid"])], text=True)) == os.getuid()
            if scenario == "positive":
                assert health == {"id": tx["id"], "version": target_version, "pid": after["pid"]}
            else:
                assert after["pid"] != observed_unhealthy["pid"]
            pending = json.loads((fixture / "close-pending.json").read_text())
            ack = json.loads((fixture / "close-ack.json").read_text())
            handoff = json.loads((fixture / "helper-handoff.json").read_text())
            assert pending["id"] == ack["id"] == before["draft_id"]
            assert ack["revision"] >= pending["revision"] and handoff["ack"] == ack
            assert pending["timestamp"] <= ack["timestamp"] <= handoff["timestamp"]
            draft = json.loads((data / "drafts" / f'{before["draft_id"]}.json').read_text())
            assert draft["draft"]["content"] == after["live_text"] == config["draft_marker"]
            installed_quarantine = quarantine(installed)
            system = policy.capture(["/usr/sbin/spctl", "--status"])
            assert "assessments enabled" in system["stdout"] + system["stderr"]
            downloaded = common.wait_file(fixture / "download-metadata.json", 5)
            image_quarantine = quarantine(Path(downloaded["path"]))
            if image_quarantine is not None:
                assert (installed_quarantine if scenario == "positive" else retained_quarantine) == image_quarantine
            (root / "transition-result.json").write_text(json.dumps({"source_sha": source_sha, "scenario": scenario, "old_version": old_version,
                "installed_version": expected_version, "installed_sha256": installed_hash, "health": health,
                "uid": os.getuid(), "new_pid": after["pid"], "real_dmg": True, "app_owned_restart": True,
                "draft_preserved": True, "installed_quarantine": installed_quarantine, "image_quarantine":image_quarantine, "system_policy_enabled":True, "production_rejected_fixture_key": True,
                "supervisor_stopped_unhealthy_candidate": observed_unhealthy["pid"] if observed_unhealthy else None}, indent=2))
            # This stops only the completed final scenario app, never the unhealthy candidate.
            os.kill(after["pid"], 15)
    finally:
        if not (root / "transition-result.json").exists():
            actions.append(policy.capture(["/usr/sbin/screencapture", "-x", str(root / "native-macos-failure.png")], 10))
        (root / "approval-actions.json").write_text(json.dumps(actions, indent=2))
        common.collect_diagnostics(root)


if __name__ == "__main__":
    main()
