"""Run only the seven reviewed MSI owner cases on the isolated Windows runner."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

PREFIX = 'maintenance_execute::uninstall::tests::msi_owner_transition_'
EXPECTED = tuple(PREFIX + name for name in (
    'binds_validated_package_target_before_execution',
    'rejects_unbound_owners_roots_versions_and_states',
    'captures_completed_controls_and_preserves_unknown_files',
    'requires_sealed_plan_and_exact_current_target',
    'rejects_tampered_completion_and_changed_current_owner')) + (
    'maintenance_execute::uninstall::tests::maintenance_uninstall_control_cleanup_removes_empty_root_and_preserves_swaps',
    'maintenance_execute::uninstall::tests::maintenance_uninstall_completed_controls_keep_unknown_transaction_neighbor')


def inventory_from_list(output):
    names = [line[:-6] for line in output.splitlines() if line.endswith(': test')]
    selected = [name for name in names if name.startswith(PREFIX) or name in EXPECTED]
    if len(selected) != len(EXPECTED) or set(selected) != set(EXPECTED):
        raise ValueError('Compiled test inventory must contain exactly the seven reviewed selected cases')
    return EXPECTED


def validate_case(name, output):
    passed = re.findall(r'^test (.+) \.\.\. ok\s*$', output, re.MULTILINE)
    summaries = re.findall(r'^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;', output, re.MULTILINE)
    if passed != [name] or summaries != [('1', '0', '0')]:
        raise ValueError('Exact test did not actually execute and pass once: ' + name)


def package_identity(manifest, msi_bytes, sha, run_id, repository, version):
    import pipeline
    name = f'WW-StudyApp-{version}-windows-x86_64.msi'
    if (set(manifest) != {'contract', 'repository', 'sha', 'version', 'run_id', 'assets'}
            or type(manifest['contract']) is not int or manifest['contract'] != 1
            or type(manifest['run_id']) is not int or manifest['run_id'] != run_id
            or manifest['repository'] != repository or manifest['sha'] != sha or manifest['version'] != version
            or set(manifest['assets']) != set(pipeline.asset_names(version))
            or not msi_bytes or hashlib.sha256(msi_bytes).hexdigest() != manifest['assets'].get(name)):
        raise ValueError('Current MSI bytes and release manifest provenance disagree')
    return manifest['assets'][name]


def run(directory, package_directory):
    if os.name != 'nt' or os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('MSI delta cases run only in isolated Windows CI')
    import pipeline
    sha = os.environ['GITHUB_SHA']
    if pipeline.git('rev-parse', 'HEAD').decode().strip() != sha:
        raise ValueError('Exact target checkout required')
    command = ['cargo', 'test', '--locked', '-p', 'studyapp-desktop', '--bin', 'studyapp',
               '--features', 'ui-test', '--no-run', '--message-format=json']
    built = subprocess.run(command, check=True, text=True, stdout=subprocess.PIPE)
    executables = []
    for line in built.stdout.splitlines():
        item = json.loads(line)
        if (item.get('reason') == 'compiler-artifact' and item.get('profile', {}).get('test') is True
                and item.get('target', {}).get('name') == 'studyapp' and item.get('executable')):
            executables.append(item['executable'])
    if len(executables) != 1:
        raise ValueError('Expected one compiled studyapp unit-test executable')
    executable = Path(executables[0]).resolve(strict=True)
    listing = subprocess.run([str(executable), '--list', '--format', 'terse'], check=True,
                             text=True, stdout=subprocess.PIPE).stdout
    inventory_from_list(listing)
    cases = []
    for name in EXPECTED:
        result = subprocess.run([str(executable), '--exact', name, '--nocapture', '--color', 'never'],
                                check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        validate_case(name, result.stdout)
        print(result.stdout, flush=True)
        cases.append({'name': name, 'passed': 1, 'stdout_sha256': hashlib.sha256(result.stdout.encode()).hexdigest()})
    version = pipeline.version_at(pipeline.tree_at(sha))
    package = Path(package_directory)
    manifest = json.loads((package / 'release-manifest.json').read_bytes())
    msi = package / f'WW-StudyApp-{version}-windows-x86_64.msi'
    msi_hash = package_identity(manifest, msi.read_bytes(), sha, int(os.environ['GITHUB_RUN_ID']),
                                os.environ['GITHUB_REPOSITORY'], version)
    record = {'contract': 1, 'sha': sha, 'run_id': int(os.environ['GITHUB_RUN_ID']), 'version': version, 'msi_sha256': msi_hash,
              'attempt': int(os.environ['GITHUB_RUN_ATTEMPT']), 'inventory': list(EXPECTED), 'cases': cases,
              'input_fingerprint': pipeline.fingerprint(pipeline.tree_at(sha), 'desktop'),
              'test_executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}
    target = Path(directory)
    target.mkdir(parents=True, exist_ok=True)
    path = target / 'msi-owner-delta.json'
    if path.exists():
        raise ValueError('Refusing to replace existing delta evidence')
    path.write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True)
    parser.add_argument('--package-directory', default='dist')
    args = parser.parse_args()
    run(args.directory, args.package_directory)
