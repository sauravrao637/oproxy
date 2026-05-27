// @ts-check
const { test, expect } = require('@playwright/test');

const SESSIONS_API = '**/api/sessions?*';

const sessionFixture = {
  id: 'smoke-session-1',
  timestamp: '2026-05-20T00:00:00.000Z',
  request: {
    method: 'GET',
    uri: 'https://api.example.test/v1/items?debug=1',
    host: 'api.example.test',
    headers: { accept: 'application/json' },
  },
  response: {
    status: 200,
    headers: { 'content-type': 'application/json' },
    body: '{"ok":true}',
    request_uri: 'https://api.example.test/v1/items?debug=1',
  },
  metrics: {
    status_code: 200,
    latency_ms: 42,
    request_size_bytes: 12,
    response_size_bytes: 11,
  },
};

test.describe('beta UI smoke', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
  });

  test('loads the developer workbench with truthful topbar controls', async ({ page }) => {
    await expect(page).toHaveTitle(/oproxy/);
    await expect(page.getByText('oproxy / traffic')).toBeVisible();
    await expect(page.getByTitle('Live refresh on (click to pause) · Space')).toBeVisible();
    await expect(page.getByTitle('Export as HAR')).toBeVisible();
    await expect(page.getByTitle('Save session log (.har)')).toHaveCount(0);
    await expect(page.getByText('command palette')).toHaveCount(0);
  });

  test('renders captured sessions from the API', async ({ page }) => {
    await page.route(SESSIONS_API, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          sessions: [sessionFixture],
        }),
      });
    });

    await page.reload();
    const sessionRow = page.locator('tbody tr').filter({ hasText: 'api.example.test' });
    await expect(sessionRow).toHaveCount(1);
    await expect(sessionRow).toContainText('GET');
    await expect(sessionRow).toContainText('200');
    await expect(sessionRow).toContainText('/v1/items');
  });

  test('distinguishes empty, filtered, and failed session states', async ({ page }) => {
    await page.route(SESSIONS_API, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions: [] }),
      });
    });
    await page.reload();
    await expect(page.getByText('No sessions captured yet.')).toBeVisible();
    await expect(page.getByText('No sessions match the current filters.')).toHaveCount(0);

    await page.unroute(SESSIONS_API);
    await page.route(SESSIONS_API, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions: [sessionFixture] }),
      });
    });
    await page.reload();
    await page.getByPlaceholder('Filter requests by method, host, path, status, or tag').fill('does-not-match');
    await expect(page.getByText('No sessions match the current filters.')).toBeVisible();

    await page.unroute(SESSIONS_API);
    await page.route(SESSIONS_API, async route => {
      await route.fulfill({ status: 500, contentType: 'text/plain', body: 'boom' });
    });
    await page.reload();
    await expect(page.getByText('Session API unavailable.')).toBeVisible();
  });

  test('surfaces runtime polling failures in status bar and settings', async ({ page }) => {
    await page.route('**/admin/config', async route => {
      await route.fulfill({ status: 503, contentType: 'text/plain', body: 'config down' });
    });
    await page.route('**/admin/throttling', async route => {
      await route.fulfill({ status: 502, contentType: 'text/plain', body: 'throttle down' });
    });
    await page.route('**/admin/upstream-proxy', async route => {
      await route.fulfill({ status: 503, contentType: 'text/plain', body: 'upstream down' });
    });
    await page.route('**/admin/socks5/status', async route => {
      await route.fulfill({ status: 504, contentType: 'text/plain', body: 'socks down' });
    });
    await page.route('**/admin/ca', async route => {
      await route.fulfill({ status: 500, contentType: 'text/plain', body: 'ca down' });
    });

    await page.reload();
    await expect(page.getByTitle(/Runtime API degraded: config: HTTP 503/)).toBeVisible();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByText('Settings API degraded.')).toBeVisible();
    await expect(page.getByText(/config: HTTP 503/)).toBeVisible();
    await expect(page.getByText(/upstream proxy: HTTP 503/)).toBeVisible();
    await expect(page.getByText(/socks5: HTTP 504/)).toBeVisible();
  });

  test('shortcut modal only advertises implemented beta shortcuts', async ({ page }) => {
    await page.getByTitle('Keyboard shortcuts · ?').click();
    await expect(page.getByRole('heading', { name: 'Keyboard shortcuts' })).toBeVisible();
    await expect(page.getByText('Focus search')).toHaveCount(2);
    await expect(page.getByText('Pause / resume live refresh')).toBeVisible();
    await expect(page.getByText('Command palette')).toHaveCount(0);
    await expect(page.getByText('Delete selected sessions')).toHaveCount(0);
    await expect(page.getByText('Save snapshot to disk')).toHaveCount(0);
  });

  test('clear sessions is confirmed before backend deletion', async ({ page }) => {
    let deleteCalls = 0;
    await page.route('/admin/sessions', async route => {
      if (route.request().method() === 'DELETE') {
        deleteCalls += 1;
        await route.fulfill({ status: 200, contentType: 'application/json', body: '{"cleared":true}' });
        return;
      }
      await route.continue();
    });

    await page.getByTitle('Clear all sessions').click();
    await expect(page.getByRole('heading', { name: 'Clear all captured sessions?' })).toBeVisible();
    await page.getByRole('button', { name: 'Cancel' }).click();
    expect(deleteCalls).toBe(0);

    await page.getByTitle('Clear all sessions').click();
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    expect(deleteCalls).toBe(1);
  });

  test('Root CA surface remains factual', async ({ page }) => {
    await page.getByRole('button', { name: 'Root CA', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Root CA', exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Download certificate' })).toHaveAttribute('href', '/admin/ca');
    await expect(page.getByText('Trust status')).toHaveCount(0);
    await expect(page.getByText('oproxy probes the OS keychain')).toHaveCount(0);
  });

  test('Rules surface is reachable from the homepage', async ({ page }) => {
    await page.getByRole('button', { name: 'Rules', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Rules' })).toBeVisible();
    await expect(page.getByRole('button', { name: /Add rule/ })).toBeVisible();
  });
});
