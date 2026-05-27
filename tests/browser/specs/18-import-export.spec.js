// @ts-check
const { test, expect } = require('@playwright/test');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { sampleSession, importSession } = require('./helpers');

test.describe('Import / Export flows', () => {
  test('export HAR from UI downloads file', async ({ page }) => {
    await page.goto('/');
    // Seed a session
    const session = {
      id: 'har-export-sess',
      timestamp: new Date().toISOString(),
      request: { method: 'GET', uri: 'http://har.example.com/', headers: {}, body: '', host: 'har.example.com', body_bytes: null },
      response: { status: 200, headers: { 'content-type': 'text/plain' }, body: 'ok', request_uri: 'http://har.example.com/', session_id: null, ttfb_ms: 0, body_ms: 0, body_bytes: null },
      metrics: { latency_ms: 10, request_size_bytes: 0, response_size_bytes: 2, status_code: 200, ttfb_ms: 0, body_ms: 0 },
      ws_frames: [],
    };
    await page.request.post('/admin/sessions/import', { data: { sessions: [session], merge: true } });
    await page.reload();

    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByTitle('Export as HAR').click(),
    ]);
    expect(download.suggestedFilename()).toBe('oproxy-session.har');
  });

  test('import JSON from UI populates sessions', async ({ page, request }) => {
    await request.delete('/admin/sessions');
    const id = `ui-json-import-${Date.now()}`;
    const filePath = path.join(os.tmpdir(), `${id}.json`);
    fs.writeFileSync(filePath, JSON.stringify([
      sampleSession({ id, host: 'ui-import.example.com' }),
    ]));

    await page.goto('/');
    const [chooser] = await Promise.all([
      page.waitForEvent('filechooser'),
      page.getByTitle('Import HAR or JSON').click(),
    ]);
    await chooser.setFiles(filePath);

    await expect(page.locator('tbody tr').first()).toContainText('ui-import.example.com');
    fs.unlinkSync(filePath);
  });

  test('/admin/sessions returns imported sessions as JSON', async ({ request }) => {
    const session = {
      id: 'json-export-sess',
      timestamp: new Date().toISOString(),
      request: { method: 'POST', uri: 'http://json.example.com/', headers: {}, body: '{}', host: 'json.example.com', body_bytes: null },
      response: null, metrics: null, ws_frames: [],
    };
    await request.post('/admin/sessions/import', { data: { sessions: [session], merge: true } });

    const res = await request.get('/admin/sessions');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    const sessions = body.sessions || body;
    expect(sessions.some(s => s.id === 'json-export-sess')).toBeTruthy();
  });

  test('/admin/sessions/export/har returns HAR', async ({ request }) => {
    const res = await request.get('/admin/sessions/export/har');
    expect(res.ok()).toBeTruthy();
    const har = await res.json();
    expect(har).toHaveProperty('log');
    expect(har.log).toHaveProperty('entries');
  });

  test('generated request snippets are parseable and omit internal headers', async ({ request, page }) => {
    const session = {
      id: 'snippet-export-sess',
      timestamp: new Date().toISOString(),
      request: {
        method: 'POST',
        uri: 'http://snippet.example.com/api',
        headers: {
          'content-type': 'application/json',
          host: 'snippet.example.com',
          'proxy-connection': 'keep-alive',
          'x-oproxy-session-id': 'internal',
        },
        body: '{"token":"secret-token","name":"dev"}',
        host: 'snippet.example.com',
        body_bytes: null,
      },
      response: {
        status: 200,
        headers: { 'content-type': 'application/octet-stream' },
        body: 'AAEC',
        request_uri: 'http://snippet.example.com/api',
        session_id: 'snippet-export-sess',
        ttfb_ms: 1,
        body_ms: 1,
        body_bytes: null,
      },
      metrics: { latency_ms: 2, request_size_bytes: 38, response_size_bytes: 3, status_code: 200, ttfb_ms: 1, body_ms: 1 },
      ws_frames: [],
    };
    await request.post('/admin/sessions/import', { data: { sessions: [session], merge: true } });

    const fetchCode = await (await request.get('/api/sessions/snippet-export-sess/export?format=fetch')).text();
    await page.evaluate(code => { new Function(code); }, fetchCode);
    expect(fetchCode).toContain('[redacted]');
    expect(fetchCode).not.toContain('secret-token');
    expect(fetchCode).not.toContain('x-oproxy-session-id');
    expect(fetchCode).not.toContain('proxy-connection');
    expect(fetchCode).not.toContain('host:');

    const rawFetchCode = await (await request.get('/api/sessions/snippet-export-sess/export?format=fetch&raw=true')).text();
    await page.evaluate(code => { new Function(code); }, rawFetchCode);
    expect(rawFetchCode).toContain('secret-token');
    expect(rawFetchCode).not.toContain('x-oproxy-session-id');

    const pythonCode = await (await request.get('/api/sessions/snippet-export-sess/export?format=python')).text();
    expect(pythonCode).toContain('requests.request("POST"');
    expect(pythonCode).toContain('[redacted]');
    expect(pythonCode).not.toContain('secret-token');
    expect(pythonCode).not.toContain('x-oproxy-session-id');

    const curlCode = await (await request.get('/api/sessions/snippet-export-sess/export')).text();
    expect(curlCode).toContain('curl -X POST');
    expect(curlCode).toContain('[redacted]');
    expect(curlCode).not.toContain('secret-token');
    expect(curlCode).not.toContain('x-oproxy-session-id');

  });

  test('raw HAR export/import roundtrips annotations, bodies, headers, and timings', async ({ request }) => {
    const id = 'har-roundtrip-rich';
    const session = sampleSession({
      id,
      host: 'roundtrip.example.com',
      method: 'POST',
      status: 202,
      uri: 'http://roundtrip.example.com/api?debug=1',
      requestHeaders: {
        'content-type': 'application/json',
        authorization: 'Bearer secret-har-token',
        cookie: 'sid=secret-cookie',
        'x-custom': 'visible',
      },
      requestBody: '{"password":"secret-password","name":"dev"}',
      responseHeaders: {
        'content-type': 'application/json',
        'set-cookie': 'session=secret-cookie; Path=/',
      },
      responseBody: '{"echo":"secret-password","ok":true}',
      metrics: {
        latency_ms: 123,
        request_size_bytes: 43,
        response_size_bytes: 36,
        status_code: 202,
        ttfb_ms: 87,
        body_ms: 36,
        dns_ms: 7,
        tcp_connect_ms: 11,
        tls_ms: 13,
      },
    });
    session.note = 'HAR roundtrip note';
    session.tags = ['auth', 'release'];
    session.updated_at = '2026-05-23T00:00:00.000Z';

    await importSession(request, session, false);
    const exportRes = await request.get(`/admin/sessions/export/har?raw=true&ids=${id}`);
    expect(exportRes.ok()).toBeTruthy();
    const har = await exportRes.json();
    expect(har.log.entries).toHaveLength(1);
    expect(har.log.entries[0].timings.wait).toBe(56);

    await request.delete('/admin/sessions');
    const importRes = await request.post('/admin/sessions/import/har?merge=false', { data: har });
    expect(importRes.ok()).toBeTruthy();
    expect(await importRes.json()).toEqual({ imported: 1 });

    const detailRes = await request.get(`/api/sessions/${id}`);
    expect(detailRes.ok()).toBeTruthy();
    const { exchange } = await detailRes.json();
    expect(exchange.note).toBe('HAR roundtrip note');
    expect(exchange.tags).toEqual(['auth', 'release']);
    expect(exchange.request.headers.authorization).toBe('Bearer secret-har-token');
    expect(exchange.request.headers['x-custom']).toBe('visible');
    expect(exchange.request.body).toContain('secret-password');
    expect(exchange.response.status).toBe(202);
    expect(exchange.response.body).toContain('secret-password');
    expect(exchange.response.ttfb_ms).toBe(87);
    expect(exchange.response.body_ms).toBe(36);
    expect(exchange.metrics.latency_ms).toBe(123);
    expect(exchange.metrics.ttfb_ms).toBe(87);
    expect(exchange.metrics.body_ms).toBe(36);
    expect(exchange.metrics.dns_ms).toBe(7);
    expect(exchange.metrics.tcp_connect_ms).toBe(11);
    expect(exchange.metrics.tls_ms).toBe(13);
  });

  test('HAR export defaults to redacted data and raw export is explicit', async ({ request }) => {
    const id = 'har-redaction-defaults';
    await importSession(request, sampleSession({
      id,
      host: 'redact.example.com',
      method: 'POST',
      uri: 'http://redact.example.com/login',
      requestHeaders: {
        'content-type': 'application/json',
        authorization: 'Bearer secret-token',
        cookie: 'sid=secret-cookie',
        'proxy-connection': 'keep-alive',
        'x-oproxy-session-id': 'internal',
      },
      requestBody: '{"password":"secret-password","name":"dev"}',
      responseHeaders: {
        'content-type': 'application/json',
        'set-cookie': 'sid=secret-cookie; Path=/',
        'x-oproxy-trace': 'internal',
      },
      responseBody: '{"echo":"secret-password","status":"ok"}',
    }), false);

    const redactedHar = await (await request.get(`/admin/sessions/export/har?ids=${id}`)).json();
    const redactedText = JSON.stringify(redactedHar);
    expect(redactedText).toContain('[redacted]');
    expect(redactedText).not.toContain('secret-token');
    expect(redactedText).not.toContain('secret-cookie');
    expect(redactedText).not.toContain('secret-password');
    expect(redactedText).not.toContain('x-oproxy-session-id');
    expect(redactedText).not.toContain('proxy-connection');

    const rawHar = await (await request.get(`/admin/sessions/export/har?raw=true&ids=${id}`)).json();
    const rawText = JSON.stringify(rawHar);
    expect(rawText).toContain('secret-token');
    expect(rawText).toContain('secret-cookie');
    expect(rawText).toContain('secret-password');
    expect(rawText).not.toContain('x-oproxy-session-id');
    expect(rawText).not.toContain('x-oproxy-trace');
    expect(rawText).not.toContain('proxy-connection');
  });

  test('raw cURL export re-imports the request while default cURL stays redacted', async ({ request }) => {
    const id = 'curl-roundtrip-rich';
    await importSession(request, sampleSession({
      id,
      host: 'curl.example.com',
      method: 'PATCH',
      uri: 'http://curl.example.com/v1/users/42',
      requestHeaders: {
        'content-type': 'application/json',
        authorization: 'Bearer curl-secret',
        'x-custom': 'visible',
        'x-oproxy-session-id': 'internal',
      },
      requestBody: '{"token":"curl-secret","name":"Dev User"}',
    }), false);

    const redactedCurl = await (await request.get(`/api/sessions/${id}/export?format=curl`)).text();
    expect(redactedCurl).toContain('[redacted]');
    expect(redactedCurl).not.toContain('curl-secret');
    expect(redactedCurl).not.toContain('x-oproxy-session-id');

    const rawCurl = await (await request.get(`/api/sessions/${id}/export?format=curl&raw=true`)).text();
    expect(rawCurl).toContain('curl-secret');
    expect(rawCurl).not.toContain('x-oproxy-session-id');

    const importRes = await request.post('/api/import/curl', { data: { curl: rawCurl } });
    expect(importRes.ok()).toBeTruthy();
    const parsed = await importRes.json();
    expect(parsed.method).toBe('PATCH');
    expect(parsed.url).toBe('http://curl.example.com/v1/users/42');
    expect(parsed.headers.authorization).toBe('Bearer curl-secret');
    expect(parsed.headers['x-custom']).toBe('visible');
    expect(parsed.body).toContain('curl-secret');
    expect(parsed.body).toContain('Dev User');
  });
});
