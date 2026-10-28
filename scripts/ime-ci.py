"""Native Pinyin evidence, verified binary reuse, and release acceptance (stdlib)."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
import urllib.request
import zipfile

import pipeline
import production_delta

CONTRACT = 2
MAIN_WORKFLOW = '.github/workflows/ci.yml'
WORKFLOW = '.github/workflows/ime.yml'
JOB = 'Windows Pinyin composition'
INPUTS = (WORKFLOW, 'scripts/ime-ci.py', 'scripts/test-ime-ci.py',
          'scripts/prepare-ime.ps1', 'scripts/smoke-ime.ps1')
MAX_ARCHIVE = 256 * 1024 * 1024
MAX_EXECUTABLE = 512 * 1024 * 1024
MAX_EVIDENCE = 64 * 1024 * 1024
RESULT = 'PASS: real Microsoft Pinyin composition, Space selection, candidate Enter suppression, and exactly-once plain Enter submission.\n'
CAPTURES = tuple(f'native-ime-{case}-{stage}.bmp' for case in ('space-candidate', 'enter-candidate')
                 for stage in ('candidate', 'submitted'))
EVIDENCE_FILES = ('native-ime-evidence.json', 'native-ime-result.txt', *CAPTURES)
PRODUCERS = (
    ('native-validation-binary', 'Windows desktop and native smoke',
     ('Run cargo build --locked -p studyapp-desktop --features ui-test', 'Preserve compiled native validation binary')),
    ('maintenance-validation-binary', 'Checked Windows release package',
     ('Build validation harness only when previous desktop proofs were reused',
      'Preserve newly compiled maintenance validation binary')))
IME_PRODUCERS = (('ime-validation-binary', JOB,
                  ('Build missing native validation executable', 'Preserve compiled Pinyin validation binary')),)


def producer_specs(workflow):
    return PRODUCERS if workflow == MAIN_WORKFLOW else IME_PRODUCERS if workflow == WORKFLOW else ()


def fingerprint(tree):
    digest = hashlib.sha256(pipeline.fingerprint(tree, 'desktop').encode())
    for path in sorted(INPUTS):
        digest.update(path.encode() + b'\0' + hashlib.sha256(tree[path]).digest())
    return digest.hexdigest()


def executable_fingerprint(tree):
    # Keep raw versions/lockfiles: a validation executable embeds CARGO_PKG_VERSION.
    # Include the whole existing desktop input closure, plus checkout transformations
    # and potential workspace-root build inputs. User docs and IME launch policy do
    # not enter the executable. The producer workflow is checked separately below.
    digest = hashlib.sha256()
    for path in sorted(tree):
        if (pipeline.suite_input(path, 'desktop') or path in ('.gitattributes', 'build.rs')
                or path.startswith('src/')):
            digest.update(path.encode() + b'\0' + hashlib.sha256(tree[path]).digest())
    return digest.hexdigest()


def ancestor_of(source, target):
    if not all(isinstance(sha, str) and re.fullmatch('[0-9a-f]{40}', sha) for sha in (source, target)):
        return False
    return source == target or subprocess.run(['git', 'merge-base', '--is-ancestor', source, target],
                                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def compatible_binary(source, target, workflow):
    if not producer_specs(workflow) or not ancestor_of(source, target):
        return False
    old, current = pipeline.tree_at(source), pipeline.tree_at(target)
    return (executable_fingerprint(old) == executable_fingerprint(current)
            and old.get(workflow) is not None and old.get(workflow) == current.get(workflow))


def positive(value):
    return type(value) is int and value > 0


def trusted_run(run, repository, workflow, sha=None, complete=True):
    try:
        return (run['event'] == 'push' and run['head_repository']['full_name'] == repository
                and (run['head_branch'] == 'main' or run['head_branch'].startswith('codex/'))
                and run['path'] == workflow and bool(re.fullmatch('[0-9a-f]{40}', run['head_sha']))
                and (sha is None or run['head_sha'] == sha) and positive(run['id'])
                and positive(run['run_attempt'])
                and ((run['status'] == 'completed' and run['conclusion'] == 'success') if complete else
                     (run['status'] in ('queued', 'in_progress', 'completed')
                      and run['conclusion'] in (None, 'success', 'failure'))))
    except (KeyError, TypeError, AttributeError):
        return False


def latest_job(jobs, name):
    found = [job for job in jobs if job.get('name') == name]
    if not found or any(not positive(job.get('run_attempt')) for job in found):
        return None
    attempt = max(job['run_attempt'] for job in found)
    latest = [job for job in found if job['run_attempt'] == attempt]
    return latest[0] if len(latest) == 1 else None


def valid_artifact(artifact, limit=MAX_ARCHIVE):
    return (positive(artifact.get('id')) and artifact.get('expired') is False
            and positive(artifact.get('size_in_bytes')) and artifact['size_in_bytes'] <= limit
            and isinstance(artifact.get('digest'), str)
            and re.fullmatch(r'sha256:[0-9a-f]{64}', artifact['digest']) is not None)


def live_artifact(artifacts, stem):
    matching = [(int(match[1]), artifact) for artifact in artifacts
                if (match := re.fullmatch(re.escape(stem) + r'-([1-9][0-9]*)', artifact.get('name', '')))]
    if not matching:
        return None
    attempt = max(item[0] for item in matching)
    found = [item for number, item in matching if number == attempt]
    return (found[0], attempt) if len(found) == 1 and valid_artifact(found[0]) else None


def binary_candidate(run, jobs, artifacts, repository, sha):
    workflow = run.get('path')
    if not producer_specs(workflow) or not trusted_run(run, repository, workflow, sha, complete=False):
        return None
    for stem, name, steps in producer_specs(workflow):
        job = latest_job(jobs, name)
        live = live_artifact(artifacts, stem)
        if job is None or live is None or job['run_attempt'] != live[1] or job['run_attempt'] > run['run_attempt']:
            continue
        if job.get('conclusion') in ('cancelled', 'skipped'):
            continue
        if all(len(found := [step for step in job.get('steps', []) if step.get('name') == name]) == 1
               and found[0].get('conclusion') == 'success' for name in steps):
            return {'run_id': run['id'], 'sha': sha, 'workflow': workflow,
                    'attempt': live[1], 'artifact': live[0]}
    return None


def verified_archive(raw, artifact, expected, total_limit):
    if (not valid_artifact(artifact) or len(raw) != artifact['size_in_bytes'] or len(raw) > MAX_ARCHIVE
            or 'sha256:' + hashlib.sha256(raw).hexdigest() != artifact['digest']):
        raise ValueError('Artifact length or GitHub SHA256 digest does not match')
    try:
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            members = archive.infolist()
            if len(members) != len(expected) or {entry.filename for entry in members} != set(expected):
                raise ValueError('Artifact must contain exactly the expected files')
            if sum(entry.file_size for entry in members) > total_limit:
                raise ValueError('Artifact exceeds uncompressed size limit')
            for entry in members:
                mode = entry.external_attr >> 16
                if (entry.is_dir() or entry.flag_bits & 1 or entry.file_size <= 0
                        or stat.S_IFMT(mode) not in (0, stat.S_IFREG)):
                    raise ValueError('Artifact contains an unsafe ZIP member')
            return {entry.filename: archive.read(entry) for entry in members}
    except (zipfile.BadZipFile, RuntimeError, NotImplementedError) as error:
        raise ValueError('Invalid artifact ZIP') from error


def verified_binary(raw, artifact):
    executable = verified_archive(raw, artifact, ('studyapp.exe',), MAX_EXECUTABLE)['studyapp.exe']
    if not executable.startswith(b'MZ') or len(executable) < 64:
        raise ValueError('Native validation artifact is not a Windows executable')
    return executable


def verified_built_binary(raw, artifact, local):
    executable = verified_binary(raw, artifact)
    if executable != local:
        raise ValueError('Uploaded validation executable differs from the local build')
    return executable


def validate_result(evidence, result, captures):
    if (result != RESULT or evidence.get('status') != 'passed' or evidence.get('error') is not None
            or evidence.get('inputMethod') != 'ordinary Win32 SendInput virtual keys; no Unicode packet input'):
        raise ValueError('Native Pinyin run did not report a real successful composition test')
    stages, events = evidence.get('stages'), evidence.get('nativeEvents')
    if not isinstance(stages, list) or not isinstance(events, list) or not 1 <= len(stages) <= 5000 or not 1 <= len(events) <= 10000:
        raise ValueError('Missing or excessive native Pinyin trace')
    restore = [item for item in stages if item.get('stage') == 'restore-profile']
    if (len(restore) != 1 or restore[0].get('attempted') is not True or restore[0].get('hresult') != 0
            or restore[0].get('errors') != [] or restore[0].get('deactivateHresult', 0) != 0):
        raise ValueError('Native Pinyin profile restoration did not succeed')
    for case in ('space-candidate', 'enter-candidate'):
        observed = [event for event in events if event.get('case') == case]
        for event in observed:
            if event.get('kind') not in ('composition', 'result'):
                continue
            units, text = event.get('utf16'), event.get('text')
            if (not isinstance(units, list) or not isinstance(text, str) or event.get('decodeError') is not None
                    or any(type(unit) is not int or not 0 <= unit <= 65535 for unit in units)
                    or b''.join(unit.to_bytes(2, 'little') for unit in units).decode('utf-16-le', errors='strict') != text
                    or event.get('message') != 0x10f or not positive(event.get('hwnd'))):
                raise ValueError('Native composition payload is invalid')
        compositions = [event for event in observed if event.get('kind') == 'composition'
                        and event.get('flags', 0) & 8 and event.get('text')]
        results = [event for event in observed if event.get('kind') == 'result' and event.get('flags', 0) & 0x800]
        committed = ''.join(event['text'] for event in results)
        if not compositions or not committed or (case == 'space-candidate' and committed != '夏天'):
            raise ValueError('Both candidate cases require actual composition and committed result events')
        for suffix, modal, commands in (('candidate-settled', True, 0), ('second-plain-enter', False, 1)):
            samples = [item for item in stages if item.get('stage') == f'{case}:{suffix}']
            if not samples or any(item.get('modalOpen') is not modal or item.get('commandCount') != commands
                                  or item.get('editorUnchanged') is not True or item.get('value') != committed
                                  for item in samples):
                raise ValueError('Candidate confirmation or exactly-once submission evidence failed')
    if set(captures) != set(CAPTURES) or any(not data.startswith(b'BM') or not 1000 <= len(data) <= 16 * 1024 * 1024
                                           for data in captures.values()):
        raise ValueError('Four native candidate and submitted screenshots are required')
    return True


def validate_bundle(files):
    validate_result(json.loads(files['native-ime-evidence.json']), files['native-ime-result.txt'].decode('utf-8'),
                    {name: files[name] for name in CAPTURES})


def binary_metadata(candidate, executable, test_sha):
    artifact = candidate['artifact']
    return {key: candidate[key] for key in ('run_id', 'sha', 'attempt', 'workflow')} | {
        'test_sha': test_sha, 'input_fingerprint': executable_fingerprint(pipeline.tree_at(test_sha)),
        'artifact_id': artifact['id'], 'artifact_name': artifact['name'], 'artifact_digest': artifact['digest'],
        'sha256': hashlib.sha256(executable).hexdigest()}


def binary_source_valid(api, metadata, sha, require_live=True):
    source_sha, workflow = metadata.get('sha'), metadata.get('workflow')
    if (metadata.get('test_sha') != sha or not re.fullmatch('[0-9a-f]{64}', metadata.get('sha256', ''))
            or not re.fullmatch('sha256:[0-9a-f]{64}', metadata.get('artifact_digest', ''))
            or not positive(metadata.get('artifact_id')) or not positive(metadata.get('attempt'))
            or not compatible_binary(source_sha, sha, workflow)
            or metadata.get('input_fingerprint') != executable_fingerprint(pipeline.tree_at(sha))):
        return False
    run_id = metadata.get('run_id')
    if not positive(run_id):
        return False
    source = api.run(run_id)
    if not trusted_run(source, api.repository, workflow, source_sha, complete=False):
        return False
    jobs = api.jobs(run_id)
    artifacts = api.pages(f'/actions/runs/{run_id}/artifacts', 'artifacts')
    if not require_live:
        # Successful immutable IME evidence outlives the 14-day compilation artifact.
        # Recheck its producer; this does not claim to download that executable again.
        if not trusted_run(source, api.repository, workflow, source_sha, complete=False):
            return False
        for stem, name, steps in producer_specs(workflow):
            if metadata.get('artifact_name') != f'{stem}-{metadata["attempt"]}':
                continue
            job = latest_job(jobs, name)
            if (job is None or job['run_attempt'] != metadata['attempt'] or job['run_attempt'] > source['run_attempt']
                    or job.get('conclusion') in ('cancelled', 'skipped')
                    or not all(len(found := [item for item in job.get('steps', []) if item.get('name') == step]) == 1
                               and found[0].get('conclusion') == 'success' for step in steps)):
                return False
            found = [item for item in artifacts if item.get('id') == metadata['artifact_id']
                     or item.get('name') == metadata['artifact_name']]
            if not found:
                print(f'Original binary artifact {metadata["artifact_id"]} is no longer retained; '
                      'using the successful IME proof binding and revalidated producer steps')
                return True
            return len(found) == 1 and all(found[0].get(key) == metadata[recorded] for key, recorded in (
                ('id', 'artifact_id'), ('name', 'artifact_name'), ('digest', 'artifact_digest')))
        return False
    candidate = binary_candidate(source, jobs, artifacts, api.repository, source_sha)
    return candidate is not None and all(metadata.get(key) == expected for key, expected in (
        ('attempt', candidate['attempt']), ('artifact_id', candidate['artifact']['id']),
        ('artifact_name', candidate['artifact']['name']), ('artifact_digest', candidate['artifact']['digest'])))


def original_applicable_proof(api, proof, tree, sha, is_ancestor=pipeline.ancestor):
    try:
        run_id, source = proof['run_id'], proof['sha']
        if (proof['contract'] != CONTRACT or proof['repository'] != api.repository or not positive(run_id)
                or not re.fullmatch('[0-9a-f]{40}', sha) or not re.fullmatch('[0-9a-f]{40}', source)
                or not is_ancestor(source)
                or proof['fingerprint'] != fingerprint(tree)
                or fingerprint(pipeline.tree_at(source)) != fingerprint(tree)):
            return False
        run = api.run(run_id)
        job = latest_job(api.jobs(run_id), JOB)
        if (not trusted_run(run, api.repository, WORKFLOW, source) or job is None
                or job.get('status') != 'completed' or job.get('conclusion') != 'success'
                or job['run_attempt'] != proof['run_attempt'] or job['run_attempt'] > run['run_attempt']):
            return False
        live = live_artifact(api.pages(f'/actions/runs/{run_id}/artifacts', 'artifacts'), 'ime-proof')
        if live is None or live[1] != proof['run_attempt']:
            return False
        files = verified_archive(api.request(f'/actions/artifacts/{live[0]["id"]}/zip', binary=True), live[0],
                                 ('ime-proof.json', *EVIDENCE_FILES), MAX_EVIDENCE)
        if json.loads(files['ime-proof.json']) != proof or not binary_source_valid(api, proof['binary'], source, require_live=False):
            return False
        validate_bundle(files)
        return True
    except (KeyError, TypeError, ValueError, OSError, AttributeError):
        return False


def proof_bridge(proof, tree):
    """Applicability metadata only: never rewrite the original IME proof ZIP."""
    try:
        if (proof['run_id'], proof['sha']) != pipeline.FIXTURE_BRIDGE_IME:
            return None
        projected = pipeline.bridge_projection(tree)
        if projected is None or proof['fingerprint'] != fingerprint(projected):
            return None
        return {'id': pipeline.FIXTURE_BRIDGE_ID, 'anchor': pipeline.FIXTURE_BRIDGE_ANCHOR,
                'run_id': proof['run_id'], 'sha': proof['sha'], 'run_attempt': proof['run_attempt'],
                'original_fingerprint': proof['fingerprint'], 'target_fingerprint': fingerprint(tree)}
    except (KeyError, TypeError, ValueError, OSError):
        return None


def applicable_proof(api, proof, tree, sha, is_ancestor=None):
    is_ancestor = is_ancestor or pipeline.ancestor
    try:
        if original_applicable_proof(api, proof, tree, sha, is_ancestor):
            return True
        delta = production_delta.ime_metadata(proof, tree, fingerprint, proof_bridge)
        if delta is not None:
            return (is_ancestor(production_delta.BASIS)
                    and original_applicable_proof(api, proof, pipeline.tree_at(proof['sha']), proof['sha'], is_ancestor))
        if (not re.fullmatch('[0-9a-f]{40}', sha) or not is_ancestor(pipeline.FIXTURE_BRIDGE_ANCHOR)
                or (proof.get('run_id'), proof.get('sha')) != pipeline.FIXTURE_BRIDGE_IME):
            return False
        # First verify the immutable source fingerprint, live ZIP digest, latest
        # successful attempt, evidence and original executable binding unchanged.
        source = proof['sha']
        if not original_applicable_proof(api, proof, pipeline.tree_at(source), source, is_ancestor):
            return False
        return proof_bridge(proof, tree) is not None
    except (KeyError, TypeError, ValueError, OSError, AttributeError, zipfile.BadZipFile):
        return False


class GitHub(pipeline.GitHub):
    def request(self, path, method='GET', data=None, binary=False):
        if not binary:
            return super().request(path, method, data, binary)
        # Bound downloads before allocation; redirects retain pipeline.py's host/auth policy.
        request = urllib.request.Request(self.base + path, headers={
            'Authorization': 'Bearer ' + os.environ['GH_TOKEN'], 'Accept': 'application/vnd.github+json',
            'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'StudyApp-native-ime'})
        with pipeline.http_open(request, timeout=60) as response:
            raw = response.read(MAX_ARCHIVE + 1)
        if len(raw) > MAX_ARCHIVE:
            raise ValueError('Artifact download exceeded size limit')
        return raw


def find_proof(api, sha):
    tree = pipeline.tree_at(sha)
    expected = fingerprint(tree)
    runs = api.request('/actions/workflows/ime.yml/runs?event=push&status=completed&per_page=100')['workflow_runs']
    for run in runs:
        if not trusted_run(run, api.repository, WORKFLOW) or not pipeline.ancestor(run['head_sha']):
            continue
        try:
            if (fingerprint(pipeline.tree_at(run['head_sha'])) != expected
                    and ((run['id'], run['head_sha']) != pipeline.FIXTURE_BRIDGE_IME
                         or (pipeline.bridge_projection(tree) is None and production_delta.scope_tree(tree) is None))):
                continue
            job = latest_job(api.jobs(run['id']), JOB)
            if job is None or job.get('status') != 'completed' or job.get('conclusion') != 'success':
                continue
            live = live_artifact(api.pages(f'/actions/runs/{run["id"]}/artifacts', 'artifacts'), 'ime-proof')
            if live is None or live[1] != job['run_attempt']:
                continue
            files = verified_archive(api.request(f'/actions/artifacts/{live[0]["id"]}/zip', binary=True), live[0],
                                     ('ime-proof.json', *EVIDENCE_FILES), MAX_EVIDENCE)
            proof = json.loads(files['ime-proof.json'])
            if proof.get('run_id') == run['id'] and applicable_proof(api, proof, tree, sha):
                return proof
        except (ValueError, KeyError, TypeError, OSError):
            continue
    return None


def fresh_directory(path):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('IME execution artifacts may only be created on GitHub Actions')
    root = Path(os.environ['RUNNER_TEMP']).resolve(strict=True)
    directory = Path(path)
    parent = directory.parent.resolve(strict=True)
    if parent == root or root not in parent.parents or directory.exists() or directory.is_symlink():
        raise ValueError('Expected a fresh directory inside an isolated RUNNER_TEMP subdirectory')
    directory.mkdir()
    return directory.resolve(strict=True)


def read_evidence(directory):
    files = {}
    for name in EVIDENCE_FILES:
        path = Path(directory) / name
        if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 16 * 1024 * 1024:
            raise ValueError('Missing, excessive, or unsafe native evidence file: ' + name)
        files[name] = path.read_bytes()
    validate_bundle(files)
    return files


def ci_binary_state(run, jobs, repository, sha):
    """Return (confirmed desktop reuse, a producer may still upload)."""
    if not trusted_run(run, repository, MAIN_WORKFLOW, sha, complete=False):
        return False, False
    plan = latest_job(jobs, 'Verification plan')
    desktop = latest_job(jobs, PRODUCERS[0][1])
    reused = (plan is not None and plan['run_attempt'] <= run['run_attempt']
              and plan.get('status') == 'completed' and plan.get('conclusion') == 'success'
              and desktop is not None and desktop['run_attempt'] <= run['run_attempt']
              and desktop.get('status') == 'completed' and desktop.get('conclusion') == 'skipped')
    pending = False
    for _, name, steps in PRODUCERS:
        job = latest_job(jobs, name)
        if job is None or job['run_attempt'] > run['run_attempt']:
            pending |= run.get('status') != 'completed'
            continue
        if job.get('status') == 'completed':
            continue
        build = [step for step in job.get('steps', []) if step.get('name') == steps[0]]
        upload = [step for step in job.get('steps', []) if step.get('name') == steps[1]]
        # A skipped build or upload proves that this job will not publish a binary.
        # No steps yet means queued/unknown, not proof that a producer was skipped.
        if any(len(found) == 1 and found[0].get('conclusion') in ('skipped', 'failure', 'cancelled')
               for found in (build, upload)):
            continue
        pending = True
    return bool(reused), pending


def exact_binary_state(api, sha):
    confirmed_reuse, pending, seen = False, False, False
    for workflow in (MAIN_WORKFLOW, WORKFLOW):
        filename = workflow.rsplit('/', 1)[1]
        runs = api.request(f'/actions/workflows/{filename}/runs?event=push&head_sha={sha}&per_page=30')['workflow_runs']
        for run in runs:
            if not trusted_run(run, api.repository, workflow, sha, complete=False):
                continue
            jobs = api.jobs(run['id'])
            candidate = binary_candidate(run, jobs, api.pages(f'/actions/runs/{run["id"]}/artifacts', 'artifacts'),
                                         api.repository, sha)
            if candidate is not None:
                return candidate, confirmed_reuse, pending, True
            if workflow == MAIN_WORKFLOW:
                reused, running = ci_binary_state(run, jobs, api.repository, sha)
                confirmed_reuse |= reused
                pending |= running
                seen = True
    return None, confirmed_reuse, pending, seen


def find_ancestor_binary(api, sha):
    for workflow in (MAIN_WORKFLOW, WORKFLOW):
        filename = workflow.rsplit('/', 1)[1]
        runs = api.request(f'/actions/workflows/{filename}/runs?event=push&status=completed&per_page=100')['workflow_runs']
        for run in runs:
            source_sha = run.get('head_sha')
            if (source_sha == sha or not trusted_run(run, api.repository, workflow, complete=False)
                    or not compatible_binary(source_sha, sha, workflow)):
                continue
            candidate = binary_candidate(run, api.jobs(run['id']),
                                         api.pages(f'/actions/runs/{run["id"]}/artifacts', 'artifacts'), api.repository, source_sha)
            if candidate is not None:
                return candidate
    return None


def save_binary(api, candidate, sha, directory, local=None):
    if not compatible_binary(candidate['sha'], sha, candidate['workflow']):
        raise ValueError('Validation executable inputs or producer recipe differ from the test checkout')
    raw = api.request(f'/actions/artifacts/{candidate["artifact"]["id"]}/zip', binary=True)
    executable = verified_binary(raw, candidate['artifact']) if local is None else verified_built_binary(raw, candidate['artifact'], local)
    target = fresh_directory(directory)
    (target / 'studyapp.exe').write_bytes(executable)
    (target / 'binary-metadata.json').write_text(json.dumps(binary_metadata(candidate, executable, sha), indent=2), encoding='utf-8')
    print(f'Using validation binary from {candidate["workflow"]} run {candidate["run_id"]}, '
          f'producer attempt {candidate["attempt"]}, source {candidate["sha"]}; test checkout {sha}')


def wait_binary(api, sha, directory, wait_seconds):
    deadline = time.monotonic() + wait_seconds
    checked_ancestors = False
    last_message = None
    while True:
        # New API instance prevents cached job state from hiding a just-completed upload.
        api = GitHub(api.repository)
        candidate, reused, pending, seen = exact_binary_state(api, sha)
        if candidate is None and reused and not checked_ancestors:
            candidate = find_ancestor_binary(api, sha)
            checked_ancestors = True
        if candidate is not None:
            save_binary(api, candidate, sha, directory)
            return False
        if seen and not pending:
            print('Main CI has no producer that can still upload a validation binary; build only the current validation executable now')
            return True
        if time.monotonic() >= deadline:
            print('The bounded binary wait ended while a producer was still running or unavailable. '
                  'A current-checkout build is now required; an overlapping producer may finish later.')
            return True
        message = 'Waiting for an active validation producer' if pending else 'Waiting for current-commit CI scheduling metadata'
        if message != last_message:
            print(message, flush=True)
            last_message = message
        time.sleep(min(15, max(0, deadline - time.monotonic())))


def bind_built_binary(api, sha, directory, executable, artifact_id, artifact_digest):
    path = Path(executable)
    if path.is_symlink() or not path.is_file() or not 64 <= path.stat().st_size <= MAX_EXECUTABLE:
        raise ValueError('Local fallback executable is missing or unsafe')
    if not positive(artifact_id) or not re.fullmatch('sha256:[0-9a-f]{64}', artifact_digest):
        raise ValueError('Fallback upload did not return an artifact ID and digest')
    run_id = int(os.environ['GITHUB_RUN_ID'])
    attempt = int(os.environ['GITHUB_RUN_ATTEMPT'])
    for _ in range(12):
        api = GitHub(api.repository)
        candidate = binary_candidate(api.run(run_id), api.jobs(run_id),
                                     api.pages(f'/actions/runs/{run_id}/artifacts', 'artifacts'), api.repository, sha)
        if candidate is not None:
            if (candidate['workflow'] != WORKFLOW or candidate['attempt'] != attempt
                    or candidate['artifact']['id'] != artifact_id or candidate['artifact']['digest'] != artifact_digest):
                raise ValueError('Fallback upload does not match the actual current producer attempt')
            save_binary(api, candidate, sha, directory, local=path.read_bytes())
            return
        time.sleep(5)
    raise ValueError('Current fallback build/upload metadata did not become available within 60 seconds')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('plan')
    sub.add_parser('verify-release')
    binary = sub.add_parser('binary')
    binary.add_argument('--directory', required=True)
    binary.add_argument('--wait-seconds', type=int, default=1200, choices=range(0, 1801))
    built = sub.add_parser('bind-built')
    built.add_argument('--directory', required=True)
    built.add_argument('--executable', required=True)
    built.add_argument('--artifact-id', type=int, required=True)
    built.add_argument('--artifact-digest', required=True)
    record = sub.add_parser('record')
    record.add_argument('--directory', required=True)
    record.add_argument('--data', required=True)
    record.add_argument('--binary-metadata', required=True)
    validate = sub.add_parser('validate-result')
    validate.add_argument('--directory', required=True)
    args = parser.parse_args()
    if args.command == 'validate-result':
        read_evidence(args.directory)
        print('Verified real Pinyin composition evidence for both candidate cases')
        return
    sha, repository = os.environ['GITHUB_SHA'], os.environ['GITHUB_REPOSITORY']
    if not re.fullmatch('[0-9a-f]{40}', sha) or pipeline.git('rev-parse', 'HEAD').decode().strip() != sha:
        raise ValueError('Checkout must match the complete target SHA')
    api = GitHub(repository)
    if args.command == 'plan':
        try:
            proof = find_proof(api, sha)
        except (OSError, ValueError, KeyError, TypeError) as error:
            print('No reusable proof could be verified: ' + str(error))
            proof = None
        pipeline.output('run', 'false' if proof else 'true')
        if proof:
            print(f'Reusing real Pinyin proof from run {proof["run_id"]}, source {proof["sha"]}')
            bridge = proof_bridge(proof, pipeline.tree_at(sha))
            if bridge:
                print('Applicable through compatibility bridge: ' + json.dumps(bridge, sort_keys=True))
            delta = production_delta.ime_metadata(proof, pipeline.tree_at(sha), fingerprint, proof_bridge)
            if delta:
                print('Unchanged Pinyin coverage under reviewed production delta: ' + json.dumps(delta, sort_keys=True))
    elif args.command in ('binary', 'bind-built'):
        if os.environ.get('GITHUB_ACTIONS') != 'true' or os.name != 'nt':
            raise ValueError('Binary reuse is restricted to the isolated Windows CI runner')
        if args.command == 'binary':
            needs_build = wait_binary(api, sha, args.directory, args.wait_seconds)
            pipeline.output('needs_build', str(needs_build).lower())
        else:
            bind_built_binary(api, sha, args.directory, args.executable, args.artifact_id, args.artifact_digest)
    elif args.command == 'record':
        files = read_evidence(args.data)
        metadata = json.loads(Path(args.binary_metadata).read_text(encoding='utf-8'))
        if not binary_source_valid(api, metadata, sha):
            raise ValueError('Test binary source no longer matches its verified producer')
        with Path(args.binary_metadata).with_name('studyapp.exe').open('rb') as stream:
            if hashlib.file_digest(stream, 'sha256').hexdigest() != metadata['sha256']:
                raise ValueError('Executed validation binary no longer matches its recorded hash')
        proof = {'contract': CONTRACT, 'repository': repository, 'sha': sha,
                 'run_id': int(os.environ['GITHUB_RUN_ID']), 'run_attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
                 'fingerprint': fingerprint(pipeline.tree_at(sha)), 'binary': metadata}
        target = fresh_directory(args.directory)
        for name, data in files.items():
            (target / name).write_bytes(data)
        (target / 'ime-proof.json').write_text(json.dumps(proof, indent=2), encoding='utf-8')
    elif args.command == 'verify-release':
        version = pipeline.version_at(pipeline.tree_at(sha))
        match = pipeline.SEMVER.fullmatch(version)
        if match is None:
            raise ValueError('Invalid workspace version')
        if tuple(map(int, match.groups()[:3])) < (0, 1, 0):
            print('Native Pinyin gate starts with version 0.1.0')
            return
        proof = find_proof(api, sha)
        if proof is None:
            raise ValueError('Release requires applicable successful real Windows Pinyin composition evidence')
        if production_delta.scope_tree(pipeline.tree_at(sha)) is not None and not production_delta.release_ready(api, sha, pipeline.tree_at(sha)):
            raise ValueError('Release requires completed exact-SHA desktop composite or full desktop evidence and full CI success')
        print(f'Native Pinyin release gate accepted proof run {proof["run_id"]} for {sha}')
        bridge = proof_bridge(proof, pipeline.tree_at(sha))
        if bridge:
            print('Applicable through compatibility bridge: ' + json.dumps(bridge, sort_keys=True))
        delta = production_delta.ime_metadata(proof, pipeline.tree_at(sha), fingerprint, proof_bridge)
        if delta:
            print('Unchanged Pinyin coverage under reviewed production delta: ' + json.dumps(delta, sort_keys=True))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError) as error:
        print('Native Pinyin verification failed: ' + str(error), file=sys.stderr)
        sys.exit(1)
