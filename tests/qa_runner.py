#!/usr/bin/env python3
"""
QA profile runner — single-step "build + launch + test + teardown" for the
configs/qa-*.yaml profiles described in tests/qa-matrix.yaml.

Usage:
    python tests/qa_runner.py <profile>      # e.g. e2e, admin, listeners, limits
    python tests/qa_runner.py all            # every profile, one after another
    python tests/qa_runner.py --list         # show available profiles

Adding a new profile means adding one entry to tests/qa-matrix.yaml (config
path, port, optional cargo features/env, and which test scripts to run with
which args) - no changes needed here.

This file is just the CLI: matrix loading lives in qa_matrix.py, process
build/launch/health/teardown lives in qa_process.py.

Requires: requests, PyYAML (auto-installed if missing)
"""

import argparse
import sys

# stdout is only line-buffered by Python when it's a real terminal; redirected
# to a file/pipe (a log file, CI, this being run non-interactively) it's fully
# block-buffered, so our own progress prints would sit invisible until exit
# while child processes' inherited-fd output streams through immediately -
# making a live run look hung and a captured log look reordered.
sys.stdout.reconfigure(line_buffering=True)

from qa_matrix import load_matrix
from qa_process import run_profile


def main():
    parser = argparse.ArgumentParser(description="Run oproxy QA profiles end to end")
    parser.add_argument("profile", nargs="?", help="Profile name from qa-matrix.yaml, or 'all'")
    parser.add_argument("--list", action="store_true", help="List available profiles and exit")
    args = parser.parse_args()

    matrix = load_matrix()

    if args.list or not args.profile:
        print("Available profiles:")
        for name, profile in matrix.items():
            print(f"  {name:12s} {profile.get('description', '')}")
        return 0 if args.list else 1

    names = list(matrix.keys()) if args.profile == "all" else [args.profile]
    unknown = [n for n in names if n not in matrix]
    if unknown:
        print(f"Unknown profile(s): {', '.join(unknown)}. Use --list to see options.")
        return 1

    all_results = {name: run_profile(name, matrix[name]) for name in names}

    print(f"\n{'=' * 72}\nSummary\n{'=' * 72}")
    failed = False
    for profile_name, scripts in all_results.items():
        for script_name, code in scripts.items():
            status = "PASS" if code == 0 else "FAIL"
            failed = failed or code != 0
            print(f"  [{status}] {profile_name} / {script_name}")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
