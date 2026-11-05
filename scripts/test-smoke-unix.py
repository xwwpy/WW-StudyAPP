"""Check Unix smoke evidence validation without opening native windows."""
import contextlib
import io
import subprocess
from unittest import mock
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
        with self.assertRaises((FileNotFoundError, ValueError)):
            smoke.verify(data, interactive=True)
        (data / "native-app-migration.txt").write_text("FAILED: profile relocation")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=True)
        (data / "native-app-migration.txt").write_text("PASS: profile relocation")
        smoke.verify(data, interactive=True)
        draft["draft"]["content"] = "stale content"
        path.write_text(json.dumps(draft), encoding="utf-8")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=True)

    def test_first_readiness_deadline_captures_evidence_without_silently_repairing(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_editor.rs").read_text(encoding="utf-8")
        body = source.split("async fn first_readonly_ready(", 1)[1].split("async fn capture_default_preview(", 1)[0]
        self.assertIn("Duration::from_secs(5)", body)
        self.assertIn("native-markdown-editor-default-ready-deadline.json", body)
        self.assertIn("panic!(", body)
        for call in (".set_visible(", ".set_theme(", ".evaluate(", ".focus(", ".requestMeasure("):
            self.assertNotIn(call, body)
        self.assertIn("const TIMEOUT: Duration = Duration::from_secs(5)",
                      (root / "crates/studyapp-desktop/src/smoke_library.rs").read_text(encoding="utf-8"))

    def test_native_failure_prints_captured_stderr_and_still_fails(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="smoke-failure-")) / "native"
        def fail(command, **kwargs):
            kwargs["stderr"].write("panic: localstorage-wal vanished\n")
            raise subprocess.CalledProcessError(101, command)
        captured = io.StringIO()
        with mock.patch.object(smoke.subprocess, "run", side_effect=fail) as run:
            with contextlib.redirect_stderr(captured):
                with self.assertRaises(subprocess.CalledProcessError):
                    smoke.run_native(Path(__file__), data, True)
        self.assertIn("panic: localstorage-wal vanished", captured.getvalue())
        self.assertIn("--smoke-app-dir", run.call_args.args[0])
        self.assertTrue(data.with_suffix(".stderr.log").is_file())


if __name__ == "__main__":
    unittest.main()
