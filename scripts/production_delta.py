"""One reviewed MSI production change: unaffected applicability and completed composite coverage."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request
import zipfile

import pipeline as p

ID = 'msi-owner-production-delta-v1'
BASIS = 'd64797551f2a8af0f7827b007b570d8b6838d83c'
MANIFEST = 'scripts/msi-owner-production-delta.json'
PRODUCTION = {
    'execute': ('crates/studyapp-desktop/src/maintenance_execute.rs',
                'be5d005ebadf0b16bcd775e706e66c15d5cf97242930059ad6f9ab5dd08ce1be',
                'f98e81a43c66102ad87ed403f20ece8bc79d2568670fa3b80d80989e3e122f77'),
    'msi': ('crates/studyapp-desktop/src/maintenance_msi.rs',
            'da4fc29347a426643f473e93053eddc2a6f254da18cf32ed76da8936dd476c52',
            'a0aeb9b7c1a4b465214935afc72c2fe618a8cbdc86f6f2b358a713696af1c8ad'),
    'uninstall': ('crates/studyapp-desktop/src/maintenance_uninstall.rs',
                  '0cb9f77595e51c95e361bdd00dddde0efdeaf93cf3dd8164f691d299d9480159',
                  'c1138bd1cdfe36f2a3a83e37007794a235dd8f5babc0b78f2b35b7371f9f3755')}
POLICY = ('.github/workflows/ci.yml', 'scripts/pipeline.py', 'scripts/ime-ci.py',
          'scripts/production_delta.py', 'scripts/test-production-delta.py', 'scripts/test-msi-owner-delta.py',
          'scripts/installer-policy.ps1', 'scripts/test-maintenance.ps1', 'scripts/smoke-maintenance.ps1',
          'scripts/test-maintenance-sources.py')
UNAFFECTED = ('editor', 'core-linux', 'core-windows', 'policy', 'security')
COVERAGE = {
    'unaffected_groups': list(UNAFFECTED),
    'ime': 'Original ordinary Pinyin composition and candidate/Enter behavior; MSI ownership transition is outside this coverage.',
    'desktop': 'Historical desktop coverage except changed MSI target binding and completed-control cleanup; compose only with seven current regressions and full current maintenance-native.'}
PACKAGE_JOB = 'Checked Windows release package'
NATIVE_JOB = 'Windows installer and maintenance lifecycle'
TEST_STEP = 'Verify exact MSI owner transition regressions'
TEST_UPLOAD = 'Preserve exact MSI owner regression evidence'
NATIVE_STEP = 'Verify authentic maintenance, installer and confirmation lifecycle'
NATIVE_UPLOAD = 'Preserve authentic maintenance evidence'
BINARY_BUILD = 'Build validation harness only when previous desktop proofs were reused'
BINARY_UPLOAD = 'Preserve newly compiled maintenance validation binary'
NATIVE_REQUIRED = ('recovery-missing-runtime', 'recovery-mixed-runtime', 'installer-cancel', 'installer-running-app',
    'locked-running-app', 'actual-storage-ready-health', 'failed-transaction-rollback', 'reboot-3010',
    'uninstall-default-retain', 'uninstall-config-only', 'uninstall-db-only', 'uninstall-both', 'cancel-selection',
    'cancel-database-review', 'cancel-database-final', 'uninstall-corrupt-db', 'uninstall-missing-db',
    'modified-and-unknown-adjacent', 'tracked-original-inside-and-outside', 'shared-library-lock',
    'separate-installed-and-portable', 'maintenance-without-webview2', 'staged-root-handles-released',
    'legacy-schema1-accepts-legacy-rejects-modern', 'legacy-skip-release', 'same-version-repair',
    'unknown-uninstaller-conflict', 'offline-repair-retry')

_spec = importlib.util.spec_from_file_location('msi_owner_exact_runner', Path(__file__).with_name('test-msi-owner-delta.py'))
runner = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(runner)


def digest(data):
    return hashlib.sha256(data).hexdigest() if data is not None else None


def target_sha(tree):
    sha = os.environ.get('GITHUB_SHA', '')
    if re.fullmatch('[0-9a-f]{40}', sha) is None:
        raise ValueError('Target tree/SHA or fixed basis ancestry mismatch')
    try:
        matches = p.tree_at(sha) == tree and p.ancestor(BASIS)
    except (OSError, subprocess.SubprocessError):
        matches = False
    if not matches:
        raise ValueError('Target tree/SHA or fixed basis ancestry mismatch')
    return sha


def scope_tree(tree):
    """Map only exact reviewed production/policy bytes to the fixed v5 basis.

    This is a coverage-impact mapping, never an executable compatibility tree.
    Original v5 projection then verifies all remaining files and fixture policy.
    """
    try:
        if not p.ancestor(BASIS): return None
        base = p.tree_at(BASIS)
        raw = tree[MANIFEST]
        data = json.loads(raw)
        if (set(data) != {'contract', 'id', 'basis', 'production', 'policy', 'coverage'}
                or type(data['contract']) is not int or data['contract'] != 1 or data['id'] != ID or data['basis'] != BASIS
                or data['coverage'] != COVERAGE or set(data['production']) != set(PRODUCTION)
                or set(data['policy']) != set(POLICY) or raw != (json.dumps(data, indent=2) + '\n').encode()):
            return None
        result = dict(tree)
        for key, (path, before, after) in PRODUCTION.items():
            if (data['production'][key] != {'old': before, 'new': after}
                    or digest(base[path]) != before or digest(tree[path]) != after):
                return None
            result[path] = base[path]
        for path in POLICY:
            record = data['policy'][path]
            if (set(record) != {'old', 'new'} or record['old'] != digest(base.get(path))
                    or not isinstance(record['new'], str) or re.fullmatch('[0-9a-f]{64}', record['new']) is None
                    or digest(tree[path]) != record['new']):
                return None
            if path in base: result[path] = base[path]
            else: result.pop(path, None)
        result.pop(MANIFEST)
        return result if p.bridge_projection(result) is not None else None
    except (KeyError, TypeError, ValueError, AttributeError, OSError, subprocess.SubprocessError):
        return None


def original_valid(api, original, group):
    # Use the unchanged v5 validator at its exact frozen current snapshot.
    required = {'fingerprint', 'sha', 'run_id', *({'migration'} if group == 'editor' else set())}
    if group not in p.FIXTURE_BRIDGE_SOURCES or not isinstance(original, dict) or set(original) != required:
        return False
    return p.bridge_proof(api, original, group, p.tree_at(BASIS)) is not None


def metadata(kind, original):
    return {'id': ID, 'basis': BASIS, 'kind': kind, 'original': original,
            'coverage': COVERAGE['desktop'] if kind.startswith('desktop-') else {'unaffected_groups': list(UNAFFECTED)}}


def unaffected_proof(api, original, group, tree):
    if group not in UNAFFECTED or scope_tree(tree) is None or not original_valid(api, original, group):
        return None
    details = metadata('unaffected', original)
    details['target_sha'] = target_sha(tree)
    return dict(original, fingerprint=p.fingerprint(tree, group), production_delta=details)


def pending_proof(api, original, tree):
    if scope_tree(tree) is None or not original_valid(api, original, 'desktop'):
        return None
    details = metadata('desktop-pending', original)
    details['target_sha'] = target_sha(tree)
    return {'fingerprint': p.fingerprint(tree, 'desktop'), 'pending_delta': details}


def trusted_producer(api, run, sha, complete):
    return (run.get('head_sha') == sha and run.get('path') == '.github/workflows/ci.yml'
            and type(run.get('run_attempt')) is int and run['run_attempt'] > 0
            and p.trusted(dict(run, conclusion='success'), api.repository)
            and ((run.get('status') == 'completed' and run.get('conclusion') == 'success') if complete else
                 (run.get('status') in ('in_progress', 'completed') and run.get('conclusion') in (None, 'success'))))


def completed_job(jobs, name, run_attempt, steps):
    found = [job for job in jobs if job.get('name') == name]
    if not found or any(type(job.get('run_attempt')) is not int or job['run_attempt'] < 1 for job in found):
        raise ValueError('Missing actual job attempts')
    latest = max(job['run_attempt'] for job in found)
    found = [job for job in found if job['run_attempt'] == latest]
    if len(found) != 1:
        raise ValueError('Ambiguous latest job')
    job = found[0]
    if (latest > run_attempt or job.get('status') != 'completed' or job.get('conclusion') != 'success'
            or type(job.get('id')) is not int or job['id'] < 1):
        raise ValueError('Latest actual job did not succeed')
    for name in steps:
        selected = [step for step in job.get('steps', []) if step.get('name') == name]
        if len(selected) != 1 or selected[0].get('status') != 'completed' or selected[0].get('conclusion') != 'success':
            raise ValueError('Required actual step did not execute successfully: ' + name)
    return {'id': job['id'], 'attempt': latest}


def live_record(artifacts, stem, attempt, limit):
    artifact = p.live_artifact(artifacts, stem)
    if (artifact is None or artifact.get('expired') is not False or artifact.get('name') != f'{stem}-{attempt}'
            or type(artifact.get('id')) is not int or artifact['id'] < 1
            or type(artifact.get('size_in_bytes')) is not int or not 0 < artifact['size_in_bytes'] <= limit
            or not isinstance(artifact.get('digest'), str) or re.fullmatch('sha256:[0-9a-f]{64}', artifact['digest']) is None):
        raise ValueError('Missing, expired or unbound supplemental artifact')
    return {key: artifact[key] for key in ('id', 'name', 'digest', 'size_in_bytes')}


def validate_evidence(data, sha, run_id, attempt, tree):
    if (set(data) != {'contract', 'sha', 'run_id', 'attempt', 'version', 'msi_sha256', 'inventory', 'cases', 'input_fingerprint', 'test_executable_sha256'}
            or type(data['contract']) is not int or data['contract'] != 1
            or type(data['run_id']) is not int or data['run_id'] != run_id
            or type(data['attempt']) is not int or data['attempt'] != attempt or data['sha'] != sha
            or data['version'] != p.version_at(tree) or re.fullmatch('[0-9a-f]{64}', data['msi_sha256']) is None
            or data['inventory'] != list(runner.EXPECTED) or len(data['cases']) != len(runner.EXPECTED)
            or data['input_fingerprint'] != p.fingerprint(tree, 'desktop')
            or re.fullmatch('[0-9a-f]{64}', data['test_executable_sha256']) is None):
        raise ValueError('Supplemental evidence source or nonempty inventory mismatch')
    for expected, case in zip(runner.EXPECTED, data['cases']):
        if (set(case) != {'name', 'passed', 'stdout_sha256'} or case['name'] != expected
                or type(case['passed']) is not int or case['passed'] != 1
                or re.fullmatch('[0-9a-f]{64}', case['stdout_sha256']) is None):
            raise ValueError('Supplemental case did not pass exactly once')


def fetch_bytes(api, record, limit):
    request = urllib.request.Request(api.base + f'/actions/artifacts/{record["id"]}/zip', headers={
        'Authorization': 'Bearer ' + os.environ['GH_TOKEN'], 'Accept': 'application/vnd.github+json',
        'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'StudyApp-production-delta'})
    with p.http_open(request, timeout=60) as response:
        return response.read(limit + 1)


def checked_archive(api, record, limit):
    raw = fetch_bytes(api, record, limit)
    if len(raw) > limit or 'sha256:' + digest(raw) != record['digest']:
        raise ValueError('Supplemental ZIP size/digest mismatch')
    return zipfile.ZipFile(io.BytesIO(raw))


def validate_native(data, sha, version, msi_hash):
    if (type(data.get('schema')) is not int or data['schema'] != 1 or data.get('source_sha') != sha
            or data.get('version') != version or data.get('msi_sha256') != msi_hash
            or not isinstance(data.get('scenarios'), list)):
        raise ValueError('Native acceptance source/version/MSI binding mismatch')
    for name in NATIVE_REQUIRED:
        records = [record for record in data['scenarios'] if record.get('scenario') == name]
        if len(records) != 1 or records[0].get('passed') is not True or records[0].get('executor') != 'real':
            raise ValueError('Native required scenario did not actually pass exactly once: ' + name)


def collect(api, sha, run_id, tree, complete=True):
    if scope_tree(tree) is None or sha != target_sha(tree) or type(run_id) is not int or run_id < 1:
        raise ValueError('Unreviewed production inputs')
    run = api.run(run_id)
    if run.get('id') != run_id or not trusted_producer(api, run, sha, complete):
        raise ValueError('Supplemental producer is not an exact-SHA trusted successful workflow')
    jobs = api.jobs(run_id)
    package = completed_job(jobs, PACKAGE_JOB, run['run_attempt'], (TEST_STEP, TEST_UPLOAD, BINARY_BUILD, BINARY_UPLOAD))
    native = completed_job(jobs, NATIVE_JOB, run['run_attempt'], (NATIVE_STEP, NATIVE_UPLOAD))
    artifacts = api.pages(f'/actions/runs/{run_id}/artifacts', 'artifacts')
    evidence = live_record(artifacts, 'msi-owner-delta', package['attempt'], 1024 * 1024)
    binary = live_record(artifacts, 'maintenance-validation-binary', package['attempt'], 512 * 1024 * 1024)
    native_evidence = live_record(artifacts, 'windows-maintenance-native', native['attempt'], 256 * 1024 * 1024)
    with checked_archive(api, evidence, 1024 * 1024) as archive:
        infos = archive.infolist()
        if (len(infos) != 1 or infos[0].filename != 'msi-owner-delta.json' or not 0 < infos[0].file_size <= 65536
                or infos[0].flag_bits & 1 or infos[0].compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)):
            raise ValueError('Unexpected supplemental ZIP contents')
        data = json.loads(archive.read(infos[0]))
    validate_evidence(data, sha, run_id, package['attempt'], tree)
    with checked_archive(api, native_evidence, 256 * 1024 * 1024) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        found = [info for info in infos if info.filename.split('/')[-1] == 'maintenance-native-acceptance.json']
        if (len(infos) > 4096 or len(set(names)) != len(names) or len(found) != 1
                or any(name.startswith(('/', '\\')) or '..' in name.split('/') or ':' in name for name in names)
                or not 0 < found[0].file_size <= 4 * 1024 * 1024 or found[0].flag_bits & 1
                or found[0].compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)):
            raise ValueError('Native acceptance ZIP missing/ambiguous/unsafe aggregate')
        native_data = json.loads(archive.read(found[0]))
    validate_native(native_data, sha, data['version'], data['msi_sha256'])
    return {'sha': sha, 'run_id': run_id, 'package_job': package, 'native_job': native,
            'regressions': evidence, 'native_evidence': native_evidence, 'binary': binary,
            'inventory': list(runner.EXPECTED)}


def composite(api, original, tree, sha, run_id, complete=True):
    if not original_valid(api, original, 'desktop'):
        raise ValueError('Original desktop evidence no longer valid')
    details = metadata('desktop-composite', original)
    details['obligations'] = collect(api, sha, run_id, tree, complete)
    return {'fingerprint': p.fingerprint(tree, 'desktop'), 'sha': sha, 'run_id': run_id, 'production_delta': details}


def source_valid(api, proof, group, tree):
    try:
        if 'pending_delta' in proof:
            return False
        info = proof['production_delta']
        if info.get('id') != ID or info.get('basis') != BASIS:
            return False
        if group in UNAFFECTED:
            return unaffected_proof(api, info['original'], group, tree) == proof
        if group == 'desktop' and info.get('kind') == 'desktop-composite':
            return composite(api, info['original'], tree, proof['sha'], proof['run_id']) == proof
        return False
    except (KeyError, TypeError, ValueError, AttributeError, OSError, zipfile.BadZipFile, subprocess.SubprocessError):
        return False


def extend_plan(api, plan, tree):
    if scope_tree(tree) is None:
        return
    receipt = api.receipt(p.FIXTURE_BRIDGE_CARRIER)
    if receipt is None:
        return
    for group in UNAFFECTED:
        if plan['groups'][group]['run']:
            proof = unaffected_proof(api, receipt.get('groups', {}).get(group, {}), group, tree)
            if proof is not None: plan['groups'][group].update(proof, run=False)
    if plan['groups']['desktop']['run']:
        proof = pending_proof(api, receipt.get('groups', {}).get('desktop', {}), tree)
        if proof is not None:
            plan['groups']['desktop'].update(proof, run=False)
            for group in ('maintenance-policy', 'maintenance-native'):
                plan['groups'][group] = {'fingerprint': p.fingerprint(tree, group), 'run': True}
            plan['package_run'] = None


def finish_pending(api, proof, tree, sha, run_id):
    try:
        pending = proof['pending_delta']
        expected = metadata('desktop-pending', pending['original'])
        expected['target_sha'] = target_sha(tree)
        if proof != {'fingerprint': p.fingerprint(tree, 'desktop'), 'pending_delta': expected}:
            return proof
        return composite(api, pending['original'], tree, sha, run_id, complete=False)
    except (KeyError, TypeError, ValueError, AttributeError, OSError, zipfile.BadZipFile, subprocess.SubprocessError):
        return proof


def ime_metadata(proof, tree, fingerprint, fixture_metadata):
    try:
        restored = scope_tree(tree)
        if restored is None or fixture_metadata(proof, restored) is None:
            return None
        return {'id': ID, 'basis': BASIS, 'target_sha': target_sha(tree), 'coverage': COVERAGE['ime'], 'sha': proof['sha'], 'run_id': proof['run_id'],
                'run_attempt': proof['run_attempt'], 'original_fingerprint': proof['fingerprint'], 'target_fingerprint': fingerprint(tree)}
    except (KeyError, TypeError, ValueError, AttributeError, OSError, subprocess.SubprocessError):
        return None


def release_ready(api, sha, tree):
    runs = api.request('/actions/workflows/ci.yml/runs?event=push&status=success&head_sha=' + sha + '&per_page=50')['workflow_runs']
    for run in runs:
        if not trusted_producer(api, run, sha, True): continue
        receipt = api.receipt(run['id'])
        if receipt is not None and all(p.receipt_matches(receipt, sha, group, p.fingerprint(tree, group))
                                       and p.source_valid(api, receipt['groups'][group], group, tree) for group in p.GROUPS):
            return True
    return False
