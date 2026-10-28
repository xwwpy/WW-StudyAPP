import copy
import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import shutil
import subprocess
import sys
import unittest
import warnings
from unittest.mock import patch
import zipfile

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location('ime_ci', Path(__file__).with_name('ime-ci.py'))
p = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(p)
SHA = 'a' * 40
REPO = 'xwwpy/WW-StudyAPP'


def tree():
    return {'Cargo.toml': b'[workspace.package]\nversion="0.8.0"\n',
            '.github/workflows/ci.yml': b'ci recipe',
            'crates/studyapp-desktop/src/main.rs': b'app',
            'vendor/gpui-pre-windows/src/events.rs': b'ime',
            **{path: b'policy' for path in ('.github/workflows/ime.yml', 'scripts/ime-ci.py',
                'scripts/test-ime-ci.py', 'scripts/prepare-ime.ps1', 'scripts/smoke-ime.ps1')}}


def run(workflow='.github/workflows/ci.yml'):
    return {'id': 42, 'head_sha': SHA, 'head_repository': {'full_name': REPO},
            'event': 'push', 'head_branch': 'codex/work', 'path': workflow,
            'status': 'in_progress', 'conclusion': None, 'run_attempt': 2}


def producer(attempt=2):
    return {'name': 'Windows desktop and native smoke', 'run_attempt': attempt,
            'status': 'in_progress', 'conclusion': None, 'steps': [
                {'name': 'Run cargo build --locked -p studyapp-desktop --features ui-test', 'conclusion': 'success'},
                {'name': 'Preserve compiled native validation binary', 'conclusion': 'success'}]}


def artifact(raw=b'raw', attempt=2):
    return {'id': 90, 'name': f'native-validation-binary-{attempt}', 'expired': False,
            'size_in_bytes': len(raw), 'digest': 'sha256:' + hashlib.sha256(raw).hexdigest()}


def archive(entries=None):
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w') as handle:
        for name, payload in (entries or [('studyapp.exe', b'MZ' + b'x' * 64)]):
            with warnings.catch_warnings():
                warnings.simplefilter('ignore', UserWarning)
                handle.writestr(name, payload)
    return output.getvalue()


def result():
    stages = [{'stage': 'restore-profile', 'attempted': True, 'hresult': 0, 'deactivateHresult': 0, 'errors': []}]
    events = []
    for name, value in [('space-candidate', '夏天'), ('enter-candidate', 'xiatian')]:
        stages += [
            {'stage': f'{name}:candidate-settled', 'modalOpen': True, 'commandCount': 0,
             'editorUnchanged': True, 'value': value},
            {'stage': f'{name}:second-plain-enter', 'modalOpen': False, 'commandCount': 1,
             'editorUnchanged': True, 'value': value}]
        events += [
            {'case': name, 'kind': 'composition', 'message': 0x10f, 'flags': 8, 'text': 'xiatian',
             'utf16': [ord(c) for c in 'xiatian'], 'decodeError': None, 'hwnd': 17, 'sequence': 1},
            {'case': name, 'kind': 'result', 'message': 0x10f, 'flags': 0x800, 'text': value,
             'utf16': [ord(c) for c in value], 'decodeError': None, 'hwnd': 17, 'sequence': 2}]
    return {'status': 'passed', 'error': None, 'inputMethod': 'ordinary Win32 SendInput virtual keys; no Unicode packet input',
            'stages': stages, 'nativeEvents': events}


RESULT = 'PASS: real Microsoft Pinyin composition, Space selection, candidate Enter suppression, and exactly-once plain Enter submission.\n'
CAPTURES = {f'native-ime-{case}-{stage}.bmp': b'BM' + b'x' * 1000
            for case in ('space-candidate', 'enter-candidate') for stage in ('candidate', 'submitted')}


