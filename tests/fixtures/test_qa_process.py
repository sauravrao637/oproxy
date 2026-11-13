"""Fault injection for required profile process failures."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import qa_process


class ProfileFailureTests(unittest.TestCase):
    def test_empty_mapping_fails_without_build(self):
        with patch.object(qa_process, "build") as build:
            result = qa_process.run_profile("empty", {})
        self.assertTrue(result)
        self.assertTrue(all(code != 0 for code in result.values()))
        build.assert_not_called()

    def test_missing_build_tool_fails(self):
        with patch.object(qa_process, "build", side_effect=FileNotFoundError("cargo")):
            result = qa_process.run_profile("missing", {"run": [{"script": "test.py"}]})
        self.assertNotEqual(result["<build>"], 0)

    def test_timed_out_child_fails_and_proxy_is_reaped(self):
        process = Mock()
        profile = {"config": "test.yaml", "port": 1234,
                   "run": [{"script": "test.py", "timeout": 1}]}
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(qa_process, "REPO_ROOT", directory), \
                 patch.object(qa_process, "build"), \
                 patch.object(qa_process, "wait_for_health", return_value=True), \
                 patch.object(qa_process.subprocess, "Popen", return_value=process), \
                 patch.object(qa_process, "run_tester", side_effect=
                              subprocess.TimeoutExpired("test.py", 1)):
                result = qa_process.run_profile("timeout", profile)
            self.assertNotEqual(result["<execution>"], 0)
            process.terminate.assert_called_once()
            process.wait.assert_called_once_with(timeout=10)


if __name__ == "__main__":
    unittest.main()
