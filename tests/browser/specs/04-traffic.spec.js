// @ts-check
const { test, expect } = require('@playwright/test');
const { sampleSession, importSession } = require('./helpers');

test.describe('Traffic view', () => {
  test.beforeEach(async ({ request }) => {
    await request.delete('/admin/sessions');
  });

  test.afterEach(async ({ request }) => {
    await request.delete('/admin/sessions');
  });

  test('empty session list shows current zero state', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByText('No sessions captured yet.')).toBeVisible();
  });

  test('live refresh toggle changes button title', async ({ page }) => {
    await page.goto('/');
    const live = page.getByTitle('Live refresh on (click to pause) · Space');
    await expect(live).toBeVisible();
    await live.click();
    await expect(page.getByTitle('Live refresh paused (click to resume) · Space')).toBeVisible();
    await page.keyboard.press('Space');
    await expect(page.getByTitle('Live refresh on (click to pause) · Space')).toBeVisible();
  });

  test('search input filters imported sessions and Ctrl+F focuses it', async ({ page, request }) => {
    await importSession(request, sampleSession({ id: 'traffic-search-1', host: 'traffic-search.example.com' }));
    await page.goto('/');
    await expect(page.locator('tbody tr')).toHaveCount(1, { timeout: 10000 });

    const search = page.getByPlaceholder('Filter requests by method, host, path, status, or tag');
    await page.keyboard.press('Control+f');
    await expect(search).toBeFocused();
    await search.fill('does-not-match');
    await expect(page.getByText('No sessions match the current filters.')).toBeVisible();
    await search.fill('traffic-search.example.com');
    await expect(page.locator('tbody tr')).toHaveCount(1);
  });

  test('status chips can filter and reset', async ({ page, request }) => {
    await importSession(request, sampleSession({ id: 'traffic-status-200', host: 'ok.example.com', status: 200 }), true);
    await importSession(request, sampleSession({ id: 'traffic-status-404', host: 'missing.example.com', status: 404 }), true);
    await page.goto('/');
    await expect(page.locator('tbody tr')).toHaveCount(2, { timeout: 10000 });

    await page.getByRole('button', { name: '2xx' }).click();
    await expect(page.getByText('ok.example.com')).toHaveCount(0);
    await expect(page.locator('tbody tr', { hasText: 'missing.example.com' })).toHaveCount(1);
    await page.getByRole('button', { name: '2xx' }).click();
    await expect(page.locator('tbody tr', { hasText: 'ok.example.com' })).toHaveCount(1);
  });

  test('column sort and structure controls are interactive', async ({ page, request }) => {
    await importSession(request, sampleSession({ id: 'traffic-sort-1', host: 'sort-a.example.com', method: 'POST' }), true);
    await importSession(request, sampleSession({ id: 'traffic-sort-2', host: 'sort-b.example.com', method: 'GET' }), true);
    await page.goto('/');
    await expect(page.locator('tbody tr')).toHaveCount(2, { timeout: 10000 });

    await page.getByTitle(/Sort by METHOD/).click();
    await expect(page.getByTitle('Reset sort to chronological')).toBeVisible();
    await page.getByRole('button', { name: 'Structure' }).click();
    await expect(page.getByRole('button', { name: 'Structure' })).toHaveClass(/on/);
    await page.getByRole('button', { name: 'Sequence' }).click();
    await expect(page.getByRole('button', { name: 'Sequence' })).toHaveClass(/on/);
  });

  test('clear sessions confirms before deletion', async ({ page }) => {
    await page.goto('/');
    let called = false;
    await page.route('/admin/sessions', async route => {
      if (route.request().method() === 'DELETE') {
        called = true;
        await route.fulfill({ status: 200, contentType: 'application/json', body: '{"cleared":true}' });
        return;
      }
      await route.continue();
    });

    await page.getByTitle('Clear all sessions').click();
    await expect(page.getByRole('heading', { name: 'Clear all captured sessions?' })).toBeVisible();
    await page.getByRole('button', { name: 'Cancel' }).click();
    expect(called).toBe(false);

    await page.getByTitle('Clear all sessions').click();
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    expect(called).toBe(true);
  });
});
