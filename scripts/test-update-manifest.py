"""Latest.json generator tests: URL shape, signature binding and tamper rejection."""
import base64
import importlib.util
import json
import tempfile
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
        with tempfile.TemporaryDirectory() as directory:
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

    def test_asset_naming_matches_packaging_script(self):
        self.assertEqual(
            manifest.setup_asset_name("0.9.0"),
            "WW-StudyApp-0.9.0-windows-x86_64-Setup.exe",
        )


if __name__ == "__main__":
    unittest.main()
