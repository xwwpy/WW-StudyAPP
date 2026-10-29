"""Real installed Linux package/card/save/restart transition on GitHub runners."""
import argparse
import base64
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[1]

def run(argv, **kwargs):
    print("RUN", *map(str, argv), flush=True)
    return subprocess.run(list(map(str, argv)), check=True, **kwargs)

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def workspace_version(root):
    return tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]


def next_patch(version):
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
    if not match:
        raise ValueError("Acceptance requires a stable three-part workspace version")
    return f"{match[1]}.{match[2]}.{int(match[3]) + 1}"


def snapshot(destination, archive, version):
    destination.mkdir()
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        source.extractall(destination, filter="data")
    workspace = destination / "Cargo.toml"
    value = workspace.read_text(encoding="utf-8")
    value = re.sub(r'(\[workspace.package\]\s*version\s*=\s*")[^"]+(")', lambda match: match[1] + version + match[2], value)
    workspace.write_text(value, encoding="utf-8")
    names = set()
    for path in destination.glob("crates/studyapp-*/Cargo.toml"):
        value = path.read_text(encoding="utf-8")
        names.add(tomllib.loads(value)["package"]["name"])
        value = re.sub(r'(?m)^(studyapp-[\w-]+\s*=\s*\{[^\n]*?version\s*=\s*")[^"]+("[^\n]*path\s*=[^\n]*)$', lambda match: match[1] + version + match[2], value)
        path.write_text(value, encoding="utf-8")
    lock = destination / "Cargo.lock"
    value = lock.read_text(encoding="utf-8")
    value = re.sub(r'(name = "([^"\n]+)"\nversion = ")[^"]+("\n)', lambda match: match[1] + version + match[3] if match[2] in names else match[0], value)
    lock.write_text(value, encoding="utf-8")

def sign_fixture(directory, package, source_sha, target, platform="linux-x86_64"):
    spec = importlib.util.spec_from_file_location("fixture_signing", ROOT / "scripts/update-signature.py")
    signing = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(signing)
    seed = bytes([17]) * 32
    payload = package.read_bytes()
    url = f"https://github.com/xwwpy/WW-StudyAPP/releases/download/v{target}/{package.name}"
    metadata = {"schema": 1, "version": target, "notes": "# Native update acceptance\n\n- Real package transition", "pub_date": "2026-10-29T00:00:00Z", "platforms": {
        platform: {"url": url, "size": len(payload), "sha256": hashlib.sha256(payload).hexdigest(), "signature": base64.b64encode(signing.sign(seed, payload)).decode()}}}
    exact = json.dumps(metadata, separators=(",", ":"), ensure_ascii=False).encode()
    envelope = {"signed_payload": base64.b64encode(exact).decode(), "manifest_signature": base64.b64encode(signing.sign(seed, b"WW-StudyApp update manifest v1\0" + exact)).decode()}
    (directory / "latest.json").write_text(json.dumps(envelope), encoding="utf-8")
    shutil.copy2(package, directory / package.name)
    config = {"source_sha": source_sha, "target_version": target, "package_name": package.name, "package_url": url, "draft_marker": "Native update unsaved draft 中文 🧠 " + str(uuid.uuid4())}
    (directory / "fixture.json").write_text(json.dumps(config, ensure_ascii=False), encoding="utf-8")
    return config

def wait_file(path, timeout, process=None):
    deadline = time.monotonic() + timeout
    while not path.is_file():
        failures = [*path.parent.glob("panic-*.json"), *path.parent.glob("startup-error-*.json"), *path.parent.glob("restart-failure.json")]
        if failures:
            raise RuntimeError("Native application failure: " + failures[0].read_text(encoding="utf-8"))
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"Application exited before evidence {path.name}: {process.returncode}")
        if time.monotonic() > deadline:
            raise TimeoutError(f"Missing native evidence {path}")
        time.sleep(.2)
    return json.loads(path.read_text(encoding="utf-8"))

