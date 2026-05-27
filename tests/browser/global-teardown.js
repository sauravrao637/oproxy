const fs = require('fs');
const path = require('path');

module.exports = async function globalTeardown() {
  const pidFile = path.join(__dirname, '.pid');
  try {
    const pid = parseInt(fs.readFileSync(pidFile, 'utf8'));
    process.kill(pid, 'SIGTERM');
    fs.unlinkSync(pidFile);
  } catch {}
};
