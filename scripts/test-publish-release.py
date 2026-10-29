import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("publish", Path(__file__).with_name("publish-release.py"))
publish = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publish)


class CompleteRelease(unittest.TestCase):
    def test_incomplete_or_tampered_packages_never_get_an_aggregate_manifest(self):
        root = Path(tempfile.mkdtemp(prefix="studyapp-publish-test-"))
        with self.assertRaises(ValueError):
            publish.prepare(root, "0.1.2")
        for name in publish.expected_assets("0.1.2"):
            (root / name).write_bytes(name.encode())
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            publish.prepare(root, "0.1.2")
        for path in root.glob("*.sha256"):
            package = root / path.name.removesuffix(".sha256")
            path.write_text(f"{publish.sha256(package)}  {package.name}\n", encoding="ascii")
        with self.assertRaises(ValueError):
            publish.verify_signed_packages(root, "0.1.2")
        spec = importlib.util.spec_from_file_location("manifest", Path(__file__).with_name("update-manifest.py"))
        manifest = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(manifest)
        seed = bytes.fromhex("11" * 32)
        body = manifest.signed_manifest("0.1.2", "v0.1.2", root / manifest.setup_asset_name("0.1.2"), seed, "notes", "date", packages_directory=root)
        manifest.write_manifest(body, root / "latest.json")
        public = manifest.signature_tool.public_key(seed)
        publish.verify_signed_packages(root, "0.1.2", public)
        (root / "WW-StudyApp-0.1.2-macos-arm64.dmg").write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            publish.verify_signed_packages(root, "0.1.2", public)
        (root / "WW-StudyApp-0.1.2-macos-arm64.dmg").write_bytes(b"WW-StudyApp-0.1.2-macos-arm64.dmg")
        assets = publish.prepare(root, "0.1.2")
        self.assertEqual(len(assets), 12)
        self.assertEqual(len((root / "SHA256SUMS.txt").read_text().splitlines()), 11)
        (root / "unexpected.exe").write_bytes(b"extra")
        with self.assertRaisesRegex(ValueError, "unexpected"):
            publish.prepare(root, "0.1.2")


if __name__ == "__main__":
    unittest.main()
