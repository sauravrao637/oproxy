// @ts-check
const { test, expect } = require('@playwright/test');
const { sampleSession, importSession } = require('./helpers');

test.describe('Session detail panel', () => {
  test.beforeEach(async ({ page, request }) => {
    await request.delete('/admin/sessions');
    await importSession(request, sampleSession({
      id: 'detail-test-sess',
      host: 'detail.example.com',
      uri: 'http://detail.example.com/path?q=1',
      requestHeaders: { accept: 'application/json' },
      responseBody: '{"result":"ok"}',
    }));
    await page.goto('/');
    await expect(page.locator('tbody tr')).toHaveCount(1, { timeout: 10000 });
  });

  test.afterEach(async ({ request }) => {
    await request.delete('/admin/sessions');
  });

  test('session row appears in traffic list', async ({ page }) => {
    await expect(page.locator('tbody')).toContainText('detail.example.com');
  });

  test('clicking session row shows detail panel', async ({ page }) => {
    await page.locator('tbody tr').first().click();
    await expect(page.locator('.detail-panel')).toContainText('detail.example.com');
    await expect(page.getByTitle('Close panel')).toBeVisible();
  });

  test('detail panel shows overview with status and method', async ({ page }) => {
    await page.locator('tbody tr').first().click();
    await expect(page.locator('.detail-panel')).toContainText('200');
    await expect(page.locator('.detail-panel')).toContainText('GET');
  });

  test('detail panel shows request and response tabs', async ({ page }) => {
    await page.locator('tbody tr').first().click();
    await expect(page.locator('.detail-tabs button', { hasText: 'Request' })).toBeVisible();
    await expect(page.locator('.detail-tabs button', { hasText: 'Response' })).toBeVisible();
  });

  test('clicking headers tab shows request headers', async ({ page }) => {
    await page.locator('tbody tr').first().click();
    await page.locator('.detail-tabs button', { hasText: 'Headers' }).click();
    await expect(page.locator('.detail-panel')).toContainText('accept');
  });

  test('clicking response tab shows response body', async ({ page }) => {
    await page.locator('tbody tr').first().click();
    await page.locator('.detail-tabs button', { hasText: 'Response' }).click();
    await expect(page.locator('.detail-panel')).toContainText('result');
  });

  test('Escape closes detail and deselects', async ({ page }) => {
    await page.locator('tbody tr').first().click();
    await expect(page.getByTitle('Close panel')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByTitle('Close panel')).toHaveCount(0);
    await expect(page.locator('.detail-panel')).toContainText('Select a session');
  });
});
