"""Build the signed latest.json manifest consumed by the in-app updater.

Metadata and each package are independently signed; legacy Windows fields
remain available to older clients.
"""
import argparse
import base64
import datetime
import hashlib
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
    packages_directory: Path | None = None,
) -> dict:
    version = version.lstrip("v")
    if tag != f"v{version}" or repository != "xwwpy/WW-StudyAPP":
        raise ValueError("Unexpected release tag or repository")
    paths = {PLATFORM: Path(setup)}
    if packages_directory is not None:
        for platform, suffix in [("linux-x86_64", ".deb"), ("macos-arm64", ".dmg"), ("macos-x86_64", ".dmg")]:
            paths[platform] = packages_directory / f"WW-StudyApp-{version}-{platform}{suffix}"
    platforms = {}
    for platform, path in paths.items():
        expected = setup_asset_name(version) if platform == PLATFORM else f"WW-StudyApp-{version}-{platform}" + (".deb" if platform.startswith("linux-") else ".dmg")
        if path.name != expected or path.is_symlink() or not path.is_file():
            raise ValueError("Invalid package path")
        payload = path.read_bytes()
        if not payload or len(payload) > 256 * 1024 * 1024:
            raise ValueError("Invalid package size")
        platforms[platform] = {
            "signature": base64.b64encode(signature_tool.sign(seed, payload)).decode("ascii"),
            "url": f"https://github.com/{repository}/releases/download/{tag}/{expected}",
            "size": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }
    body = {"schema": 1, "version": version, "notes": notes, "pub_date": pub_date, "platforms": platforms}
    encoded = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    return {**body, "signed_payload": base64.b64encode(encoded).decode("ascii"),
            "manifest_signature": base64.b64encode(signature_tool.sign(seed, b"WW-StudyApp update manifest v1\0" + encoded)).decode("ascii")}



def write_manifest(manifest: dict, target: Path) -> None:
    target.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--setup", required=True)
    parser.add_argument("--private", required=True)
    parser.add_argument("--packages-directory", type=Path)
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
        packages_directory=arguments.packages_directory,
    )
    write_manifest(manifest, Path(arguments.output))
    print(manifest["platforms"][PLATFORM]["url"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
