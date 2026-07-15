// @ts-check
//
// Do not use page.waitForLoadState('networkidle') / { waitUntil: 'networkidle' }
// anywhere against this app. The Sessions view opens
// `new EventSource('/api/sessions/stream')`
// (see src/design/app.jsx, src/design/surfaces.jsx) as soon as it mounts and
// keeps that connection open indefinitely for live updates - Playwright (and
// Chromium's underlying network-idle tracking) counts an open, still-streaming
// response as outstanding network activity, so "networkidle" never resolves
// on any page that has the Sessions view mounted; it will hang until its own
// timeout. This is also why Playwright's own docs discourage networkidle in
// general (WebSockets, long-polling, and analytics beacons have the same
// effect) - it isn't specific to a bug in this app.
//
// None of the existing specs under ./specs use networkidle - they rely on
// Playwright's auto-waiting assertions (expect(...).toBeVisible(), etc.) or
// explicit waitForSelector/waitForResponse, which don't have this problem.
// Keep doing that. If a future test genuinely needs to wait for the initial
// session list fetch specifically, wait for that one response
// (page.waitForResponse(/\/api\/sessions/)) or a concrete DOM state, not for
// the network to go idle as a whole.
const { defineConfig } = require('@playwright/test');

const chromiumExecutablePath = process.env.CHROMIUM_EXECUTABLE_PATH;

module.exports = defineConfig({
  testDir: './specs',
  timeout: 30000,
  workers: 1,
  retries: 1,
  reporter: [['list'], ['html', { outputFolder: 'report', open: 'never' }]],
  use: {
    baseURL: process.env.OPROXY_BASE_URL || 'http://localhost:18080',
    headless: true,
    viewport: { width: 1400, height: 900 },
    screenshot: 'only-on-failure',
    video: process.env.OPROXY_E2E_VIDEO || 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        browserName: 'chromium',
        launchOptions: chromiumExecutablePath ? { executablePath: chromiumExecutablePath } : undefined,
      },
    },
  ],
  globalSetup: process.env.OPROXY_SKIP_GLOBAL_SETUP ? undefined : require.resolve('./global-setup.js'),
  globalTeardown: process.env.OPROXY_SKIP_GLOBAL_SETUP ? undefined : require.resolve('./global-teardown.js'),
});
