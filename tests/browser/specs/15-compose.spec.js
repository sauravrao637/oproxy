// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

test.describe('Compose', () => {
  test.beforeEach(async ({ page }) => {
    await gotoRail(page, 'Compose');
  });

  test('compose view renders with empty state and new tab button', async ({ page }) => {
    await expect(page.locator('.cmp-tab-new')).toBeVisible();
    await expect(page.getByText('No request open.')).toBeVisible();
  });

  test('clicking + creates new request tab and shows editor', async ({ page }) => {
    await page.locator('.cmp-tab-new').click();
    await expect(page.locator('.cmp-editor')).toBeVisible();
    await expect(page.locator('.cmp-method')).toBeVisible();
    await expect(page.locator('.cmp-url')).toBeVisible();
    await expect(page.getByRole('button', { name: /Send/ })).toBeVisible();
  });

  test('can type URL in compose editor', async ({ page }) => {
    await page.locator('.cmp-tab-new').click();
    await page.locator('.cmp-url').fill('https://httpbin.org/get');
    await expect(page.locator('.cmp-url')).toHaveValue('https://httpbin.org/get');
  });

  test('New Request button in empty state creates tab', async ({ page }) => {
    await page.getByRole('button', { name: '+ New request' }).click();
    await expect(page.locator('.cmp-method')).toBeVisible();
  });

  test('collections sidebar can create a collection', async ({ page }) => {
    await page.getByRole('button', { name: /Collection/ }).click();
    await expect(page.getByText('Collection 1')).toBeVisible();
  });

  test('vars panel can add a variable row', async ({ page }) => {
    await page.getByTitle('New variable').click();
    await expect(page.locator('.cmp-var')).toHaveCount(1);
    await expect(page.getByText('var_1')).toBeVisible();
  });

  test('collections and variables persist across reloads', async ({ page }) => {
    await page.getByRole('button', { name: /Collection/ }).click();
    await page.getByTitle('New variable').click();

    await page.locator('.cmp-tab-new').click();
    await page.locator('.cmp-url').fill('https://persist.example.com/api');
    await page.getByRole('button', { name: 'Save' }).click();
    await expect(page.locator('.cmp-req-name', { hasText: 'Untitled' })).toBeVisible();

    await page.reload();
    await page.getByRole('button', { name: 'Compose', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Compose' })).toBeVisible();
    await expect(page.getByText('Collection 1')).toBeVisible();
    await expect(page.locator('.cmp-req-name', { hasText: 'Untitled' })).toBeVisible();
    await expect(page.getByText('var_1')).toBeVisible();
  });

  test('response headers and timing tabs render without crashing', async ({ page }) => {
    const errors = [];
    page.on('pageerror', err => errors.push(String(err)));

    await page.route('/admin/forward', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 200,
          statusText: 'OK',
          body: '{"ok":true}',
          headers: {
            'content-type': 'application/json',
            'x-test': 'yes',
          },
        }),
      });
    });

    await page.locator('.cmp-tab-new').click();
    await page.locator('.cmp-url').fill('https://example.com/api');
    await page.getByRole('button', { name: /Send/ }).click();

    await page.getByRole('button', { name: 'headers', exact: true }).click();
    await expect(page.getByText('content-type')).toBeVisible();
    await expect(page.getByText('application/json')).toBeVisible();

    await page.getByRole('button', { name: 'timing', exact: true }).click();
    await expect(page.locator('.cmp-response .kv .k', { hasText: 'Request' })).toBeVisible();
    await expect(page.locator('.cmp-response .kv .k', { hasText: 'Total' })).toBeVisible();

    expect(errors).toEqual([]);
  });
});