class Policy(unittest.TestCase):
    def test_fingerprint_tracks_consumed_sources_and_all_ime_orchestration(self):
        original = tree()
        self.assertEqual(len(p.fingerprint(original)), 64)
        for path in original:
            if path == 'Cargo.toml':
                continue
            modified = dict(original, **{path: b'changed'})
            self.assertNotEqual(p.fingerprint(original), p.fingerprint(modified), path)
        self.assertEqual(p.fingerprint(original), p.fingerprint(dict(original, **{'docs/user-guide.md': b'usage'})))
        self.assertEqual(p.fingerprint(original), p.fingerprint(dict(original, **{'Cargo.toml': original['Cargo.toml'].replace(b'0.8.0', b'0.8.1')})))
        incomplete = dict(original)
        incomplete.pop('scripts/prepare-ime.ps1')
        with self.assertRaises(KeyError):
            p.fingerprint(incomplete)

    def test_binary_requires_current_sha_and_actual_latest_attempt_producer(self):
        candidate = p.binary_candidate(run(), [producer()], [artifact()], REPO, SHA)
        self.assertIsNotNone(candidate)
        self.assertIsNone(p.binary_candidate(run(), [producer(1)], [artifact()], REPO, SHA))
        self.assertIsNone(p.binary_candidate(run(), [producer()], [artifact(attempt=1)], REPO, SHA))
        self.assertIsNone(p.binary_candidate(run(), [producer()], [artifact()], REPO, 'b' * 40))
        self.assertIsNone(p.binary_candidate(run(), [producer()], [artifact(), artifact()], REPO, SHA))
        failed = producer()
        failed['steps'][1]['conclusion'] = 'failure'
        self.assertIsNone(p.binary_candidate(run(), [failed], [artifact()], REPO, SHA))
        self.assertIsNotNone(p.binary_candidate(run(), [producer(1)], [artifact(attempt=1)], REPO, SHA))
        self.assertIsNone(p.binary_candidate(run(), [producer(1), failed], [artifact(attempt=1)], REPO, SHA))

    def test_binary_fallback_still_requires_its_own_completed_build_and_upload(self):
        job = {'name': 'Checked Windows release package', 'run_attempt': 2, 'conclusion': None,
               'steps': [{'name': name, 'conclusion': 'success'} for name in (
                   'Build validation harness only when previous desktop proofs were reused',
                   'Preserve newly compiled maintenance validation binary')]}
        saved = dict(artifact(), name='maintenance-validation-binary-2')
        self.assertIsNotNone(p.binary_candidate(run(), [job], [saved], REPO, SHA))
        job['steps'][0]['conclusion'] = 'skipped'
        self.assertIsNone(p.binary_candidate(run(), [job], [saved], REPO, SHA))

    def test_live_artifacts_reject_expiry_missing_digest_and_ambiguous_newer_attempts(self):
        for change in ({'expired': True}, {'digest': None}, {'size_in_bytes': p.MAX_ARCHIVE + 1}):
            self.assertIsNone(p.binary_candidate(run(), [producer()], [dict(artifact(), **change)], REPO, SHA))
        self.assertIsNone(p.binary_candidate(run(), [producer()], [artifact(), artifact(attempt=3)], REPO, SHA))

    def test_proof_rechecks_source_job_attempt_ancestry_and_live_evidence(self):
        proof = {'contract': p.CONTRACT, 'repository': REPO, 'sha': SHA, 'run_id': 73, 'run_attempt': 1,
                 'fingerprint': p.fingerprint(tree()), 'binary': {'run_id': 42, 'sha': SHA,
                 'test_sha': SHA, 'workflow': '.github/workflows/ci.yml', 'input_fingerprint': p.executable_fingerprint(tree()),
                 'attempt': 2, 'artifact_id': 90, 'artifact_name': 'native-validation-binary-2',
                 'artifact_digest': artifact()['digest'], 'sha256': 'b' * 64}}
        raw = archive([('ime-proof.json', json.dumps(proof)), ('native-ime-evidence.json', json.dumps(result())),
                       ('native-ime-result.txt', RESULT), *CAPTURES.items()])
        live = dict(artifact(raw), id=91, name='ime-proof-1')
        source = dict(run('.github/workflows/ime.yml'), id=73, status='completed', conclusion='success')
        binary_artifacts = [artifact()]
        binary_jobs = [producer()]
        job = {'name': 'Windows Pinyin composition', 'status': 'completed', 'conclusion': 'success', 'run_attempt': 1}
        class API:
            repository = REPO
            def run(self, rid):
                return source if rid == 73 else run()
            def jobs(self, rid):
                return [job] if rid == 73 else binary_jobs
            def pages(self, path, key):
                return [live] if '/73/' in path else binary_artifacts
            def request(self, path, binary=False):
                return raw
        with patch.object(p.pipeline, 'tree_at', return_value=tree()):
            self.assertTrue(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
            for status, conclusion in (('in_progress', None), ('completed', 'failure')):
                source.update(status=status, conclusion=conclusion)
                self.assertFalse(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
            source.update(status='completed', conclusion='success')
            binary_artifacts[0]['expired'] = True
            self.assertTrue(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
            self.assertFalse(p.binary_source_valid(API(), proof['binary'], SHA))
            binary_artifacts.clear()
            self.assertTrue(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
            binary_jobs.append(dict(producer(3), conclusion='failure'))
            self.assertFalse(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
            binary_jobs.pop()
            binary_artifacts.append(artifact())
            binary_artifacts[0]['digest'] = 'sha256:' + '0' * 64
            self.assertFalse(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
            binary_artifacts[0] = artifact()
            self.assertFalse(p.applicable_proof(API(), proof, tree(), SHA, lambda _: False))
            for change in ({'conclusion': 'skipped'}, {'run_attempt': 2}, {'status': 'in_progress'}):
                old = dict(job)
                job.update(change)
                self.assertFalse(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))
                job.clear()
                job.update(old)
            live['expired'] = True
            self.assertFalse(p.applicable_proof(API(), proof, tree(), SHA, lambda _: True))

    def test_source_run_rejects_forks_prs_cancelled_wrong_workflow_and_dispatch(self):
        source = run()
        self.assertTrue(p.trusted_run(source, REPO, '.github/workflows/ci.yml', SHA, complete=False))
        for change in ({'event': 'pull_request'}, {'event': 'workflow_dispatch'}, {'head_branch': 'dependabot/a'},
                       {'head_repository': {'full_name': 'other/repo'}}, {'conclusion': 'cancelled'},
                       {'path': '.github/workflows/other.yml'}):
            self.assertFalse(p.trusted_run(dict(source, **change), REPO, '.github/workflows/ci.yml', SHA, complete=False))
        self.assertFalse(p.trusted_run(source, REPO, '.github/workflows/ci.yml', SHA))

    def test_binary_archive_requires_digest_and_one_plain_exact_executable(self):
        good = archive()
        self.assertEqual(p.verified_binary(good, artifact(good)), b'MZ' + b'x' * 64)
        for entries in ([('../studyapp.exe', b'MZ')], [('studyapp.exe', b'MZ'), ('other.txt', b'x')],
                        [('studyapp.exe', b'MZ'), ('studyapp.exe', b'MZ')], [('folder/studyapp.exe', b'MZ')],
                        [('studyapp.exe', b'not an exe')]):
            bad = archive(entries)
            with self.assertRaises(ValueError):
                p.verified_binary(bad, artifact(bad))
        with self.assertRaises(ValueError):
            p.verified_binary(good, artifact(b'wrong'))
        symlink = zipfile.ZipInfo('studyapp.exe')
        symlink.create_system = 3
        symlink.external_attr = (stat.S_IFLNK | 0o777) << 16
        bad = archive([(symlink, b'MZ' + b'x' * 64)])
        with self.assertRaises(ValueError):
            p.verified_binary(bad, artifact(bad))

    def test_real_result_requires_both_compositions_candidates_single_submits_and_captures(self):
        self.assertTrue(p.validate_result(result(), RESULT, CAPTURES))
        for mutate in (lambda e: e.update(status='incomplete'), lambda e: e.update(nativeEvents=[]),
                       lambda e: e['stages'].pop(), lambda e: e['stages'][-1].update(commandCount=2),
                       lambda e: e['nativeEvents'][1].update(text='wrong'), lambda e: e.update(error='overflow'),
                       lambda e: e['stages'][0].update(errors=['restore failed']),
                       lambda e: e['nativeEvents'][0].update(flags=0),
                       lambda e: e['nativeEvents'][0].update(decodeError='bad UTF16')):
            evidence = result()
            mutate(evidence)
            with self.assertRaises(ValueError):
                p.validate_result(evidence, RESULT, CAPTURES)
        with self.assertRaises(ValueError):
            p.validate_result(result(), 'PASS: Unicode injection', CAPTURES)
        with self.assertRaises(ValueError):
            p.validate_result(result(), RESULT, {})


class BinaryReuse(unittest.TestCase):
    def test_raw_inputs_preserve_versions_additions_deletions_and_build_environment(self):
        original = tree() | {'Cargo.lock': b'lock', 'rust-toolchain.toml': b'1.98.0',
                            '.gitattributes': b'*.rs text eol=lf', '.github/workflows/ci.yml': b'env: {}'}
        expected = p.executable_fingerprint(original)
        for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.gitattributes', '.github/workflows/ci.yml'):
            self.assertNotEqual(expected, p.executable_fingerprint(original | {name: original[name] + b' changed'}), name)
        for name in ('crates/new.rs', 'vendor/new.rs', 'assets/new.png', '.cargo/config.toml', 'build.rs'):
            added = original | {name: b'new'}
            self.assertNotEqual(expected, p.executable_fingerprint(added), name)
            self.assertEqual(expected, p.executable_fingerprint({key: value for key, value in added.items() if key != name}))
        self.assertEqual(expected, p.executable_fingerprint(original | {'docs/storage.md': b'usage'}))
        self.assertEqual(expected, p.executable_fingerprint(original | {'scripts/prepare-ime.ps1': b'new setup'}))

    def test_ancestor_binary_requires_identical_raw_inputs_and_ancestry_to_test_sha(self):
        old = 'b' * 40
        trees = {SHA: tree(), old: tree() | {'scripts/prepare-ime.ps1': b'older'}}
        with patch.object(p.pipeline, 'tree_at', side_effect=trees.__getitem__), patch.object(p, 'ancestor_of', return_value=True) as ancestor:
            self.assertTrue(p.compatible_binary(old, SHA, '.github/workflows/ci.yml'))
            ancestor.assert_called_with(old, SHA)
            trees[old]['Cargo.toml'] = b'[workspace.package]\nversion="0.7.3"\n'
            self.assertFalse(p.compatible_binary(old, SHA, '.github/workflows/ci.yml'))
            trees[old] = tree()
            ancestor.return_value = False
            self.assertFalse(p.compatible_binary(old, SHA, '.github/workflows/ci.yml'))

    def test_ime_build_recipe_change_rejects_prior_ime_binary(self):
        old = 'b' * 40
        trees = {SHA: tree(), old: tree() | {'.github/workflows/ime.yml': b'old toolchain or env'}}
        with patch.object(p.pipeline, 'tree_at', side_effect=trees.__getitem__), patch.object(p, 'ancestor_of', return_value=True):
            self.assertFalse(p.compatible_binary(old, SHA, '.github/workflows/ime.yml'))
            self.assertTrue(p.compatible_binary(old, SHA, '.github/workflows/ci.yml'))

    def test_observed_skips_allow_immediate_build_but_running_producers_wait(self):
        jobs = [{'name': 'Verification plan', 'run_attempt': 2, 'status': 'completed', 'conclusion': 'success'},
                dict(producer(), status='completed', conclusion='skipped'),
                {'name': 'Checked Windows release package', 'run_attempt': 2, 'status': 'in_progress',
                 'conclusion': None, 'steps': [{'name': 'Build validation harness only when previous desktop proofs were reused',
                                              'conclusion': 'skipped'}]}]
        self.assertEqual(p.ci_binary_state(run(), jobs, REPO, SHA), (True, False))
        jobs[-1]['steps'][0]['conclusion'] = None
        self.assertEqual(p.ci_binary_state(run(), jobs, REPO, SHA), (True, True))
        jobs[0]['conclusion'] = 'failure'
        self.assertEqual(p.ci_binary_state(run(), jobs, REPO, SHA), (False, True))
        self.assertEqual(p.ci_binary_state(dict(run(), event='pull_request'), jobs, REPO, SHA), (False, False))

    def test_fallback_artifact_binds_actual_ime_producer_attempt_even_before_job_finishes(self):
        source = run('.github/workflows/ime.yml')
        job = {'name': 'Windows Pinyin composition', 'run_attempt': 1, 'status': 'in_progress', 'conclusion': None,
               'steps': [{'name': name, 'conclusion': 'success'} for name in (
                   'Build missing native validation executable', 'Preserve compiled Pinyin validation binary')]}
        saved = dict(artifact(attempt=1), name='ime-validation-binary-1')
        self.assertIsNotNone(p.binary_candidate(source, [job], [saved], REPO, SHA))
        self.assertFalse(p.trusted_run(source, REPO, '.github/workflows/ime.yml', SHA))
        job['run_attempt'] = 2
        self.assertIsNone(p.binary_candidate(source, [job], [saved], REPO, SHA))

    def test_uploaded_fallback_must_match_local_executable_and_github_archive_digest(self):
        raw = archive()
        local = b'MZ' + b'x' * 64
        self.assertEqual(p.verified_built_binary(raw, artifact(raw), local), local)
        with self.assertRaises(ValueError):
            p.verified_built_binary(raw, artifact(raw), b'MZ' + b'y' * 64)
        with self.assertRaises(ValueError):
            p.verified_built_binary(raw, artifact(b'wrong'), local)

    def test_observed_no_producer_builds_immediately_without_sleep_or_old_suite_execution(self):
        api = p.GitHub(REPO)
        with patch.object(p, 'GitHub', return_value=api), patch.object(p, 'exact_binary_state', return_value=(None, True, False, True)), \
             patch.object(p, 'find_ancestor_binary', return_value=None), patch.object(p.time, 'sleep') as sleep:
            self.assertTrue(p.wait_binary(api, SHA, 'unused', 1200))
            sleep.assert_not_called()

    def test_current_sha_binary_wins_and_active_producer_waits_for_upload(self):
        api = p.GitHub(REPO)
        candidate = {'source': 'fixture'}
        with patch.object(p, 'GitHub', return_value=api), \
             patch.object(p, 'exact_binary_state', side_effect=[(None, False, True, True), (candidate, False, True, True)]), \
             patch.object(p, 'find_ancestor_binary') as ancestor, patch.object(p, 'save_binary') as save, \
             patch.object(p.time, 'sleep') as sleep:
            self.assertFalse(p.wait_binary(api, SHA, 'unused', 1200))
            sleep.assert_called_once()
            ancestor.assert_not_called()
            save.assert_called_once_with(api, candidate, SHA, 'unused')

    def test_ancestor_lookup_requires_observed_desktop_reuse_and_no_exact_binary(self):
        api = p.GitHub(REPO)
        candidate = {'source': 'fixture'}
        with patch.object(p, 'GitHub', return_value=api), \
             patch.object(p, 'exact_binary_state', return_value=(None, True, True, True)), \
             patch.object(p, 'find_ancestor_binary', return_value=candidate) as ancestor, \
             patch.object(p, 'save_binary') as save, patch.object(p.time, 'sleep') as sleep:
            self.assertFalse(p.wait_binary(api, SHA, 'unused', 1200))
            ancestor.assert_called_once_with(api, SHA)
            save.assert_called_once_with(api, candidate, SHA, 'unused')
            sleep.assert_not_called()

    def test_long_running_producer_has_bounded_wait_then_explicit_build_fallback(self):
        api = p.GitHub(REPO)
        with patch.object(p, 'GitHub', return_value=api), \
             patch.object(p, 'exact_binary_state', return_value=(None, False, True, True)), \
             patch.object(p, 'find_ancestor_binary') as ancestor, patch.object(p.time, 'sleep') as sleep, \
             patch.object(p.time, 'monotonic', side_effect=[0, 1201]):
            self.assertTrue(p.wait_binary(api, SHA, 'unused', 1200))
            sleep.assert_not_called()
            ancestor.assert_not_called()

    def test_metadata_binds_binary_source_and_test_sha_separately(self):
        old = 'b' * 40
        source = dict(run(), head_sha=old)
        saved = artifact()
        trees = {old: tree(), SHA: tree() | {'scripts/prepare-ime.ps1': b'changed setup'}}
        class API:
            repository = REPO
            def run(self, rid):
                return source
            def jobs(self, rid):
                return [producer()]
            def pages(self, path, key):
                return [saved]
        with patch.object(p.pipeline, 'tree_at', side_effect=trees.__getitem__), patch.object(p, 'ancestor_of', return_value=True):
            candidate = p.binary_candidate(source, [producer()], [saved], REPO, old)
            metadata = p.binary_metadata(candidate, b'MZ' + b'x' * 64, SHA)
            self.assertEqual(metadata['sha'], old)
            self.assertEqual(metadata['test_sha'], SHA)
            self.assertTrue(p.binary_source_valid(API(), metadata, SHA))
            self.assertFalse(p.binary_source_valid(API(), dict(metadata, test_sha=old), SHA))
            trees[old]['Cargo.toml'] = b'[workspace.package]\nversion="0.7.3"\n'
            self.assertFalse(p.binary_source_valid(API(), metadata, SHA))


class PrepareLanguages(unittest.TestCase):
    def powershell(self, body):
        path = str(Path(__file__).with_name('prepare-ime.ps1').resolve()).replace("'", "''")
        script = r'''
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$imeTokens = $null
$imeErrors = $null
$imeAst = [Management.Automation.Language.Parser]::ParseFile('__PATH__', [ref]$imeTokens, [ref]$imeErrors)
if ($imeErrors.Count) { throw 'Preparation source did not parse' }
$imeHelpers = @('Test-ImeChineseTag', 'Get-ImeLanguageRecords', 'Test-ImePreservedLanguageOrder', 'Add-ImePinyinProfile', 'Get-ImeSetupInventory')
foreach ($imeFunction in $imeAst.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    if ($imeFunction.Name -in $imeHelpers) { Invoke-Expression $imeFunction.Extent.Text }
}
function New-MockLanguage($Tag, $Tips) {
    $tipList = [Collections.Generic.List[string]]::new()
    foreach ($tip in $Tips) { $tipList.Add($tip) }
    return [pscustomobject]@{ LanguageTag = $Tag; InputMethodTips = $tipList }
}
# Only these pure mocks are callable by the extracted inventory helper.
# The guarded script entry point and all OS servicing/setter commands are never run.
function Get-WindowsCapability { [CmdletBinding()] param([switch]$Online, [string]$Name) [pscustomobject]@{ Name = $Name; State = 'Installed' } }
function Get-WinUserLanguageList { Write-Output -NoEnumerate $script:mockLanguages }
$imeCapability = 'Language.Basic~~~zh-CN~0.0.1.0'
$imeTip = '0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}'
'''.replace('__PATH__', path) + body
        executable = shutil.which('pwsh') or shutil.which('powershell')
        self.assertIsNotNone(executable, 'PowerShell is required for the pure language-list policy test')
        encoded = base64.b64encode(script.encode('utf-16-le')).decode('ascii')
        completed = subprocess.run([executable, '-NoProfile', '-NonInteractive', '-EncodedCommand', encoded],
                                   capture_output=True, text=True, encoding='utf-8', timeout=30)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        return json.loads(completed.stdout)

    def test_inventory_enumerates_non_pipeline_expanded_language_list(self):
        inventory = self.powershell(r'''
$script:mockLanguages = [Collections.Generic.List[object]]::new()
$script:mockLanguages.Add((New-MockLanguage 'en-US' @('0409:00000409')))
$script:mockLanguages.Add((New-MockLanguage 'zh-Hans-CN' @($imeTip)))
Get-ImeSetupInventory | ConvertTo-Json -Depth 8 -Compress
''')
        self.assertEqual(len(inventory['languages']), 2)
        self.assertEqual([record['language'] for record in inventory['languages']], ['en-US', 'zh-Hans-CN'])
        self.assertEqual(inventory['languages'][0]['tips'], ['0409:00000409'])
        self.assertEqual(len(inventory['languages'][1]['tips']), 1)

    def test_canonical_and_legacy_chinese_are_not_duplicated_and_exact_tip_is_ensured(self):
        results = self.powershell(r'''
$results = [Collections.Generic.List[object]]::new()
foreach ($tag in @('zh-Hans-CN', 'zh-CN')) {
    $languages = [Collections.Generic.List[object]]::new()
    $languages.Add((New-MockLanguage 'en-US' @('0409:00000409')))
    $languages.Add((New-MockLanguage $tag @($imeTip)))
    $state = Add-ImePinyinProfile -Languages $languages -Tip $imeTip -NewLanguage { throw 'Must not append an existing Chinese profile' }
    $results.Add(@{ name = $tag; changed = $state.Changed; order = $state.OriginalOrder; records = @(Get-ImeLanguageRecords -Languages $languages) })
}
$languages = [Collections.Generic.List[object]]::new()
$languages.Add((New-MockLanguage 'en-US' @('0409:00000409')))
$languages.Add((New-MockLanguage 'fr-FR' @('040c:0000040c')))
$state = Add-ImePinyinProfile -Languages $languages -Tip $imeTip -NewLanguage { New-MockLanguage 'zh-Hans-CN' @() }
$results.Add(@{ name = 'append'; changed = $state.Changed; order = $state.OriginalOrder; records = @(Get-ImeLanguageRecords -Languages $languages) })
$languages = [Collections.Generic.List[object]]::new()
$languages.Add((New-MockLanguage 'zh-Hans-CN' @('0404:wrong-tip')))
$state = Add-ImePinyinProfile -Languages $languages -Tip $imeTip -NewLanguage { throw 'Must add the TIP, not another language' }
$results.Add(@{ name = 'ensure-tip'; changed = $state.Changed; order = $state.OriginalOrder; records = @(Get-ImeLanguageRecords -Languages $languages) })
ConvertTo-Json -InputObject @($results.ToArray()) -Depth 8 -Compress
''')
        for result in results[:2]:
            self.assertFalse(result['changed'])
            self.assertEqual(len(result['records']), 2)
            self.assertEqual(result['order'], ['en-US', result['name']])
        self.assertTrue(results[2]['changed'])
        self.assertEqual(results[2]['order'], ['en-US', 'fr-FR'])
        self.assertEqual([r['language'] for r in results[2]['records']], ['en-US', 'fr-FR', 'zh-Hans-CN'])
        self.assertTrue(results[3]['changed'])
        self.assertEqual(len(results[3]['records']), 1)
        self.assertEqual(len(results[3]['records'][0]['tips']), 2)
        expected_tip = '0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}'
        self.assertEqual(results[3]['records'][0]['tips'][1], expected_tip)
        self.assertEqual(results[2]['records'][2]['tips'], [expected_tip])

    def test_language_order_allows_tag_canonicalization_but_rejects_reordering(self):
        results = self.powershell(r'''
@{
    alias = (Test-ImePreservedLanguageOrder -Before @('en-US','zh-CN') -After @('en-US','zh-Hans-CN'))
    append = (Test-ImePreservedLanguageOrder -Before @('en-US','fr-FR') -After @('en-US','fr-FR','zh-Hans-CN'))
    reversed = (Test-ImePreservedLanguageOrder -Before @('en-US','fr-FR') -After @('fr-FR','en-US','zh-Hans-CN'))
    lost = (Test-ImePreservedLanguageOrder -Before @('en-US','fr-FR') -After @('en-US'))
} | ConvertTo-Json -Compress
''')
        self.assertEqual(results, {'alias': True, 'append': True, 'reversed': False, 'lost': False})


if __name__ == '__main__':
    unittest.main()
