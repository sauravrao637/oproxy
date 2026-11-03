"""Process lifecycle for a single QA profile: cargo build (with the profile's
features), launch oproxy with its config/env, wait for /health, run the
profile's mapped test script(s), then tear the server down. Kept separate from
CLI/matrix concerns so it can be reused (or ported) without dragging argparse
or YAML parsing along with it.
"""

import os
import subprocess
import sys
import time

try:
    import requests
except ImportError:
    raise RuntimeError("Missing QA prerequisite: requests; install before running")

from qa_matrix import REPO_ROOT

HEALTH_TIMEOUT_SECS = 20


def binary_path():
    exe = "oproxy.exe" if os.name == "nt" else "oproxy"
    return os.environ.get("OPROXY_TEST_BINARY", os.path.join(
        os.environ.get("CARGO_TARGET_DIR", os.path.join(REPO_ROOT, "target")), "debug", exe))


def build(profile):
    cmd = ["cargo", "build", "--offline", "--locked"]
    features = profile.get("features")
    if features:
        cmd += ["--features", ",".join(features)]
    print(f"  $ {' '.join(cmd)}")
    subprocess.run(cmd, cwd=REPO_ROOT, check=True, timeout=600)


def wait_for_health(port, log_path):
    deadline = time.time() + HEALTH_TIMEOUT_SECS
    while time.time() < deadline:
        try:
            r = requests.get(f"http://127.0.0.1:{port}/health", timeout=1)
            if r.ok:
                return True
        except requests.exceptions.RequestException:
            pass
        time.sleep(0.3)
    print(f"  oproxy did not become healthy within {HEALTH_TIMEOUT_SECS}s. "
          f"Log tail ({log_path}):")
    try:
        with open(log_path, encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
        print("".join(lines[-30:]))
    except OSError:
        pass
    return False


def run_profile(name, profile):
    print(f"\n{'=' * 72}\nProfile: {name} — {profile.get('description', '')}\n{'=' * 72}")

    if not profile.get("run"):
        return {"<configuration: no required scripts>": 1}

    try:
        build(profile)
    except (OSError, subprocess.SubprocessError) as exc:
        print(f"  BLOCKED: build failed: {exc}")
        return {"<build>": 1}

    env = {**os.environ, "OPROXY_CONFIG": profile["config"], **profile.get("env", {})}
    log_path = os.path.join(REPO_ROOT, f"qa-runner-{name}.log")
    log_file = open(log_path, "w", encoding="utf-8")
    proc = None
    results = {}
    try:
        proc = subprocess.Popen([binary_path()], cwd=REPO_ROOT, env=env,
                                stdout=log_file, stderr=subprocess.STDOUT)
        if not wait_for_health(profile["port"], log_path):
            results["<startup>"] = 1
            return results

        for entry in profile.get("run", []):
            script = os.path.join(REPO_ROOT, "tests", entry["script"])
            cmd = [sys.executable, script, *entry.get("args", [])]
            print(f"\n--- {entry['script']} ---\n  $ {' '.join(cmd[1:])}")
            result = subprocess.run(cmd, cwd=REPO_ROOT, timeout=entry.get("timeout", 300))
            results[entry["script"]] = result.returncode
    except (OSError, subprocess.SubprocessError) as exc:
        print(f"  FAIL: profile execution did not complete: {exc}")
        results["<execution>"] = 1
    finally:
        if proc is not None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
        log_file.close()

    return results
