// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Page load', () => {
  test('loads the current app shell', async ({ page }) => {
    await page.goto('/');
    await expect(page).toHaveTitle(/oproxy/);
    await expect(page.locator('link[rel="icon"]')).toHaveAttribute('href', '/icons/icon.svg?v=2');
    await expect(page.getByText('oproxy / traffic')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sessions', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Compose', exact: true })).toBeVisible();
  });

  test('toolbar controls are visible and named', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByTitle(/Live refresh/)).toBeVisible();
    await expect(page.getByTitle('Clear all sessions')).toBeVisible();
    await expect(page.getByTitle('Export as HAR')).toBeVisible();
    await expect(page.getByPlaceholder('Filter requests by method, host, path, status, or tag')).toBeVisible();
    await expect(page.getByTitle('Keyboard shortcuts · ?')).toBeVisible();
  });

  test('health endpoint returns JSON', async ({ request }) => {
    const res = await request.get('/health');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body).toHaveProperty('mitm_enabled');
  });

  test('default theme follows app defaults', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('html')).toHaveAttribute('data-theme', /^(dark|light)$/);
  });
});
