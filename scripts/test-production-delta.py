"""Pure policy tests for the fixed MSI owner production delta; never run native tests."""
import importlib.util
import copy
import ast
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
import zipfile

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


class RunnerPolicy(unittest.TestCase):
    def setUp(self):
        self.assertTrue((ROOT / 'test-msi-owner-delta.py').is_file(), 'Fixed exact-test runner is not implemented')
        self.runner = module('msi_delta_runner', 'test-msi-owner-delta.py')

    def test_fixed_nonempty_inventory(self):
        listing = '\n'.join(name + ': test' for name in self.runner.EXPECTED)
        self.assertEqual(self.runner.inventory_from_list(listing), self.runner.EXPECTED)

    def test_zero_missing_duplicate_or_extra_selected_test_is_rejected(self):
        names = self.runner.EXPECTED
        for found in ([], names[:-1], [*names, names[0]], [*names, self.runner.PREFIX + 'unreviewed']):
            with self.subTest(found=found):
                with self.assertRaises(ValueError):
                    self.runner.inventory_from_list('\n'.join(name + ': test' for name in found))

    def test_unrelated_tests_are_only_listed(self):
        listing = '\n'.join(name + ': test' for name in (*self.runner.EXPECTED, 'unrelated::old_test'))
        self.assertEqual(self.runner.inventory_from_list(listing), self.runner.EXPECTED)

    def test_exact_case_result_requires_actual_one_passed_name(self):
        name = self.runner.EXPECTED[0]
        good = f'test {name} ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 406 filtered out\n'
        self.runner.validate_case(name, good)
        for bad in (good.replace(name, 'forged'), good.replace('1 passed', '0 passed'),
                    good.replace('0 ignored', '1 ignored'), good.replace('0 failed', '1 failed'), good + good):
            with self.subTest(output=bad):
                with self.assertRaises(ValueError):
                    self.runner.validate_case(name, bad)

    def test_package_manifest_and_real_msi_bytes_are_bound(self):
        import pipeline
        body = b'actual MSI bytes'
        assets = {name: 'f' * 64 for name in pipeline.asset_names('0.8.0')}
        name = 'WW-StudyApp-0.8.0-windows-x86_64.msi'
        assets[name] = hashlib.sha256(body).hexdigest()
        manifest = {'contract': 1, 'repository': 'xwwpy/WW-StudyAPP', 'sha': 'c' * 40, 'version': '0.8.0', 'run_id': 7001, 'assets': assets}
        self.assertEqual(self.runner.package_identity(manifest, body, 'c' * 40, 7001, 'xwwpy/WW-StudyAPP', '0.8.0'), assets[name])
        for change in ({'sha': 'a' * 40}, {'run_id': True}, {'run_id': 7002}, {'version': '0.8.1'},
                       {'contract': True}, {'repository': 'fork/repo'}):
            with self.subTest(change=change):
                with self.assertRaises(ValueError):
                    self.runner.package_identity(dict(manifest, **change), body, 'c' * 40, 7001, 'xwwpy/WW-StudyAPP', '0.8.0')
        with self.assertRaises(ValueError):
            self.runner.package_identity(manifest, body + b'changed', 'c' * 40, 7001, 'xwwpy/WW-StudyAPP', '0.8.0')


class FreshProductionContract(unittest.TestCase):
    def setUp(self):
        import pipeline
        import production_delta
        self.p, self.d = pipeline, production_delta
        self.tree = {'Cargo.toml': b'[workspace.package]\nversion="0.1.0"\n'}

    def test_unrelated_history_cannot_reuse_production_delta(self):
        with patch.object(self.p, 'ancestor', return_value=False), patch.object(self.p, 'tree_at') as read:
            self.assertIsNone(self.d.scope_tree(self.tree))
            read.assert_not_called()

    def test_missing_basis_cannot_prove_scope(self):
        import subprocess
        with patch.object(self.p, 'ancestor', return_value=True), patch.object(self.p, 'tree_at', side_effect=subprocess.CalledProcessError(128, ['git'])):
            self.assertIsNone(self.d.scope_tree(self.tree))

    def test_target_requires_actual_ancestry_even_with_matching_source(self):
        with patch.dict(os.environ, {'GITHUB_SHA': 'c' * 40}), patch.object(self.p, 'tree_at', return_value=self.tree), patch.object(self.p, 'ancestor', return_value=False):
            with self.assertRaises(ValueError):
                self.d.target_sha(self.tree)

    def test_initial_plan_cannot_mark_any_suite_reused(self):
        from unittest.mock import Mock
        api = Mock()
        plan = {'groups': {g: {'run': True, 'fingerprint': 'a' * 64} for g in self.p.GROUPS}}
        before = copy.deepcopy(plan)
        with patch.object(self.p, 'ancestor', return_value=False):
            self.d.extend_plan(api, plan, self.tree)
        self.assertEqual(plan, before)
        api.request.assert_not_called()

if __name__ == '__main__':
    unittest.main()
