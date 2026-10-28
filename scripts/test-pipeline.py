import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
from unittest.mock import patch
from types import SimpleNamespace
import io
import zipfile
import unittest
import sys
from contextlib import redirect_stdout
sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location('pipeline', Path(__file__).with_name('pipeline.py'))
p = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(p)


class Fingerprints(unittest.TestCase):
    def setUp(self):
        self.tree = {'Cargo.toml': b'[workspace.package]\nversion="0.0.2"\n[profile.release]\nlto="thin"\n',
                     'Cargo.lock': b'version=4\n[[package]]\nname="studyapp-core"\nversion="0.0.2"\n[[package]]\nname="dep"\nversion="1.0.0"\n',
                     'crates/studyapp-core/src/lib.rs': b'core',
                     'crates/studyapp-sync/src/lib.rs': b'sync',
                     'crates/studyapp-ui/src/lib.rs': b'ui',
                     'crates/studyapp-desktop/src/main.rs': b'desktop',
                     'crates/studyapp-desktop/editor/src/editor.ts': b'editor',
                     '.github/workflows/ci.yml': b'ci',
                     '.github/workflows/editor.yml': b'editor tests',
                     '.github/workflows/security.yml': b'audit',
                     'scripts/pipeline.py': b'planner'}

    def changed(self, path, value):
        other = self.tree.copy()
        if value is None:
            del other[path]
        else:
            other[path] = value
        return {g for g in p.GROUPS if p.fingerprint(self.tree, g) != p.fingerprint(other, g)}

    def test_identical_and_version_only_content_reuses_tests(self):
        self.assertEqual(self.changed('Cargo.toml', self.tree['Cargo.toml'].replace(b'0.0.2', b'0.0.3')), set())
        self.assertEqual(self.changed('Cargo.lock', self.tree['Cargo.lock'].replace(b'0.0.2', b'0.0.3')), set())
        self.assertEqual(p.fingerprint(self.tree, 'desktop'), p.fingerprint(dict(self.tree), 'desktop'))

    def test_real_dependency_and_profile_changes_invalidate(self):
        self.assertIn('desktop', self.changed('Cargo.lock', self.tree['Cargo.lock'].replace(b'1.0.0', b'1.1.0')))
        self.assertIn('core-linux', self.changed('Cargo.toml', self.tree['Cargo.toml'].replace(b'thin', b'fat')))

    def test_ui_and_editor_have_separate_consumers(self):
        self.assertEqual(self.changed('crates/studyapp-ui/src/lib.rs', b'new'), {'desktop', 'maintenance-native'})
        self.assertEqual(self.changed('crates/studyapp-desktop/editor/src/editor.ts', b'new'), {'editor', 'desktop', 'maintenance-native'})

    def test_additions_deletions_and_contract_changes_are_inputs(self):
        self.assertIn('desktop', self.changed('crates/studyapp-desktop/src/new.rs', b'new'))
        self.assertIn('desktop', self.changed('crates/studyapp-desktop/src/main.rs', None))
        self.assertEqual(self.changed('scripts/pipeline.py', b'new planner'), set(p.GROUPS))
        self.assertEqual(self.changed('.github/workflows/ci.yml', b'new tests'), set(p.GROUPS))

    def test_migration_excludes_only_new_orchestration(self):
        other = dict(self.tree, **{'.github/workflows/ci.yml': b'old ci'})
        self.assertEqual(p.fingerprint(self.tree, 'desktop', migration=True), p.fingerprint(other, 'desktop', migration=True))
        other['scripts/smoke.ps1'] = b'new smoke'
        self.assertNotEqual(p.fingerprint(self.tree, 'desktop', migration=True), p.fingerprint(other, 'desktop', migration=True))

    def test_new_installer_policy_does_not_repeat_frozen_legacy_helper(self):
        self.assertEqual(self.changed('installer/Product.wxs', b'new installer'), {'maintenance-policy', 'maintenance-native'})
        self.assertIn('maintenance-policy', self.changed('scripts/modern-package.ps1', b'new package'))
        self.assertNotIn('policy', self.changed('scripts/modern-package.ps1', b'new package'))
        self.assertNotIn('policy', self.changed('crates/studyapp-desktop/src/update_backend.rs', b'new native owner selector'))
        for path in ('scripts/setup-native-tests.cs', 'scripts/smoke-maintenance.ps1', 'scripts/test-installer-rollback.ps1', 'scripts/test-maintenance.ps1', 'scripts/msi-control-native-tests.cs', 'scripts/maintenance-legacy-native.ps1', 'crates/studyapp-desktop/src/maintenance_acceptance.rs', 'crates/studyapp-desktop/src/maintenance_acceptance_process.rs', 'crates/studyapp-desktop/src/maintenance_cancel_acceptance.rs'):
            self.assertIn('maintenance-policy', self.changed(path, b'changed policy input'))

    def test_installer_only_changes_do_not_repeat_desktop_scenarios(self):
        for path in ('scripts/setup-native-tests.cs', 'scripts/msi-control-native-tests.cs', 'scripts/build-installers.ps1', 'scripts/test-installers.ps1', 'scripts/test-maintenance.ps1', 'scripts/smoke-maintenance.ps1'):
            with self.subTest(path=path):
                self.assertEqual(self.changed(path, b'changed'), {'maintenance-policy', 'maintenance-native'})
        for path in ('scripts/smoke.ps1', 'scripts/run-dev.ps1', 'scripts/smoke-inputs.ps1', 'scripts/maintenance-legacy-native.ps1'):
            self.assertIn('desktop', self.changed(path, b'changed'))

    def test_recovery_acceptance_source_invalidates_its_policy_consumer(self):
        self.assertEqual(self.changed('crates/studyapp-desktop/src/maintenance_recovery_acceptance.rs', b'changed recovery acceptance'),
                         {'desktop', 'maintenance-policy', 'maintenance-native'})


