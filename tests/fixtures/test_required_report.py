"""Required coverage cannot be replaced by skipped, absent or duplicate cases."""

import contextlib
import importlib
import io
import sys
import unittest
import ast
from pathlib import Path
from unittest.mock import patch


class RequiredReportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with patch.object(sys, "argv", ["e2e_protocol_test.py"]):
            cls.suite = importlib.import_module("e2e_protocol_test")

    def verdict(self, results):
        with patch.object(self.suite, "RESULTS", results), contextlib.redirect_stdout(io.StringIO()):
            return self.suite.print_report()

    def test_only_complete_unique_passes_succeed(self):
        results = [{"name": name, "status": "PASS", "detail": ""}
                   for name in self.suite.REQUIRED_CASES]
        self.assertEqual(self.verdict(results), 0)
        for incomplete in ([], results[:-1], results + [results[0]]):
            self.assertNotEqual(self.verdict(incomplete), 0)
        for status in ("SKIP", "FAIL", "invalid"):
            altered = [dict(r) for r in results]
            altered[0]["status"] = status
            self.assertNotEqual(self.verdict(altered), 0)

    def test_protocol_summary_rejects_skips_empty_and_duplicates(self):
        # Load the reporting function without starting protocol fixture dependencies.
        source = Path(__file__).resolve().parents[1] / "proxy_tester.py"
        tree = ast.parse(source.read_text(encoding="utf-8"))
        function = next(n for n in tree.body if isinstance(n, ast.FunctionDef)
                        and n.name == "_print_final_summary")
        namespace = {}
        exec(compile(ast.Module(body=[function], type_ignores=[]), str(source), "exec"), namespace)
        report = namespace["_print_final_summary"]
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(report([("HTTP", "response", True)]))
            for results in ([], [("HTTP", "response", None)],
                            [("HTTP", "response", False)],
                            [("HTTP", "response", True)] * 2):
                self.assertFalse(report(results))


if __name__ == "__main__":
    unittest.main()
