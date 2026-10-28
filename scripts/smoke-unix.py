"""Run native startup/editor checks only on remote Unix CI runners."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid


def verify(data, interactive):
    reports = ["smoke-result.txt"]
    for report in reports:
        if not (data / report).read_text(encoding="utf-8").startswith("PASS:"):
            raise ValueError(f"Native smoke did not pass: {report}")
    if interactive:
        captures = [f"page-{index}.png" for index in range(5)]
        if sys.platform.startswith("linux"):
            captures.append("native-markdown-editor.png")
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
    with data.with_suffix(".stdout.log").open("w") as stdout, data.with_suffix(".stderr.log").open("w") as stderr:
        subprocess.run([str(args.binary.resolve(strict=True)), "--smoke-test", "--data-dir", str(data)],
                       stdout=stdout, stderr=stderr, timeout=480, check=True)
    verify(data, args.interactive)
    print(f"Native startup and WebKit smoke passed: {data}")


if __name__ == "__main__":
    main()
