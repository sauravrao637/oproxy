// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

test.describe('Throttle', () => {
  test.afterEach(async ({ request }) => {
    await request.post('/admin/throttling', { data: { latency_ms: 0, bandwidth_limit_kbps: 0, enabled: false } });
  });

  test('throttle controls render under Rules', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Throttling/ }).click();
    await expect(page.getByRole('heading', { name: 'Network throttling' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Apply throttling' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save throttle' })).toHaveCount(0);
    const presets = page.locator('.preset-row');
    await expect(presets.getByRole('button', { name: 'Off' })).toHaveCount(0);
    await expect(presets.getByRole('button', { name: 'Offline' })).toHaveCount(0);
    await expect(page.getByLabel('Throttle upload kilobits per second')).toHaveCount(0);
    await expect(page.getByLabel('Throttle jitter milliseconds')).toHaveCount(0);
  });

  test('GET /admin/throttling returns JSON with enabled field', async ({ request }) => {
    const res = await request.get('/admin/throttling');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body).toHaveProperty('enabled');
  });

  test('can set and clear throttle via API', async ({ request }) => {
    const set = await request.post('/admin/throttling', {
      data: { latency_ms: 500, bandwidth_limit_kbps: 0, enabled: true },
    });
    expect(set.ok()).toBeTruthy();
    const clear = await request.post('/admin/throttling', {
      data: { latency_ms: 0, bandwidth_limit_kbps: 0, enabled: false },
    });
    expect(clear.ok()).toBeTruthy();
  });

  test('UI has one real off control and presets enable persisted throttling', async ({ page, request }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Throttling/ }).click();

    const toggle = page.getByLabel('Enable network throttling');
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');

    await page.getByRole('button', { name: 'Wifi' }).click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');
    await page.getByRole('button', { name: 'Apply throttling' }).click();

    let state = await (await request.get('/admin/throttling')).json();
    expect(state.enabled).toBe(true);
    expect(state.latency_ms).toBe(2);
    expect(state.bandwidth_limit_kbps).toBe(30000);

    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await page.getByRole('button', { name: 'Apply throttling' }).click();

    state = await (await request.get('/admin/throttling')).json();
    expect(state.enabled).toBe(false);
  });
});
