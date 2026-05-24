const { spawn } = require('child_process');
const path = require('path');

module.exports = async function globalSetup() {
  const bin = path.resolve(__dirname, '../../target/debug/oproxy');
  const proc = spawn(bin, [], {
    env: { ...process.env, OPROXY_PORT: '18080' },
    stdio: 'pipe',
  });

  global.__OPROXY_PID__ = proc.pid;
  // Write PID to file so teardown can kill it
  require('fs').writeFileSync(path.join(__dirname, '.pid'), String(proc.pid));

  // Wait until server is accepting connections
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('oproxy failed to start in 10s')), 10000);
    const tryConnect = () => {
      const http = require('http');
      const req = http.get('http://localhost:18080/health', (res) => {
        clearTimeout(timeout);
        resolve();
      });
      req.on('error', () => setTimeout(tryConnect, 200));
    };
    proc.on('error', reject);
    setTimeout(tryConnect, 300);
  });
};
