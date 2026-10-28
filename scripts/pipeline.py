"""Content-addressed CI receipts and promotion of checked release assets (stdlib only)."""
import argparse
from functools import cache
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile

import production_delta

GROUPS = ('editor', 'core-linux', 'core-windows', 'desktop', 'policy', 'maintenance-policy', 'maintenance-native', 'security')
JOBS = {'editor': 'editor / editor', 'core-linux': 'Core and sync (ubuntu-latest)',
        'core-windows': 'Core and sync (windows-latest)', 'desktop': 'Windows desktop and native smoke',
        'policy': 'Updater and package policies', 'maintenance-policy': 'Installer and modern package policies',
        'maintenance-native': 'Windows installer and maintenance lifecycle',
        'security': 'security / dependencies'}
BASELINE = '078468be14c23bd928e50390f3959144f2921261'
OLD_CI_BLOB = 'e139de17b285ad5c8ea74e97a65569fea71aa61e'
BASELINE_RUNS = (40327592511, 40323961294)
BASELINE_SECURITY = 40327592288
CONTRACT = 1
SEMVER = re.compile(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?')
WORKSPACE_NAMES = {'studyapp-core', 'studyapp-sync', 'studyapp-ui', 'studyapp-desktop'}


def git(*args):
    return subprocess.check_output(['git', *args], stderr=subprocess.DEVNULL)


@cache
def tree_at(sha):
    entries = git('ls-tree', '-rz', sha).split(b'\0')
    blobs = []
    for entry in entries:
        if not entry:
            continue
        meta, name = entry.split(b'\t', 1)
        mode, kind, oid = meta.split()
        if kind != b'blob':
            raise ValueError('Submodules are not supported as verified suite inputs')
        blobs.append((name.decode('utf-8'), oid))
    raw = subprocess.check_output(['git', 'cat-file', '--batch'], input=b'\n'.join(oid for _, oid in blobs)+b'\n')
    stream = io.BytesIO(raw)
    result = {}
    for name, _ in blobs:
        header = stream.readline().split()
        size = int(header[2])
        result[name] = stream.read(size)
        stream.read(1)
    return result


def normalized(path, content):
    if path in ('Cargo.toml', 'Cargo.lock'):
        data = tomllib.loads(content.decode('utf-8'))
        if path == 'Cargo.toml':
            package = data.get('workspace', {}).get('package', {})
            if 'version' in package:
                package['version'] = '<workspace-version>'
        else:
            for package in data.get('package', []):
                if package['name'] in WORKSPACE_NAMES and 'source' not in package:
                    package['version'] = '<workspace-version>'
        return json.dumps(data, sort_keys=True, separators=(',', ':')).encode()
    return content


def suite_input(path, group, migration=False):
    editor = 'crates/studyapp-desktop/editor/'
    common = path in ('Cargo.toml', 'Cargo.lock') or path.startswith(('.cargo/', 'rust-toolchain'))
    orchestration = path in ('.github/workflows/ci.yml', '.github/pipeline-migration.json', 'scripts/pipeline.py', 'scripts/test-pipeline.py')
    if orchestration:
        return not migration
    if group == 'editor':
        return path.startswith(editor) or path == '.github/workflows/editor.yml'
    if group.startswith('core-'):
        return common or path.startswith(('crates/studyapp-core/', 'crates/studyapp-sync/'))
    if group == 'maintenance-native':
        return suite_input(path, 'desktop', migration) or suite_input(path, 'maintenance-policy', migration)
    if group == 'desktop':
        desktop_scripts = ('scripts/run-dev.ps1', 'scripts/smoke.ps1', 'scripts/smoke-settings.ps1',
                           'scripts/smoke-window-drag.ps1', 'scripts/smoke-clipboard.ps1',
                           'scripts/smoke-inputs.ps1', 'scripts/smoke-updates.ps1',
                           'scripts/smoke-note-rename.ps1', 'scripts/maintenance-legacy-native.ps1')
        return common or path.startswith(('crates/', 'vendor/', 'assets/')) or path in desktop_scripts or path == 'run-dev.cmd'
    if group == 'policy':
        return path in ('scripts/test-update-helper.ps1', 'scripts/test-package.ps1', 'scripts/check-package.ps1', 'scripts/package.ps1', 'scripts/windows-runtime.ps1', 'crates/studyapp-desktop/src/update_helper.ps1')
    if group == 'maintenance-policy':
        return path.startswith('installer/') or path.startswith(('scripts/test-installer', 'scripts/test-maintenance')) or path in ('scripts/setup-native-tests.cs', 'scripts/msi-control-native-tests.cs', 'scripts/maintenance-legacy-native.ps1', 'crates/studyapp-desktop/src/maintenance_acceptance.rs', 'crates/studyapp-desktop/src/maintenance_acceptance_process.rs', 'crates/studyapp-desktop/src/maintenance_cancel_acceptance.rs', 'crates/studyapp-desktop/src/maintenance_recovery_acceptance.rs', 'scripts/smoke-maintenance.ps1', 'scripts/installer-policy.ps1', 'scripts/modern-package.ps1', 'scripts/test-modern-package.ps1', 'scripts/build-installers.ps1', 'scripts/check-package.ps1', 'scripts/package.ps1', 'scripts/windows-runtime.ps1')
    if group == 'security':
        return common or path.endswith('Cargo.toml') or path.startswith('vendor/') or path == 'deny.toml' or (path == '.github/workflows/security.yml' and not migration)
    raise ValueError(group)


def fingerprint(tree, group, migration=False):
    digest = hashlib.sha256()
    for path in sorted(tree):
        if suite_input(path, group, migration):
            content = tree[path]
            if migration and path == '.github/workflows/editor.yml':
                content = content.replace(b'name: offline-editor-tests-${{ github.run_attempt }}\n', b'name: offline-editor-tests\n')
            digest.update(path.encode()+b'\0'+hashlib.sha256(normalized(path, content)).digest())
    return digest.hexdigest()


def ancestor(sha):
    return subprocess.run(['git', 'merge-base', '--is-ancestor', sha, os.environ['GITHUB_SHA']],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def trusted(run, repository, is_ancestor=ancestor):
    try:
        return (run['event'] == 'push' and run['conclusion'] == 'success'
                and run['head_repository']['full_name'] == repository
                and (run['head_branch'] == 'main' or run['head_branch'].startswith('codex/'))
                and bool(re.fullmatch('[0-9a-f]{40}', run['head_sha'])) and is_ancestor(run['head_sha']))
    except (KeyError, TypeError, RuntimeError, OSError, subprocess.SubprocessError):
        return False


def trusted_suite_run(run, repository):
    # An unrelated failed job does not erase an independently completed pass.
    # Publication still requires trusted(), i.e. a wholly successful workflow.
    return (run.get('status') == 'completed' and run.get('conclusion') in ('success', 'failure')
            and trusted(dict(run, conclusion='success'), repository))


def live_artifact(artifacts, name):
    attempts = []
    for artifact in artifacts:
        match = re.fullmatch(re.escape(name)+r'-(\d+)', artifact['name'])
        if artifact['name'] == name or match:
            attempts.append((int(match[1]) if match else 0, artifact))
    if not attempts:
        return None
    latest = max(attempt for attempt, _ in attempts)
    found = [artifact for attempt, artifact in attempts if attempt == latest]
    return found[0] if len(found) == 1 and not found[0].get('expired', True) else None


def receipt_matches(receipt, sha, group, fp):
    try:
        proof = receipt['groups'][group]
        return (receipt['contract'] == CONTRACT and receipt['sha'] == sha and proof['fingerprint'] == fp
                and isinstance(proof['run_id'], int) and re.fullmatch('[0-9a-f]{40}', proof['sha']) is not None)
    except (KeyError, TypeError):
        return False


def job_passed(jobs, group, migration=False):
    names = {JOBS[group]}
    if group == 'editor':
        names |= {'editor / editor'}
    if group == 'security':
        names |= {'dependencies'}
    if migration and group == 'policy':
        return job_passed(jobs, 'desktop') and job_passed(jobs, 'core-linux') and job_passed(jobs, 'core-windows')
    return any(j.get('name') in names and j.get('conclusion') == 'success' for j in jobs)


def migration_guard(ci_blob, old_editor, current_editor):
    if ci_blob != OLD_CI_BLOB:
        return False
    if old_editor == current_editor:
        return True
    old_name = b'name: offline-editor-tests\n'
    new_name = b'name: offline-editor-tests-${{ github.run_attempt }}\n'
    return (isinstance(old_editor, bytes) and isinstance(current_editor, bytes)
            and old_editor.count(old_name) == 1 and current_editor.count(new_name) == 1
            and current_editor.replace(new_name, old_name, 1) == old_editor)


def allowed_https(url):
    parsed = urllib.parse.urlparse(url)
    host = parsed.hostname or ''
    return parsed.scheme == 'https' and (host in ('api.github.com', 'github.com', 'uploads.github.com', 'release-assets.githubusercontent.com') or host.endswith('.blob.core.windows.net') or host.endswith('.actions.githubusercontent.com'))


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        if not allowed_https(newurl):
            raise ValueError('Refusing artifact redirect outside approved HTTPS storage hosts')
        redirected = super().redirect_request(request, fp, code, msg, headers, newurl)
        if redirected is not None and urllib.parse.urlparse(request.full_url).hostname != urllib.parse.urlparse(newurl).hostname:
            redirected.remove_header('Authorization')
        return redirected


def http_open(request, timeout=60):
    if not allowed_https(request.full_url):
        raise ValueError('Unapproved GitHub API or artifact host')
    return urllib.request.build_opener(SafeRedirect()).open(request, timeout=timeout)


class GitHub:
    def __init__(self, repository):
        if not re.fullmatch(r'[\w.-]+/[\w.-]+', repository):
            raise ValueError('Invalid repository')
        self.repository = repository
        self.base = 'https://api.github.com/repos/'+repository

    def request(self, path, method='GET', data=None, binary=False):
        url = path if path.startswith('https://') else self.base+path
        headers = {'Authorization': 'Bearer '+os.environ['GH_TOKEN'], 'Accept': 'application/vnd.github+json',
                   'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'StudyApp-pipeline'}
        if data is not None:
            headers['Content-Type'] = 'application/json'
            data = json.dumps(data).encode()
        request = urllib.request.Request(url, data=data, method=method, headers=headers)
        with http_open(request, timeout=60) as response:
            payload = response.read()
        return payload if binary else (json.loads(payload) if payload else None)

    @cache
    def run(self, run_id):
        return self.request(f'/actions/runs/{run_id}')

    @cache
    def jobs(self, run_id):
        # A failed-jobs rerun omits earlier successful jobs from filter=latest.
        # Keep the most recent attempt of each job, never an earlier success
        # when that same job was subsequently retried and failed or skipped.
        attempts = {}
        for job in self.pages(f'/actions/runs/{run_id}/jobs?filter=all', 'jobs'):
            name, attempt = job.get('name'), job.get('run_attempt')
            if not isinstance(name, str) or not name or type(attempt) is not int or attempt < 1:
                raise ValueError('Job attempt metadata unavailable')
            attempts.setdefault(name, {}).setdefault(attempt, []).append(job)
        latest = [versions[max(versions)] for versions in attempts.values()]
        if any(len(jobs) != 1 for jobs in latest):
            raise ValueError('Ambiguous latest job attempt')
        return [jobs[0] for jobs in latest]

    def pages(self, path, key):
        result = []
        for page in range(1, 11):
            data = self.request(path+('&' if '?' in path else '?')+f'per_page=100&page={page}')
            items = data if key is None else data[key]
            result += items
            if len(items) < 100:
                return result
        raise ValueError('API pagination exceeded bounded lookup')

    def artifact(self, run_id, name):
        artifact = live_artifact(self.pages(f'/actions/runs/{run_id}/artifacts', 'artifacts'), name)
        if artifact is None:
            return None
        return self.request(f'/actions/artifacts/{artifact["id"]}/zip', binary=True)

    def receipt(self, run_id):
        raw = self.artifact(run_id, 'ci-receipt')
        if raw is None:
            return None
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            return json.loads(archive.read('ci-receipt.json'))


def migration_contract_valid(tree):
    try:
        snapshot = json.loads(tree['.github/pipeline-migration.json'])
        required = {'.github/workflows/ci.yml', '.github/workflows/editor.yml', '.github/workflows/security.yml', 'scripts/pipeline.py', 'scripts/test-pipeline.py'}
        return set(snapshot) == required and all(hashlib.sha256(tree[path]).hexdigest() == digest for path, digest in snapshot.items())
    except (KeyError, TypeError, ValueError):
        return False


def original_source_valid(api, proof, group, current_tree):
    run = api.run(proof['run_id'])
    if run['head_sha'] != proof['sha'] or not (trusted(run, api.repository) if proof.get('migration') else trusted_suite_run(run, api.repository)):
        return False
    if proof.get('migration'):
        if proof['sha'] != BASELINE or proof['run_id'] not in (*BASELINE_RUNS, BASELINE_SECURITY):
            return False
        if not migration_contract_valid(current_tree):
            return False
        old = tree_at(BASELINE)
        blob = git('rev-parse', BASELINE+':.github/workflows/ci.yml').decode().strip()
        if not migration_guard(blob, old['.github/workflows/editor.yml'], current_tree['.github/workflows/editor.yml']):
            return False
        if fingerprint(old, group, True) != fingerprint(current_tree, group, True):
            return False
    elif fingerprint(tree_at(proof['sha']), group) != fingerprint(current_tree, group):
        return False
    expected_workflow = '.github/workflows/security.yml' if proof.get('migration') and group == 'security' else '.github/workflows/ci.yml'
    return run.get('path') == expected_workflow and job_passed(api.jobs(proof['run_id']), group, proof.get('migration', False))


# This is a single reviewed fixture transition, not a path exclusion. The old
# fingerprint and migration contracts above remain the source-proof contract.
FIXTURE_BRIDGE_ID = 'maintenance-upgrade-fixture-09883d8-v5'
FIXTURE_BRIDGE_ANCHOR = '09883d80a8718a167bf0352e24c7489c35c32002'
FIXTURE_BRIDGE_CARRIER = 40407387092
FIXTURE_BRIDGE_MANIFEST = 'scripts/fixture-proof-bridge.json'
FIXTURE_BRIDGE_PATH = 'crates/studyapp-desktop/src/maintenance_acceptance.rs'
FIXTURE_BRIDGE_HASHES = ('e5ac0ac7e4de1eaef54b5ab57b57a0f516802d2edcb5a281b72b9f7100b01f79',
                         '79646c94719e23aeeaba67a483bebb96716cb32cfc7549de283d90881b9d192e')
FIXTURE_BRIDGE_REPAIR_PATH = 'crates/studyapp-desktop/src/update_repair_acceptance.rs'
FIXTURE_BRIDGE_REPAIR_HASHES = ('9d056e367790c235071375dd009eb3ec6573324da7a1c3b92eb74238766f5c8f',
                               '25cb9638e3a0b9c5e0fe4dfd762cbfce6923840208df75ec201f757cfb568f5d')
FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_PATH = 'scripts/test-maintenance.ps1'
FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_HASHES = ('96774377484ba32415c0836a2c90203935677a11b9a97f407a5a9d153d5d0a09',
                                           '43d9fe79da5b1cbff7bde38ecfef9b4df6a3aa34b2ca9f8f14386a018b1a91b0')
FIXTURE_BRIDGE_MAINTENANCE_TEST_PATH = 'scripts/test-maintenance-sources.py'
FIXTURE_BRIDGE_MAINTENANCE_TEST_HASHES = ('23724b577b7ef6271f6ff975917795ce05e287f91ad51d2c392c13f8262d29c0',
                                         '7c4c74a370c0a5afdb899f60c64ad0278631e2ec0f068a32be056e34b4569d9e')
FIXTURE_BRIDGE_SMOKE_PATH = 'scripts/smoke-maintenance.ps1'
FIXTURE_BRIDGE_SMOKE_HASHES = ('2784ef780820400baf886335bd3bb6245b9f80c2f8aa787d8483348eaf9566e1',
                              '4eb3ac0104e5756a57959c018e7a7774bda318c156bacaa11088c9d384ed603c')
FIXTURE_BRIDGE_POLICY = ('scripts/pipeline.py', 'scripts/test-pipeline.py', 'scripts/ime-ci.py',
                         'scripts/test-fixture-proof-bridge.py')
FIXTURE_BRIDGE_SOURCES = {
    'editor': (40327592511, '078468be14c23bd928e50390f3959144f2921261'),
    'core-linux': (40375676648, '22b458871eb7ad494c99eabe45b7d7c51c86a6e3'),
    'core-windows': (40375676648, '22b458871eb7ad494c99eabe45b7d7c51c86a6e3'),
    'desktop': (40396096491, '839ffd96353f9b36d823a3cddd5351bb5ad8e0fe'),
    'policy': (40375676648, '22b458871eb7ad494c99eabe45b7d7c51c86a6e3'),
    'security': (40396096491, '839ffd96353f9b36d823a3cddd5351bb5ad8e0fe')}
FIXTURE_BRIDGE_IME = (40404385477, '4f76297a9043ffd7335c5144f4ba4c370809d50d')


def bridge_projection(current_tree):
    """Check the fixed reviewed manifest and project only that exact transition.

    The manifest is repository policy (never supplied by a receipt). Its exact
    schema pins the old anchor, five exact fixture transitions and policy files.
    Keeping the final policy hashes outside those files avoids self-hash cycles.
    All other source/recipe/checkout inputs must match the immutable anchor.
    """
    try:
        old = tree_at(FIXTURE_BRIDGE_ANCHOR)
        raw = current_tree[FIXTURE_BRIDGE_MANIFEST]
        manifest = json.loads(raw)
        if (set(manifest) != {'id', 'anchor', 'groups', 'fixture', 'repair_fixture',
                             'maintenance_script_fixture', 'maintenance_source_tests_fixture', 'smoke_fixture', 'policy'}
                or manifest['id'] != FIXTURE_BRIDGE_ID or manifest['anchor'] != FIXTURE_BRIDGE_ANCHOR
                or manifest['groups'] != [*FIXTURE_BRIDGE_SOURCES, 'ime']
                or manifest['fixture'] != dict(zip(('old', 'new'), FIXTURE_BRIDGE_HASHES))
                or manifest['repair_fixture'] != dict(zip(('old', 'new'), FIXTURE_BRIDGE_REPAIR_HASHES))
                or manifest['maintenance_script_fixture'] != dict(zip(('old', 'new'), FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_HASHES))
                or manifest['maintenance_source_tests_fixture'] != dict(zip(('old', 'new'), FIXTURE_BRIDGE_MAINTENANCE_TEST_HASHES))
                or manifest['smoke_fixture'] != dict(zip(('old', 'new'), FIXTURE_BRIDGE_SMOKE_HASHES))
                or set(manifest['policy']) != set(FIXTURE_BRIDGE_POLICY)
                or raw != (json.dumps(manifest, indent=2) + '\n').encode()):
            return None
        digest = lambda content: hashlib.sha256(content).hexdigest() if content is not None else None
        if (digest(old[FIXTURE_BRIDGE_PATH]) != FIXTURE_BRIDGE_HASHES[0]
                or digest(current_tree[FIXTURE_BRIDGE_PATH]) != FIXTURE_BRIDGE_HASHES[1]
                or digest(old[FIXTURE_BRIDGE_REPAIR_PATH]) != FIXTURE_BRIDGE_REPAIR_HASHES[0]
                or digest(current_tree[FIXTURE_BRIDGE_REPAIR_PATH]) != FIXTURE_BRIDGE_REPAIR_HASHES[1]
                or digest(old[FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_PATH]) != FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_HASHES[0]
                or digest(current_tree[FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_PATH]) != FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_HASHES[1]
                or digest(old[FIXTURE_BRIDGE_MAINTENANCE_TEST_PATH]) != FIXTURE_BRIDGE_MAINTENANCE_TEST_HASHES[0]
                or digest(current_tree[FIXTURE_BRIDGE_MAINTENANCE_TEST_PATH]) != FIXTURE_BRIDGE_MAINTENANCE_TEST_HASHES[1]
                or digest(old[FIXTURE_BRIDGE_SMOKE_PATH]) != FIXTURE_BRIDGE_SMOKE_HASHES[0]
                or digest(current_tree[FIXTURE_BRIDGE_SMOKE_PATH]) != FIXTURE_BRIDGE_SMOKE_HASHES[1]):
            return None
        for path, hashes in manifest['policy'].items():
            if (set(hashes) != {'old', 'new'} or hashes['old'] != digest(old.get(path))
                    or not isinstance(hashes['new'], str) or re.fullmatch('[0-9a-f]{64}', hashes['new']) is None
                    or hashes['new'] != digest(current_tree[path])):
                return None
        exceptions = {FIXTURE_BRIDGE_PATH, FIXTURE_BRIDGE_REPAIR_PATH, FIXTURE_BRIDGE_MAINTENANCE_SCRIPT_PATH,
                      FIXTURE_BRIDGE_MAINTENANCE_TEST_PATH, FIXTURE_BRIDGE_SMOKE_PATH, FIXTURE_BRIDGE_MANIFEST,
                      *FIXTURE_BRIDGE_POLICY}
        for path in set(old) | set(current_tree):
            if path in exceptions or path.startswith('docs/'):
                continue
            if path not in old or path not in current_tree:
                return None
            expected = old[path]
            if path in ('Cargo.toml', 'Cargo.lock'):
                # Allow the existing release-only workspace version change, but
                # do not silently normalize additional lockfile bytes/comments.
                previous, current = version_at(old), version_at(current_tree)
                if SEMVER.fullmatch(current) is None:
                    return None
                if path == 'Cargo.toml':
                    expected = expected.replace(b'[workspace.package]\nversion = "' + previous.encode() + b'"',
                                                b'[workspace.package]\nversion = "' + current.encode() + b'"', 1)
                else:
                    for name in WORKSPACE_NAMES:
                        prefix = ('name = "' + name + '"\nversion = "').encode()
                        expected = expected.replace(prefix + previous.encode() + b'"', prefix + current.encode() + b'"', 1)
            if current_tree[path] != expected:
                return None
        projected = dict(current_tree)
        # The new files did not exist at the anchor, and are not build inputs.
        for path in exceptions:
            if path in old:
                projected[path] = old[path]
            else:
                projected.pop(path, None)
        return projected
    except (KeyError, TypeError, ValueError, AttributeError, OSError, subprocess.SubprocessError):
        return None


def bridge_proof(api, proof, group, current_tree):
    """Return an explicitly bound reuse envelope after revalidating the old proof."""
    try:
        if group not in FIXTURE_BRIDGE_SOURCES:
            return None  # In particular neither maintenance group can use this bridge.
        original = proof.get('bridge', {}).get('original', proof)
        expected_run, expected_sha = FIXTURE_BRIDGE_SOURCES[group]
        if (set(original) != ({'fingerprint', 'run_id', 'sha', 'migration'} if group == 'editor'
                             else {'fingerprint', 'run_id', 'sha'})
                or (original['run_id'], original['sha']) != (expected_run, expected_sha)
                or (group == 'editor' and original['migration'] is not True)
                or not ancestor(FIXTURE_BRIDGE_ANCHOR)):
            return None
        anchor = tree_at(FIXTURE_BRIDGE_ANCHOR)
        if original['fingerprint'] != fingerprint(anchor, group):
            return None
        # Validate the immutable original contract at its frozen current snapshot.
        # This deliberately includes the old editor migration manifest unchanged.
        if not original_source_valid(api, original, group, anchor):
            return None
        carrier = api.run(FIXTURE_BRIDGE_CARRIER)
        if (carrier.get('head_sha') != FIXTURE_BRIDGE_ANCHOR or not trusted_suite_run(carrier, api.repository)
                or carrier.get('path') != '.github/workflows/ci.yml'):
            return None
        receipt = api.receipt(FIXTURE_BRIDGE_CARRIER)
        if (receipt is None or receipt.get('contract') != CONTRACT or receipt.get('sha') != FIXTURE_BRIDGE_ANCHOR
                or receipt.get('groups', {}).get(group) != original):
            return None
        projected = bridge_projection(current_tree)
        if projected is None or fingerprint(projected, group) != fingerprint(anchor, group):
            return None
        target = fingerprint(current_tree, group)
        result = dict(original, fingerprint=target, bridge={
            'id': FIXTURE_BRIDGE_ID, 'anchor': FIXTURE_BRIDGE_ANCHOR,
            'original': dict(original), 'target_fingerprint': target})
        if 'bridge' in proof and proof != result:
            return None  # No chaining, rewritten source identity or editable target binding.
        return result
    except (KeyError, TypeError, ValueError, AttributeError, OSError, subprocess.SubprocessError):
        return None


def source_valid(api, proof, group, current_tree):
    if 'pending_delta' in proof or 'production_delta' in proof:
        return production_delta.source_valid(api, proof, group, current_tree)
    if 'bridge' in proof:
        return bridge_proof(api, proof, group, current_tree) is not None
    return original_source_valid(api, proof, group, current_tree)


def make_plan(api, sha, force=False):
    tree = tree_at(sha)
    plan = {'contract': CONTRACT, 'sha': sha, 'groups': {}, 'package_run': None, 'diagnostics': []}
    for group in GROUPS:
        plan['groups'][group] = {'fingerprint': fingerprint(tree, group), 'run': True}
    if not force:
        try:
            runs = api.request('/actions/workflows/ci.yml/runs?event=push&status=completed&per_page=50')['workflow_runs']
            for run in runs:
                if not trusted_suite_run(run, api.repository):
                    continue
                try:
                    receipt = api.receipt(run['id'])
                    if receipt is None:
                        plan['diagnostics'].append(f'Run {run["id"]}: no live verification receipt; no implicit test success')
                        continue
                    for group in GROUPS:
                        state = plan['groups'][group]
                        if not state['run']:
                            continue
                        if receipt_matches(receipt, run['head_sha'], group, state['fingerprint']):
                            proof = receipt['groups'][group]
                            if source_valid(api, proof, group, tree):
                                state.update(proof, run=False)
                                continue
                        # The ordinary fast filter cannot match an old fingerprint.
                        # Still require its original receipt schema before bridging.
                        proof = receipt.get('groups', {}).get(group, {})
                        if receipt_matches(receipt, run['head_sha'], group, proof.get('fingerprint')):
                            bridged = bridge_proof(api, proof, group, tree)
                            if bridged is not None:
                                state.update(bridged, run=False)
                    if run['head_sha'] == sha and plan['package_run'] is None and trusted(run, api.repository):
                        raw = api.artifact(run['id'], 'windows-release')
                        if raw is not None:
                            verify_archive(raw, version_at(tree), sha, api)
                            plan['package_run'] = run['id']
                        else:
                            plan['diagnostics'].append(f'Run {run["id"]}: checked package artifact missing or expired; package rebuild required')
                except (ValueError, KeyError, OSError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
                    plan['diagnostics'].append(f'Run {run["id"]}: receipt/package unavailable or invalid ({type(error).__name__}); no implicit success')
                    continue
            # One-time transition: exact pinned old CI and actual successful jobs, unchanged suite inputs.
            for group in GROUPS:
                state = plan['groups'][group]
                if not state['run']:
                    continue
                for run_id in ((BASELINE_SECURITY,) if group == 'security' else BASELINE_RUNS):
                    proof = {'run_id': run_id, 'sha': BASELINE, 'migration': True}
                    try:
                        if source_valid(api, proof, group, tree):
                            state.update(proof, run=False)
                            break
                    except (ValueError, KeyError, OSError, subprocess.SubprocessError):
                        continue
        except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
            # Missing API/history never proves a test passed. Already proven receipts remain valid.
            plan['diagnostics'].append(f'Candidate lookup unavailable ({type(error).__name__}); unproven groups run normally')
    if not force:
        try:
            production_delta.extend_plan(api, plan, tree)
        except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError):
            plan['diagnostics'].append('Production delta unavailable; no unverified coverage completed')
    return plan


def version_at(tree):
    return tomllib.loads(tree['Cargo.toml'].decode())['workspace']['package']['version']


def check_tag(tag, version):
    if SEMVER.fullmatch(version) is None or tag != 'v'+version:
        raise ValueError('Release tag must exactly equal the workspace SemVer prefixed with v')


def asset_names(version):
    parsed = SEMVER.fullmatch(version)
    if parsed is None:
        raise ValueError('Invalid package version')
    stem = f'WW-StudyApp-{version}-windows-x86_64'
    suffixes = ('.zip', '-portable.zip', '.msi', '-Setup.exe') if tuple(map(int, parsed.groups()[:3])) >= (0, 8, 0) else ('.zip',)
    return tuple(name for suffix in suffixes for name in (stem+suffix, stem+suffix+'.sha256'))


def make_manifest(directory, version, sha, run_id, repository):
    directory = Path(directory)
    expected = set(asset_names(version))
    actual = {p.name for p in directory.iterdir() if p.is_file()} - {'release-manifest.json'}
    if actual != expected:
        raise ValueError('Expected only explicitly supported release assets')
    assets = {name: hashlib.sha256((directory/name).read_bytes()).hexdigest() for name in sorted(expected)}
    for name in asset_names(version)[::2]:
        if (directory/(name+'.sha256')).read_text(encoding='utf-8').strip() != assets[name]+'  '+name:
            raise ValueError('Package checksum does not match its payload')
    return {'contract': CONTRACT, 'repository': repository, 'sha': sha, 'version': version,
            'run_id': int(run_id), 'assets': assets}


def verify_manifest(directory, manifest, version, sha, repository, run_id):
    expected = make_manifest(directory, version, sha, run_id, repository)
    if manifest != expected:
        raise ValueError('Release manifest provenance or asset hashes do not match')


def unpack_release(raw, destination):
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        for item in archive.infolist():
            if item.filename != Path(item.filename).name or item.is_dir():
                raise ValueError('Unexpected nested artifact path')
            (destination/item.filename).write_bytes(archive.read(item))


def verify_archive(raw, version, sha, api):
    # In-memory verification never extracts an untrusted path.
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)) or set(names) != {*asset_names(version), 'release-manifest.json'}:
            raise ValueError('Unexpected release artifact members')
        manifest = json.loads(archive.read('release-manifest.json'))
        if any(manifest.get(k) != v for k, v in {'contract': CONTRACT, 'sha': sha, 'version': version, 'repository': api.repository}.items()):
            raise ValueError('Release artifact provenance mismatch')
        origin = api.run(manifest['run_id'])
        if origin['head_sha'] != sha or not trusted(origin, api.repository) or origin.get('path') != '.github/workflows/ci.yml':
            raise ValueError('Package was not produced by successful trusted exact-SHA CI')
        if not any(j['name'] == 'Checked Windows release package' and j['conclusion'] == 'success' for j in api.jobs(origin['id'])):
            raise ValueError('Package build job was not successful')
        hashes = {name: hashlib.sha256(archive.read(name)).hexdigest() for name in asset_names(version)}
        if manifest.get('assets') != hashes:
            raise ValueError('Release asset checksum mismatch')
        for name in asset_names(version)[::2]:
            if archive.read(name+'.sha256').decode().strip() != hashes[name]+'  '+name:
                raise ValueError('Package checksum file mismatch')
        return manifest


def publish_delta(expected, existing, draft):
    if set(existing)-set(expected) or any(expected[name] != value for name, value in existing.items()):
        raise ValueError('Existing release assets differ; no asset will be overwritten')
    missing = sorted(set(expected)-set(existing))
    if missing and not draft:
        raise ValueError('Published release is incomplete; refusing to mutate it')
    return missing


def publish(api, directory, tag, manifest):
    directory = Path(directory)
    version = manifest['version']
    check_tag(tag, version)
    verify_manifest(directory, manifest, version, os.environ['GITHUB_SHA'], api.repository, manifest['run_id'])
    # Tag lookup can omit drafts. Authenticated listing includes drafts and supports interrupted-upload recovery.
    matches = [release for release in api.pages('/releases', None) if release['tag_name'] == tag]
    if len(matches) > 1:
        raise ValueError('Ambiguous existing releases for this tag; refusing to choose or overwrite one')
    if matches:
        release = matches[0]
    else:
        # Tag must preexist and resolve to the checked SHA; the workflow checkout verifies it too.
        release = api.request('/releases', 'POST', {'tag_name': tag, 'name': 'WW-StudyApp '+tag,
                              'draft': True, 'body': Path('docs/release-notes.md').read_text(encoding='utf-8')})
    def assets_now():
        assets = api.request(f'/releases/{release["id"]}/assets?per_page=100')
        if len(assets) >= 100:
            raise ValueError('Unexpected release asset count')
        existing = {}
        for asset in assets:
            if asset['name'] in existing:
                raise ValueError('Duplicate release asset name')
            payload = download_asset(asset['id'])
            existing[asset['name']] = hashlib.sha256(payload).hexdigest()
        return existing
    def download_asset(asset_id):
        # Asset bytes use the dedicated octet-stream endpoint, never browser HTML.
        request = urllib.request.Request(api.base+f'/releases/assets/{asset_id}', headers={
            'Authorization': 'Bearer '+os.environ['GH_TOKEN'], 'Accept': 'application/octet-stream',
            'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'StudyApp-pipeline'})
        with http_open(request, timeout=60) as response:
            return response.read()
    missing = publish_delta(manifest['assets'], assets_now(), release['draft'])
    for name in missing:
        upload = release['upload_url'].split('{', 1)[0]+'?name='+urllib.parse.quote(name)
        request = urllib.request.Request(upload, data=(directory/name).read_bytes(), method='POST', headers={
            'Authorization': 'Bearer '+os.environ['GH_TOKEN'], 'Content-Type': 'application/octet-stream',
            'User-Agent': 'StudyApp-pipeline'})
        with http_open(request, timeout=120) as response:
            response.read()
    if publish_delta(manifest['assets'], assets_now(), release['draft']):
        raise ValueError('Release upload incomplete')
    if release['draft']:
        api.request(f'/releases/{release["id"]}', 'PATCH', {'draft': False})
    print('Verified complete release: '+tag)


def output(key, value):
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as stream:
        stream.write(f'{key}={value}\n')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=('plan', 'receipt', 'manifest', 'promote', 'publish'))
    parser.add_argument('--force-all', default='false')
    parser.add_argument('--directory', default='dist')
    parser.add_argument('--tag')
    args = parser.parse_args()
    sha = os.environ['GITHUB_SHA']
    repository = os.environ['GITHUB_REPOSITORY']
    api = GitHub(repository)
    directory = Path(args.directory)
    if args.command == 'plan':
        plan = make_plan(api, sha, args.force_all == 'true' or os.environ['GITHUB_EVENT_NAME'] == 'pull_request')
        Path('ci-plan.json').write_text(json.dumps(plan, indent=2)+'\n')
        for group in GROUPS:
            output(group, str(plan['groups'][group]['run']).lower())
        output('desktop_delta', str('pending_delta' in plan['groups']['desktop']).lower())
        output('package_run', plan['package_run'] or '')
        output('plan', json.dumps(plan, separators=(',', ':')))
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a', encoding='utf-8') as stream:
            stream.write('| Group | Decision | Executed source |\n|---|---|---|\n')
            for group, state in plan['groups'].items():
                pending = state.get('pending_delta')
                source = pending['original'] if pending else state
                decision = 'pending production delta' if pending else 'run' if state['run'] else 'reuse'
                if state.get('production_delta', {}).get('kind') == 'desktop-composite': decision = 'composite coverage'
                stream.write(f'| {group} | {decision} | {source.get("run_id", "this run")} / {source.get("sha", sha)} |\n')
            for message in plan['diagnostics'][:10]:
                stream.write('\n- '+message+'\n')
            stream.write(f'\nPackage: {"promote run "+str(plan["package_run"]) if plan["package_run"] else "build normal release binary and check package"}\n')
    elif args.command == 'receipt':
        plan = json.loads(os.environ['CI_PLAN'])
        receipt = {'contract': CONTRACT, 'sha': sha, 'groups': {}}
        for group, state in plan['groups'].items():
            proof = {k: v for k, v in state.items() if k != 'run'}
            if state['run']:
                proof.update(run_id=int(os.environ['GITHUB_RUN_ID']), sha=sha)
            elif 'pending_delta' in proof:
                proof = production_delta.finish_pending(api, proof, tree_at(sha), sha, int(os.environ['GITHUB_RUN_ID']))
            receipt['groups'][group] = proof
        Path('ci-receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
        if any('pending_delta' in proof for proof in receipt['groups'].values()):
            raise ValueError('Production delta remains pending; current regressions/native evidence did not verify')
    elif args.command == 'manifest':
        manifest = make_manifest(directory, version_at(tree_at(sha)), sha, int(os.environ['GITHUB_RUN_ID']), repository)
        (directory/'release-manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
    elif args.command == 'promote':
        version = version_at(tree_at(sha))
        if args.tag:
            check_tag(args.tag, version)
            if git('rev-parse', args.tag+'^{commit}').decode().strip() != sha:
                raise ValueError('Tag does not identify the checked source SHA')
        run_id = os.environ.get('PACKAGE_RUN')
        runs = ([api.run(int(run_id))] if run_id else api.request('/actions/workflows/ci.yml/runs?event=push&status=success&head_sha='+sha+'&per_page=50')['workflow_runs'])
        for run in runs:
            if run['head_sha'] != sha or not trusted(run, repository) or run.get('path') != '.github/workflows/ci.yml':
                continue
            receipt = api.receipt(run['id'])
            tree = tree_at(sha)
            if receipt is None or not all(receipt_matches(receipt, sha, group, fingerprint(tree, group)) and source_valid(api, receipt['groups'][group], group, tree) for group in GROUPS):
                continue
            raw = api.artifact(run['id'], 'windows-release')
            if raw is None:
                continue
            verify_archive(raw, version, sha, api)
            directory.mkdir(parents=True, exist_ok=True)
            unpack_release(raw, directory)
            print(f'Promoted verified exact-SHA package from CI run {run["id"]}')
            return
        raise ValueError('No complete verified exact-SHA CI artifact; run CI first, then retry promotion')
    elif args.command == 'publish':
        manifest = json.loads((directory/'release-manifest.json').read_text())
        publish(api, directory, args.tag, manifest)


if __name__ == '__main__':
    main()
