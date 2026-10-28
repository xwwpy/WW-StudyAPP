"""Build the signed latest.json manifest consumed by the in-app updater.

Mirrors the reference project's updater manifest: one platform entry with the
NSIS setup URL and an Ed25519 signature over the exact setup bytes.
"""
import argparse
import base64
import datetime
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_SIGNATURE_PATH = Path(__file__).with_name("update-signature.py")
_spec = importlib.util.spec_from_file_location("update_signature", _SIGNATURE_PATH)
signature_tool = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(signature_tool)

PLATFORM = "windows-x86_64"


def setup_asset_name(version: str) -> str:
    return f"WW-StudyApp-{version}-windows-x86_64-Setup.exe"


def setup_url(repository: str, tag: str, version: str) -> str:
    return (
        f"https://github.com/{repository}/releases/download/{tag}/"
        f"{setup_asset_name(version)}"
    )


def signed_manifest(
    version: str,
    tag: str,
    setup: Path,
    seed: bytes,
    notes: str,
    pub_date: str,
    repository: str = "xwwpy/WW-StudyAPP",
) -> dict:
    payload = Path(setup).read_bytes()
    signature = base64.b64encode(signature_tool.sign(seed, payload)).decode("ascii")
    public = signature_tool.public_key(seed).hex()
    if not signature_tool.verify(bytes.fromhex(public), payload, base64.b64decode(signature)):
        raise SystemExit("signature self-check failed")
    return {
        "version": version.lstrip("v"),
        "notes": notes,
        "pub_date": pub_date,
        "platforms": {
            PLATFORM: {
                "signature": signature,
                "url": setup_url(repository, tag, version.lstrip("v")),
            }
        },
    }


def write_manifest(manifest: dict, target: Path) -> None:
    target.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--setup", required=True)
    parser.add_argument("--private", required=True)
    parser.add_argument("--notes-file", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--repository", default="xwwpy/WW-StudyAPP")
    parser.add_argument("--pub-date")
    arguments = parser.parse_args()

    seed = signature_tool.read_private(arguments.private)
    notes = Path(arguments.notes_file).read_text(encoding="utf-8").strip()
    pub_date = arguments.pub_date or datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    manifest = signed_manifest(
        arguments.version,
        arguments.tag,
        Path(arguments.setup),
        seed,
        notes,
        pub_date,
        arguments.repository,
    )
    write_manifest(manifest, Path(arguments.output))
    print(manifest["platforms"][PLATFORM]["url"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