class Trust(unittest.TestCase):
    def setUp(self):
        self.run = {'id': 11, 'head_sha': 'a'*40, 'event': 'push', 'conclusion': 'success',
                    'head_branch': 'codex/work', 'head_repository': {'full_name': 'owner/repo'}}

    def test_trusted_successful_push_requires_ancestor_and_repository(self):
        self.assertTrue(p.trusted(self.run, 'owner/repo', lambda sha: True))
        for key, value in [('event', 'pull_request'), ('conclusion', 'failure'), ('head_branch', 'other'),
                           ('head_repository', {'full_name': 'fork/repo'})]:
            with self.subTest(key=key):
                bad = dict(self.run, **{key: value})
                self.assertFalse(p.trusted(bad, 'owner/repo', lambda sha: True))
        self.assertFalse(p.trusted(self.run, 'owner/repo', lambda sha: False))
        self.assertFalse(p.trusted(self.run, 'owner/repo', lambda sha: (_ for _ in ()).throw(RuntimeError())))

    def test_missing_expired_or_wrong_artifact_is_not_reusable(self):
        good = {'name': 'ci-receipt', 'expired': False, 'id': 3}
        self.assertEqual(p.live_artifact([good], 'ci-receipt'), good)
        for artifacts in ([], [dict(good, expired=True)], [dict(good, name='other')]):
            self.assertIsNone(p.live_artifact(artifacts, 'ci-receipt'))

    def test_artifact_attempt_selection_recovers_reruns_without_overwrite(self):
        first = {'name': 'ci-receipt-1', 'expired': False, 'id': 1}
        second = {'name': 'ci-receipt-2', 'expired': False, 'id': 2}
        self.assertEqual(p.live_artifact([first, second], 'ci-receipt'), second)
        self.assertIsNone(p.live_artifact([first, dict(second, expired=True)], 'ci-receipt'))
        self.assertIsNone(p.live_artifact([second, dict(second, id=3)], 'ci-receipt'))

    def test_editor_migration_normalizes_only_attempt_artifact_name(self):
        old = b'tests unchanged\n          name: offline-editor-tests\n'
        equivalent = old.replace(b'offline-editor-tests', b'offline-editor-tests-${{ github.run_attempt }}')
        self.assertTrue(p.migration_guard(p.OLD_CI_BLOB, old, equivalent))
        self.assertFalse(p.migration_guard(p.OLD_CI_BLOB, old, equivalent.replace(b'tests unchanged', b'tests changed')))
        self.assertFalse(p.migration_guard(p.OLD_CI_BLOB, old, equivalent.replace(b'github.run_attempt', b'github.run_id')))

    def test_receipt_requires_matching_fingerprint_and_executed_job(self):
        receipt = {'contract': 1, 'sha': 'b'*40, 'groups': {'desktop': {'fingerprint': 'fp', 'run_id': 11, 'sha': 'a'*40}}}
        self.assertTrue(p.receipt_matches(receipt, 'b'*40, 'desktop', 'fp'))
        self.assertFalse(p.receipt_matches(receipt, 'c'*40, 'desktop', 'fp'))
        self.assertFalse(p.receipt_matches(receipt, 'b'*40, 'desktop', 'stale'))
        self.assertFalse(p.job_passed([{'name': 'Windows desktop and native smoke', 'conclusion': 'skipped'}], 'desktop'))
        self.assertTrue(p.job_passed([{'name': 'Windows desktop and native smoke', 'conclusion': 'success'}], 'desktop'))

    def test_partial_rerun_preserves_unrerun_successful_job(self):
        api = p.GitHub('owner/repo')
        jobs = [{'name': p.JOBS['desktop'], 'run_attempt': 1, 'conclusion': 'success'},
                {'name': p.JOBS['maintenance-native'], 'run_attempt': 1, 'conclusion': 'failure'},
                {'name': p.JOBS['maintenance-native'], 'run_attempt': 2, 'conclusion': 'success'}]
        with patch.object(api, 'pages', return_value=jobs) as pages:
            selected = api.jobs(11)
        pages.assert_called_once_with('/actions/runs/11/jobs?filter=all', 'jobs')
        self.assertEqual(len(selected), 2)
        self.assertTrue(p.job_passed(selected, 'desktop'))
        self.assertTrue(p.job_passed(selected, 'maintenance-native'))

    def test_rerun_never_falls_back_to_earlier_success(self):
        for conclusion in ('failure', 'skipped', 'cancelled', None):
            api = p.GitHub('owner/repo')
            jobs = [{'name': p.JOBS['desktop'], 'run_attempt': 2, 'conclusion': conclusion},
                    {'name': p.JOBS['desktop'], 'run_attempt': 1, 'conclusion': 'success'}]
            with self.subTest(conclusion=conclusion), patch.object(api, 'pages', return_value=jobs):
                self.assertFalse(p.job_passed(api.jobs(11), 'desktop'))

    def test_ambiguous_or_missing_attempt_metadata_is_not_proof(self):
        good = {'name': p.JOBS['desktop'], 'run_attempt': 2, 'conclusion': 'success'}
        for jobs in ([good, dict(good)], [dict(good, run_attempt=None)], [dict(good, run_attempt=0)]):
            api = p.GitHub('owner/repo')
            with self.subTest(jobs=jobs), patch.object(api, 'pages', return_value=jobs), self.assertRaises(ValueError):
                api.jobs(11)

    def test_migration_requires_exact_old_ci_blob_and_editor_blob(self):
        self.assertTrue(p.migration_guard(p.OLD_CI_BLOB, 'same', 'same'))
        self.assertFalse(p.migration_guard('changed', 'same', 'same'))
        self.assertFalse(p.migration_guard(p.OLD_CI_BLOB, 'old', 'new'))


