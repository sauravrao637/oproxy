"""Process lifecycle for a single QA profile: cargo build (with the profile's
features), launch oproxy with its config/env, wait for /health, run the
profile's mapped test script(s), then tear the server down. Kept separate from
CLI/matrix concerns so it can be reused (or ported) without dragging argparse
or YAML parsing along with it.
"""

import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import time

try:
    import requests
    import yaml
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


def print_log_tail(log_path):
    print(f"  Proxy diagnostic ({log_path}):")
    try:
        with open(log_path, "rb") as log:
            log.seek(max(0, os.path.getsize(log_path) - 16384))
            print(log.read().decode("utf-8", errors="replace"))
    except OSError:
        pass


def wait_for_health(port, proc, ca_path, admin_token=None):
    deadline = time.monotonic() + HEALTH_TIMEOUT_SECS
    with requests.Session() as client:
        client.trust_env = False
        if admin_token:
            client.headers["x-oproxy-admin-token"] = admin_token
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                print(f"  Proxy exited before readiness: {proc.returncode}")
                return False
            try:
                url = f"http://127.0.0.1:{port}"
                if client.get(url + "/health", timeout=1).ok:
                    response = client.get(url + "/admin/ca", timeout=1)
                    response.raise_for_status()
                    owned = Path(ca_path).read_text(encoding="utf-8")
                    if ssl.PEM_cert_to_DER_cert(response.text) != ssl.PEM_cert_to_DER_cert(owned):
                        raise RuntimeError("Readiness CA does not belong to the launched proxy")
                    return proc.poll() is None
            except (requests.RequestException, FileNotFoundError):
                pass
            time.sleep(0.1)
    print(f"  Proxy readiness timed out after {HEALTH_TIMEOUT_SECS}s")
    return False


def check_listener_ports(config):
    """Reject existing TCP/QUIC listeners before starting the tested process."""
    host = config.get("bind_host", "127.0.0.1")
    listeners = [(config.get(key), socket.SOCK_STREAM)
                 for key in ("port", "https_port", "socks5_port")]
    if config.get("http3_enabled"):
        listeners.append((config.get("http3_port"), socket.SOCK_DGRAM))
    for port, kind in listeners:
        if port is None:
            continue
        family = socket.AF_INET6 if ":" in host else socket.AF_INET
        with socket.socket(family, kind) as listener:
            if os.name == "nt":
                listener.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
            listener.bind((host, port))


def run_profile(name, profile):
    print(f"\n{'=' * 72}\nProfile: {name} — {profile.get('description', '')}\n{'=' * 72}")

    if not profile.get("run"):
        return {"<configuration: no required scripts>": 1}

    try:
        build(profile)
    except (OSError, subprocess.SubprocessError) as exc:
        print(f"  BLOCKED: build failed: {exc}")
        return {"<build>": 1}

    try:
        with tempfile.TemporaryDirectory(prefix="qa-profile-") as scratch:
            return run_prepared_profile(profile, Path(scratch))
    except (OSError, ValueError, RuntimeError, yaml.YAMLError, subprocess.SubprocessError) as exc:
        print(f"  FAIL: profile preparation did not complete: {exc}")
        return {"<preparation>": 1}


def run_prepared_profile(profile, root):
    config = yaml.safe_load((Path(REPO_ROOT) / profile["config"]).read_text(encoding="utf-8"))
    if not isinstance(config, dict):
        raise ValueError("Profile config must be a mapping")
    if config.get("port") != profile["port"] or not config.get("mitm", {}).get("enabled"):
        raise ValueError("Profile requires matching listener port and MITM for owned CA readiness")
    check_listener_ports(config)
    config["storage_path"] = str(root / "storage")
    config["mitm"]["root_ca_path"] = str(root / "certs")
    config.setdefault("log", {}).update(dir=str(root), file="proxy.log")
    config["update_check"] = False
    config_path = root / "config.yaml"
    config_path.write_text(yaml.safe_dump(config), encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if not k.startswith("OPROXY_")}
    env.update(profile.get("env", {}))
    env.update(OPROXY_CONFIG=str(config_path), OPROXY_UPDATE_CHECK="false")
    log_path = root / "startup.log"
    log_file = log_path.open("w", encoding="utf-8")
    proc = None
    results = {}
    try:
        proc = subprocess.Popen([os.path.abspath(binary_path())], cwd=root, env=env,
                                stdout=log_file, stderr=subprocess.STDOUT)
        if not wait_for_health(profile["port"], proc, root / "certs" / "root.crt",
                               config.get("admin_token")):
            results["<startup>"] = 1
            return results

        for entry in profile.get("run", []):
            script = os.path.join(REPO_ROOT, "tests", entry["script"])
            cmd = [sys.executable, script, *entry.get("args", [])]
            print(f"\n--- {entry['script']} ---\n  $ {' '.join(cmd[1:])}")
            test_env = {**os.environ, "OPROXY_TEST_BINARY": os.path.abspath(binary_path())}
            results[entry["script"]] = run_tester(cmd, test_env, entry.get("timeout", 300))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
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
        if any(code != 0 for code in results.values()):
            print_log_tail(log_path)

    return results
