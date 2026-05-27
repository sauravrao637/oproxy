// @ts-check
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
    video: 'retain-on-failure',
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