class Manifests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[1] / '.scratch' / 'pipeline-tests'
        base.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(dir=base))
        self.name = 'WW-StudyApp-0.0.3-windows-x86_64.zip'
        (self.root / self.name).write_bytes(b'normal production package')
        digest = hashlib.sha256((self.root / self.name).read_bytes()).hexdigest()
        (self.root / (self.name + '.sha256')).write_text(f'{digest}  {self.name}\n')
        self.manifest = p.make_manifest(self.root, '0.0.3', 'a'*40, 11, 'owner/repo')

    def test_provenance_and_hashes_roundtrip(self):
        p.verify_manifest(self.root, self.manifest, '0.0.3', 'a'*40, 'owner/repo', 11)

    def test_wrong_version_sha_repository_run_or_hash_fails(self):
        for field, value in [('version', '0.0.4'), ('sha', 'b'*40), ('repository', 'fork/repo'), ('run_id', 12)]:
            bad = dict(self.manifest, **{field: value})
            with self.subTest(field=field), self.assertRaises(ValueError):
                p.verify_manifest(self.root, bad, '0.0.3', 'a'*40, 'owner/repo', 11)
        (self.root / self.name).write_bytes(b'tampered')
        with self.assertRaises(ValueError):
            p.verify_manifest(self.root, self.manifest, '0.0.3', 'a'*40, 'owner/repo', 11)

    def test_arbitrary_artifacts_and_wrong_checksum_rejected(self):
        (self.root / 'private.txt').write_text('do not ship')
        with self.assertRaises(ValueError):
            p.make_manifest(self.root, '0.0.3', 'a'*40, 11, 'owner/repo')
        (self.root / 'private.txt').unlink()
        (self.root / (self.name + '.sha256')).write_text('0'*64 + '  ' + self.name)
        with self.assertRaises(ValueError):
            p.make_manifest(self.root, '0.0.3', 'a'*40, 11, 'owner/repo')

    def test_semver_tag_exactly_matches_workspace(self):
        p.check_tag('v0.0.3', '0.0.3')
        for tag in ('v0.0.2', '0.0.3', 'v00.0.3', 'v0.0.3-01'):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                p.check_tag(tag, '0.0.3')

    def test_partial_draft_recovers_without_overwriting_existing_assets(self):
        expected = {'a.zip': 'aaa', 'a.zip.sha256': 'bbb'}
        self.assertEqual(p.publish_delta(expected, {'a.zip': 'aaa'}, True), ['a.zip.sha256'])
        self.assertEqual(p.publish_delta(expected, expected, False), [])
        for existing, draft in [({'a.zip': 'wrong'}, True), ({'a.zip': 'aaa'}, False),
                                ({**expected, 'extra': 'x'}, False)]:
            with self.subTest(existing=existing), self.assertRaises(ValueError):
                p.publish_delta(expected, existing, draft)



