"""Inventory only fixed shipped files; this metadata is authenticated by latest.json."""
import argparse
import hashlib
import json
from pathlib import Path
import re

PLATFORMS = ('windows-x86_64', 'linux-x86_64', 'macos-arm64', 'macos-x86_64')
GUIDES = ('object-storage.md', 'release-notes.md', 'storage.md', 'sync-guide.md', 'updates.md', 'user-guide.md')
IMAGES = ('dashboard.png', 'managed-images.png', 'note-editor.png', 'sync-settings.png', 'workspace.png')
MAX_FILE = 256 * 1024 * 1024


def allowed_paths(platform):
    if platform == 'windows-x86_64':
        return ('WW-StudyApp.exe', 'vcruntime140.dll', 'vcruntime140_1.dll', 'README.md', 'LICENSE',
                'RELEASE-NOTES.md', 'THIRD-PARTY-NOTICES.txt')
    if platform == 'linux-x86_64':
        base = 'share/doc/studyapp/'
        return ('bin/studyapp', 'share/applications/studyapp.desktop',
                'share/icons/hicolor/512x512/apps/studyapp.png', base + 'copyright', base + 'README.md',
                *(base + 'docs/' + name for name in GUIDES),
                *(base + 'docs/images/' + name for name in IMAGES))
    if platform in ('macos-arm64', 'macos-x86_64'):
        return ('MacOS/studyapp', 'Info.plist', '_CodeSignature/CodeResources',
                'Resources/studyapp.icns', 'Resources/LICENSE', 'Resources/README.md',
                *('Resources/docs/' + name for name in GUIDES),
                *('Resources/docs/images/' + name for name in IMAGES))
    raise ValueError('Unsupported platform')


def validate(baseline, version, platform, package_hash):
    if (set(baseline) != {'schema', 'version', 'platform', 'package_sha256', 'files'}
            or type(baseline['schema']) is not int or baseline['schema'] != 1
            or baseline['version'] != version or baseline['platform'] != platform
            or baseline['package_sha256'] != package_hash):
        raise ValueError('Integrity baseline identity mismatch')
    if not re.fullmatch(r'[a-f0-9]{64}', package_hash):
        raise ValueError('Invalid package digest')
    files = baseline['files']
    allowed = set(allowed_paths(platform))
    if not isinstance(files, list) or len(files) != len(allowed):
        raise ValueError('Incomplete integrity baseline')
    seen = set()
    for entry in files:
        if (not isinstance(entry, dict) or set(entry) != {'path', 'size', 'sha256'}
                or entry['path'] not in allowed or entry['path'] in seen
                or type(entry['size']) is not int or not 0 < entry['size'] <= MAX_FILE
                or not isinstance(entry['sha256'], str)
                or not re.fullmatch(r'[a-f0-9]{64}', entry['sha256'])):
            raise ValueError('Invalid integrity file')
        seen.add(entry['path'])
    return baseline


def digest(path):
    if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= MAX_FILE:
        raise ValueError('Unsupported inventory file')
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def inventory(root, platform, version, package):
    if not re.fullmatch(r'\d+\.\d+\.\d+', version) or root.is_symlink() or not root.is_dir():
        raise ValueError('Invalid inventory root or version')
    rows = []
    for name in allowed_paths(platform):
        path = root / name
        for parent in (path, *path.parents):
            if parent == root.parent:
                break
            if parent.is_symlink():
                raise ValueError('Inventory links are not allowed')
        rows.append({'path': name, 'size': path.stat().st_size, 'sha256': digest(path)})
    baseline = {'schema': 1, 'version': version, 'platform': platform,
                'package_sha256': digest(package), 'files': rows}
    return validate(baseline, version, platform, baseline['package_sha256'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--platform', choices=PLATFORMS, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--package', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.output.is_symlink() or args.output.resolve().is_relative_to(args.root.resolve()):
        raise ValueError('Inventory output must be new and outside the installed payload')
    body = inventory(args.root, args.platform, args.version, args.package)
    with args.output.open('x', encoding='utf-8') as stream:
        json.dump(body, stream, ensure_ascii=False, separators=(',', ':'))
        stream.write('\n')


if __name__ == '__main__':
    main()
