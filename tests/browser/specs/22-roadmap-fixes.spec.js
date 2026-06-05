// @ts-check
const { test, expect } = require('@playwright/test');
const http = require('http');
const { resetWorkspace } = require('./helpers');

function proxyGet(baseURL, targetUrl) {
  const proxy = new URL(baseURL);
  const target = new URL(targetUrl);
  return new Promise((resolve, reject) => {
    const req = http.request({
      host: proxy.hostname,
      port: proxy.port || 80,
      method: 'GET',
      path: targetUrl,
      headers: { Host: target.host },
    }, res => {
      let body = '';
      res.on('data', chunk => { body += chunk; });
      res.on('end', () => resolve({ status: res.statusCode, body }));
    });
    req.on('error', reject);
    req.end();
  });
}

test.describe('roadmap fixes', () => {
  test('forward rejects invalid method before recording a session', async ({ request }) => {
    const before = await (await request.get('/api/sessions')).json();
    const beforeCount = (before.sessions || before).length;

    const res = await request.post('/admin/forward', {
      data: {
        method: 'BAD METHOD',
        url: 'https://example.com/bad-method',
        headers: {},
        body: null,
      },
    });

    expect(res.status()).toBe(400);
    const body = await res.json();
    expect(body.error).toContain('Invalid HTTP method');

    const sessions = await (await request.get('/api/sessions')).json();
    expect((sessions.sessions || sessions).length).toBe(beforeCount);
  });

  test('curl import rejects text that is not a curl command', async ({ request }) => {
    const res = await request.post('/api/import/curl', {
      data: { curl: 'not curl' },
    });

    expect(res.status()).toBe(422);
    const body = await res.json();
    expect(body.error).toContain('start with curl');
  });

  test('mock responses are recorded as completed sessions', async ({ request, baseURL }) => {
    const create = await request.post('/admin/mock/rules', {
      data: {
        id: '',
        name: 'roadmap-mock-complete',
        enabled: true,
        location: {
          host: null,
          path: '^/roadmap-mock$',
          port: null,
          protocol: null,
          query: null,
          methods: ['GET'],
          mode: 'regex',
        },
        responses: [{
          status: 203,
          headers: { 'content-type': 'text/plain' },
          body: 'mocked from roadmap test',
          delay_ms: 0,
        }],
      },
    });
    expect(create.ok()).toBeTruthy();

    const rules = await (await request.get('/admin/mock/rules')).json();
    const rule = rules.find(r => r.name === 'roadmap-mock-complete');
    expect(rule).toBeTruthy();

    const proxied = await proxyGet(baseURL, 'http://roadmap-mock.example.com/roadmap-mock');
    expect(proxied.status).toBe(203);
    expect(proxied.body).toBe('mocked from roadmap test');

    await expect.poll(async () => {
      const list = await (await request.get('/api/sessions')).json();
      const sessions = list.sessions || list;
      return sessions.find(s => s.request?.uri?.includes('/roadmap-mock'))?.response?.status;
    }).toBe(203);

    await request.delete(`/admin/mock/rules/${rule.id}`);
  });

  test('setup guide is honest about CA URL and does not render fake QR', async ({ request }) => {
    const setup = await request.get('/setup/mobile');
    expect(setup.ok()).toBeTruthy();
    const html = await setup.text();
    expect(html).toContain('Open the CA URL on the device');
    expect(html).not.toContain('<canvas id="qr-canvas"');

    const info = await (await request.get('/admin/setup/network-info')).json();
    expect(info).toHaveProperty('localhost_proxy');
    expect(info.localhost_proxy).toMatch(/^127\.0\.0\.1:\d+$/);
    expect(info).toHaveProperty('ca_local_url');
    if (info.running_in_container) {
      // With host networking the container shares the host's network stack and may
      // resolve a real LAN IP.  Just verify the field is present and is a string.
      expect(typeof info.lan_ip === 'string' || info.lan_ip === null).toBe(true);
    }
  });

  test('Compose validates empty URL and renders text backend errors cleanly', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Compose', exact: true }).click();
    await page.getByTitle('New tab').click();

    await expect(page.getByText('Enter an absolute http:// or https:// URL.')).toBeVisible();
    await expect(page.getByRole('button', { name: /Send/ })).toBeDisabled();

    await page.route('/admin/forward', async route => {
      await route.fulfill({ status: 400, contentType: 'text/plain', body: 'plain backend error' });
    });
    await page.locator('.cmp-url').fill('http://example.test/');
    await expect(page.getByRole('button', { name: /Send/ })).toBeEnabled();
    await page.getByRole('button', { name: /Send/ }).click();
    await expect(page.getByText('plain backend error')).toBeVisible();
    await expect(page.getByText(/Unexpected token/)).toHaveCount(0);
  });

  test('status and settings distinguish client proxy from bind address', async ({ page, request }) => {
    await resetWorkspace(request);
    await page.goto('/');
    const port = new URL(process.env.OPROXY_BASE_URL || 'http://localhost:18080').port || '80';
    const proxyPattern = new RegExp(`PROXY (127\\\\.0\\\\.0\\\\.1|localhost):${port}`);
    await expect(page.getByRole('button', { name: proxyPattern })).toBeVisible();

    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
    const listener = page.locator('.insp-card').filter({ hasText: 'Listener' });
    await expect(listener).toContainText('Client proxy');
    await expect(listener).toContainText(new RegExp(`(127\\\\.0\\\\.0\\\\.1|localhost):${port}`));
    // SOCKS5 row shows the active mode; 'tunnel-only' is the default when mode is unset.
    const socks5Info = await (await request.get('/admin/socks5/status')).json();
    const expectedMode = socks5Info.mode || 'tunnel-only';
    await expect(listener).toContainText(expectedMode);
  });
});
