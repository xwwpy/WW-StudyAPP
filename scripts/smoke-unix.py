"""Run native startup/editor checks only on remote Unix CI runners."""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid


def verify(data, interactive):
    reports = ["smoke-result.txt"]
    if interactive:
        reports.append("native-app-migration.txt")
    for report in reports:
        if not (data / report).read_text(encoding="utf-8").startswith("PASS:"):
            raise ValueError(f"Native smoke did not pass: {report}")
    if interactive:
        verify_pin(data)
        captures = [f"page-{index}.png" for index in range(5)]
        captures.extend(["native-timer-pin-true.png", "native-timer-pin-false.png"])
        if sys.platform.startswith("linux"):
            captures.append("native-markdown-editor.png")
        elif sys.platform == "darwin":
            captures.append("native-markdown-editor-root.png")
        for name in captures:
            image = data / name
            if not image.is_file() or image.stat().st_size < 1000:
                raise ValueError(f"Missing native page capture: {image}")
        draft_id = (data / "native-close-draft-id.txt").read_text(encoding="utf-8").strip()
        if str(uuid.UUID(draft_id)) != draft_id:
            raise ValueError("Invalid final draft identity")
        draft = json.loads((data / "drafts" / f"{draft_id}.json").read_text(encoding="utf-8"))
        if (draft["schema"] != 2 or draft["id"] != draft_id or draft["status"]["kind"] != "active"
                or draft["revision"] < 1 or draft["draft"]["content"] != "关闭窗口前的末次输入 🧠"):
            raise ValueError("Native close did not persist the final editor input")


def verify_pin(data):
    evidence = json.loads((data / "native-timer-pin.json").read_text(encoding="utf-8"))
    states = evidence.get("states", [])
    if evidence.get("schema") != 1 or evidence.get("desktopClosed") is not True or len(states) != 3:
        raise ValueError("Incomplete native timer pin evidence")
    baseline = states[0]["native"]
    if baseline.get("focus") is None or len(baseline.get("bounds", [])) != 4:
        raise ValueError("Missing native timer focus or geometry")
    for state, target in zip(states, [False, True, False]):
        native = state["native"]
        if (native.get("pinned") is not target or state.get("checked") is not target
                or native.get("managed") is not True or native.get("focus") != baseline["focus"]
                or native.get("bounds") != baseline["bounds"]
                or state.get("label") != ("取消置顶" if target else "置顶")):
            raise ValueError("Native timer pin acknowledgement, focus or geometry mismatch")


@contextmanager
def window_manager(data, interactive):
    # This function is called only after main's GitHub/Unix guard, under the
    # runner's isolated Xvfb display. Never take over an existing desktop WM.
    if not interactive or not sys.platform.startswith("linux"):
        yield
        return
    with data.with_suffix(".wm.log").open("w") as log:
        process = subprocess.Popen(["openbox", "--sm-disable"], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 5
            while True:
                if process.poll() is not None:
                    raise RuntimeError("Isolated EWMH window manager exited before readiness")
                supported = subprocess.run(["xprop", "-root", "_NET_SUPPORTED"],
                                           capture_output=True, text=True, check=True, timeout=2)
                if "_NET_WM_STATE_ABOVE" in supported.stdout:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("Isolated EWMH window manager did not become ready")
                time.sleep(0.05)
            yield
        finally:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)


def run_native(binary, data, interactive):
    stderr_path = data.with_suffix(".stderr.log")
    try:
        with data.with_suffix(".stdout.log").open("w") as stdout, stderr_path.open("w") as stderr:
            subprocess.run([str(binary.resolve(strict=True)), "--smoke-test",
                            "--smoke-app-dir" if interactive else "--data-dir", str(data)],
                           stdout=stdout, stderr=stderr, timeout=480, check=True)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        print(stderr_path.read_text(encoding="utf-8", errors="replace"), file=sys.stderr)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--data-dir", required=True, type=Path)
    parser.add_argument("--interactive", action="store_true")
    args = parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.name != "posix":
        raise RuntimeError("Native Unix smoke is restricted to GitHub Unix runners")
    data = args.data_dir.resolve()
    if data.exists():
        raise ValueError("Smoke data directory must be new")
    data.parent.mkdir(parents=True, exist_ok=True)
    with window_manager(data, args.interactive):
        run_native(args.binary, data, args.interactive)
    verify(data, args.interactive)
    print(f"Native startup and WebKit smoke passed: {data}")


if __name__ == "__main__":
    main()
