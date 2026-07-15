// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

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

const ruleSet = (overrides = {}) => ({
  id: '',
  name: 'test-rw-ui',
  enabled: true,
  applies_to: 'request',
  location: location({ host: 'rw-test.example.com' }),
  actions: [{ type: 'set_header', name: 'X-Rewrite', value: 'yes' }],
  ...overrides,
});

test.describe('Rules / rewrites', () => {
  test.afterEach(async ({ request }) => {
    const rules = await (await request.get('/admin/rule-sets')).json();
    for (const rule of rules) {
      if (String(rule.name || '').startsWith('test-rw')) {
        await request.delete(`/admin/rule-sets/${rule.id}`);
      }
    }
  });

  test('rules view exposes current location-based tabs', async ({ page, request }) => {
    // First-run seeding (src/examples.rs) writes one disabled "Example: Map
    // api.test.com to github.com" rule to map_remote_rules.json the first
    // time oproxy starts against a storage dir that doesn't have that file
    // yet - which is exactly the state of a fresh CI checkout, since
    // Clear Map Remote rules so the empty state is deterministic in fresh and
    // reused test environments. This is the only spec that changes these rules.
    const existing = await (await request.get('/admin/map-remote-rules')).json();
    for (const rule of existing) {
      await request.delete(`/admin/map-remote-rules/${rule.id}`);
    }

    await gotoRail(page, 'Rules');
    await expect(page.getByRole('button', { name: 'Rule sets', exact: true })).toHaveClass(/on/);
    await page.getByRole('button', { name: 'Map Remote', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Map Remote', exact: true })).toHaveClass(/on/);
    await expect(page.getByText('No Map Remote rules')).toBeVisible();
    await page.getByRole('button', { name: 'Access', exact: true }).click();
    await expect(page.getByText('Block rules 403 matching requests.')).toBeVisible();
  });

  test('Add rule opens unified rule-set form', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Add rule/ }).click();
    const dialog = page.locator('.ui-dialog');
    await expect(page.getByRole('heading', { name: 'New rule set' })).toBeVisible();
    await expect(page.getByPlaceholder('e.g. Add CORS headers')).toBeVisible();
    await expect(dialog.getByText(/Actions/)).toBeVisible();
    await page.getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByRole('heading', { name: 'New rule set' })).toHaveCount(0);
  });

  test('create request rewrite rule via API and see it in UI', async ({ request, page }) => {
    const res = await request.post('/admin/rule-sets', { data: ruleSet() });
    expect(res.ok()).toBeTruthy();

    await gotoRail(page, 'Rules');
    await expect(page.getByText('rw-test.example.com')).toBeVisible();
    await expect(page.getByText('set X-Rewrite')).toBeVisible();
  });

  test('create response status rewrite via API and see it in UI', async ({ request, page }) => {
    const res = await request.post('/admin/rule-sets', {
      data: ruleSet({
        name: 'test-rw-status',
        applies_to: 'response',
        location: location({ path: '/ui-mod' }),
        actions: [{ type: 'set_status', code: 203 }],
      }),
    });
    expect(res.ok()).toBeTruthy();

    await gotoRail(page, 'Rules');
    await expect(page.getByText('/ui-mod')).toBeVisible();
    await expect(page.getByText('203')).toBeVisible();
  });
});
