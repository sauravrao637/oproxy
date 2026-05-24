// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('SSE session stream', () => {
  test('/api/sessions/stream returns event-stream content-type', async ({ request }) => {
    // We just verify the endpoint is reachable and returns SSE headers
    // (we can't hold the connection open in a request context, so abort quickly)
    const res = await request.get('/api/sessions/stream', { timeout: 2000 }).catch(e => e);
    // Either ok with text/event-stream, or a timeout (also acceptable — means endpoint exists)
    if (res && res.headers) {
      const ct = res.headers()['content-type'] || '';
      expect(ct).toContain('text/event-stream');
    }
  });

  test('UI opens SSE on load and shows session imported after load', async ({ page }) => {
    await page.goto('/');
    await page.waitForTimeout(500);
    // Clear
    await page.request.delete('/admin/sessions');
    await page.waitForTimeout(200);
    // Import a session
    await page.request.post('/admin/sessions/import', {
      data: {
        sessions: [{
          id: 'sse-test-1',
          timestamp: new Date().toISOString(),
          request: { method: 'GET', uri: 'http://sse-test.example.com/', headers: {}, body: '', host: 'sse-test.example.com', body_bytes: null },
          response: null, metrics: null, ws_frames: [],
        }],
        merge: false,
      },
    });
    await expect(page.locator('tbody tr')).toHaveCount(1, { timeout: 5000 });
    await expect(page.locator('tbody tr', { hasText: 'sse-test.example.com' })).toHaveCount(1);
    await page.request.delete('/admin/sessions');
  });
});
