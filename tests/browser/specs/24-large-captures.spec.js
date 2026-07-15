// @ts-check
const { test, expect } = require('@playwright/test');
const { resetWorkspace } = require('./helpers');

function makeSession(i) {
  const id = `large-capture-${i}`;
  return {
    id,
    timestamp: new Date(Date.now() + i).toISOString(),
    request: {
      method: 'GET',
      uri: `http://large.example.com/items/${i}`,
      headers: {},
      body: '',
      host: 'large.example.com',
      body_bytes: null,
    },
    response: {
      status: 200,
      headers: { 'content-type': 'application/json' },
      body: `{"id":${i}}`,
      request_uri: `http://large.example.com/items/${i}`,
      session_id: id,
      ttfb_ms: 1,
      body_ms: 1,
      body_bytes: null,
    },
    metrics: { latency_ms: 2, request_size_bytes: 0, response_size_bytes: 8, status_code: 200, ttfb_ms: 1, body_ms: 1 },
    ws_frames: [],
  };
}

test.describe('Large captures', () => {
  test.beforeEach(async ({ request }) => {
    await request.delete('/admin/sessions');
    await resetWorkspace(request);
  });

  test.afterEach(async ({ request }) => {
    await request.delete('/admin/sessions');
  });

  test('session list paginates rendered rows for high-volume captures', async ({ page, request }) => {
    const sessions = Array.from({ length: 750 }, (_, i) => makeSession(i));
    await request.post('/admin/sessions/import', { data: { sessions, merge: true } });

    await page.goto('/');
    await page.getByPlaceholder(/Filter requests/).fill('large.example.com');

    await expect(page.locator('tbody tr')).toHaveCount(250, { timeout: 10000 });
    await expect(page.locator('.page-more')).toContainText('Showing 250 of 750 matching sessions');

    await page.getByRole('button', { name: 'Show next 250' }).click();
    await expect(page.locator('tbody tr')).toHaveCount(500);
    await expect(page.locator('.page-more')).toContainText('Showing 500 of 750 matching sessions');
  });

  test('structure selection does not collapse expanded render window after workspace sync', async ({ page, request }) => {
    const sessions = Array.from({ length: 750 }, (_, i) => makeSession(i));
    await request.post('/admin/sessions/import', { data: { sessions, merge: true } });

    await page.goto('/');
    const filterPatch = page.waitForResponse((res) => res.url().includes('/admin/workspace') && res.request().method() === 'PATCH');
    await page.getByPlaceholder(/Filter requests/).fill('large.example.com');
    await filterPatch;
    await expect(page.locator('.page-more')).toContainText('Showing 250 of 750 matching sessions', { timeout: 10000 });

    const viewPatch = page.waitForResponse((res) => res.url().includes('/admin/workspace') && res.request().method() === 'PATCH');
    await page.getByRole('button', { name: 'Structure' }).click();
    await viewPatch;
    await page.locator('.tree-node', { hasText: '/items' }).click();
    await expect(page.locator('.tree-leaf')).toHaveCount(250);

    await page.getByRole('button', { name: 'Show next 250' }).click();
    await expect(page.locator('.tree-leaf')).toHaveCount(500);
    await expect(page.locator('.page-more')).toContainText('Showing 500 of 750 matching sessions');

    const workspacePatch = page.waitForResponse((res) => res.url().includes('/admin/workspace') && res.request().method() === 'PATCH');
    await page.locator('.tree-leaf').nth(300).click();
    await workspacePatch;

    await expect(page.locator('.page-more')).toContainText('Showing 500 of 750 matching sessions');
    await expect(page.locator('.tree-leaf')).toHaveCount(500);
  });
});