class Planner(unittest.TestCase):
    def setUp(self):
        fixture = Fingerprints()
        fixture.setUp()
        self.tree = fixture.tree
        self.sha = 'a'*40
        self.run = {'id': 11, 'head_sha': self.sha, 'event': 'push', 'conclusion': 'success', 'status': 'completed',
                    'head_branch': 'main', 'head_repository': {'full_name': 'owner/repo'},
                    'path': '.github/workflows/ci.yml'}
        self.proofs = {g: {'fingerprint': p.fingerprint(self.tree, g), 'run_id': 11, 'sha': self.sha} for g in p.GROUPS}
        self.receipt = {'contract': 1, 'sha': self.sha, 'groups': self.proofs}
        self.jobs = [{'name': p.JOBS[g], 'conclusion': 'success'} for g in p.GROUPS]
        owner = self
        class API:
            repository = 'owner/repo'
            def request(self, path):
                return {'workflow_runs': [owner.run]}
            def receipt(self, run_id):
                return owner.receipt
            def run(self, run_id):
                if run_id != 11:
                    raise ValueError('No migration run')
                return owner.run
            def jobs(self, run_id):
                return owner.jobs
            def artifact(self, run_id, name):
                return None
        self.api = API()
        self.tree_patch = patch.object(p, 'tree_at', return_value=self.tree)
        self.tree_patch.start()
        self.addCleanup(self.tree_patch.stop)
        self.ancestry = patch.object(p.subprocess, 'run', return_value=SimpleNamespace(returncode=0))
        self.ancestry.start()
        self.addCleanup(self.ancestry.stop)
        self.environment = patch.dict(p.os.environ, {'GITHUB_SHA': self.sha})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def test_unchanged_source_reuses_successful_executed_groups(self):
        plan = p.make_plan(self.api, self.sha)
        self.assertTrue(all(not state['run'] and state['run_id'] == 11 for state in plan['groups'].values()))
        self.assertIsNone(plan['package_run'])

    def test_stale_group_fingerprint_runs_only_affected_group(self):
        self.proofs['desktop']['fingerprint'] = 'stale'
        plan = p.make_plan(self.api, self.sha)
        self.assertEqual({g for g, state in plan['groups'].items() if state['run']}, {'desktop'})

    def test_skipped_origin_job_cannot_be_a_success_receipt(self):
        self.jobs = [dict(j, conclusion='skipped') if j['name'] == p.JOBS['desktop'] else j for j in self.jobs]
        plan = p.make_plan(self.api, self.sha)
        self.assertTrue(plan['groups']['desktop']['run'])
        self.assertFalse(plan['groups']['editor']['run'])

    def test_failed_workflow_reuses_only_its_successful_independent_jobs(self):
        self.run['conclusion'] = 'failure'
        self.jobs = [dict(j, conclusion='failure') if j['name'] == p.JOBS['desktop'] else j for j in self.jobs]
        with patch.object(self.api, 'artifact', side_effect=AssertionError('A failed workflow cannot supply packages')):
            plan = p.make_plan(self.api, self.sha)
        self.assertEqual({g for g, state in plan['groups'].items() if state['run']}, {'desktop'})
        self.assertIsNone(plan['package_run'])

    def test_cancelled_or_unfinished_workflow_never_supplies_receipts(self):
        for conclusion, status in [('cancelled', 'completed'), (None, 'in_progress')]:
            with self.subTest(conclusion=conclusion):
                self.run.update(conclusion=conclusion, status=status)
                self.assertTrue(all(s['run'] for s in p.make_plan(self.api, self.sha)['groups'].values()))

    def test_native_failure_is_not_covered_by_passing_static_policy_and_desktop(self):
        self.run['conclusion'] = 'failure'
        self.jobs = [dict(j, conclusion='failure') if j['name'] == p.JOBS['maintenance-native'] else j for j in self.jobs]
        plan = p.make_plan(self.api, self.sha)
        self.assertEqual({g for g, state in plan['groups'].items() if state['run']}, {'maintenance-native'})
        self.assertIsNone(plan['package_run'])

    def test_wrong_workflow_does_not_supply_executed_proofs(self):
        self.run['path'] = '.github/workflows/other.yml'
        self.assertTrue(all(s['run'] for s in p.make_plan(self.api, self.sha)['groups'].values()))

    def test_api_uncertainty_and_force_all_fall_back_to_running_tests(self):
        with patch.object(self.api, 'request', side_effect=OSError('API unavailable')):
            self.assertTrue(all(s['run'] for s in p.make_plan(self.api, self.sha)['groups'].values()))
        self.assertTrue(all(s['run'] for s in p.make_plan(self.api, self.sha, force=True)['groups'].values()))

    def test_missing_candidate_history_falls_back_to_tests(self):
        current = 'b'*40
        def read_tree(sha):
            if sha == current:
                return self.tree
            raise p.subprocess.CalledProcessError(128, 'git')
        with patch.object(p, 'tree_at', side_effect=read_tree):
            self.assertTrue(all(s['run'] for s in p.make_plan(self.api, current)['groups'].values()))

    def test_policy_suite_does_not_rerun_for_unrelated_docs_or_ui(self):
        before = p.fingerprint(self.tree, 'policy')
        self.tree['docs/release-notes.md'] = b'new notes'
        self.tree['crates/studyapp-ui/src/lib.rs'] = b'new UI'
        self.assertEqual(before, p.fingerprint(self.tree, 'policy'))
        self.tree['scripts/test-update-helper.ps1'] = b'changed update checks'
        self.assertNotEqual(before, p.fingerprint(self.tree, 'policy'))

    def test_migration_snapshot_rejects_future_contract_edits(self):
        required = {'.github/workflows/ci.yml', '.github/workflows/editor.yml', '.github/workflows/security.yml', 'scripts/pipeline.py', 'scripts/test-pipeline.py'}
        self.tree['scripts/test-pipeline.py'] = b'tests'
        self.tree['.github/pipeline-migration.json'] = json.dumps({name: hashlib.sha256(self.tree[name]).hexdigest() for name in required}).encode()
        self.assertTrue(p.migration_contract_valid(self.tree))
        self.tree['.github/workflows/ci.yml'] += b'changed suite'
        self.assertFalse(p.migration_contract_valid(self.tree))


