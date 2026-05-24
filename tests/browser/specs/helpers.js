// @ts-check
const { expect } = require('@playwright/test');

async function gotoRail(page, name, heading = name) {
  await page.goto('/');
  await page.getByRole('button', { name, exact: true }).click();
  await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible();
}

function sampleSession(overrides = {}) {
  const id = overrides.id || `session-${Date.now()}`;
  const host = overrides.host || 'example.test';
  const uri = overrides.uri || `http://${host}/path?q=1`;
  return {
    id,
    timestamp: overrides.timestamp || new Date().toISOString(),
    request: {
      method: overrides.method || 'GET',
      uri,
      headers: overrides.requestHeaders || { accept: 'application/json' },
      body: overrides.requestBody || '',
      host,
      body_bytes: null,
    },
    response: overrides.response === undefined ? {
      status: overrides.status || 200,
      headers: overrides.responseHeaders || { 'content-type': 'application/json' },
      body: overrides.responseBody || '{"ok":true}',
      request_uri: uri,
      session_id: id,
      ttfb_ms: 5,
      body_ms: 3,
      body_bytes: null,
    } : overrides.response,
    metrics: overrides.metrics === undefined ? {
      latency_ms: 42,
      request_size_bytes: 0,
      response_size_bytes: 11,
      status_code: overrides.status || 200,
      ttfb_ms: 5,
      body_ms: 3,
    } : overrides.metrics,
    ws_frames: overrides.ws_frames || [],
  };
}

async function importSession(request, session, merge = false) {
  const res = await request.post('/admin/sessions/import', { data: { sessions: [session], merge } });
  expect(res.ok()).toBeTruthy();
}

module.exports = { gotoRail, sampleSession, importSession };
