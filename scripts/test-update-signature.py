"""Update signature tool tests: RFC 8032 vector, roundtrip and tamper rejection."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "update_signature", ROOT / "scripts/update-signature.py"
)
signature = importlib.util.module_from_spec(spec)
spec.loader.exec_module(signature)


class UpdateSignature(unittest.TestCase):
    def test_rfc8032_test_vector_one(self):
        seed = bytes.fromhex(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
        )
        public = bytes.fromhex(
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        )
        expected = bytes.fromhex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        )
        self.assertEqual(signature.public_key(seed), public)
        self.assertEqual(signature.sign(seed, b""), expected)
        self.assertTrue(signature.verify(public, b"", expected))

    def test_roundtrip_and_tamper_rejection(self):
        seed = bytes.fromhex("00" * 31 + "01")
        public = signature.public_key(seed)
        message = b"WW-StudyApp setup payload"
        signed = signature.sign(seed, message)
        self.assertTrue(signature.verify(public, message, signed))
        self.assertFalse(signature.verify(public, message + b"x", signed))
        corrupted = bytearray(signed)
        corrupted[0] ^= 1
        self.assertFalse(signature.verify(public, message, bytes(corrupted)))
        self.assertFalse(signature.verify(public[:31], message, signed))
        self.assertFalse(signature.verify(public, message, signed[:63]))


if __name__ == "__main__":
    unittest.main()