def collect_diagnostics(root):
    fixture = root / "fixture"
    attempts = []
    try:
        attempts.append(Path(json.loads((fixture / "attempt.json").read_text())["directory"]))
    except (OSError, ValueError, KeyError):
        pass
    try:
        old_pid = json.loads((fixture / "old-observation.json").read_text())["pid"]
        import tempfile
        for directory in Path(tempfile.gettempdir()).glob("studyapp-install-*"):
            try:
                value = json.loads((directory / "transaction.json").read_text())
                if value.get("old_pid", value.get("transaction", {}).get("old_pid")) == old_pid:
                    attempts.append(directory)
            except (OSError, ValueError):
                pass
    except (OSError, ValueError, KeyError):
        pass
    destination = root / "transaction-evidence"
    destination.mkdir(exist_ok=True)
    for directory in set(attempts):
        target = destination / directory.name
        target.mkdir(exist_ok=True)
        for path in directory.glob("*.json"):
            try:
                shutil.copy2(path, target / path.name)
            except OSError as error:
                (target / "copy-error.txt").write_text(str(error), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--production-binary", type=Path, required=True)
    args = parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.uname().sysname != "Linux" or os.getuid() == 0:
        raise RuntimeError("Native transition runs only as an ordinary GitHub Linux runner user")
    root = args.work_dir.resolve()
    root.mkdir()
    source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    archive = subprocess.check_output(["git", "archive", "--format=tar", "HEAD"], cwd=ROOT)
    packages = {}
    binaries = {}
    fixture_target = ROOT / "target/update-acceptance"
    environment = dict(os.environ, CARGO_TARGET_DIR=str(fixture_target))
    old_version = workspace_version(ROOT)
    candidate_version = next_patch(old_version)
    for label, version in [("old", old_version), ("candidate", candidate_version)]:
        source = root / label
        snapshot(source, archive, version)
        run(["cargo", "build", "--locked", "-p", "studyapp-desktop", "--bin", "studyapp", "--features", "update-acceptance"], cwd=source, env=environment)
        binary = root / f"{label}-studyapp"
        shutil.copy2(fixture_target / "debug/studyapp", binary)
        output = root / f"{label}-packages"
        run(["python3", source / "scripts/package-unix.py", "deb", "--binary", binary, "--output", output], cwd=source)
        packages[label] = next(output.glob("*.deb"))
        binaries[label] = binary
    fixture = root / "fixture"
    fixture.mkdir()
    config = sign_fixture(fixture, packages["candidate"], source_sha, candidate_version)
    verify = root / "verify-production"
    verify.mkdir()
    shutil.copy2(fixture / "latest.json", verify / "manifest.json")
    shutil.copy2(packages["candidate"], verify / "package")
    rejected = subprocess.run([str(args.production_binary.resolve()), "--update-verify-package", str(verify)], env=dict(os.environ, STUDYAPP_UPDATE_ACCEPTANCE_DIR=str(fixture)), timeout=20)
    if rejected.returncode != 1:
        raise RuntimeError("Production binary did not reject the fixture signing key")
    run(["sudo", "apt-get", "install", "--reinstall", "-y", str(packages["old"])])
    user = subprocess.check_output(["id", "-un"], text=True).strip()
    rule = root / "update-acceptance.rules"
    rule.write_text('polkit.addRule(function(action, subject) { if (action.id == "org.freedesktop.policykit.exec" && subject.user == ' + json.dumps(user) + ' && action.lookup("program") == "/usr/bin/studyapp" && action.lookup("command_line").indexOf("/usr/bin/studyapp --update-worker ") == 0) { return polkit.Result.YES; } });\n', encoding="utf-8")
    run(["sudo", "install", "-m", "0644", rule, "/etc/polkit-1/rules.d/49-studyapp-update-acceptance.rules"])
    run(["sudo", "systemctl", "restart", "polkit"])
    data = root / "library"
    try:
        with (root / "application.log").open("wb") as log:
            old = subprocess.Popen(["/usr/bin/studyapp", "--data-dir", str(data)], env=dict(os.environ, STUDYAPP_UPDATE_ACCEPTANCE_DIR=str(fixture)), stdout=log, stderr=subprocess.STDOUT)
            before = wait_file(fixture / "old-observation.json", 120, old)
            old.wait(timeout=120)
            if old.returncode != 0:
                raise RuntimeError(f"Original application did not exit cleanly: {old.returncode}")
            after = wait_file(fixture / "new-observation.json", 180)
            transaction = Path(after["transaction"])
            tx = json.loads((transaction / "transaction.json").read_text())
            receipt = wait_file(transaction / "health.json", 30)
            deadline = time.monotonic() + 30
            while not list(transaction.glob("status-healthy-*.json")):
                if time.monotonic() > deadline:
                    raise TimeoutError("Supervisor did not confirm real restart health")
                time.sleep(.2)
            pending = json.loads((fixture / "close-pending.json").read_text())
            ack = json.loads((fixture / "close-ack.json").read_text())
            handoff = json.loads((fixture / "helper-handoff.json").read_text())
            assert ack["id"] == pending["id"] == before["draft_id"]
            assert ack["revision"] >= pending["revision"] and handoff["ack"] == ack
            assert pending["timestamp"] <= ack["timestamp"] <= handoff["timestamp"]
            installed = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", "studyapp"], text=True).strip()
            assert installed == after["version"] == candidate_version
            assert before["pid"] == old.pid != after["pid"]
            assert tx["old_pid"] == old.pid and tx["uid"] == os.getuid()
            assert receipt == {"id": tx["id"], "version": candidate_version, "pid": after["pid"]}
            assert sha("/usr/bin/studyapp") == sha(binaries["candidate"])
            status = Path(f'/proc/{after["pid"]}/status').read_text()
            uid_line = next(line for line in status.splitlines() if line.startswith("Uid:"))
            assert all(int(value) == os.getuid() for value in uid_line.split()[1:])
            assert os.readlink(f'/proc/{after["pid"]}/exe') == "/usr/bin/studyapp"
            draft = json.loads((data / "drafts" / f'{before["draft_id"]}.json').read_text())
            assert draft["draft"]["content"] == config["draft_marker"] == after["live_text"]
            for name in ("update-card.png", "restored.png"):
                assert (fixture / name).stat().st_size > 1000
            result = {"source_sha": source_sha, "installed_version": installed, "installed_sha256": sha("/usr/bin/studyapp"), "candidate_sha256": sha(binaries["candidate"]), "old_pid": old.pid, "new_pid": after["pid"], "uid": os.getuid(), "real_pkexec": True, "system_dialog_exercised": False, "production_rejected_fixture_key": True, "health": receipt, "draft_preserved": True}
            (root / "transition-result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
            # End only the exact observed fixture process after evidence is complete.
            os.kill(after["pid"], 15)
    finally:
        collect_diagnostics(root)

if __name__ == "__main__":
    main()
