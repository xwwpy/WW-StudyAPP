"""Check Unix smoke evidence validation without opening native windows."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("smoke_unix", Path(__file__).with_name("smoke-unix.py"))
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)


class SmokeTests(unittest.TestCase):
    def test_missing_report_and_false_success_are_rejected(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="smoke-proof-"))
        with self.assertRaises((FileNotFoundError, ValueError)):
            smoke.verify(data, interactive=False)
        (data / "smoke-result.txt").write_text("FAILED")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=False)
        (data / "smoke-result.txt").write_text("PASS: WebKit ready")
        smoke.verify(data, interactive=False)
        with self.assertRaises((FileNotFoundError, ValueError)):
            smoke.verify(data, interactive=True)
        for index in range(5):
            (data / f"page-{index}.png").write_bytes(b"page-image" * 200)
        (data / "native-markdown-editor.png").write_bytes(b"initialized-editor" * 200)
        (data / "native-markdown-editor-root.png").write_bytes(b"native-screen" * 200)
        draft_id = "00000000-0000-4000-8000-000000000001"
        (data / "native-close-draft-id.txt").write_text(draft_id)
        (data / "drafts").mkdir()
        draft = {"schema": 2, "id": draft_id, "status": {"kind": "active"}, "revision": 1,
                 "draft": {"content": "关闭窗口前的末次输入 🧠"}}
        path = data / "drafts" / f"{draft_id}.json"
        path.write_text(json.dumps(draft), encoding="utf-8")
        smoke.verify(data, interactive=True)
        draft["draft"]["content"] = "stale content"
        path.write_text(json.dumps(draft), encoding="utf-8")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=True)


if __name__ == "__main__":
    unittest.main()
