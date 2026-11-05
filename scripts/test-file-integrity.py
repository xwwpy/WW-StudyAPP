"""Offline inventory checks; never launches a native app."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('integrity', Path(__file__).with_name('file-integrity.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class InventoryTests(unittest.TestCase):
    def fixture(self, base, platform):
        root = base / platform
        root.mkdir()
        for name in m.allowed_paths(platform):
            file = root / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(('fixture ' + name).encode())
        package = base / (platform + '.package')
        package.write_bytes(b'package')
        return root, package

    def test_every_platform_is_read_only_and_excludes_user_neighbors(self):
        base = Path(tempfile.mkdtemp(prefix='studyapp-integrity-'))
        for platform in m.PLATFORMS:
            root, package = self.fixture(base, platform)
            neighbor = root / 'private-notes.sqlite3'
            neighbor.write_bytes(b'not an application file')
            before = {f.relative_to(root).as_posix(): f.read_bytes() for f in root.rglob('*') if f.is_file()}
            baseline = m.inventory(root, platform, '0.3.0', package)
            self.assertNotIn('private-notes.sqlite3', {f['path'] for f in baseline['files']})
            self.assertEqual(before, {f.relative_to(root).as_posix(): f.read_bytes() for f in root.rglob('*') if f.is_file()})
            self.assertFalse((root / 'support/install-receipt.json').exists())

    def test_invalid_identity_traversal_duplicates_and_incomplete_are_rejected(self):
        base = Path(tempfile.mkdtemp(prefix='studyapp-integrity-'))
        root, package = self.fixture(base, 'windows-x86_64')
        baseline = m.inventory(root, 'windows-x86_64', '0.3.0', package)
        mutations = [lambda b: b.update(version='0.3.1'), lambda b: b.update(platform='macos-arm64'),
                     lambda b: b.update(package_sha256='0' * 64), lambda b: b.update(extra=True),
                     lambda b: b['files'][0].update(path='../private'),
                     lambda b: b['files'][0].update(path='private-notes.sqlite3'),
                     lambda b: b['files'][0].update(size=True),
                     lambda b: b['files'][0].update(sha256='x' * 64),
                     lambda b: b['files'].append(b['files'][0]), lambda b: b['files'].pop()]
        for mutate in mutations:
            altered = copy.deepcopy(baseline)
            mutate(altered)
            with self.assertRaises(ValueError):
                m.validate(altered, '0.3.0', 'windows-x86_64', baseline['package_sha256'])

    def test_incomplete_package_cannot_generate_a_baseline(self):
        base = Path(tempfile.mkdtemp(prefix='studyapp-integrity-'))
        root, package = self.fixture(base, 'windows-x86_64')
        (root / 'vcruntime140.dll').rename(root / 'vcruntime140.dll.missing')
        with self.assertRaises((ValueError, FileNotFoundError)):
            m.inventory(root, 'windows-x86_64', '0.3.0', package)


    def test_release_signature_binds_complete_inventories_to_exact_packages(self):
        def load(name, file):
            spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(file))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            return module
        manifest = load('manifest', 'update-manifest.py')
        publish = load('publish', 'publish-release.py')
        base = Path(tempfile.mkdtemp(prefix='studyapp-integrity-signing-'))
        baselines = {}
        for platform in m.PLATFORMS:
            root, package = self.fixture(base, platform)
            suffix = '-Setup.exe' if platform.startswith('windows-') else '.deb' if platform.startswith('linux-') else '.dmg'
            destination = base / ('WW-StudyApp-0.3.0-' + platform + suffix)
            package.rename(destination)
            baselines[platform] = m.inventory(root, platform, '0.3.0', destination)
        setup = base / manifest.setup_asset_name('0.3.0')
        seed = bytes.fromhex('11' * 32)
        args = ('0.3.0', 'v0.3.0', setup, seed, 'notes', 'date')
        body = manifest.signed_manifest(*args, packages_directory=base)
        manifest.write_manifest(body, base / 'latest.json')
        public = manifest.signature_tool.public_key(seed)
        with self.assertRaisesRegex(ValueError, 'authenticated file baselines'):
            publish.verify_signed_packages(base, '0.3.0', public)
        body = manifest.signed_manifest(*args, packages_directory=base, integrity=baselines)
        manifest.write_manifest(body, base / 'latest.json')
        publish.verify_signed_packages(base, '0.3.0', public)
        altered = copy.deepcopy(baselines)
        altered['macos-arm64']['package_sha256'] = '0' * 64
        with self.assertRaises(ValueError):
            manifest.signed_manifest(*args, packages_directory=base, integrity=altered)
        body['integrity']['windows-x86_64']['files'][0]['path'] = 'private-notes.sqlite3'
        manifest.write_manifest(body, base / 'latest.json')
        with self.assertRaises(ValueError):
            publish.verify_signed_packages(base, '0.3.0', public)


if __name__ == '__main__':
    unittest.main()
