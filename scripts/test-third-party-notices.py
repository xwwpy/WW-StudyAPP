"""Regression tests for redistributing embedded document-rendering resources."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('notices', Path(__file__).with_name('third-party-notices.py'))
notices = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(notices)


class NoticeTests(unittest.TestCase):
    def setUp(self):
        evidence = Path(__file__).resolve().parents[1] / '.scratch/notice-tests'
        evidence.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=evidence))
        editor = self.root / 'crates/studyapp-desktop/editor/dist/THIRD_PARTY_LICENSES.txt'
        editor.parent.mkdir(parents=True)
        editor.write_text('Editor and ZIP licenses', encoding='utf-8')

    def fixture(self, name, relative):
        directory = self.root / name
        asset = directory / relative
        asset.parent.mkdir(parents=True, exist_ok=True)
        asset.write_text('Copyright embedded data ' + name, encoding='utf-8')
        (directory / 'LICENSE').write_text('MIT main crate license', encoding='utf-8')
        return dict(id=name, name=name, version='1.0.0', manifest_path=str(directory / 'Cargo.toml'), source='registry+https://github.com/rust-lang/crates.io-index', license='MIT')

    def test_embedded_notices_survive_alongside_root_licenses(self):
        packages = [self.fixture(name, asset[0]) for name, asset in notices.ASSET_NOTICES.items()]
        text = notices.collect(dict(packages=packages, workspace_members=[]), self.root, '0.3.5')
        for name, asset in notices.ASSET_NOTICES.items():
            self.assertIn(asset[0], text)
            self.assertIn('Copyright embedded data ' + name, text)
        self.assertIn('Editor and ZIP licenses', text)
        self.assertIn('MIT main crate license', text)

    def test_missing_required_resource_notice_fails_packaging(self):
        package = self.fixture('hayro-font', 'assets/incorrect-license')
        with self.assertRaises(FileNotFoundError):
            notices.collect(dict(packages=[package], workspace_members=[]), self.root, '0.3.5')

    def test_unreviewed_local_dependency_is_rejected(self):
        package = self.fixture('unknown', 'LICENSE')
        package['source'] = None
        with self.assertRaises(ValueError):
            notices.collect(dict(packages=[package], workspace_members=[]), self.root, '0.3.5')


if __name__ == '__main__':
    unittest.main()
