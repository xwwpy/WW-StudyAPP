import importlib.util
from pathlib import Path
import unittest
spec = importlib.util.spec_from_file_location("assessment", Path(__file__).with_name("assess-macos-update.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
class LaunchEvidence(unittest.TestCase):
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
