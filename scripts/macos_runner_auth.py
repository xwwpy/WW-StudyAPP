"""Normal administrator authentication on disposable GitHub macOS runners only."""
import contextlib
import getpass
import json
import os
from pathlib import Path
import secrets
import re
import subprocess
import sys
import time
import uuid
from datetime import datetime


def authentication_log_summary(records, since, secret=None, username=None):
    summary = []
    for record in records:
        process = str(record.get("processImagePath", "")).rsplit("/", 1)[-1]
        if process not in ("coreautha", "authd"):
            continue
        try:
            stamp = datetime.fromisoformat(record["timestamp"]).timestamp()
        except (KeyError, ValueError):
            continue
        if stamp < since:
            continue
        message = str(record.get("eventMessage", ""))
        codes = re.findall(r"\b(?:error|status|result|code)\s*[:=]\s*(-?\d+)\b", message, re.IGNORECASE)
        names = re.findall(r"\berr(?:Authorization|Sec)[A-Za-z]+\b", message)
        signals = [word for word in ("authentication succeeded", "authentication failed", "authorization denied", "policy denied", "credential rejected") if word in message.lower()]
        if codes or names or signals:
            entry = {"process": process, "timestamp": record["timestamp"], "codes": codes, "names": names, "signals": signals,
                     "subsystem": record.get("subsystem"), "category": record.get("category")}
            if secret and username and record.get("subsystem") == "com.apple.LocalAuthentication" and record.get("category") == "Passcode" and "5100" in codes:
                reason = message.replace(secret, "[credential]").replace(username, "[test-user]")
                reason = re.sub(r"/[\w./~-]+", "[path]", reason)
                reason = re.sub(r"\b[0-9a-fA-F-]{24,}\b", "[identifier]", reason)
                entry["reason"] = " ".join(reason.split())[:240]
            summary.append(entry)
    return summary


def authentication_diagnostics(since, credential, username):
    result = subprocess.run(["/usr/bin/log", "show", "--last", "1m", "--style", "json", "--predicate",
                             'process == "coreautha" OR process == "authd"'], capture_output=True, text=True, timeout=20)
    if result.returncode != 0:
        return {"available": False, "exit_code": result.returncode}
    try:
        records = json.loads(result.stdout)
        if not isinstance(records, list) or len(records) > 5000:
            return {"available": False, "reason": "bounded log query exceeded"}
        secret = credential.read_text(encoding="utf-8")
        return {"available": True, "records": authentication_log_summary(records, since, secret, username)}
    except ValueError:
        return {"available": False, "reason": "log format unavailable"}


def authentication_candidates(tree):
    candidates = []
    for index, window in enumerate(tree.get("children", [])):
        if window.get("role") != "AXWindow" or window.get("visible") is not True:
            continue
        flat = []
        def walk(node, path):
            flat.append((path, node))
            for child_index, child in enumerate(node.get("children", [])):
                walk(child, [*path, child_index])
        walk(window, [index])
        texts = " ".join(str(node.get(key) or "") for _, node in flat for key in ("name", "value"))
        if "Privacy & Security" not in texts or "attempting to open an app" not in texts or "administrator" not in texts:
            continue
        passwords = [path for path, node in flat if node.get("subrole") == "AXSecureTextField" and node.get("visible") is True and node.get("enabled") is True]
        users = [path for path, node in flat if node.get("role") == "AXTextField" and node.get("subrole") != "AXSecureTextField" and node.get("value") == "runner" and node.get("visible") is True and node.get("enabled") is True]
        buttons = [path for path, node in flat if node.get("role") == "AXButton" and node.get("name") == "OK" and node.get("visible") is True and node.get("enabled") is True]
        if len(passwords) == len(users) == len(buttons) == 1:
            candidates.append({"container_path": [index], "container": window, "button_path": buttons[0],
                               "password_path": passwords[0], "username_path": users[0], "label": "OK"})
    return candidates


def select_authentication(tree):
    candidates = authentication_candidates(tree)
    return candidates[0] if len(candidates) == 1 else None


