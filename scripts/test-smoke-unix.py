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
        with self.assertRaises(FileNotFoundError):
            smoke.verify(data, interactive=True)
        (data / "native-timer-pin.json").write_text(json.dumps(self.pin_fixture()), encoding="utf-8")
        for target in ("true", "false"):
            (data / f"native-timer-pin-{target}.png").write_bytes(b"pin-fixture-image" * 200)
        smoke.verify(data, interactive=True)
        draft["draft"]["content"] = "stale content"
        path.write_text(json.dumps(draft), encoding="utf-8")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=True)

    @staticmethod
    def pin_fixture():
        # Fabricated evidence belongs only to these verifier unit-test fixtures.
        return {"schema": 1, "desktopClosed": True, "states": [
            {"checked": target, "label": "取消置顶" if target else "置顶", "native": {"pinned": target, "managed": True,
             "focus": 123, "bounds": [80, 80, 380, 300]}}
            for target in (False, True, False)]}

    def test_native_pin_verifier_rejects_missing_ack_focus_move_and_incomplete_close(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="pin-proof-"))
        cases = [("pinned", False), ("focus", 456), ("bounds", [0, 0, 380, 300]), ("managed", False)]
        for key, value in cases:
            evidence = self.pin_fixture()
            evidence["states"][1]["native"][key] = value
            (data / "native-timer-pin.json").write_text(json.dumps(evidence), encoding="utf-8")
            with self.subTest(key=key), self.assertRaises(ValueError):
                smoke.verify_pin(data)
        for bad in ({"schema": 1}, {**self.pin_fixture(), "desktopClosed": False}):
            (data / "native-timer-pin.json").write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaises(ValueError):
                smoke.verify_pin(data)

    def test_isolated_wm_waits_for_ewmh_and_stops_its_own_child_on_failure(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="wm-proof-")) / "native"
        process = mock.Mock()
        process.poll.return_value = None
        reply = mock.Mock(stdout="_NET_SUPPORTED(ATOM) = _NET_WM_STATE_ABOVE")
        with (
            mock.patch.object(smoke.sys, "platform", "linux"),
            mock.patch.object(smoke.subprocess, "Popen", return_value=process) as start,
            mock.patch.object(smoke.subprocess, "run", return_value=reply),
        ):
            with self.assertRaisesRegex(RuntimeError, "fixture failure"):
                with smoke.window_manager(data, True):
                    raise RuntimeError("fixture failure")
        self.assertEqual(start.call_args.args[0], ["openbox", "--sm-disable"])
        process.terminate.assert_called_once()
        process.wait.assert_called_once_with(timeout=5)

    def test_wm_exit_is_not_treated_as_readiness_and_noninteractive_skips_it(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="wm-exit-proof-")) / "native"
        process = mock.Mock()
        process.poll.return_value = 1
        with (
            mock.patch.object(smoke.sys, "platform", "linux"),
            mock.patch.object(smoke.subprocess, "Popen", return_value=process) as start,
        ):
            with self.assertRaisesRegex(RuntimeError, "exited before readiness"):
                with smoke.window_manager(data, True):
                    self.fail("must not launch app without WM readiness")
            with smoke.window_manager(data, False):
                pass
            self.assertEqual(start.call_count, 1)
        process.terminate.assert_not_called()
        process.wait.assert_called_once_with(timeout=5)

    def test_native_pointer_launch_does_not_hide_the_window_or_create_a_console(self):
        root = Path(__file__).resolve().parents[1]
        for name in ("smoke-window-drag.ps1", "smoke-timer-widget.ps1"):
            source = (root / "scripts" / name).read_text(encoding="utf-8")
            launch = next(line for line in source.splitlines() if "Start-Process" in line)
            self.assertIn("-NoNewWindow", launch)
            self.assertNotIn("-WindowStyle", launch)
            self.assertIn("$env:GITHUB_ACTIONS -ne 'true'", source)
        pointer = (root / "crates/studyapp-desktop/src/update_acceptance_pointer.rs").read_text(encoding="utf-8")
        for check in ("WindowFromPoint(point)", "assert_eq!(hit_root, parent", "IsWindowVisible(parent)",
                      'fixture must own its target window'):
            self.assertIn(check, pointer)
        self.assertNotIn("HWND_TOPMOST", pointer)

    def test_mac_editor_has_no_persistent_browser_state_and_keeps_origin_checks(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/web_editor_host_unix.rs").read_text(encoding="utf-8")
        self.assertIn(".with_incognito(true)", source)
        self.assertIn("BackgroundThrottlingPolicy::Disabled", source)
        for boundary in (".with_url(EDITOR_PAGE)", ".with_visible(false)", ".with_focused(false)",
                         "PermissionResponse::Deny", "NewWindowResponse::Deny",
                         "editor_page_allowed(&url)", "ipc_allowed(&request.uri().to_string()"):
            self.assertIn(boundary, source)

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
