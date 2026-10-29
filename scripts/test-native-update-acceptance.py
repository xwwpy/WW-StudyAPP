import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("acceptance", Path(__file__).with_name("native-update-acceptance.py"))
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)

class FixtureTests(unittest.TestCase):
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
