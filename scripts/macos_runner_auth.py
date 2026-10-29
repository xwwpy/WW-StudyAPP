"""Normal administrator authentication on disposable GitHub macOS runners only."""
import contextlib
import getpass
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time
import uuid


def select_authentication(tree):
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
        users = [node for _, node in flat if node.get("role") == "AXTextField" and node.get("subrole") != "AXSecureTextField" and node.get("value") == "runner" and node.get("visible") is True and node.get("enabled") is True]
        buttons = [path for path, node in flat if node.get("role") == "AXButton" and node.get("name") == "OK" and node.get("visible") is True and node.get("enabled") is True]
        if len(passwords) == len(users) == len(buttons) == 1:
            candidates.append({"container_path": [index], "container": window, "button_path": buttons[0],
                               "password_path": passwords[0], "label": "OK"})
    return candidates[0] if len(candidates) == 1 else None


@contextlib.contextmanager
def runner_credential():
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true" or getpass.getuser() != "runner":
        raise RuntimeError("Test authentication requires the disposable GitHub macOS runner")
    if "admin" not in subprocess.check_output(["/usr/bin/id", "-Gn"], text=True).split():
        raise RuntimeError("Runner is not an administrator")
    import pty
    import select
    import signal
    import termios
    password = secrets.token_urlsafe(36)
    path = Path(os.environ["RUNNER_TEMP"]) / ("approval-credential-" + str(uuid.uuid4()))
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        output.write(password)
    child = None
    try:
        child, terminal = pty.fork()
        if child == 0:
            os.execve("/usr/bin/sudo", ["sudo", "-n", "/usr/bin/dscl", ".", "-passwd", "/Users/runner"],
                      {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(Path.home()), "USER": "runner"})
        deadline = time.monotonic() + 15
        pending = b""
        supplied = 0
        completed = False
        try:
            while time.monotonic() < deadline:
                pid, status = os.waitpid(child, os.WNOHANG)
                if pid:
                    completed = True
                    if os.waitstatus_to_exitcode(status) != 0 or supplied == 0:
                        raise RuntimeError("Cannot prepare disposable runner authentication")
                    break
                if select.select([terminal], [], [], 0.1)[0]:
                    try:
                        pending += os.read(terminal, 4096)
                    except OSError:
                        continue
                    if b"password:" in pending.lower():
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
        yield path
    finally:
        path.unlink(missing_ok=True)
