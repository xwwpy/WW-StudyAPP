import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("acceptance", Path(__file__).with_name("native-update-acceptance.py"))
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)

class FixtureTests(unittest.TestCase):
    @staticmethod
    def recovery_receipt():
        # Fabricated evidence belongs only to this verifier unit-test fixture.
        return {"pid": 123, "visible_queue": True, "queue_id": 7, "card_id": "notice-7",
                "event_id": "EV-00000000-0000-4000-8000-000000000001", "category": "startup_recovery",
                "severity": "error", "action": "open_update", "dedupe_key": "startup-update-recovery",
                "notice": {"Err": "系统授权已取消或被拒绝，本次更新未安装。可在设置中重试更新。"},
                "bounds": {"x": 16, "y": 32, "width": 360, "height": 160}}

    def test_recovery_receipt_is_bound_to_the_specific_failure_and_real_card_identity(self):
        fixture = Path(tempfile.mkdtemp(prefix="recovery-verifier-unit-"))
        receipt = fixture / "recovery-notice.json"
        (fixture / "recovery-notice.png").write_bytes(bytes([137,80,78,71,13,10,26,10]) + b"unit-fixture" * 200)
        value = self.recovery_receipt()
        receipt.write_text(json.dumps(value), encoding="utf-8")
        self.assertEqual(acceptance.verify_recovery_notice(fixture, 123, "authorization-denied"), value)
        value["notice"] = {"Err": "上次更新未完成，应用已重新打开。请在设置中重试更新。"}
        receipt.write_text(json.dumps(value), encoding="utf-8")
        self.assertEqual(acceptance.verify_recovery_notice(fixture, 123, "health-failure"), value)
        with self.assertRaises(ValueError):
            acceptance.verify_recovery_notice(fixture, 123, "authorization-denied")
        source = Path(acceptance.__file__).read_text(encoding="utf-8")
        self.assertIn('verify_recovery_notice(fixture, after["pid"], "authorization-denied")', source)
        macos = Path(acceptance.__file__).with_name("native-update-macos.py").read_text(encoding="utf-8")
        self.assertIn('common.verify_recovery_notice(fixture, after["pid"], "health-failure")', macos)

    def test_recovery_verifier_rejects_unrelated_messages_identity_bounds_and_missing_pixels(self):
        fixture = Path(tempfile.mkdtemp(prefix="recovery-reject-unit-"))
        receipt = fixture / "recovery-notice.json"
        image = fixture / "recovery-notice.png"
        image.write_bytes(bytes([137,80,78,71,13,10,26,10]) + b"unit-fixture" * 200)
        mutations = [("pid", 124), ("visible_queue", False), ("queue_id", True), ("queue_id", 0),
                     ("card_id", "floating-notice"), ("event_id", "EV-invalid"), ("category", "review_due"),
                     ("severity", "warning"), ("action", "open_reviews"), ("dedupe_key", "reviews-due"),
                     ("notice", {"Ok": "复习已到期"}), ("notice", {"Err": "unrelated error"}), ("bounds", None)]
        for key, bad in mutations:
            value = self.recovery_receipt()
            value[key] = bad
            receipt.write_text(json.dumps(value), encoding="utf-8")
            with self.subTest(key=key, bad=bad), self.assertRaises(ValueError):
                acceptance.verify_recovery_notice(fixture, 123, "authorization-denied")
        for key, bad in (("x", -1), ("y", float("nan")), ("width", 0), ("height", float("inf"))):
            value = self.recovery_receipt()
            value["bounds"][key] = bad
            receipt.write_text(json.dumps(value), encoding="utf-8")
            with self.subTest(key=key), self.assertRaises(ValueError):
                acceptance.verify_recovery_notice(fixture, 123, "authorization-denied")
        receipt.write_text(json.dumps(self.recovery_receipt()), encoding="utf-8")
        for pixels in (b"short", b"not-a-png" * 200):
            image.write_bytes(pixels)
            with self.assertRaisesRegex(ValueError, "PNG evidence"):
                acceptance.verify_recovery_notice(fixture, 123, "authorization-denied")

    def test_linux_transition_uses_the_existing_isolated_wm_and_cleans_up_on_failure(self):
        import contextlib
        from types import SimpleNamespace
        from unittest import mock
        root = Path(tempfile.mkdtemp(prefix="native-update-desktop-test-"))
        events = []
        @contextlib.contextmanager
        def window_manager(path, interactive):
            self.assertEqual(path, root / "session")
            self.assertTrue(interactive)
            events.append("managed")
            try:
                yield
            finally:
                events.append("closed")
        desktop = SimpleNamespace(window_manager=window_manager)
        spec = mock.Mock()
        def transition(path, binary):
            self.assertEqual(events[-1], "managed")
            self.assertEqual(path, root)
            self.assertEqual(binary, Path("production-studyapp"))
            raise RuntimeError("fixture failure")
        with (mock.patch.object(acceptance.importlib.util, "spec_from_file_location", return_value=spec) as load,
              mock.patch.object(acceptance.importlib.util, "module_from_spec", return_value=desktop),
              mock.patch.object(acceptance, "transition", side_effect=transition)):
            with self.assertRaisesRegex(RuntimeError, "fixture failure"):
                acceptance.managed_transition(root, Path("production-studyapp"))
        self.assertEqual(load.call_args.args[1], acceptance.ROOT / "scripts/smoke-unix.py")
        spec.loader.exec_module.assert_called_once_with(desktop)
        self.assertEqual(events, ["managed", "closed"])
        source = Path(acceptance.__file__).read_text(encoding="utf-8")
        main = source.split("def main():", 1)[1].split("def managed_transition", 1)[0]
        self.assertLess(main.index('os.environ.get("GITHUB_ACTIONS")'), main.index("managed_transition(root"))
        self.assertIn("os.getuid() == 0", main)
        body = source.split("def managed_transition", 1)[1].split("def transition", 1)[0]
        self.assertNotIn("windowactivate", body)
        self.assertNotIn("activate_window", body)

    def test_recovery_failure_is_persistent_without_bypassing_user_policy_or_visibility(self):
        root = Path(__file__).resolve().parents[1]
        feedback = (root / "crates/studyapp-desktop/src/feedback.rs").read_text(encoding="utf-8")
        body = feedback.split("pub(crate) fn deliver_update_notice", 1)[1].split("fn push_context_notice", 1)[0]
        self.assertIn("NoticeSeverity::Error", body)
        self.assertNotIn("NoticeSeverity::Warning", body)
        self.assertIn("self.emit_notification(event, cx)", body)
        self.assertNotIn("activate_window", body)
        acceptance = (root / "crates/studyapp-desktop/src/update_acceptance.rs").read_text(encoding="utf-8")
        self.assertIn('"recovery-wait-start.json"', acceptance)
        self.assertIn('"windowActive":window.is_window_active()', acceptance)
        self.assertIn('"desktopNotices":app.notifications.read(cx).desktop_items().len()', acceptance)
        self.assertIn('"visible recovery notification",|a,cx|recovery_notice(a.notifications.read(cx)).is_some()', acceptance)
        self.assertNotIn('let notice=window.update(cx,|_,_,cx|view.read(cx).notice.clone())', acceptance)
        self.assertIn('assert_eq!(event.severity,studyapp_core::NoticeSeverity::Error', acceptance)
        self.assertIn('assert_eq!(event.summary,expected)', acceptance)
        self.assertIn('window.find(card_id.clone()).bounds()', acceptance)
        self.assertIn('assert!(bounds.size.width>px(0.)&&bounds.size.height>px(0.))', acceptance)
        self.assertIn('Duration::from_secs(60)', acceptance)

    def test_editor_diagnostics_only_inspect_profiles_inside_fixture_temp(self):
        root = Path(tempfile.mkdtemp(prefix="native-update-profile-test-"))
        fixture = root / "fixture"
        fixture.mkdir()
        profile = root / "temp" / "studyapp-runtime-test-123" / "cache" / "editor"
        (fixture / "editor-profile-123.json").write_text(json.dumps({"profile": str(profile)}))
        self.assertEqual(acceptance.editor_profile_paths(root), [profile.resolve()])
        outside = root.parent / "outside-user-profile"
        (fixture / "editor-profile-456.json").write_text(json.dumps({"profile": str(outside)}))
        with self.assertRaisesRegex(ValueError, "outside fixture temp"):
            acceptance.editor_profile_paths(root)

    def test_editor_diagnostics_accept_stable_runtime_profile_only(self):
        root = Path(tempfile.mkdtemp(prefix="native-update-stable-profile-test-"))
        fixture = root / "fixture"
        fixture.mkdir()
        record = fixture / "editor-profile-123.json"
        profile = fixture / ".runtime" / "cache" / "editor"
        record.write_text(json.dumps({"profile": str(profile)}))
        self.assertEqual(acceptance.editor_profile_paths(root), [profile.resolve()])
        record.write_text(json.dumps({"profile": str(fixture / ".runtime" / "unrelated")}))
        with self.assertRaises(ValueError):
            acceptance.editor_profile_paths(root)

    @unittest.skipUnless(os.name == "nt", "Windows extended drive paths")
    def test_extended_drive_profile_is_contained_without_allowing_an_external_profile(self):
        root = Path(tempfile.mkdtemp(prefix="native-update-extended-profile-"))
        fixture = root / "fixture"
        fixture.mkdir()
        profile = fixture / ".runtime" / "cache" / "editor"
        profile.mkdir(parents=True)
        prefix = chr(92) * 2 + "?" + chr(92)
        record = fixture / "editor-profile-123.json"
        record.write_text(json.dumps({"profile": prefix + str(profile.resolve())}), encoding="utf-8")
        self.assertEqual(acceptance.editor_profile_paths(root), [profile.resolve()])
        record.write_text(json.dumps({"profile": prefix + str((root.parent / "unrelated-profile").resolve())}), encoding="utf-8")
        with self.assertRaises(ValueError):
            acceptance.editor_profile_paths(root)

    def test_acl_boundary_stays_inside_the_exact_fixture_runtime_or_temp(self):
        root = Path(tempfile.mkdtemp(prefix="native-update-profile-boundary-"))
        runtime = root / "fixture" / ".runtime"
        stable = runtime / "cache" / "editor"
        self.assertEqual(acceptance.editor_profile_boundary(root, stable), runtime.resolve())
        temporary = root / "temp" / "test-profile" / "editor"
        self.assertEqual(acceptance.editor_profile_boundary(root, temporary), (root / "temp").resolve())
        for outside in (runtime / "unrelated", root / "fixture", root.parent / "real-user"):
            with self.assertRaises(ValueError):
                acceptance.editor_profile_boundary(root, outside)

    def test_next_fixture_version_and_snapshot_leave_external_versions_unchanged(self):
        import io, tarfile
        self.assertEqual(acceptance.next_patch("2.8.19"), "2.8.20")
        files = {
            "Cargo.toml": '[workspace]\nmembers = ["crates/studyapp-core"]\n[workspace.package]\nversion = "2.8.19"\n',
            "crates/studyapp-core/Cargo.toml": '[package]\nname = "studyapp-core"\nversion.workspace = true\n[dependencies]\nexternal = { version = "2.8.19" }\nstudyapp-ui = { version = "2.8.19", path = "../studyapp-ui" }\n',
            "Cargo.lock": '[[package]]\nname = "studyapp-core"\nversion = "2.8.19"\n[[package]]\nname = "external"\nversion = "2.8.19"\n',
        }
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w") as tar:
            for name, value in files.items():
                body = value.encode()
                entry = tarfile.TarInfo(name)
                entry.size = len(body)
                tar.addfile(entry, io.BytesIO(body))
        root = Path(tempfile.mkdtemp(prefix="native-update-versions-")) / "snapshot"
        acceptance.snapshot(root, archive.getvalue(), "2.8.20")
        self.assertIn('version = "2.8.20"', (root / "Cargo.toml").read_text())
        manifest = (root / "crates/studyapp-core/Cargo.toml").read_text()
        self.assertIn('external = { version = "2.8.19" }', manifest)
        self.assertIn('studyapp-ui = { version = "2.8.20"', manifest)
        self.assertIn('name = "external"\nversion = "2.8.19"', (root / "Cargo.lock").read_text())

    def test_failed_attempt_is_collected_without_any_success_observation(self):
        root = Path(tempfile.mkdtemp(prefix="native-update-failed-test-"))
        fixture = root / "fixture"
        fixture.mkdir()
        attempt = root / "attempt"
        attempt.mkdir()
        (attempt / "status-failed.json").write_text('{"state":"failed","message":"installer failure"}')
        (fixture / "attempt.json").write_text(json.dumps({"directory": str(attempt)}))
        acceptance.collect_diagnostics(root)
        retained = root / "transaction-evidence" / "attempt" / "status-failed.json"
        self.assertEqual(json.loads(retained.read_text())["message"], "installer failure")
        self.assertFalse((fixture / "new-observation.json").exists())

    def test_wait_exit_reports_terminal_failure_while_original_app_is_live(self):
        root = Path(tempfile.mkdtemp(prefix="native-handoff-failure-"))
        fixture = root / "fixture"
        directory = fixture / ".runtime" / "updates" / "attempt"
        directory.mkdir(parents=True)
        (fixture / "attempt.json").write_text(json.dumps({"directory": str(directory)}), encoding="utf-8")
        (directory / "status-failed-test.json").write_text('{"state":"failed","message":"exit 0xC0000142"}', encoding="utf-8")
        class LiveProcess:
            def poll(self): return None
        with self.assertRaisesRegex(RuntimeError, "0xC0000142"):
            acceptance.wait_exit(LiveProcess(), fixture, 1)

    def test_wait_exit_rejects_evidence_outside_fixture_runtime(self):
        root = Path(tempfile.mkdtemp(prefix="native-handoff-boundary-"))
        fixture = root / "fixture"
        fixture.mkdir()
        (fixture / "attempt.json").write_text(json.dumps({"directory": str(root / "outside")}), encoding="utf-8")
        class LiveProcess:
            def poll(self): return None
        with self.assertRaisesRegex(ValueError, "outside the fixture runtime"):
            acceptance.wait_exit(LiveProcess(), fixture, 1)

    def test_fixture_binds_exact_package_digest_size_and_version(self):
        root = Path(tempfile.mkdtemp(prefix="native-update-signing-test-"))
        packages = root / "packages"
        packages.mkdir()
        package = packages / "WW-StudyApp-0.1.4-linux-x86_64.deb"
        package.write_bytes(b"fixture package")
        fixture = root / "fixture"
        fixture.mkdir()
        config = acceptance.sign_fixture(fixture, package, "a" * 40, "0.1.4")
        import base64
        envelope = json.loads((fixture / "latest.json").read_text())
        payload = base64.b64decode(envelope["signed_payload"])
        metadata = json.loads(payload)
        platform = metadata["platforms"]["linux-x86_64"]
        self.assertEqual(platform["sha256"], acceptance.sha(package))
        self.assertEqual(platform["size"], len(b"fixture package"))
        self.assertEqual(platform["url"], config["package_url"])
        self.assertEqual(metadata["version"], "0.1.4")
        self.assertEqual(config["source_sha"], "a" * 40)

if __name__ == "__main__":
    unittest.main()
