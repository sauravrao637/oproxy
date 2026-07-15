"""Loads and validates tests/qa-matrix.yaml — the profile → config/env/features/
tests mapping used by qa_runner.py. Kept separate from process orchestration so
the matrix format can be inspected/tested without spinning up any process.
"""

import os
import subprocess
import sys

try:
    import yaml
except ImportError:
    subprocess.run([sys.executable, "-m", "pip", "install", "pyyaml"], check=True)
    import yaml

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MATRIX_PATH = os.path.join(REPO_ROOT, "tests", "qa-matrix.yaml")


def load_matrix():
    with open(MATRIX_PATH, encoding="utf-8") as f:
        return yaml.safe_load(f)["profiles"]
