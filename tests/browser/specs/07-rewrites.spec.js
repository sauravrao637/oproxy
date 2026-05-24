// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

test.describe('Rules / rewrites', () => {
  test.afterEach(async ({ request }) => {
    const rewrites = await (await request.get('/admin/rewrites')).json();
    for (let i = rewrites.length - 1; i >= 0; i--) {
      if (String(rewrites[i].name || '').includes('test-rw')) await request.delete(`/admin/rewrites/${i}`);
    }
    const mods = await (await request.get('/admin/modifications')).json();
    for (let i = mods.length - 1; i >= 0; i--) {
      if (mods[i].request_uri_pattern === '/ui-mod') await request.delete(`/admin/modifications/${i}`);
    }
  });

  test('rules view exposes rewrite and modification tabs', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Rewrites/ }).click();
    await expect(page.getByText('Target (regex)')).toBeVisible();
    await page.getByRole('button', { name: /Modifications/ }).click();
    await expect(page.getByText('URI contains')).toBeVisible();
  });

  test('Add rule opens rewrite form on rewrite tab', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Rewrites/ }).click();
    await page.getByRole('button', { name: /Add rule/ }).click();
    await expect(page.getByRole('heading', { name: 'Add rewrite' })).toBeVisible();
    await expect(page.locator('[name="pattern"]')).toBeVisible();
    await page.getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByRole('heading', { name: 'Add rewrite' })).toHaveCount(0);
  });

  test('create rewrite rule via API and see it in UI', async ({ request, page }) => {
    await request.post('/admin/rewrites', {
      data: {
        name: 'test-rw-ui',
        enabled: true,
        criteria: { Host: 'rw-test.example.com' },
        action: { AddHeader: { name: 'X-Rewrite', value: 'yes' } },
      },
    });
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Rewrites/ }).click();
    await expect(page.getByText('host contains rw-test.example.com')).toBeVisible();
    await expect(page.getByText('add X-Rewrite: yes')).toBeVisible();
  });

  test('Add response modification creates a visible rule', async ({ page }) => {
    await gotoRail(page, 'Rules');
    await page.getByRole('button', { name: /Modifications/ }).click();
    await page.getByRole('button', { name: /Add rule/ }).click();
    await expect(page.getByRole('heading', { name: 'Add response modification' })).toBeVisible();
    await page.locator('[name="pattern"]').fill('/ui-mod');
    await page.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByText('/ui-mod')).toBeVisible();
  });
});