class ArchiveVerification(unittest.TestCase):
    def archive(self, sha='a'*40, version='0.0.3', corrupt=False):
        name = p.asset_names(version)[0]
        payload = b'package'
        checksum = (hashlib.sha256(payload).hexdigest()+'  '+name+'\n').encode()
        manifest = {'contract': 1, 'sha': sha, 'version': version, 'run_id': 11, 'repository': 'owner/repo',
                    'assets': {name: hashlib.sha256(payload).hexdigest(), name+'.sha256': hashlib.sha256(checksum).hexdigest()}}
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, 'w') as archive:
            archive.writestr(name, b'corrupt' if corrupt else payload)
            archive.writestr(name+'.sha256', checksum)
            archive.writestr('release-manifest.json', json.dumps(manifest))
        return raw.getvalue()

    def test_exact_sha_successful_package_job_and_bytes_required(self):
        run = {'id': 11, 'head_sha': 'a'*40, 'event': 'push', 'conclusion': 'success', 'head_branch': 'main',
               'head_repository': {'full_name': 'owner/repo'}, 'path': '.github/workflows/ci.yml'}
        api = SimpleNamespace(repository='owner/repo', run=lambda _: run,
                              jobs=lambda _: [{'name': 'Checked Windows release package', 'conclusion': 'success'}])
        with patch.object(p.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), patch.dict(p.os.environ, {'GITHUB_SHA': 'a'*40}):
            p.verify_archive(self.archive(), '0.0.3', 'a'*40, api)
            for raw in (self.archive(sha='b'*40), self.archive(version='0.0.4'), self.archive(corrupt=True)):
                with self.subTest(raw=hashlib.sha256(raw).hexdigest()), self.assertRaises(ValueError):
                    p.verify_archive(raw, '0.0.3', 'a'*40, api)
            run['conclusion'] = 'failure'
            with self.assertRaises(ValueError):
                p.verify_archive(self.archive(), '0.0.3', 'a'*40, api)


