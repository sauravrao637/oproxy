// @ts-check
const http = require('http');
const { test, expect } = require('@playwright/test');
const { resetWorkspace, sampleSession } = require('./helpers');

function startFakeProvider(handler) {
  const server = http.createServer(handler);
  return new Promise(resolve => {
    server.listen(0, '127.0.0.1', () => resolve(server));
  });
}

function protocolSessions() {
  const now = Date.now();
  return [
    sampleSession({
      id: `protocol-h2-${now}`,
      host: 'protocol-h2.example',
      uri: 'https://protocol-h2.example/api',
      protocol: 'HTTP/2',
      protocol_context: {
        downstream: 'http2',
        upstream: 'http2',
        application: 'http',
        body_mode: 'full',
        scheme: 'https',
      },
    }),
    sampleSession({
      id: `protocol-ws-${now}`,
      host: 'protocol-ws.example',
      method: 'WS',
      uri: 'ws://protocol-ws.example/socket',
      status: 101,
      requestHeaders: { 'sec-websocket-protocol': 'chat' },
      responseHeaders: { upgrade: 'websocket' },
      responseBody: '',
      metrics: {
        latency_ms: 18,
        request_size_bytes: 0,
        response_size_bytes: 0,
        status_code: 101,
        ttfb_ms: 4,
        body_ms: 14,
        protocol: 'WebSocket',
      },
      protocol_context: {
        downstream: 'web_socket',
        upstream: 'web_socket',
        application: 'http',
        body_mode: 'frames',
        scheme: 'ws',
      },
      ws_frames: [
        { timestamp: new Date(now).toISOString(), direction: 'ClientToServer', opcode: 1, payload_len: 8, payload_text: 'hello-ws', payload_hex: null },
        { timestamp: new Date(now + 1).toISOString(), direction: 'ServerToClient', opcode: 1, payload_len: 8, payload_text: 'hello-ws', payload_hex: null },
      ],
    }),
    sampleSession({
      id: `protocol-grpc-${now}`,
      host: 'protocol-grpc.example',
      method: 'POST',
      uri: 'https://protocol-grpc.example/pkg.Service/Unary',
      requestHeaders: { 'content-type': 'application/grpc+proto' },
      requestBody: 'grpc-request',
      responseHeaders: { 'content-type': 'application/grpc+proto' },
      responseBody: 'grpc-response',
      protocol: 'HTTP/2',
      protocol_context: {
        downstream: 'http2',
        upstream: 'http2',
        application: 'grpc',
        body_mode: 'stream_messages',
        scheme: 'https',
      },
      inspector_data: {
        grpc: {
          service: 'pkg.Service',
          method: 'Unary',
          messages: [{ direction: 'request', compressed: false, length: 12, fields: [] }],
        },
      },
    }),
    sampleSession({
      id: `protocol-socks-${now}`,
      host: 'protocol-socks.example:443',
      method: 'CONNECT',
      uri: 'socks5://protocol-socks.example:443',
      status: 200,
      responseBody: 'up=0 down=0',
      metrics: {
        latency_ms: 0,
        request_size_bytes: 60,
        response_size_bytes: 152,
        status_code: 200,
        protocol: 'SOCKS5',
      },
      downstream_protocol: 'SOCKS5',
      protocol_context: {
        downstream: 'socks5',
        upstream: 'socks5',
        application: 'binary',
        body_mode: 'tunnel',
        scheme: 'socks5',
      },
    }),
  ];
}

async function importProtocolSessions(request) {
  await resetWorkspace(request);
  const res = await request.post('/admin/sessions/import', {
    data: { sessions: protocolSessions(), merge: false },
  });
  if (!res.ok()) throw new Error(await res.text());
}

