"""Ed25519 signing for the update manifest (RFC 8032, standard library only).

The application verifies this signature before it runs the NSIS setup. Only the
release workflow holds the private key; the repository stores the public key.

Usage:
  python scripts/update-signature.py keygen --private <path>
  python scripts/update-signature.py sign --private <path> --file <setup.exe> [--out sig]
  python scripts/update-signature.py verify --public <hex> --file <setup.exe> --signature <base64>
"""
import argparse
import base64
import hashlib
import os
import sys

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = (-121665 * pow(121666, P - 2, P)) % P
I = pow(2, (P - 1) // 4, P)


def inv(value: int) -> int:
    return pow(value, P - 2, P)


def point_add(first: tuple[int, int], second: tuple[int, int]) -> tuple[int, int]:
    x1, y1 = first
    x2, y2 = second
    factor = (D * x1 * x2 * y1 * y2) % P
    x3 = (x1 * y2 + x2 * y1) % P * inv(1 + factor) % P
    y3 = (y1 * y2 + x1 * x2) % P * inv(1 - factor) % P
    return (x3, y3)


def point_mul(scalar: int, point: tuple[int, int]) -> tuple[int, int]:
    result = (0, 1)
    addend = point
    while scalar:
        if scalar & 1:
            result = point_add(result, addend)
        addend = point_add(addend, addend)
        scalar >>= 1
    return result


def recover_x(y: int, sign: int) -> int:
    if y >= P:
        raise ValueError("point is not canonical")
    x2 = (y * y - 1) * inv(D * y * y + 1) % P
    if x2 == 0:
        if sign:
            raise ValueError("invalid point sign")
        return 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P != 0:
        x = x * I % P
    if (x * x - x2) % P != 0:
        raise ValueError("point is not on the curve")
    if (x & 1) != sign:
        x = P - x
    return x


BASE_Y = 4 * inv(5) % P
BASE = (recover_x(BASE_Y, 0), BASE_Y)


def compress(point: tuple[int, int]) -> bytes:
    x, y = point
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def decompress(data: bytes) -> tuple[int, int]:
    if len(data) != 32:
        raise ValueError("point must be 32 bytes")
    value = int.from_bytes(data, "little")
    y = value & ((1 << 255) - 1)
    return (recover_x(y, value >> 255), y)


def expand(seed: bytes) -> tuple[int, bytes]:
    if len(seed) != 32:
        raise ValueError("seed must be 32 bytes")
    digest = hashlib.sha512(seed).digest()
    scalar = int.from_bytes(digest[:32], "little")
    scalar &= (1 << 254) - 8
    scalar |= 1 << 254
    return scalar, digest[32:]


def public_key(seed: bytes) -> bytes:
    scalar, _ = expand(seed)
    return compress(point_mul(scalar, BASE))


def sign(seed: bytes, message: bytes) -> bytes:
    scalar, prefix = expand(seed)
    key = compress(point_mul(scalar, BASE))
    nonce = int.from_bytes(hashlib.sha512(prefix + message).digest(), "little") % L
    point = compress(point_mul(nonce, BASE))
    challenge = int.from_bytes(hashlib.sha512(point + key + message).digest(), "little") % L
    response = (nonce + challenge * scalar) % L
    return point + response.to_bytes(32, "little")


def verify(key: bytes, message: bytes, signature: bytes) -> bool:
    if len(key) != 32 or len(signature) != 64:
        return False
    try:
        public = decompress(key)
        point = decompress(signature[:32])
    except ValueError:
        return False
    response = int.from_bytes(signature[32:], "little")
    if response >= L:
        return False
    challenge = int.from_bytes(
        hashlib.sha512(signature[:32] + key + message).digest(), "little"
    ) % L
    return point_mul(response, BASE) == point_add(point, point_mul(challenge, public))


def read_private(path: str) -> bytes:
    text = open(path, "r", encoding="ascii").read().strip()
    if len(text) != 64:
        raise SystemExit("private key must be 64 hex characters")
    return bytes.fromhex(text)


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    keygen = commands.add_parser("keygen")
    keygen.add_argument("--private", required=True)
    signer = commands.add_parser("sign")
    signer.add_argument("--private", required=True)
    signer.add_argument("--file", required=True)
    signer.add_argument("--out")
    verifier = commands.add_parser("verify")
    verifier.add_argument("--public", required=True)
    verifier.add_argument("--file", required=True)
    verifier.add_argument("--signature", required=True)
    arguments = parser.parse_args()

    if arguments.command == "keygen":
        seed = os.urandom(32)
        with open(arguments.private, "w", encoding="ascii", newline="\n") as handle:
            handle.write(seed.hex() + "\n")
        print(public_key(seed).hex())
        return 0

    if arguments.command == "sign":
        seed = read_private(arguments.private)
        message = open(arguments.file, "rb").read()
        encoded = base64.b64encode(sign(seed, message)).decode("ascii")
        if arguments.out:
            with open(arguments.out, "w", encoding="ascii", newline="\n") as handle:
                handle.write(encoded + "\n")
        else:
            print(encoded)
        return 0

    key = bytes.fromhex(arguments.public)
    message = open(arguments.file, "rb").read()
    signature = base64.b64decode(arguments.signature, validate=True)
    return 0 if verify(key, message, signature) else 1


if __name__ == "__main__":
    sys.exit(main())