class ModernManifests(unittest.TestCase):
    def test_restarted_version_requires_installer_and_portable_assets(self):
        names = p.asset_names('0.1.0')
        self.assertEqual(len(names), 8)
        self.assertIn('WW-StudyApp-0.1.0-windows-x86_64.msi', names)
        self.assertIn('WW-StudyApp-0.1.0-windows-x86_64-portable.zip', names)

    def fixture(self):
        base = Path(__file__).resolve().parents[1] / '.scratch' / 'pipeline-tests'
        base.mkdir(parents=True, exist_ok=True)
        root = Path(tempfile.mkdtemp(dir=base))
        stem = 'WW-StudyApp-0.1.0-windows-x86_64'
        names = [stem + suffix for suffix in ('.zip', '-portable.zip', '.msi', '-Setup.exe')]
        for name in names:
            payload = ('verified ' + name).encode()
            (root / name).write_bytes(payload)
            (root / (name + '.sha256')).write_text(hashlib.sha256(payload).hexdigest() + '  ' + name + '\n')
        return root, names

    def test_exact_eight_assets_and_every_checksum_required(self):
        root, names = self.fixture()
        manifest = p.make_manifest(root, '0.1.0', 'a'*40, 11, 'owner/repo')
        self.assertEqual(set(manifest['assets']), {n for name in names for n in (name, name + '.sha256')})
        for name in names:
            with self.subTest(name=name):
                path = root / (name + '.sha256')
                original = path.read_bytes()
                path.write_text('0'*64 + '  ' + name)
                with self.assertRaises(ValueError):
                    p.make_manifest(root, '0.1.0', 'a'*40, 11, 'owner/repo')
                path.write_bytes(original)

    def test_modern_provenance_verifies_each_payload(self):
        root, names = self.fixture()
        manifest = p.make_manifest(root, '0.1.0', 'a'*40, 11, 'owner/repo')
        (root / 'release-manifest.json').write_text(json.dumps(manifest))
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, 'w') as archive:
            for path in root.iterdir():
                archive.writestr(path.name, path.read_bytes())
        run = {'id': 11, 'head_sha': 'a'*40, 'event': 'push', 'conclusion': 'success', 'head_branch': 'main',
               'head_repository': {'full_name': 'owner/repo'}, 'path': '.github/workflows/ci.yml'}
        api = SimpleNamespace(repository='owner/repo', run=lambda _: run,
            jobs=lambda _: [{'name': 'Checked Windows release package', 'conclusion': 'success'}])
        with patch.object(p.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), patch.dict(p.os.environ, {'GITHUB_SHA': 'a'*40}):
            self.assertEqual(p.verify_archive(raw.getvalue(), '0.1.0', 'a'*40, api), manifest)
        for name in names:
            with self.subTest(name=name):
                path = root / name
                original = path.read_bytes()
                path.write_bytes(b'wrong package')
                with self.assertRaises(ValueError):
                    p.verify_manifest(root, manifest, '0.1.0', 'a'*40, 'owner/repo', 11)
                path.write_bytes(original)


