import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("acceptance", Path(__file__).with_name("native-update-acceptance.py"))
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)

class FixtureTests(unittest.TestCase):
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
