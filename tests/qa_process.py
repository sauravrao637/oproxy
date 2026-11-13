"""Process lifecycle for a single QA profile: cargo build (with the profile's
features), launch oproxy with its config/env, wait for /health, run the
profile's mapped test script(s), then tear the server down. Kept separate from
CLI/matrix concerns so it can be reused (or ported) without dragging argparse
or YAML parsing along with it.
"""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

try:
    import requests
except ImportError:
    raise RuntimeError("Missing QA prerequisite: requests; install before running")

from qa_matrix import REPO_ROOT

HEALTH_TIMEOUT_SECS = 20


def run_tester(cmd, env, timeout):
    """Own tester descendants and scratch, including forced timeout cleanup."""
    with tempfile.TemporaryDirectory(prefix="qa-tester-") as scratch:
        env = {**env, "TEMP": scratch, "TMP": scratch, "TMPDIR": scratch}
        job = child = None
        try:
            if os.name == "nt":
                from fixtures.windows_job import WindowsJob
                job = WindowsJob()
                # Wait before launch so no descendants can escape job assignment.
                bootstrap = ("import subprocess,sys; sys.stdin.buffer.read(1); "
                             "sys.exit(subprocess.call(sys.argv[1:]))")
                child = subprocess.Popen([sys.executable, "-c", bootstrap, *cmd],
                                         cwd=REPO_ROOT, env=env, stdin=subprocess.PIPE)
                job.assign(child)
                child.stdin.write(b"1")
                child.stdin.close()
            else:
                child = subprocess.Popen(cmd, cwd=REPO_ROOT, env=env, start_new_session=True)
            return child.wait(timeout=timeout)
        except BaseException:
            if job is not None:
                job.terminate()
            elif child is not None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if child is not None:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=10)
            for path in sorted(Path(scratch).rglob("startup.log"))[:20]:
                with path.open("rb") as log:
                    log.seek(max(0, path.stat().st_size - 16384))
                    print("Timed-out tester diagnostic:\n" +
                          log.read().decode("utf-8", errors="replace"), file=sys.stderr)
            raise
        finally:
            if job is not None:
                try:
                    job.terminate()
                finally:
                    job.close()
            elif child is not None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if child is not None and child.stdin is not None:
                child.stdin.close()


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
            test_env = {**os.environ, "OPROXY_TEST_BINARY": os.path.abspath(binary_path())}
            results[entry["script"]] = run_tester(cmd, test_env, entry.get("timeout", 300))
    except (OSError, RuntimeError, subprocess.SubprocessError) as exc:
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
