"""Collect locked native and editor license notices for each supported package."""
import argparse
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
# These embedded data sets have notices separate from their Rust crate licenses.
ASSET_NOTICES = {
    'hayro-font': ('assets/LICENSE_FOXIT',),
    'hayro-cmap': ('assets/LICENSE.txt',),
    'hayro-interpret': ('assets/CGATS_LICENSE.txt',),
}


def collect(metadata, root, version):
    notices = ['WW-StudyApp — Third-party notices\n\nOriginal licenses and copyright notices follow.\n',
               (root / 'crates/studyapp-desktop/editor/dist/THIRD_PARTY_LICENSES.txt').read_text(encoding='utf-8')]
    members = set(metadata['workspace_members'])
    dependencies = sorted((p for p in metadata['packages'] if p['id'] not in members),
                          key=lambda p: (p['name'], p['version']))
    for package in dependencies:
        directory = Path(package['manifest_path']).parent
        name, release = package['name'], package['version']
        source = package.get('source') or ''
        if source.startswith('registry+'):
            origin = f'https://crates.io/api/v1/crates/{name}/{release}/download'
        elif not source and name in ('gpui-pre', 'gpui-pre-windows', 'gpui-component') and directory.resolve() == (root / 'vendor' / name).resolve():
            origin = f'https://github.com/xwwpy/WW-StudyAPP/tree/v{version}/vendor/{name} (modified for native rendering, menus and scrollbars)'
        else:
            raise ValueError(f'Review non-registry dependency: {package["id"]}')
        notices.append(f'## {name} {release}\nLicense expression: {package.get("license")}\nSource: {origin}\nUpstream: {package.get("repository") or ""}\nDeclared authors: {", ".join(package.get("authors", []))}\n')
        files = {p for p in directory.iterdir() if p.is_file() and re.match(r'^(LICEN[CS]E|COPYING|NOTICE|AUTHORS)($|[._-])', p.name, re.I)}
        if package.get('license_file'):
            files.add(directory / package['license_file'])
        # Subdirectories can contain additional copyright obligations even when
        # the crate already has a root LICENSE file.
        if (directory / 'licenses').is_dir():
            files.update(p for p in (directory / 'licenses').rglob('*') if p.is_file())
        for asset in ASSET_NOTICES.get(name, ()):
            files.add(directory / asset)
        for file in sorted(files):
            notices.append(f'--- {name} {release}: {file.relative_to(directory).as_posix()} ---\n')
            notices.append(file.read_text(encoding='utf-8-sig'))
    return '\n'.join(notices)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    subprocess.run(['cargo', 'fetch', '--locked', '--target', args.target], cwd=ROOT, check=True)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--all-features', '--filter-platform', args.target, '--format-version', '1'], cwd=ROOT))
    args.output.write_text(collect(metadata, ROOT, next(p['version'] for p in metadata['packages'] if p['id'] in metadata['workspace_members'])), encoding='utf-8')


if __name__ == '__main__':
    main()