class RedirectSecurity(unittest.TestCase):
    def test_signed_storage_redirect_never_receives_github_token(self):
        handler = p.SafeRedirect()
        request = p.urllib.request.Request('https://api.github.com/repos/owner/repo/actions/artifacts/1/zip', headers={'Authorization': 'Bearer secret'})
        redirected = handler.redirect_request(request, None, 302, '', {}, 'https://productionresultssa1.blob.core.windows.net/signed')
        self.assertIsNone(redirected.get_header('Authorization'))
        same = handler.redirect_request(request, None, 302, '', {}, 'https://api.github.com/another')
        self.assertEqual(same.get_header('Authorization'), 'Bearer secret')
        for target in ('http://productionresultssa1.blob.core.windows.net/signed', 'https://attacker.example/file'):
            with self.subTest(target=target), self.assertRaises(ValueError):
                handler.redirect_request(request, None, 302, '', {}, target)


class FakePublishAPI:
    repository = 'owner/repo'
    base = 'https://api.github.com/repos/owner/repo'

    def __init__(self):
        self.releases = []
        self.files = {}
        self.created = 0
        self.edited = 0
        self.uploads = 0
        self.fail_upload = None

    def add_draft(self, tag):
        self.created += 1
        release = {'id': self.created, 'tag_name': tag, 'draft': True,
                   'upload_url': f'https://uploads.github.com/repos/owner/repo/releases/{self.created}/assets{{?name,label}}'}
        self.releases.append(release)
        self.files[release['id']] = {}
        return release

    def pages(self, path, key):
        if path == '/releases' and key is None:
            return list(self.releases)
        raise AssertionError(path)

    def request(self, path, method='GET', data=None):
        if path.startswith('/releases/tags/'):
            # API tag lookup can return404 for an existing draft, the original defect.
            raise p.urllib.error.HTTPError(self.base+path, 404, 'not found', {}, None)
        if path == '/releases' and method == 'POST':
            return self.add_draft(data['tag_name'])
        match = p.re.fullmatch(r'/releases/(\d+)/assets\?per_page=100', path)
        if match:
            release_id = int(match[1])
            return [{'id': release_id*100+index, 'name': name} for index, name in enumerate(self.files[release_id],1)]
        match = p.re.fullmatch(r'/releases/(\d+)', path)
        if match and method == 'PATCH':
            release = next(r for r in self.releases if r['id'] == int(match[1]))
            release.update(data)
            self.edited += 1
            return release
        raise AssertionError((path, method))

    def http_open(self, request, timeout=60):
        url = p.urllib.parse.urlparse(request.full_url)
        if url.hostname == 'uploads.github.com':
            self.uploads += 1
            if self.uploads == self.fail_upload:
                raise OSError('Interrupted upload')
            release_id = int(url.path.split('/')[5])
            name = p.urllib.parse.parse_qs(url.query)['name'][0]
            if name in self.files[release_id]:
                raise AssertionError('Attempted asset overwrite')
            self.files[release_id][name] = request.data
            return io.BytesIO(b'{}')
        asset_id = int(url.path.rsplit('/',1)[1])
        release_id, index = divmod(asset_id,100)
        return io.BytesIO(list(self.files[release_id].values())[index-1])


