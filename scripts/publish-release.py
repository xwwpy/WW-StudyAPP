"""Publish one complete multi-platform release after all native jobs succeed."""
import argparse
import base64
import importlib.util
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib


RELEASE_KEY = bytes.fromhex("e4019e1a946ea16fbc828977bf61a6ac33045191841f3d73727e4148ec07094b")

def verify_signed_packages(directory, version, key=RELEASE_KEY):
    spec = importlib.util.spec_from_file_location("update_manifest", Path(__file__).with_name("update-manifest.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    try:
        envelope = json.loads((directory / "latest.json").read_text(encoding="utf-8"))
        payload = base64.b64decode(envelope["signed_payload"], validate=True)
        signed = base64.b64decode(envelope["manifest_signature"], validate=True)
        if not module.signature_tool.verify(key, b"WW-StudyApp update manifest v1\0" + payload, signed):
            raise ValueError("Invalid metadata signature")
        body = json.loads(payload)
        if body["schema"] != 1 or body["version"] != version:
            raise ValueError("Wrong metadata version")
        for field in ("schema", "version", "notes", "pub_date", "platforms"):
            if envelope[field] != body[field]:
                raise ValueError("Legacy manifest fields differ")
        expected = {"windows-x86_64": "-Setup.exe", "linux-x86_64": ".deb", "macos-arm64": ".dmg", "macos-x86_64": ".dmg"}
        if set(body["platforms"]) != set(expected):
            raise ValueError("Missing platform")
        for platform, suffix in expected.items():
            name = f"WW-StudyApp-{version}-{platform}{suffix}"
            entry = body["platforms"][platform]
            if entry["url"] != f"https://github.com/xwwpy/WW-StudyAPP/releases/download/v{version}/{name}":
                raise ValueError("Wrong package URL")
            path = directory / name
            if path.is_symlink() or not path.is_file():
                raise ValueError("Invalid package")
            data = path.read_bytes()
            if entry["size"] != len(data) or entry["sha256"] != hashlib.sha256(data).hexdigest():
                raise ValueError("Package digest mismatch")
            if not module.signature_tool.verify(key, data, base64.b64decode(entry["signature"], validate=True)):
                raise ValueError("Invalid package signature")
    except (KeyError, TypeError, OSError) as error:
        raise ValueError("Invalid signed release") from error


def expected_assets(version):
    prefix = f"WW-StudyApp-{version}"
    windows = prefix + "-windows-x86_64"
    packages = [prefix + "-linux-x86_64.deb", prefix + "-macos-arm64.dmg", prefix + "-macos-x86_64.dmg"]
    return {windows + suffix for suffix in ("-Setup.exe", "-Setup.exe.sig")} | {
        prefix + "-installer-x64.zip", prefix + "-portable-x64.zip",
        "latest.json", *packages, *(name + ".sha256" for name in packages)
    }


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prepare(directory, version):
    expected = expected_assets(version)
    actual = {p.name for p in directory.iterdir() if p.name != "SHA256SUMS.txt"}
    if actual != expected:
        raise ValueError(f"Incomplete release: missing={expected - actual}, unexpected={actual - expected}")
    for name in sorted(expected):
        path = directory / name
        if not path.is_file() or path.is_symlink() or path.stat().st_size == 0:
            raise ValueError(f"Invalid release asset: {name}")
        if name.endswith(".sha256"):
            package = name.removesuffix(".sha256")
            if path.read_text(encoding="ascii").strip() != f"{sha256(directory / package)}  {package}":
                raise ValueError(f"Native package checksum mismatch: {package}")
    sums = directory / "SHA256SUMS.txt"
    sums.write_text("".join(f"{sha256(directory / name)}  {name}\n" for name in sorted(expected)), encoding="ascii")
    return [directory / name for name in sorted(expected | {sums.name})]


def gh(*args):
    return subprocess.check_output(["gh", *map(str, args)], text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--directory", type=Path, default=Path("release"))
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    if args.tag != f"v{version}":
        raise ValueError("Release tag does not match workspace version")
    verify_signed_packages(args.directory, version)
    assets = prepare(args.directory, version)
    # Stage assets in a draft so users never see a partial cross-platform release.
    result = subprocess.run(["gh", "release", "view", args.tag, "--json", "isDraft"], capture_output=True, text=True)
    if result.returncode == 0:
        if not json.loads(result.stdout)["isDraft"]:
            raise ValueError("This release is already published; its assets will not be replaced")
    else:
        gh("release", "create", args.tag, "--verify-tag", "--draft", "--title", f"WW-StudyApp {version}", "--notes-file", "docs/release-notes.md")
    gh("release", "upload", args.tag, *assets, "--clobber")
    uploaded = json.loads(gh("release", "view", args.tag, "--json", "assets"))["assets"]
    if {a["name"]: a["size"] for a in uploaded} != {p.name: p.stat().st_size for p in assets}:
        raise ValueError("Uploaded release asset inventory differs from the verified packages")
    gh("release", "edit", args.tag, "--draft=false", "--latest", "--notes-file", "docs/release-notes.md")
    print(gh("release", "view", args.tag, "--json", "url"))


if __name__ == "__main__":
    main()
