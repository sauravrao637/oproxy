// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Sessions API', () => {
  test('GET /api/sessions returns sessions object with sessions array', async ({ request }) => {
    const res = await request.get('/api/sessions');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body).toHaveProperty('sessions');
    expect(Array.isArray(body.sessions)).toBeTruthy();
  });

  test('GET /admin/sessions returns sessions object', async ({ request }) => {
    const res = await request.get('/admin/sessions');
    expect(res.ok()).toBeTruthy();
  });

  test('GET /admin/metrics returns summary object', async ({ request }) => {
    await request.get('/api/sessions?limit=5');
    const res = await request.get('/admin/metrics');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body).toHaveProperty('captured_session_count');
    expect(body).toHaveProperty('active_requests');
    expect(body).toHaveProperty('sessions');
    expect(body.sessions).toHaveProperty('captured');
    expect(body.sessions).toHaveProperty('by_source');
    expect(body).toHaveProperty('endpoint_timings');
    expect(body.endpoint_timings.summaries).toHaveProperty('/api/sessions');
    expect(body.endpoint_timings.summaries).toHaveProperty('/admin/metrics');
    expect(body.endpoint_timings.summaries['/api/sessions']).toHaveProperty('last_ms');
    expect(body.endpoint_timings.recent.length).toBeGreaterThan(0);
    expect(body).not.toHaveProperty('total_requests');
    expect(body).not.toHaveProperty('active_sessions');
  });

  test('import sessions via /admin/sessions/import', async ({ request }) => {
    const session = {
      id: 'test-import-session-1',
      timestamp: new Date().toISOString(),
      request: { method: 'GET', uri: 'http://import-test.example.com/', headers: {}, body: '', host: 'import-test.example.com', body_bytes: null },
      response: null,
      metrics: null,
      ws_frames: [],
    };
    const res = await request.post('/admin/sessions/import', {
      data: { sessions: [session], merge: true },
    });
    expect(res.ok()).toBeTruthy();
    const result = await res.json();
    expect(result.imported).toBe(1);
    // Cleanup
    await request.delete('/admin/sessions');
  });

  test('GET /api/sessions/:id/export returns data for known session', async ({ request }) => {
    const session = {
      id: 'test-export-session-1',
      timestamp: new Date().toISOString(),
      request: { method: 'GET', uri: 'http://export-test.example.com/', headers: {}, body: '', host: 'export-test.example.com', body_bytes: null },
      response: null, metrics: null, ws_frames: [],
    };
    await request.post('/admin/sessions/import', { data: { sessions: [session], merge: true } });
    const res = await request.get('/api/sessions/test-export-session-1/export');
    expect(res.ok()).toBeTruthy();
    await request.delete('/admin/sessions');
  });

  test('GET /api/sessions with limit param returns sessions object', async ({ request }) => {
    const res = await request.get('/api/sessions?limit=5');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body).toHaveProperty('sessions');
    expect(body.sessions.length).toBeLessThanOrEqual(5);
  });

  test('DELETE /admin/sessions clears all', async ({ request }) => {
    const del = await request.delete('/admin/sessions');
    expect(del.ok()).toBeTruthy();
    const after = await (await request.get('/api/sessions')).json();
    expect(after.sessions.length).toBe(0);
  });
});