def unique_authentication(snapshots):
    if any(result["exit_code"] != 0 for _, result in snapshots):
        return None
    candidates = [(process, candidate) for process, result in snapshots
                  for candidate in authentication_candidates(json.loads(result["stdout"]))]
    return candidates[0] if len(candidates) == 1 else None


def password_command(command, password):
    import pty
    import select
    import signal
    import termios
    child, terminal = pty.fork()
    if child == 0:
        os.execve(command[0], command,
                  {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(Path.home()), "USER": "runner"})
    deadline = time.monotonic() + 15
    pending = b""
    supplied = 0
    stages = []
    directory_errors = set()
    completed = False
    try:
        while time.monotonic() < deadline:
            pid, status = os.waitpid(child, os.WNOHANG)
            if pid:
                completed = True
                if os.waitstatus_to_exitcode(status) != 0 or supplied == 0:
                    raise RuntimeError(f"Cannot prepare disposable runner authentication (exit={os.waitstatus_to_exitcode(status)}, prompts={stages}, codes={sorted(directory_errors)})")
                break
            if select.select([terminal], [], [], 0.1)[0]:
                try:
                    pending += os.read(terminal, 4096)
                    directory_errors.update(code.decode("ascii") for code in re.findall(rb"\beDS[A-Za-z]+\b", pending))
                except OSError:
                    continue
                if re.search(rb"password[^\r\n]{0,80}:\s*$", pending.lower()):
                    prompt = pending.lower()
                    stage = "old" if b"old password" in prompt or b"current password" in prompt or b"admin password" in prompt else "repeat" if b"retype" in prompt or b"verify" in prompt else "new" if b"new" in prompt else "unspecified"
                    stages.append(stage)
                    if stage == "old":
                        raise RuntimeError("Runner password reset requires its existing password; no credential guessed")
                    supplied += 1
                    if supplied > 2:
                        raise RuntimeError("Unexpected runner password prompts")
                    attributes = termios.tcgetattr(terminal)
                    attributes[3] &= ~termios.ECHO
                    termios.tcsetattr(terminal, termios.TCSANOW, attributes)
                    os.write(terminal, password.encode() + b"\n")
                    pending = b""
        if not completed:
            raise RuntimeError("Runner authentication preparation timed out")
    finally:
        os.close(terminal)
        if not completed:
            os.kill(child, signal.SIGTERM)
            os.waitpid(child, 0)


@contextlib.contextmanager
def runner_credential():
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true" or getpass.getuser() != "runner":
        raise RuntimeError("Test authentication requires the disposable GitHub macOS runner")
    if "admin" not in subprocess.check_output(["/usr/bin/id", "-Gn"], text=True).split():
        raise RuntimeError("Runner is not an administrator")
    password = secrets.token_hex(24)
    username = "studyapp_" + uuid.uuid4().hex[:12]
    path = Path(os.environ["RUNNER_TEMP"]) / ("approval-credential-" + str(uuid.uuid4()))
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w") as output:
            output.write(password)
        record = "/Users/" + username
        home = Path(os.environ["RUNNER_TEMP"]).resolve() / ("approval-user-" + str(uuid.uuid4()))
        password_command(["/usr/bin/sudo", "-n", "/usr/sbin/sysadminctl", "-addUser", username,
                          "-fullName", "Update acceptance administrator", "-home", str(home), "-password", "-", "-admin"], password)
        identity = subprocess.run(["/usr/bin/dscl", ".", "-read", record, "GeneratedUID"], capture_output=True, text=True, timeout=10)
        authority = subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/dscl", ".", "-read", record, "AuthenticationAuthority"], capture_output=True, text=True, timeout=10)
        if identity.returncode != 0 or authority.returncode != 0 or "ShadowHash" not in authority.stdout:
            raise RuntimeError("Disposable account lacks normal local authentication support")
        password_command(["/usr/bin/dscl", ".", "-authonly", username], password)
        if "admin" not in subprocess.check_output(["/usr/bin/id", "-Gn", username], text=True).split():
            raise RuntimeError("Disposable account is not an administrator")
        yield path, username
    finally:
        path.unlink(missing_ok=True)
