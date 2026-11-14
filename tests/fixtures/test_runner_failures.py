"""Real child exits must reach qa_runner's final failing verdict.

Requires the tested binary, curl and provisioned Python suite dependencies.
Run with PYTHONPATH=tests python -m unittest discover -s tests/fixtures
-p test_runner_failures.py -v. Cargo builds are bypassed only in the fault driver.
"""

from pathlib import Path
import subprocess
import sys
import unittest


class RunnerFailureTests(unittest.TestCase):
    def test_missing_runner_module_fails_before_matrix_import(self):
        script = """import builtins
original = builtins.__import__
def importing(name, *args, **kwargs):
    if name == 'yaml':
        raise ModuleNotFoundError('injected missing yaml prerequisite')
    if name == 'qa_matrix':
        raise AssertionError('matrix imported before preflight')
    return original(name, *args, **kwargs)
builtins.__import__ = importing
import qa_runner
"""
        result = subprocess.run([sys.executable, "-c", script], capture_output=True,
                                text=True, encoding="utf-8", timeout=10)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("missing QA prerequisite", result.stderr)
        self.assertIn("injected missing yaml", result.stderr)

    def test_timeout_reaps_live_isolated_proxy_and_scratch(self):
        result = self.run_fault("timeout")
        combined = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, combined)
        self.assertIn("[FAIL] fault /", combined)
        self.assertIn("Timeout cleanup verified", combined)

    def run_fault(self, fault):
        driver = Path(__file__).with_name("runner_fault_driver.py")
        return subprocess.run([sys.executable, str(driver), "runner", fault],
                              capture_output=True, text=True, encoding="utf-8", timeout=65)

    def test_real_child_failures_propagate(self):
        for fault, diagnostic in (
                ("report-pass", "61/61 tests passed"),
                ("report-missing", "Incomplete required coverage"),
                ("report-zero", "Incomplete required coverage"),
                ("report-duplicate", "FINAL TEST SUMMARY"),
                ("report-skip", "SKIP"),
                ("missing-curl", "Missing required prerequisite: curl"),
                ("missing-binary", "Missing tested binary"),
                ("missing-fixture-module", "injected missing fixture prerequisite: trustme"),
                ("client-trust", "Untrusted client certificate rejected: curl exit60"),
                ("rule-cleanup", "Rule cleanup failed (/admin/mock/rules): deleted rule still present"),
                ("stopped-origin", "AssertionError"),
                ("wrong-response", "Unexpected HTTPS payload"),
                ("suppressed-capture", "No completed GET capture")):
            with self.subTest(fault=fault):
                result = self.run_fault(fault)
                combined = result.stdout + result.stderr
                self.assertIn(diagnostic, combined)
                self.assertEqual(result.returncode, 0 if fault == "report-pass" else 1, combined)
                self.assertIn("[PASS] fault /" if fault == "report-pass" else "[FAIL] fault /", combined)


if __name__ == "__main__":
    unittest.main()
