"""Actual NSIS card/download/durable-save/install/self-restart transition on Windows CI."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import winreg


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


common = load("update_common", "native-update-acceptance.py")
windows = load("update_windows", "windows-update-launch.py")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--production-binary", type=Path)
    parser.add_argument("--preflight-only", action="store_true")
    args = parser.parse_args()
    if os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Native NSIS transition requires a disposable Windows runner")
    root = args.work_dir.resolve()
    root.mkdir()
    allowed = {"PATH", "SYSTEMROOT", "WINDIR", "SYSTEMDRIVE", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "COMSPEC", "USERNAME", "USERDOMAIN", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMDATA"}
    child_env = {key: value for key, value in os.environ.items() if key.upper() in allowed}
    child_env["GITHUB_ACTIONS"] = "true"
    harness_context = windows.inspect_process(os.getpid())
    # Only this fresh CI fixture directory receives access for the existing user SID.
    common.run(["icacls", root, "/grant:r", f'*{harness_context["user_sid"]}:(OI)(CI)M'])
    preflight = {"harness": harness_context, "fixture_directory": str(root)}
    try:
        preflight["matrix"] = []
        selected = None
        for inherit in (True, False):
            for no_window in (False, True):
                trial = {"inherit_desktop": inherit, "no_window": no_window}
                try:
                    process = windows.launch(Path(os.environ["SYSTEMROOT"]) / "System32/cmd.exe", ["/d", "/c", "exit", "0"], child_env,
                                             headless=no_window, inherit_desktop=inherit)
                    trial["exit_code"] = process.wait(20)
                    if trial["exit_code"] == 0 and not no_window and selected is None:
                        selected = inherit
                except Exception as error:
                    trial["error"] = str(error)
                trial["selected_launch"] = dict(windows.LAST_LAUNCH)
                preflight["matrix"].append(trial)
        if selected is None:
            raise RuntimeError("All ordinary-user desktop/console initialization variants failed; inspect matrix")
        windows.INHERIT_DESKTOP = selected
        preflight["selected_inherit_desktop"] = selected
    except Exception as error:
        preflight["error"] = str(error)
        raise
    finally:
        preflight["selected_launch"] = windows.LAST_LAUNCH
        preflight["acl"] = subprocess.run(["icacls", str(root)], capture_output=True, text=True).stdout
        (root / "token-preflight.json").write_text(json.dumps(preflight, indent=2), encoding="utf-8")
    if args.preflight_only:
        return
    if args.production_binary is None:
        parser.error("--production-binary is required for the full transition")
    source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=common.ROOT, text=True).strip()
    archive = subprocess.check_output(["git", "archive", "--format=tar", "HEAD"], cwd=common.ROOT)
    old_version = common.workspace_version(common.ROOT)
    candidate_version = common.next_patch(old_version)
    fixture_target = common.ROOT / "target/update-acceptance"
    environment = dict(os.environ, CARGO_TARGET_DIR=str(fixture_target))
    makensis = Path(r"C:\Program Files (x86)\NSIS\makensis.exe")
    if not makensis.is_file():
        common.run(["choco", "install", "nsis", "-y", "--no-progress"])
    packages, binaries = {}, {}
    for label, version in [("old", old_version), ("candidate", candidate_version)]:
        source = root / label
        common.snapshot(source, archive, version)
        common.run(["cargo", "build", "--locked", "-p", "studyapp-desktop", "--bin", "studyapp", "--features", "update-acceptance"], cwd=source, env=environment)
        binary = root / f"{label}-studyapp.exe"
        shutil.copy2(fixture_target / "debug/studyapp.exe", binary)
        output = root / f"{label}-packages"
        common.run(["pwsh", "-NoProfile", "-File", source / "scripts/package.ps1", "-Version", version,
                    "-BinaryPath", binary, "-OutputDirectory", output], cwd=source, env=environment)
        package = output / f"WW-StudyApp-{version}-windows-x86_64-Setup.exe"
        payload = output / f"WW-StudyApp-{version}-windows-x86_64"
        common.run([makensis, f"/DVERSION={version}", f"/DPAYLOAD={payload}",
                    f"/DICON={source / 'assets/icon.ico'}", f"/DOUTFILE={package}", source / "installer/studyapp.nsi"], cwd=source)
        packages[label], binaries[label] = package, binary
    fixture = root / "fixture"
    fixture.mkdir()
    config = common.sign_fixture(fixture, packages["candidate"], source_sha, candidate_version, "windows-x86_64")
    verify = root / "verify-production"
    verify.mkdir()
    shutil.copy2(fixture / "latest.json", verify / "manifest.json")
    shutil.copy2(packages["candidate"], verify / "package.exe")
    rejected = subprocess.run([str(args.production_binary.resolve()), "--update-verify-package", str(verify)], timeout=20)
    if rejected.returncode != 1:
        raise RuntimeError("Feature-off production binary did not reject fixture trust")
    child_env["STUDYAPP_UPDATE_ACCEPTANCE_DIR"] = str(fixture)
    installed_dir = root / "installed"
    installer = windows.launch(packages["old"], ["/S", f"/D={installed_dir}"], child_env)
    if installer.wait(120) != 0:
        raise RuntimeError("Initial ordinary-user NSIS installation failed")
    installed = installed_dir / "WW-StudyApp.exe"
    data = root / "library"
    try:
        old = windows.launch(installed, ["--data-dir", data], child_env)
        old_context = windows.inspect_process(old.pid)
        assert not old_context["elevated"] and old_context["user_sid"] == harness_context["user_sid"]
        assert int(old_context["integrity_sid"].rsplit("-", 1)[1]) <= 8192
        before = common.wait_file(fixture / "old-observation.json", 120, old)
        assert old.wait(180) == 0
        after = common.wait_file(fixture / "new-observation.json", 180)
        transaction = Path(after["transaction"])
        outer = json.loads((transaction / "transaction.json").read_text(encoding="utf-8"))
        tx = outer["transaction"]
        receipt = common.wait_file(transaction / "health.json", 30)
        deadline = time.monotonic() + 30
        while not list(transaction.glob("status-healthy-*.json")):
            if time.monotonic() > deadline:
                raise TimeoutError("Windows supervisor did not confirm installed app health")
            time.sleep(.2)
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\WW-StudyApp\Installer") as key:
            registered_version = winreg.QueryValueEx(key, "Version")[0]
            registered_path = winreg.QueryValueEx(key, "InstallLocation")[0]
        assert registered_version == after["version"] == candidate_version
        assert Path(registered_path).resolve() == installed_dir.resolve()
        assert common.sha(installed) == common.sha(binaries["candidate"])
        assert before["pid"] == old.pid == tx["old_pid"] != after["pid"]
        assert receipt == {"id": tx["id"], "version": candidate_version, "pid": after["pid"]}
        new_context = windows.inspect_process(after["pid"])
        assert not new_context["elevated"] and Path(new_context["executable"]).resolve() == installed.resolve()
        assert new_context["user_sid"] == old_context["user_sid"] == harness_context["user_sid"]
        assert int(new_context["integrity_sid"].rsplit("-", 1)[1]) <= 8192
        pending = json.loads((fixture / "close-pending.json").read_text(encoding="utf-8"))
        ack = json.loads((fixture / "close-ack.json").read_text(encoding="utf-8"))
        handoff = json.loads((fixture / "helper-handoff.json").read_text(encoding="utf-8"))
        assert pending["id"] == ack["id"] == before["draft_id"]
        assert ack["revision"] >= pending["revision"] and handoff["ack"] == ack
        assert pending["timestamp"] <= ack["timestamp"] <= handoff["timestamp"]
        draft = json.loads((data / "drafts" / f'{before["draft_id"]}.json').read_text(encoding="utf-8"))
        assert draft["draft"]["content"] == after["live_text"] == config["draft_marker"]
        for name in ("update-card.png", "restored.png"):
            assert (fixture / name).stat().st_size > 1000
        result = {"source_sha": source_sha, "old_version": old_version, "installed_version": registered_version,
                  "installed_sha256": common.sha(installed), "candidate_sha256": common.sha(binaries["candidate"]),
                  "old_process": old_context, "new_process": new_context, "harness_user_sid": harness_context["user_sid"], "health": receipt,
                  "real_nsis": True, "app_owned_restart": True, "draft_preserved": True,
                  "production_rejected_fixture_key": True}
        (root / "transition-result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    finally:
        common.collect_diagnostics(root)


if __name__ == "__main__":
    main()
