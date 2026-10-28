"""Fresh-history verification policy; no old Git objects or network required."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch
import pipeline as p

class FreshHistory(unittest.TestCase):
    def setUp(self):
        self.tree = {'Cargo.toml': b'[workspace.package]\nversion="0.1.0"\n',
                     'crates/studyapp-desktop/src/main.rs': b'app'}
        self.missing = subprocess.CalledProcessError(128, ['git', 'ls-tree'])

    def test_missing_history_cannot_project_legacy_fixture_proofs(self):
        with patch.object(p, 'tree_at', side_effect=self.missing):
            self.assertIsNone(p.bridge_projection(self.tree))

    def test_old_receipts_cannot_cross_unrelated_root(self):
        api = Mock()
        with patch.object(p, 'ancestor', return_value=False):
            for group, (run, sha) in p.FIXTURE_BRIDGE_SOURCES.items():
                proof = {'run_id': run, 'sha': sha, 'fingerprint': 'a' * 64,
                         **({'migration': True} if group == 'editor' else {})}
                self.assertIsNone(p.bridge_proof(api, proof, group, self.tree))
        api.run.assert_not_called()

    def test_empty_repository_runs_every_suite_and_builds_package(self):
        sha = 'b' * 40
        api = Mock(repository='xwwpy/WW-StudyAPP')
        api.request.return_value = {'workflow_runs': []}
        api.run.side_effect = ValueError('old run absent in new repository')
        with patch.object(p, 'tree_at', return_value=self.tree), patch.object(p, 'ancestor', return_value=False):
            plan = p.make_plan(api, sha)
        self.assertEqual(set(plan['groups']), set(p.GROUPS))
        self.assertTrue(all(item['run'] for item in plan['groups'].values()))
        self.assertIsNone(plan['package_run'])

    def test_force_all_never_consults_previous_receipts(self):
        api = Mock(repository='xwwpy/WW-StudyAPP')
        with patch.object(p, 'tree_at', return_value=self.tree):
            plan = p.make_plan(api, 'b' * 40, force=True)
        self.assertTrue(all(item['run'] for item in plan['groups'].values()))
        api.request.assert_not_called()
        api.run.assert_not_called()

    def test_pinyin_proof_requires_available_history(self):
        spec = importlib.util.spec_from_file_location('fresh_ime', Path(__file__).with_name('ime-ci.py'))
        ime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(ime)
        run, sha = p.FIXTURE_BRIDGE_IME
        proof = {'run_id': run, 'sha': sha, 'fingerprint': 'a' * 64, 'run_attempt': 1}
        with patch.object(p, 'tree_at', side_effect=self.missing):
            self.assertIsNone(ime.proof_bridge(proof, self.tree))

if __name__ == '__main__':
    unittest.main()
