// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Settings', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
  });

  test('settings view renders', async ({ page }) => {
    await expect(page.getByText('Listener')).toBeVisible();
    await expect(page.getByText('Client proxy')).toBeVisible();
  });

  test('GET /admin/config returns config JSON', async ({ request }) => {
    const res = await request.get('/admin/config');
    expect(res.ok()).toBeTruthy();
    const cfg = await res.json();
    expect(cfg).toHaveProperty('port');
  });

  test('GET /admin/upstream-proxy returns JSON', async ({ request }) => {
    const res = await request.get('/admin/upstream-proxy');
    expect(res.ok()).toBeTruthy();
  });

  test('GET /admin/socks5/status returns JSON', async ({ request }) => {
    const res = await request.get('/admin/socks5/status');
    expect(res.ok()).toBeTruthy();
  });
});
