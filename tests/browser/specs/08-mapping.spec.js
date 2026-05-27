// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

test.describe('Rules / routes and maps', () => {
  test.afterEach(async ({ request }) => {
    const routes = await (await request.get('/admin/routes')).json();
    let routesChanged = false;
    for (const key of Object.keys(routes)) {
      if (key.startsWith('ui-route-')) {
        delete routes[key];
        routesChanged = true;
      }
    }
    if (routes['old.example.com']) {
      delete routes['old.example.com'];
      routesChanged = true;
    }
    if (routesChanged) await request.post('/admin/routes', { data: routes });
    const headers = await (await request.get('/admin/header-maps')).json();
    for (const h of headers) {
      if (h.name === 'X-Test') await request.delete(`/admin/header-maps/${h.id}`);
    }
  });

  test('routes tab is active by default', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await expect(page.getByRole('button', { name: /Routes/ })).toHaveClass(/on/);
    await expect(page.getByText('Destination')).toBeVisible();
  });

  test('can add a host route via API and see it', async ({ request, page }) => {
    const host = `ui-route-${Date.now()}.example.com`;
    const existing = await (await request.get('/admin/routes')).json();
    existing[host] = 'http://new.example.com';
    await request.post('/admin/routes', { data: existing });

    await gotoRail(page, 'Rules');
    await expect(page.locator('.col-match').filter({ hasText: host })).toBeVisible();
    await expect(page.getByText('http://new.example.com')).toBeVisible();
  });

  test('header maps and map local tabs switch view', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Header maps/ }).click();
    await expect(page.getByText('Target')).toBeVisible();
    await page.getByRole('button', { name: /Map local/ }).click();
    await expect(page.getByText('Local file')).toBeVisible();
  });

  test('add header map rule via API appears in UI', async ({ request, page }) => {
    await request.post('/admin/header-maps', {
      data: { id: '', name: 'X-Test', enabled: true, scope: 'all', match: '.*', action: 'Set', value: 'hello' },
    });
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Header maps/ }).click();
    await expect(page.getByText('Set X-Test: hello')).toBeVisible();
  });
});
