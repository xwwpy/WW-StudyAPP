import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch, MagicMock
import contextlib
import tempfile
import os
from macos_runner_auth import select_authentication, authentication_candidates, runner_credential
spec = importlib.util.spec_from_file_location("assessment", Path(__file__).with_name("assess-macos-update.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
class LaunchEvidence(unittest.TestCase):
    def test_runner_credential_write_failure_removes_the_single_file(self):
        original = os.fdopen
        @contextlib.contextmanager
        def fail_write(fd, mode):
            with original(fd, mode):
                writer = MagicMock()
                writer.write.side_effect = OSError("simulated write failure")
                yield writer
        with tempfile.TemporaryDirectory() as root:
            with patch("macos_runner_auth.sys.platform", "darwin"), patch("macos_runner_auth.getpass.getuser", return_value="runner"), \
                    patch("macos_runner_auth.subprocess.check_output", return_value="staff admin"), \
                    patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_TEMP": root}), \
                    patch.dict("sys.modules", {"pty": MagicMock(), "termios": MagicMock()}), \
                    patch("macos_runner_auth.os.fdopen", side_effect=fail_write):
                with self.assertRaises(OSError):
                    with runner_credential():
                        self.fail("Cannot yield an incomplete credential")
            self.assertEqual(list(Path(root).iterdir()), [])
    def test_runner_authentication_requires_unique_visible_exact_prompt(self):
        window = {"role": "AXWindow", "visible": True, "name": "Privacy & Security", "children": [
            {"role": "AXStaticText", "value": "You are attempting to open an app. Enter an administrator password."},
            {"role": "AXTextField", "value": "runner", "visible": True, "enabled": True},
            {"role": "AXTextField", "subrole": "AXSecureTextField", "visible": True, "enabled": True},
            {"role": "AXButton", "name": "OK", "visible": True, "enabled": True},
        ]}
        self.assertIsNotNone(select_authentication({"children": [window]}))
        self.assertIsNone(select_authentication({"children": [window, window]}))
        self.assertEqual(len(authentication_candidates({"children": [window, window]})), 2)
        window["children"][1]["value"] = "another-user"
        self.assertIsNone(select_authentication({"children": [window]}))
    def test_final_open_anyway_confirmation_is_bound_to_the_app(self):
        tree = {"role": "AXWindow", "subrole": "AXSystemDialog", "children": [
            {"role": "AXStaticText", "value": 'Open “WW-StudyApp”?'},
            {"role": "AXButton", "name": "Open Anyway", "visible": True, "enabled": True},
        ]}
        self.assertIsNotNone(module.select_approval(tree, "Open Anyway"))
        tree["children"][0]["value"] = 'Open “AnotherApp”?'
        self.assertIsNone(module.select_approval(tree, "Open Anyway"))
    def test_scroll_selection_cannot_authorize_hidden_button(self):
        tree = {"role": "AXGroup", "children": [
            {"role": "AXStaticText", "name": '“WW-StudyApp” was blocked'},
            {"role": "AXButton", "name": "Open Anyway", "visible": False, "enabled": True},
        ]}
        self.assertIsNone(module.select_approval(tree, "Open Anyway"))
        self.assertIsNotNone(module.select_approval(tree, "Open Anyway", for_scroll=True))
        tree["children"][0]["name"] = '“AnotherApp” was blocked'
        self.assertIsNone(module.select_approval(tree, "Open Anyway", for_scroll=True))
    def test_system_dialog_is_a_related_warning_container(self):
        tree = {"role": "AXWindow", "subrole": "AXSystemDialog", "children": [
            {"role": "AXStaticText", "name": '“WW-StudyApp” Not Opened'},
            {"role": "AXButton", "name": "Done", "visible": True, "enabled": True},
        ]}
        self.assertEqual(module.select_approval(tree, "Done")["button_path"], [1])
        tree["children"][0]["name"] = '“AnotherApp” Not Opened'
        self.assertIsNone(module.select_approval(tree, "Done"))
    def test_approval_is_bound_to_own_warning_not_another_container(self):
        def warning(name, enabled=True):
            return {"role": "AXGroup", "children": [
                {"role": "AXStaticText", "name": f'“{name}” was blocked'},
                {"role": "AXButton", "name": "Open Anyway", "visible": True, "enabled": enabled},
            ]}
        own, other = warning("WW-StudyApp.app"), warning("Other.app")
        tree = {"role": "AXWindow", "children": [own, other]}
        choice = module.select_approval(tree, "Open Anyway")
        self.assertEqual(choice["button_path"], [0, 1])
        own["children"][1]["enabled"] = False
        self.assertIsNone(module.select_approval(tree, "Open Anyway"))
        own["children"][1]["enabled"] = True
        own["children"][1]["visible"] = False
        self.assertIsNone(module.select_approval(tree, "Open Anyway"))
        own["children"][1]["visible"] = True
        tree["children"].append(warning("WW-StudyApp.app"))
        self.assertIsNone(module.select_approval(tree, "Open Anyway"))

    def test_system_dialog_timeout_still_produces_diagnostic_evidence(self):
        from unittest.mock import patch
        import subprocess
        with patch.object(module.subprocess, "run", side_effect=subprocess.TimeoutExpired(["open"], 120)):
            result = module.capture(["open"])
        self.assertEqual(result["exit_code"], 124)
        self.assertTrue(result["timed_out"])

    def test_real_launch_receipt_is_bound_and_requires_enabled_system_policy(self):
        request = {"nonce": "attempt", "version": "0.1.4", "sha256": "digest"}
        receipt = {**request, "pid": 42}
        self.assertTrue(module.launch_accepted(True, True, request, receipt))
        self.assertFalse(module.launch_accepted(False, True, request, receipt))
        self.assertFalse(module.launch_accepted(True, False, request, receipt))
        for field in ("nonce", "version", "sha256", "pid"):
            wrong = {**receipt, field: None}
            self.assertFalse(module.launch_accepted(True, True, request, wrong))
if __name__ == "__main__":
    unittest.main()
