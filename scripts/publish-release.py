"""Publish one complete multi-platform release after all native jobs succeed."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib


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