class PublishRecovery(unittest.TestCase):
    def setUp(self):
        Manifests.setUp(self)
        self.api = FakePublishAPI()
        self.patch = patch.object(p, 'http_open', side_effect=self.api.http_open)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.environment = patch.dict(p.os.environ, {'GITHUB_SHA': 'a'*40, 'GH_TOKEN': 'fake'})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def publish(self):
        with redirect_stdout(io.StringIO()):
            p.publish(self.api, self.root, 'v0.0.3', self.manifest)

    def test_interrupted_upload_resumes_same_draft_without_reuploading_good_asset(self):
        self.api.fail_upload = 2
        with self.assertRaises(OSError):
            self.publish()
        self.assertEqual(self.api.created, 1)
        self.assertTrue(self.api.releases[0]['draft'])
        self.assertEqual(len(self.api.files[1]), 1)
        self.api.fail_upload = None
        self.publish()
        self.assertEqual(self.api.created, 1)
        self.assertEqual(self.api.uploads, 3)
        self.assertFalse(self.api.releases[0]['draft'])
        self.assertEqual(set(self.api.files[1]), set(self.manifest['assets']))

    def test_duplicate_matching_drafts_are_rejected(self):
        self.api.add_draft('v0.0.3')
        self.api.add_draft('v0.0.3')
        with self.assertRaises(ValueError):
            self.publish()
        self.assertEqual(self.api.created,2)
        self.assertEqual(self.api.uploads,0)

    def test_published_release_rerun_only_verifies_existing_bytes(self):
        self.publish()
        before = (self.api.created,self.api.uploads,self.api.edited)
        self.publish()
        self.assertEqual((self.api.created,self.api.uploads,self.api.edited),before)
        self.api.files[1][self.name] = b'mismatch'
        with self.assertRaises(ValueError):
            self.publish()
        self.assertEqual((self.api.created,self.api.uploads,self.api.edited),before)

def load_tests(loader, tests, pattern):
    # Existing CI entry point also runs the narrowly scoped compatibility policy.
    spec = importlib.util.spec_from_file_location('fixture_proof_bridge_tests', Path(__file__).with_name('test-fixture-proof-bridge.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    tests.addTests(loader.loadTestsFromModule(module))
    return tests


if __name__ == '__main__':
    unittest.main()
