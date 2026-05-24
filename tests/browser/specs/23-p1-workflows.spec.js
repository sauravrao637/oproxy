// @ts-check
const { test, expect } = require('@playwright/test');
const http = require('http');

async function withServer(handler, run) {
  const server = http.createServer(handler);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  try {
    await run(port);
  } finally {
    await new Promise(resolve => server.close(resolve));
  }
}

function session(overrides = {}) {
  return {
    id: `p1-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    timestamp: new Date().toISOString(),
    request: {
      method: 'GET',
      uri: 'http://p1-detail.example.com/api?q=1',
      headers: { accept: 'application/json' },
      body: '',
      host: 'p1-detail.example.com',
      body_bytes: null,
    },
    response: {
      status: 200,
      headers: { 'content-type': 'application/json' },
      body: '{"ok":true}',
      request_uri: 'http://p1-detail.example.com/api?q=1',
      session_id: null,
      ttfb_ms: 5,
      body_ms: 3,
      body_bytes: null,
    },
    metrics: { latency_ms: 42, request_size_bytes: 0, response_size_bytes: 11, status_code: 200, ttfb_ms: 5, body_ms: 3 },
    ws_frames: [],
    tags: [],
    ...overrides,
  };
}

test.describe('P1 developer workflows', () => {
  test('session detail exposes redacted/raw views, binary body state, and inspector data', async ({ page, request }) => {
    const id = `p1-detail-${Date.now()}`;
    await request.post('/admin/sessions/import', {
      data: {
        merge: true,
        sessions: [session({
          id,
          request: {
            method: 'POST',
            uri: 'http://p1-detail.example.com/login',
            headers: { 'content-type': 'application/json', authorization: 'Bearer secret-header' },
            body: '{"token":"secret-token","name":"dev"}',
            host: 'p1-detail.example.com',
            body_bytes: null,
          },
          response: {
            status: 200,
            headers: { 'content-type': 'image/png' },
            body: 'iVBORw0KGgo=',
            request_uri: 'http://p1-detail.example.com/login',
            session_id: id,
            ttfb_ms: 5,
            body_ms: 2,
            body_bytes: null,
          },
          metrics: { latency_ms: 12, request_size_bytes: 37, response_size_bytes: 8, status_code: 200, ttfb_ms: 5, body_ms: 2 },
          inspector_data: { jwt: { header: { alg: 'HS256' }, claims: { sub: 'dev-user' }, expired: false, alg_none_warning: false } },
        })],
      },
    });

    await page.goto('/');
    await page.getByPlaceholder(/Filter requests/).fill('p1-detail.example.com');
    await page.locator('tr', { hasText: 'p1-detail.example.com' }).first().click();

    await page.getByRole('button', { name: 'Request' }).click();
    await expect(page.locator('.detail-panel')).not.toContainText('secret-token');
    await expect(page.locator('.detail-panel')).toContainText('••••••');

    await page.getByRole('button', { name: 'Raw', exact: true }).click();
    await page.getByRole('button', { name: 'Show raw' }).click();
    await expect(page.locator('.detail-panel')).toContainText('secret-token');

    await page.getByRole('button', { name: 'Response' }).click();
    await expect(page.locator('.detail-panel')).toContainText('binary/base64');

    await page.locator('.detail-tabs button', { hasText: 'Inspector' }).click();
    await expect(page.locator('.detail-panel')).toContainText('Decoded JWT');
    await expect(page.locator('.detail-panel')).toContainText('dev-user');
  });

  test('replay preserves raw request data and creates a distinguishable session', async ({ page, request }) => {
    const id = `p1-replay-${Date.now()}`;
    const url = `http://127.0.0.1:9/replay-${Date.now()}`;
    await request.post('/admin/sessions/import', {
      data: {
        merge: true,
        sessions: [session({
          id,
          request: {
            method: 'POST',
            uri: url,
            headers: { 'content-type': 'application/json', 'x-custom': 'kept', authorization: 'Bearer replay-secret' },
            body: '{"token":"raw-replay-secret"}',
            host: '127.0.0.1:9',
            body_bytes: null,
          },
          response: null,
          metrics: null,
        })],
      },
    });

    await page.goto('/');
    await page.getByPlaceholder(/Filter requests/).fill(url);
    await page.locator('tr', { hasText: '127.0.0.1:9' }).first().click();
    await page.getByTitle('Replay this request').click();

    await expect.poll(async () => {
      const body = await (await request.get('/api/sessions')).json();
      return (body.sessions || []).some(s => s.id !== id && s.tags?.includes('replay') && s.request?.uri === url);
    }).toBeTruthy();

    const body = await (await request.get('/api/sessions')).json();
    const listed = (body.sessions || []).find(s => s.id !== id && s.tags?.includes('replay') && s.request?.uri === url);
    expect(listed).toBeTruthy();
    const found = await (await request.get(`/api/sessions/${listed.id}`)).json();
    const exchange = found.exchange;
    expect(exchange.request.method).toBe('POST');
    expect(exchange.request.headers['x-custom']).toBe('kept');
    expect(exchange.request.body).toContain('raw-replay-secret');
    expect(exchange.note).toContain(id);
  });

  test('Compose bearer auth helper sends Authorization header', async ({ page }) => {
    await withServer((req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ auth: req.headers.authorization || '' }));
    }, async port => {
      await page.goto('/');
      await page.getByRole('button', { name: 'Compose' }).click();
      await page.locator('.cmp-tab-new').click();
      await page.locator('.cmp-url').fill(`http://127.0.0.1:${port}/auth`);
      await page.getByRole('button', { name: 'Auth' }).click();
      await page.locator('.cmp-pane select').selectOption('bearer');
      await page.getByPlaceholder('{{token}} or token value').fill('compose-token');
      await page.getByRole('button', { name: 'Send' }).click();
      await expect(page.locator('.cmp-response')).toContainText('Bearer compose-token');
    });
  });

  test('structured creation dialogs expose required fields before saving', async ({ page, request }) => {
    await page.goto('/');

    await page.getByRole('button', { name: 'Rules', exact: true }).click();
    await page.getByRole('button', { name: 'Add rule' }).click();
    await expect(page.locator('.ui-dialog')).toContainText('Add route');
    await expect(page.locator('.ui-dialog')).toContainText('Source host');
    await expect(page.locator('.ui-dialog')).toContainText('Destination base URL');
    await page.getByRole('button', { name: 'Cancel' }).click();

    await page.getByRole('button', { name: 'Breakpoints', exact: true }).click();
    await page.getByRole('button', { name: 'Add breakpoint' }).click();
    await expect(page.locator('.ui-dialog')).toContainText('URI/body regex');
    await expect(page.locator('.ui-dialog')).toContainText('Pause on');
    await page.getByRole('button', { name: 'Cancel' }).click();

    await page.getByRole('button', { name: 'Mock Server', exact: true }).click();
    await page.getByRole('button', { name: 'Add mock' }).click();
    await expect(page.locator('.ui-dialog')).toContainText('Path regex');
    await expect(page.locator('.ui-dialog')).toContainText('HTTP status');
    await expect(page.locator('.ui-dialog')).toContainText('Response body');
    await page.getByRole('button', { name: 'Cancel' }).click();

    await page.getByRole('button', { name: 'DNS Override', exact: true }).click();
    await page.getByRole('button', { name: 'Add override' }).click();
    await expect(page.locator('.ui-dialog')).toContainText('Hostname');
    await expect(page.locator('.ui-dialog')).toContainText('Override IP');
    await page.getByRole('button', { name: 'Cancel' }).click();

    await page.getByRole('button', { name: 'Webhooks', exact: true }).click();
    await page.getByRole('button', { name: 'Add webhook' }).click();
    await expect(page.locator('.ui-dialog')).toContainText('Webhook URL');
    await expect(page.locator('.ui-dialog')).toContainText('Events');
    await page.getByRole('button', { name: 'Cancel' }).click();

    const rules = await (await request.get('/admin/breakpoints')).json();
    for (const r of rules) await request.delete(`/admin/breakpoints/${r.id}`);
  });
});
