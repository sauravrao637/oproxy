// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail, resetWorkspace } = require('./helpers');

const location = (overrides = {}) => ({
  host: null,
  path: null,
  port: null,
  protocol: null,
  query: null,
  methods: [],
  mode: 'glob',
  ...overrides,
});

test.describe('Rules / mapping and access', () => {
  test.beforeEach(async ({ request }) => {
    await resetWorkspace(request);
    // Clean up all rule-sets so the empty-state text is visible
    const ruleSets = await (await request.get('/admin/rule-sets')).json();
    for (const r of ruleSets) await request.delete(`/admin/rule-sets/${r.id}`);
  });

  test.afterEach(async ({ request }) => {
    for (const endpoint of ['map-remote-rules', 'map-local-rules', 'access-rules', 'rule-sets']) {
      const rules = await (await request.get(`/admin/${endpoint}`)).json();
      for (const rule of rules) {
        if (String(rule.name || '').startsWith('ui-') || String(rule.name || '').startsWith('Assistant')) {
          await request.delete(`/admin/${endpoint}/${rule.id}`);
        }
      }
    }
  });

  test('rule sets tab is active by default', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await expect(page.getByRole('button', { name: 'Rule sets', exact: true })).toHaveClass(/on/);
    await expect(page.getByText('Rule sets match by location')).toBeVisible();
  });

  test('can add a Map Remote rule via API and see it', async ({ request, page }) => {
    const host = `ui-route-${Date.now()}.example.com`;
    const res = await request.post('/admin/map-remote-rules', {
      data: {
        id: '',
        name: 'ui-map-remote',
        enabled: true,
        location: location({ host }),
        destination: 'http://new.example.com',
      },
    });
    expect(res.ok()).toBeTruthy();

    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: 'Map Remote', exact: true }).click();
    await expect(page.locator('.col-match').filter({ hasText: host })).toBeVisible();
    await expect(page.getByText('http://new.example.com')).toBeVisible();
  });

  test('Map Local and Access tabs switch view', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: 'Map Local', exact: true }).click();
    await expect(page.getByText('No Map Local rules')).toBeVisible();
    await page.getByRole('button', { name: 'Access', exact: true }).click();
    await expect(page.getByText('Block rules 403 matching requests.')).toBeVisible();
  });

  test('add Access rule via API appears in UI', async ({ request, page }) => {
    const res = await request.post('/admin/access-rules', {
      data: {
        id: '',
        name: 'ui-access-block',
        enabled: true,
        location: location({ host: 'blocked.example.com' }),
        action: 'block',
      },
    });
    expect(res.ok()).toBeTruthy();

    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: 'Access', exact: true }).click();
    await expect(page.getByText('blocked.example.com')).toBeVisible();
    await expect(page.locator('.rule-row', { hasText: 'blocked.example.com' }).getByText('BLOCK', { exact: true })).toBeVisible();
  });
});
