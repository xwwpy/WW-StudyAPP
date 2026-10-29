import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch, MagicMock
import contextlib
import tempfile
import os
from macos_runner_auth import select_authentication, authentication_candidates, unique_authentication, runner_credential, authentication_log_summary, authentication_identity_summary
import json
spec = importlib.util.spec_from_file_location("assessment", Path(__file__).with_name("assess-macos-update.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
class LaunchEvidence(unittest.TestCase):
    def test_authentication_identity_diagnostics_never_publish_identity_values(self):
        base = {"processImagePath": "/system/coreautha", "timestamp": "2026-10-29T19:00:00+00:00",
                "subsystem": "com.apple.LocalAuthentication", "category": "Passcode"}
        records = [{**base, "eventMessage": "target uid=5501 username=studyapp_example method=verifyPassword"},
                   {**base, "eventMessage": "uid=501 username=<private>"}]
        result = authentication_identity_summary(records, 0, "studyapp_example", "5501")
        self.assertEqual(result[0]["target_uid_matches"], True)
        self.assertEqual(result[0]["username_matches"], True)
        self.assertEqual(result[1]["unspecified_uid_matches"], False)
        self.assertIsNone(result[1]["username_matches"])
        for private in ("5501", "501", "studyapp_example"):
            self.assertNotIn(private, json.dumps(result))
    def test_account_is_verified_before_the_system_authentication_dialog_opens(self):
        events = []
        @contextlib.contextmanager
        def credential():
            events.append("created")
            yield Path("private-credential"), "test-user"
            events.append("removed")
        def capture(*args):
            events.append("verified")
            return {"exit_code": 0, "stdout": '{"verified":true}'}
        def approve(output, path, username, actions):
            events.append("dialog")
            self.assertEqual(username, "test-user")
            return actions
        with patch.object(module, "runner_credential", credential), patch.object(module, "capture", capture), \
                patch.object(module, "system_approval_with_credential", approve, create=True):
            module.system_approval(Path("assessment.json"))
        self.assertEqual(events, ["created", "verified", "dialog", "removed"])
    def test_authentication_log_summary_retains_codes_without_message_contents(self):
        records = [{"processImagePath": "/system/authd", "timestamp": "2026-10-29T19:00:00+00:00",
                    "eventMessage": "private-credential-and-user error=-60005 errAuthorizationDenied"}]
        summary = authentication_log_summary(records, 0)
        self.assertEqual(summary[0]["codes"], ["-60005"])
        self.assertEqual(summary[0]["names"], ["errAuthorizationDenied"])
        self.assertNotIn("private-credential", json.dumps(summary))
        records[0].update(subsystem="com.apple.LocalAuthentication", category="Passcode",
                          eventMessage="error=5100 reason for private-credential user-one at /private/secret/location")
        summary = authentication_log_summary(records, 0, "private-credential", "user-one")
        self.assertIn("[credential]", summary[0]["reason"])
        for sensitive in ("private-credential", "user-one", "/private/secret/location"):
            self.assertNotIn(sensitive, json.dumps(summary))
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
        valid = ("pid:1", {"exit_code": 0, "stdout": json.dumps({"children": [window]})})
        self.assertIsNotNone(unique_authentication([valid]))
        self.assertIsNone(unique_authentication([valid, ("pid:2", {"exit_code": 124, "stdout": ""})]))
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
