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
