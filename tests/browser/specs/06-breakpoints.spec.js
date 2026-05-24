// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

test.describe('Breakpoints', () => {
  test.beforeEach(async ({ request }) => {
    const rules = await (await request.get('/admin/breakpoints')).json();
    for (const rule of rules) await request.delete(`/admin/breakpoints/${rule.id}`);
  });

  test.afterEach(async ({ request }) => {
    const rules = await (await request.get('/admin/breakpoints')).json();
    for (const rule of rules) await request.delete(`/admin/breakpoints/${rule.id}`);
  });

  test('breakpoints view loads and opens add dialog', async ({ page }) => {
    await gotoRail(page, 'Breakpoints');
    await expect(page.getByText('No requests are paused.')).toBeVisible();
    await page.getByRole('button', { name: /Add breakpoint/ }).click();
    await expect(page.getByRole('heading', { name: 'Add breakpoint' })).toBeVisible();
    await expect(page.locator('[name="pattern"]')).toHaveValue('/api');
    await expect(page.locator('[name="bpType"]')).toHaveValue('Request');
  });

  test('adds breakpoint rule through UI', async ({ page, request }) => {
    await gotoRail(page, 'Breakpoints');
    await page.getByRole('button', { name: /Add breakpoint/ }).click();
    await page.locator('[name="pattern"]').fill('ui-break.example.com');
    await page.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByText('ui-break.example.com')).toBeVisible();

    const rules = await (await request.get('/admin/breakpoints')).json();
    expect(rules.some(r => r.pattern === 'ui-break.example.com')).toBeTruthy();
  });

  test('delete breakpoint rule removes it', async ({ page, request }) => {
    await request.post('/admin/breakpoints', {
      data: { id: '', pattern: 'del-break.example.com', bp_type: 'Request', enabled: true },
    });
    await gotoRail(page, 'Breakpoints');
    await expect(page.getByText('del-break.example.com')).toBeVisible();

    await page.locator('.rule-row', { hasText: 'del-break.example.com' }).getByText('×').click();
    await page.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByText('del-break.example.com')).toHaveCount(0);
  });

  test('breakpoint row toggle persists enabled state', async ({ page, request }) => {
    await request.post('/admin/breakpoints', {
      data: { id: '', pattern: 'toggle-break.example.com', bp_type: 'Request', enabled: true },
    });
    await gotoRail(page, 'Breakpoints');
    const toggle = page.getByLabel('Toggle rule toggle-break.example.com');
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');

    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    const rules = await (await request.get('/admin/breakpoints')).json();
    const rule = rules.find(r => r.pattern === 'toggle-break.example.com');
    expect(rule).toBeTruthy();
    expect(rule.enabled).toBe(false);
  });

  test('Disable all turns off rules and releases held requests', async ({ page, request }) => {
    await request.post('/admin/breakpoints', {
      data: { id: '', pattern: 'disable-all-release.example.com', bp_type: 'Request', enabled: true },
    });
    await gotoRail(page, 'Breakpoints');

    // Ensure no stale pending entries interfere with this case.
    const pendingBefore = await (await request.get('/admin/breakpoints/pending')).json();
    for (const p of pendingBefore) {
      await request.post(`/admin/breakpoints/pending/${encodeURIComponent(p.id)}/resolve`, {
        data: { action: 'continue' },
      });
    }

    // Trigger one request that matches the rule so it enters the held queue.
    await page.request.get('http://disable-all-release.example.com/');
    await page.waitForTimeout(200);
    const pendingNow = await (await request.get('/admin/breakpoints/pending')).json();
    expect(pendingNow.length).toBeGreaterThan(0);

    await page.getByRole('button', { name: 'Disable all' }).click();
    await page.waitForTimeout(250);

    const rules = await (await request.get('/admin/breakpoints')).json();
    expect(rules.find(r => r.pattern === 'disable-all-release.example.com')?.enabled).toBeFalsy();
    const pendingAfter = await (await request.get('/admin/breakpoints/pending')).json();
    expect(pendingAfter.length).toBe(0);
  });
});
