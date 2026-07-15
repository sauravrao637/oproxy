const path = require('path');

function resolveOproxyBinary(browserTestsDir, platform = process.platform) {
  const executable = platform === 'win32' ? 'oproxy.exe' : 'oproxy';
  return path.resolve(browserTestsDir, '../../target/debug', executable);
}

module.exports = { resolveOproxyBinary };
