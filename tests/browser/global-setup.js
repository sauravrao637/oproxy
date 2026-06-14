const { spawn } = require('child_process');
const net = require('net');
const fs = require('fs');
const http = require('http');
const path = require('path');

const STARTUP_TIMEOUT_MS = 15000;

// Keep the spawned port in sync with the Playwright baseURL.
const baseURL = process.env.OPROXY_BASE_URL || 'http://localhost:18080';
const PORT = new URL(baseURL).port || '18080';

// Quick TCP probe: resolves true if something is already listening on `port`.
function portInUse(port) {
  return new Promise((resolve) => {
    const socket = net
      .connect({ host: '127.0.0.1', port: Number(port) })
      .once('connect', () => {
        socket.destroy();
        resolve(true);
      })
      .once('error', () => resolve(false));
    socket.setTimeout(500, () => {
      socket.destroy();
      resolve(false);
    });
  });
}

module.exports = async function globalSetup() {
  const bin = path.resolve(__dirname, '../../target/debug/oproxy');

  if (!fs.existsSync(bin)) {
    throw new Error(
      `oproxy binary not found at ${bin}.\n` +
        `Build it first: \`cargo build\` (or run \`make test-ui\`, which builds it).`,
    );
  }

  // oproxy is spawned with cwd = this directory, so its relative `./storage` and
  // `./certs` resolve to the fixtures here. But `Config::load()` defaults to
  // `./configs/default.yaml` and panics if it's missing, so point it at the
  // repo's config explicitly (absolute path). Respect a caller-set OPROXY_CONFIG.
  const configPath =
    process.env.OPROXY_CONFIG || path.resolve(__dirname, '../../configs/default.yaml');
  if (!fs.existsSync(configPath)) {
    throw new Error(`oproxy config not found at ${configPath}.`);
  }

  // Fail fast (instead of after a 10s health timeout) if the port is already
  // taken — almost always a leftover oproxy from a run that wasn't torn down.
  if (await portInUse(PORT)) {
    throw new Error(
      `Port ${PORT} is already in use — a leftover oproxy is likely still running, ` +
        `so the test server can't bind it.\n` +
        `  lsof -i :${PORT}   # find it\n` +
        `  pkill -f target/debug/oproxy && rm -f tests/browser/.pid`,
    );
  }

  const proc = spawn(bin, [], {
    env: { ...process.env, OPROXY_PORT: PORT, OPROXY_CONFIG: configPath },
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  // Buffer the child's output so a startup failure (e.g. "Address already in
  // use") is surfaced instead of a blind 10s timeout.
  let output = '';
  const capture = (chunk) => {
    output += chunk.toString();
  };
  proc.stdout.on('data', capture);
  proc.stderr.on('data', capture);

  global.__OPROXY_PID__ = proc.pid;
  fs.writeFileSync(path.join(__dirname, '.pid'), String(proc.pid));

  await new Promise((resolve, reject) => {
    let settled = false;
    const finish = (fn, arg) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn(arg);
    };

    const timer = setTimeout(() => {
      finish(
        reject,
        new Error(
          `oproxy did not answer http://localhost:${PORT}/health within ` +
            `${STARTUP_TIMEOUT_MS / 1000}s.\n` +
            `Most common cause: port ${PORT} is already in use by a leftover oproxy ` +
            `(a previous run that wasn't torn down).\n` +
            `  lsof -i :${PORT}   # find it\n` +
            `  pkill -f target/debug/oproxy && rm -f tests/browser/.pid\n` +
            `\n--- oproxy output ---\n${output || '(no output captured)'}`,
        ),
      );
    }, STARTUP_TIMEOUT_MS);

    // If the process dies before becoming healthy, fail immediately with its output.
    proc.on('exit', (code, signal) => {
      finish(
        reject,
        new Error(
          `oproxy exited before becoming healthy (code=${code}, signal=${signal}).\n` +
            `--- oproxy output ---\n${output || '(no output captured)'}`,
        ),
      );
    });
    proc.on('error', (err) => finish(reject, err));

    const tryConnect = () => {
      if (settled) return;
      const req = http.get(`http://localhost:${PORT}/health`, (res) => {
        res.resume();
        finish(resolve);
      });
      req.on('error', () => setTimeout(tryConnect, 200));
    };
    setTimeout(tryConnect, 300);
  });
};
