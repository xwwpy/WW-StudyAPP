"""Portable tests for Unix package metadata, payloads and integrity files."""
import hashlib
import os
import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("package_unix", Path(__file__).with_name("package-unix.py"))
package = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(package)


class PackageTests(unittest.TestCase):
    def setUp(self):
        # Keep isolated evidence; never recursively remove files or directories.
        evidence = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        evidence.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(prefix="package-test-", dir=evidence))
        self.binary = self.root / "studyapp"
        self.binary.write_bytes(b"package executable fixture")

    def test_dependencies_keep_generated_versions_and_dynamic_runtime_libraries(self):
        deps = package.debian_dependencies("shlibs:Depends=libc6 (>= 2.38), libgtk-3-0t64 (>= 3.24)\n")
        self.assertIn("libc6 (>= 2.38)", deps)
        for required in ("libwebkit2gtk-4.1-0", "xwayland", "mesa-vulkan-drivers", "fonts-noto-cjk"):
            self.assertIn(required, deps)
        with self.assertRaises(ValueError):
            package.debian_dependencies("warning only")

    def test_debian_payload_has_executable_desktop_icon_and_license(self):
        stage = self.root / "deb"
        package.stage_debian(stage, self.binary)
        self.assertEqual((stage / "usr/bin/studyapp").read_bytes(), self.binary.read_bytes())
        if os.name == "posix":
            self.assertEqual((stage / "usr/bin/studyapp").stat().st_mode & 0o777, 0o755)
        desktop = (stage / "usr/share/applications/studyapp.desktop").read_text()
        self.assertIn("Exec=studyapp", desktop)
        self.assertIn("Icon=studyapp", desktop)
        self.assertTrue((stage / "usr/share/icons/hicolor/512x512/apps/studyapp.png").is_file())
        self.assertTrue((stage / "usr/share/doc/studyapp/copyright").is_file())
        self.assertTrue((stage / "usr/share/doc/studyapp/docs/user-guide.md").is_file())

    def test_macos_bundle_version_minimum_os_and_native_executable(self):
        app = self.root / "WW-StudyApp.app"
        package.stage_macos(app, self.binary, "0.1.2")
        info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        self.assertEqual(info["CFBundleShortVersionString"], "0.1.2")
        self.assertEqual(info["CFBundleExecutable"], "studyapp")
        self.assertEqual(info["LSMinimumSystemVersion"], "15.0")
        self.assertEqual(info["CFBundleIdentifier"], "io.github.xwwpy.studyapp")
        self.assertTrue((app / "Contents/MacOS/studyapp").is_file())
        self.assertTrue((app / "Contents/Resources/docs/user-guide.md").is_file())

    def test_checksum_refers_to_download_name_and_exact_payload(self):
        package.write_checksum(self.binary)
        digest, name = self.binary.with_name("studyapp.sha256").read_text().strip().split("  ")
        self.assertEqual(name, self.binary.name)
        self.assertEqual(digest, hashlib.sha256(self.binary.read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
