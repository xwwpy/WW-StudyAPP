"""Build native Debian or macOS packages on their matching CI runners.

Staging directories are unique and retained; this script never recursively deletes.
"""
import argparse
import hashlib
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
APP_NAME = "WW-StudyApp"


def run(*args, **kwargs):
    return subprocess.check_output([str(arg) for arg in args], text=True, **kwargs).strip()


def version():
    value = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", value):
        raise ValueError("A stable SemVer workspace version is required")
    return value


def copy(source, destination, executable=False):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o755 if executable else 0o644)


def copy_guides(destination):
    for guide in (ROOT / "docs").rglob("*"):
        if guide.is_file():
            copy(guide, destination / "docs" / guide.relative_to(ROOT / "docs"))


def stage_debian(stage, binary):
    copy(binary, stage / "usr/bin/studyapp", executable=True)
    copy(ROOT / "assets/icon.png", stage / "usr/share/icons/hicolor/512x512/apps/studyapp.png")
    copy(ROOT / "LICENSE", stage / "usr/share/doc/studyapp/copyright")
    copy(ROOT / "README.md", stage / "usr/share/doc/studyapp/README.md")
    copy_guides(stage / "usr/share/doc/studyapp")
    desktop = stage / "usr/share/applications/studyapp.desktop"
    desktop.parent.mkdir(parents=True, exist_ok=True)
    desktop.write_text("[Desktop Entry]\nType=Application\nName=WW-StudyApp\n"
                       "Comment=Local-first study notes\nExec=studyapp\nIcon=studyapp\n"
                       "Terminal=false\nCategories=Education;Office;\nStartupWMClass=studyapp\n",
                       encoding="utf-8")


def debian_dependencies(output):
    match = re.search(r"^shlibs:Depends=(.+)$", output, re.MULTILINE)
    if not match or "libc6" not in match[1]:
        raise ValueError("dpkg-shlibdeps did not provide native library dependencies")
    # GPUI loads several libraries and graphics drivers dynamically. XWayland
    # is needed when the GTK WebKit child is embedded in a GPUI X11 window.
    runtime = ["libwebkit2gtk-4.1-0", "libvulkan1", "libasound2t64", "libfontconfig1",
               "libxkbcommon-x11-0", "libwayland-client0", "mesa-vulkan-drivers",
               "xwayland", "fonts-noto-cjk", "dbus-user-session", "gnome-keyring", "pkexec"]
    return ", ".join(dict.fromkeys(match[1].split(", ") + runtime))


def stage_macos(app, binary, release_version):
    copy(binary, app / "Contents/MacOS/studyapp", executable=True)
    copy(ROOT / "LICENSE", app / "Contents/Resources/LICENSE")
    copy(ROOT / "README.md", app / "Contents/Resources/README.md")
    copy_guides(app / "Contents/Resources")
    info = {"CFBundleName": APP_NAME, "CFBundleDisplayName": APP_NAME,
            "CFBundleIdentifier": "io.github.xwwpy.studyapp", "CFBundleExecutable": "studyapp",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": release_version,
            "CFBundleVersion": release_version, "CFBundleIconFile": "studyapp.icns",
            "LSMinimumSystemVersion": "15.0", "NSHighResolutionCapable": True,
            "NSPrincipalClass": "NSApplication", "LSApplicationCategoryType": "public.app-category.education"}
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))


def write_checksum(path):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    path.with_name(path.name + ".sha256").write_text(f"{digest}  {path.name}\n", encoding="ascii")


def deb(binary, destination, release_version):
    if platform.system() != "Linux" or platform.machine() not in ("x86_64", "amd64"):
        raise ValueError("Debian package must be built on native Linux x86_64")
    stage = Path(tempfile.mkdtemp(prefix="studyapp-deb-"))
    stage.chmod(0o755)
    stage_debian(stage, binary)
    debian = stage / "debian"
    debian.mkdir()
    (debian / "control").write_text("Source: studyapp\nSection: education\nPriority: optional\n"
                                    "Maintainer: WW-StudyAPP <noreply@github.com>\n\n"
                                    "Package: studyapp\nArchitecture: amd64\nDescription: Study notes\n")
    deps = debian_dependencies(run("dpkg-shlibdeps", "-O", "-e" + str(binary), cwd=stage))
    # dpkg tooling metadata is outside the installed payload.
    (debian / "control").unlink()
    debian.rmdir()
    control = stage / "DEBIAN/control"
    control.parent.mkdir()
    size = sum(p.stat().st_size for p in (stage / "usr").rglob("*") if p.is_file()) // 1024 + 1
    control.write_text(f"Package: studyapp\nVersion: {release_version}\nArchitecture: amd64\n"
                       "Maintainer: WW-StudyAPP <noreply@github.com>\nSection: education\nPriority: optional\n"
                       f"Installed-Size: {size}\nDepends: {deps}\nHomepage: https://github.com/xwwpy/WW-StudyAPP\n"
                       "Description: Local-first learning notes and spaced review\n"
                       " Markdown notes, learning records, backups and synchronization.\n")
    output = destination / f"{APP_NAME}-{release_version}-linux-x86_64.deb"
    run("dpkg-deb", "--root-owner-group", "--build", stage, output)
    run("dpkg-deb", "--info", output)
    run("desktop-file-validate", stage / "usr/share/applications/studyapp.desktop")
    return output


def dmg(binary, destination, release_version):
    if platform.system() != "Darwin":
        raise ValueError("DMG must be built on native macOS")
    arch = platform.machine()
    if arch not in ("arm64", "x86_64"):
        raise ValueError("Unsupported macOS architecture")
    run("lipo", binary, "-verify_arch", arch)
    stage = Path(tempfile.mkdtemp(prefix="studyapp-dmg-"))
    stage.chmod(0o755)
    app = stage / f"{APP_NAME}.app"
    stage_macos(app, binary, release_version)
    from PIL import Image
    iconset = Path(tempfile.mkdtemp(prefix="studyapp-icon-")) / "studyapp.iconset"
    iconset.mkdir()
    with Image.open(ROOT / "assets/icon.ico") as icon:
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                suffix = "@2x" if scale == 2 else ""
                icon.convert("RGBA").resize((size * scale, size * scale), Image.Resampling.LANCZOS).save(
                    iconset / f"icon_{size}x{size}{suffix}.png")
    run("iconutil", "-c", "icns", iconset, "-o", app / "Contents/Resources/studyapp.icns")
    run("codesign", "--force", "--sign", "-", app)
    run("codesign", "--verify", "--strict", "--verbose=2", app)
    (stage / "Applications").symlink_to("/Applications", target_is_directory=True)
    output = destination / f"{APP_NAME}-{release_version}-macos-{arch}.dmg"
    run("hdiutil", "create", "-volname", APP_NAME, "-srcfolder", stage, "-format", "UDZO", output)
    run("hdiutil", "verify", output)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("deb", "dmg"))
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/studyapp")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    result = {"deb": deb, "dmg": dmg}[args.kind](args.binary.resolve(strict=True), args.output.resolve(), version())
    write_checksum(result)
    print(result)


if __name__ == "__main__":
    main()
