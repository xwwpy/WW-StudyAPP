"""Latest.json generator tests: URL shape, signature binding and tamper rejection."""
import base64
import importlib.util
import json
import tempfile
from contextlib import contextmanager

@contextmanager
def fixture_directory():
    # Keep fixtures for inspection; no recursive cleanup.
    yield tempfile.mkdtemp(prefix="update-manifest-")

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


manifest = load("update_manifest", "scripts/update-manifest.py")
signature = manifest.signature_tool


class UpdateManifest(unittest.TestCase):
    def test_manifest_binds_signature_to_exact_setup_bytes(self):
        seed = bytes.fromhex("11" * 32)
        with fixture_directory() as directory:
            setup = Path(directory) / "WW-StudyApp-0.9.0-windows-x86_64-Setup.exe"
            setup.write_bytes(b"nsis payload")
            payload = manifest.signed_manifest(
                "v0.9.0",
                "v0.9.0",
                setup,
                seed,
                "- 新版本",
                "2026-10-28T00:00:00Z",
            )
            self.assertEqual(payload["version"], "0.9.0")
            entry = payload["platforms"]["windows-x86_64"]
            self.assertEqual(
                entry["url"],
                "https://github.com/xwwpy/WW-StudyAPP/releases/download/v0.9.0/"
                "WW-StudyApp-0.9.0-windows-x86_64-Setup.exe",
            )
            public = signature.public_key(seed)
            signed = base64.b64decode(entry["signature"])
            self.assertTrue(signature.verify(public, b"nsis payload", signed))
            self.assertFalse(signature.verify(public, b"nsis payload ", signed))
            output = Path(directory) / "latest.json"
            manifest.write_manifest(payload, output)
            written = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(written, payload)

    def test_metadata_is_signed_and_all_native_packages_are_bound(self):
        import hashlib
        seed = bytes.fromhex("11" * 32)
        with fixture_directory() as directory:
            root = Path(directory)
            setup = root / manifest.setup_asset_name("0.9.0")
            setup.write_bytes(b"setup")
            for platform, suffix in [("linux-x86_64", ".deb"), ("macos-arm64", ".dmg"), ("macos-x86_64", ".dmg")]:
                (root / f"WW-StudyApp-0.9.0-{platform}{suffix}").write_bytes(platform.encode())
            result = manifest.signed_manifest("0.9.0", "v0.9.0", setup, seed, "notes", "date", packages_directory=root)
            signed = base64.b64decode(result["signed_payload"])
            self.assertTrue(signature.verify(signature.public_key(seed), b"WW-StudyApp update manifest v1\0" + signed, base64.b64decode(result["manifest_signature"])))
            body = json.loads(signed)
            self.assertEqual(len(body["platforms"]), 4)
            self.assertEqual(body["platforms"], result["platforms"])
            for entry in body["platforms"].values():
                data = (root / entry["url"].rsplit("/", 1)[1]).read_bytes()
                self.assertEqual(entry["size"], len(data))
                self.assertEqual(entry["sha256"], hashlib.sha256(data).hexdigest())
                self.assertTrue(signature.verify(signature.public_key(seed), data, base64.b64decode(entry["signature"])))
            self.assertFalse(signature.verify(signature.public_key(seed), b"WW-StudyApp update manifest v1\0" + signed.replace(b"0.9.0", b"9.9.9"), base64.b64decode(result["manifest_signature"])))

    def test_asset_naming_matches_packaging_script(self):
        self.assertEqual(
            manifest.setup_asset_name("0.9.0"),
            "WW-StudyApp-0.9.0-windows-x86_64-Setup.exe",
        )


if __name__ == "__main__":
    unittest.main()