test.describe('Session protocol CTA parity', () => {
  test.beforeEach(async ({ page, request }) => {
    await importProtocolSessions(request);
    await page.addInitScript(() => {
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: {
          writeText: async text => { window.__copiedText = text; },
        },
      });
    });
  });

  test('wire and app chips filter HTTP/2, WebSocket, gRPC, and SOCKS rows', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('tbody tr', { hasText: 'protocol-h2.example' })).toBeVisible();
    await expect(page.locator('tbody tr', { hasText: 'protocol-ws.example' })).toBeVisible();
    await expect(page.locator('tbody tr', { hasText: 'protocol-grpc.example' })).toBeVisible();
    await expect(page.locator('tbody tr', { hasText: 'protocol-socks.example' })).toBeVisible();

    await page.locator('.filter-bar button[title="WebSocket"]').click();
    await expect(page.locator('tbody tr', { hasText: 'protocol-ws.example' })).toBeVisible();
    await expect(page.locator('tbody tr', { hasText: 'protocol-grpc.example' })).toHaveCount(0);

    await page.locator('.filter-bar button[title="gRPC"]').click();
    await expect(page.locator('tbody tr', { hasText: 'protocol-ws.example' })).toBeVisible();
    await expect(page.locator('tbody tr', { hasText: 'protocol-grpc.example' })).toBeVisible();
    await page.locator('.filter-bar button[title="WebSocket"]').click();
    await expect(page.locator('tbody tr', { hasText: 'protocol-ws.example' })).toHaveCount(0);
    await expect(page.locator('tbody tr', { hasText: 'protocol-grpc.example' })).toBeVisible();
    await page.locator('.filter-bar button[title="gRPC"]').click();

    await page.getByTitle(/Downstream wire protocol/).click();
    await expect(page.getByTitle('Reset sort to chronological')).toBeVisible();
    await expect(page.locator('tbody tr', { hasText: 'protocol-socks.example' })).toBeVisible();
  });

  test('detail CTAs generate protocol-aware commands and Compose imports', async ({ page }) => {
    await page.goto('/');

    await page.locator('tbody tr', { hasText: 'protocol-ws.example' }).click();
    await expect(page.locator('.detail-panel')).toContainText('WebSocket');
    await page.getByRole('button', { name: 'websocat', exact: true }).click();
    await expect.poll(() => page.evaluate(() => window.__copiedText)).toContain('websocat');
    await expect.poll(() => page.evaluate(() => window.__copiedText)).toContain('ws://protocol-ws.example/socket');
    await page.locator('.detail-panel [aria-label="Send to builder"]').click();
    await expect(page.getByRole('heading', { name: 'Compose', exact: true })).toBeVisible();
    await expect(page.locator('.cmp-kind button.on', { hasText: 'WS' })).toBeVisible();
    await expect(page.locator('.cmp-url')).toHaveValue('ws://protocol-ws.example/socket');
    await expect(page.getByLabel('WebSocket frame payload').first()).toHaveValue('hello-ws');

    await page.getByRole('button', { name: 'Sessions', exact: true }).click();
    await page.locator('tbody tr', { hasText: 'protocol-grpc.example' }).click();
    await expect(page.locator('.detail-panel')).toContainText('gRPC');
    await page.getByRole('button', { name: 'cURL', exact: true }).click();
    await expect.poll(() => page.evaluate(() => window.__copiedText)).toContain('curl --http2');
    await page.locator('.detail-panel [aria-label="Send to builder"]').click();
    await expect(page.locator('.cmp-kind button.on', { hasText: 'gRPC' })).toBeVisible();
    await expect(page.locator('.cmp-url')).toHaveValue('https://protocol-grpc.example/pkg.Service/Unary');
    await expect(page.getByLabel('gRPC unary message payload')).toHaveValue('grpc-request');
  });

  test('replay routes WebSocket and gRPC through protocol-aware endpoints while tunnels stay metadata-only', async ({ page }) => {
    let wsReplayPayload;
    let grpcReplayPayload;
    await page.route('/admin/forward/websocket', async route => {
      wsReplayPayload = route.request().postDataJSON();
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ status: 101, session_id: 'replayed-ws', frames: [] }) });
    });
    await page.route('/admin/forward', async route => {
      grpcReplayPayload = route.request().postDataJSON();
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ status: 200, headers: {}, body: 'ok', session_id: 'replayed-grpc' }) });
    });

    await page.goto('/');
    await page.locator('tbody tr', { hasText: 'protocol-ws.example' }).click();
    await page.getByTitle('Replay this request').click();
    await expect.poll(() => wsReplayPayload).toMatchObject({
      url: 'ws://protocol-ws.example/socket',
      frames: [{ opcode: 'text', payload: 'hello-ws' }],
    });

    await page.locator('tbody tr', { hasText: 'protocol-grpc.example' }).click();
    await page.getByTitle('Replay this request').click();
    await expect.poll(() => grpcReplayPayload).toMatchObject({
      kind: 'grpc',
      method: 'POST',
      url: 'https://protocol-grpc.example/pkg.Service/Unary',
      body: 'grpc-request',
    });

    await page.locator('tbody tr', { hasText: 'protocol-socks.example' }).click();
    await expect(page.locator('.detail-panel')).toContainText('SOCKS5');
    await expect(page.getByLabel('Send to builder')).toHaveCount(0);
    await expect(page.getByTitle('Replay this request')).toHaveCount(0);
  });

  test('session row context menus are protocol-aware', async ({ page }) => {
    await page.goto('/');

    await page.locator('tbody tr', { hasText: 'protocol-h2.example' }).click({ button: 'right' });
    await expect(page.getByRole('menuitem', { name: 'Ask Assistant' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Open in Compose' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Replay' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Copy as cURL' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Open details' })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Create Map Remote rule' })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Filter by this host' })).toHaveCount(0);
    await page.keyboard.press('Escape');

    await page.locator('tbody tr', { hasText: 'protocol-ws.example' }).click({ button: 'right' });
    await expect(page.getByRole('menuitem', { name: 'Open WS in Compose' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Replay client frames' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Copy as websocat' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Copy frame transcript' })).toHaveCount(0);
    await page.keyboard.press('Escape');

    await page.locator('tbody tr', { hasText: 'protocol-grpc.example' }).click({ button: 'right' });
    await expect(page.getByRole('menuitem', { name: 'Open gRPC in Compose' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Replay gRPC request' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Copy as cURL' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Copy RPC path' })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Create gRPC mock' })).toHaveCount(0);
    await page.keyboard.press('Escape');

    await page.locator('tbody tr', { hasText: 'protocol-socks.example' }).click({ button: 'right' });
    await expect(page.getByRole('menuitem', { name: 'Open in Compose' })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Replay' })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Copy destination' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Add access rule for destination' })).toHaveCount(0);
  });

  test('structure view session leaves expose the same lean context menu', async ({ page }) => {
    await page.goto('/');

    await page.getByRole('button', { name: 'Structure' }).click();
    await page.locator('.tree-node', { hasText: '/socket' }).click();
    await page.locator('.tree-leaf', { hasText: '/socket' }).click({ button: 'right' });

    await expect(page.getByRole('menuitem', { name: 'Ask Assistant' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Open WS in Compose' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Replay client frames' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Copy as websocat' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Open details' })).toHaveCount(0);
  });

  test('Ask Assistant from a session menu attaches context without sending', async ({ page }) => {
    let assistantClientContext;
    await page.route('/admin/assistant/chat', async route => {
      assistantClientContext = route.request().postDataJSON().client_context;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ message: 'ok', tool_events: [], proposed_actions: [] }),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'Sequence' }).click();

    await page.locator('tbody tr', { hasText: 'protocol-h2.example' }).click();
    await page.locator('tbody tr', { hasText: 'protocol-grpc.example' }).click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Ask Assistant' }).click();

    await expect(page.getByRole('dialog', { name: 'Assistant' })).toBeVisible();
    await expect(page.getByLabel(/Assistant context/)).toContainText('POST protocol-grpc.example /pkg.Service/Unary');
    await expect(page.getByLabel('Assistant message')).toBeFocused();
    await expect(page.locator('.assistant-msg.user')).toHaveCount(0);

    await page.getByLabel('Assistant message').fill('what is this request?');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => assistantClientContext).toMatchObject({
      ignore_selected_session: true,
      ui_state: { selected_session_id: null, selected_session_ignored: true },
      attachment: { kind: 'session' },
    });

    await page.getByLabel('Remove assistant context').click();
    await expect(page.getByLabel(/Assistant context/)).toHaveCount(0);
  });

  test('Ask Assistant attachment is marked primary in provider context', async ({ page }) => {
    let providerPayload;
    const provider = await startFakeProvider((req, res) => {
      let body = '';
      req.on('data', chunk => { body += chunk; });
      req.on('end', () => {
        providerPayload = JSON.parse(body);
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({
          choices: [{
            message: {
              role: 'assistant',
              content: 'This attached gRPC request is the subject.',
            },
          }],
        }));
      });
    });

    try {
      const port = provider.address().port;
      await page.goto('/');
      await page.locator('tbody tr', { hasText: 'protocol-h2.example' }).click();
      await page.locator('tbody tr', { hasText: 'protocol-grpc.example' }).click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Ask Assistant' }).click();

      await page.getByLabel('Provider base URL').fill(`http://127.0.0.1:${port}/v1`);
      await page.getByLabel('Model').fill('fake-model');
      await page.getByLabel('Assistant message').fill('explain this');
      await page.getByRole('button', { name: 'Send', exact: true }).click();

      await expect.poll(() => providerPayload?.messages?.[0]?.content || '').toContain('primary_subject');
      const systemPrompt = providerPayload.messages[0].content;
      expect(systemPrompt).toContain('context_priority');
      expect(systemPrompt).toContain('attached request as the primary subject');
      expect(systemPrompt).toContain('Use primary context first');
      expect(systemPrompt).toContain('secondary');
      expect(systemPrompt).toContain('this request');
      expect(systemPrompt.toLowerCase()).toContain('do not explain the complete sessions page');
      expect(systemPrompt).toContain('protocol-grpc.example');
      expect(systemPrompt).toContain('/pkg.Service/Unary');
    } finally {
      provider.close();
    }
  });
});
