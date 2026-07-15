// @ts-check
const http = require('http');
const { test, expect } = require('@playwright/test');

function startFakeProvider(handler) {
  const server = http.createServer(handler);
  return new Promise(resolve => {
    server.listen(0, '127.0.0.1', () => resolve(server));
  });
}

test.describe('Assistant', () => {
  test('assistant proposes and executes a confirmed rule action', async ({ page, request }) => {
    const provider = await startFakeProvider((req, res) => {
      let body = '';
      req.on('data', chunk => { body += chunk; });
      req.on('end', () => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({
          choices: [{
            message: {
              role: 'assistant',
              content: 'I prepared a rewrite rule for review.',
              tool_calls: [{
                id: 'call_1',
                type: 'function',
                function: {
                  name: 'propose_action',
                  arguments: JSON.stringify({
                    method: 'POST',
                    endpoint: '/admin/rule-sets',
                    summary: 'Create Assistant test rule',
                    payload: {
                      id: '',
                      name: 'Assistant test rule',
                      enabled: true,
                      location: { path: '/assistant-test', mode: 'glob' },
                      applies_to: 'request',
                      actions: [{ type: 'set_header', name: 'x-assistant', value: 'yes' }],
                    },
                  }),
                },
              }],
            },
          }],
        }));
      });
    });

    try {
      const port = provider.address().port;
      await page.goto('/');
      await page.getByRole('button', { name: 'Open assistant' }).click({ force: true });
      await expect(page.getByRole('heading', { name: 'Assistant', exact: true })).toBeVisible();
      await page.getByLabel('Provider base URL').fill(`http://127.0.0.1:${port}/v1`);
      await page.getByLabel('Model').fill('fake-model');
      await page.getByLabel('API key').fill('sk-browser-only');
      await page.getByLabel('Assistant message').fill('Create a header rewrite rule for /assistant-test');
      await page.getByRole('button', { name: /Send/ }).click();

      await expect(page.locator('.assistant-action-head b')).toHaveText('Assistant test rule');
      await expect(page.locator('.assistant-action')).toContainText('Set header x-assistant to yes');
      await page.locator('.assistant-action-technical summary').click();
      await expect(page.locator('.assistant-action-technical')).toContainText('POST /admin/rule-sets');

      const localStorageKeys = await page.evaluate(() => Object.keys(localStorage));
      expect(localStorageKeys.join(' ')).not.toContain('sk-browser-only');

      await page.getByRole('button', { name: 'Apply' }).click();
      await expect(page.getByText('Applied: Create Assistant test rule')).toBeVisible();

      const rules = await (await request.get('/admin/rule-sets')).json();
      expect(rules.some(rule => rule.name === 'Assistant test rule')).toBeTruthy();
    } finally {
      provider.close();
    }
  });

  test('assistant tools endpoint exposes grouped tools', async ({ request }) => {
    const res = await request.get('/admin/assistant/tools');
    expect(res.ok()).toBeTruthy();
    const tools = await res.json();
    expect(Array.isArray(tools.read)).toBeTruthy();
    expect(Array.isArray(tools.mutate)).toBeTruthy();
    expect(Array.isArray(tools.ui)).toBeTruthy();
    const listSessions = tools.read.find(tool => tool.name === 'list_sessions');
    const proposeAction = tools.mutate.find(tool => tool.name === 'propose_action');
    const proposeDnsOverride = tools.mutate.find(tool => tool.name === 'propose_dns_override');
    const proposeThrottling = tools.mutate.find(tool => tool.name === 'propose_throttling');
    const proposeRewriteRule = tools.mutate.find(tool => tool.name === 'propose_rewrite_rule');
    const proposeMockRule = tools.mutate.find(tool => tool.name === 'propose_mock_rule');
    const proposeAccessRule = tools.mutate.find(tool => tool.name === 'propose_access_rule');
    const proposeCaptureFilter = tools.mutate.find(tool => tool.name === 'propose_capture_filter');
    const proposeUpstreamProxy = tools.mutate.find(tool => tool.name === 'propose_upstream_proxy');
    const applyFilter = tools.ui.find(tool => tool.name === 'workspace_sessions_apply_filter');
    expect(listSessions).toBeTruthy();
    expect(proposeAction).toBeTruthy();
    expect(proposeDnsOverride).toBeTruthy();
    expect(proposeThrottling).toBeTruthy();
    expect(proposeRewriteRule).toBeTruthy();
    expect(proposeMockRule).toBeTruthy();
    expect(proposeAccessRule).toBeTruthy();
    expect(proposeCaptureFilter).toBeTruthy();
    expect(proposeUpstreamProxy).toBeTruthy();
    expect(applyFilter).toBeTruthy();
    expect(listSessions.execution_kind).toBe('read');
    expect(listSessions.requires_confirmation).toBe(false);
    expect(listSessions.risk).toBe('read');
    expect(proposeAction.execution_kind).toBe('proposal');
    expect(proposeAction.requires_confirmation).toBe(true);
    expect(proposeDnsOverride.refreshed_resources).toContain('dns');
    expect(proposeThrottling.refreshed_resources).toContain('throttling');
    expect(proposeRewriteRule.refreshed_resources).toContain('rule_sets');
    expect(proposeMockRule.refreshed_resources).toContain('mock');
    expect(proposeAccessRule.refreshed_resources).toContain('access');
    expect(proposeCaptureFilter.refreshed_resources).toContain('capture_filter');
    expect(proposeUpstreamProxy.refreshed_resources).toContain('upstream_proxy');
    expect(applyFilter.execution_kind).toBe('workspace');
    expect(applyFilter.refreshed_resources).toContain('sessions');
  });

  test('assistant compatibility probes primary context and secondary fallback', async ({ request }) => {
    const provider = await startFakeProvider((req, res) => {
      let body = '';
      req.on('data', chunk => { body += chunk; });
      req.on('end', () => {
        const payload = JSON.parse(body);
        const tools = payload.tools || [];
        const promptText = JSON.stringify(payload.messages || []);
        let message = { role: 'assistant', content: 'reachable' };

        if (promptText.includes('primary.example.com')) {
          message = { role: 'assistant', content: 'primary.example.com /primary' };
        } else if (promptText.includes('fallback.example.com')) {
          message = { role: 'assistant', content: 'fallback.example.com /secondary' };
        } else if (tools.some(tool => tool.function?.name === 'oproxy_probe_echo')) {
          message = {
            role: 'assistant',
            content: '',
            tool_calls: [{
              id: 'call_probe',
              type: 'function',
              function: {
                name: 'oproxy_probe_echo',
                arguments: JSON.stringify({ message: 'ping' }),
              },
            }],
          };
        }

        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ choices: [{ message }] }));
      });
    });

    try {
      const port = provider.address().port;
      const res = await request.post('/admin/assistant/compatibility', {
        data: {
          provider: { base_url: `http://127.0.0.1:${port}/v1`, model: 'fake-model' },
          api_key: 'sk-browser-only',
        },
      });
      expect(res.ok()).toBeTruthy();
      const report = await res.json();
      const primary = report.checks.find(check => check.id === 'context_primary_priority');
      const fallback = report.checks.find(check => check.id === 'context_secondary_fallback');

      expect(primary).toMatchObject({ required: true, ok: true });
      expect(primary.detail).toContain('primary context');
      expect(fallback).toMatchObject({ required: true, ok: true });
      expect(fallback.detail).toContain('secondary context');
    } finally {
      provider.close();
    }
  });

  test('assistant renders markdown responses as rich chat content', async ({ page }) => {
    const provider = await startFakeProvider((req, res) => {
      req.resume();
      req.on('end', () => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({
          choices: [{
            message: {
              role: 'assistant',
              content: [
                '# Summary',
                '',
                'This is **important** and uses `inline-code`.',
                '',
                '| Field | Value |',
                '|---|---|',
                '| Status | 200 OK |',
                '',
                '- first point',
                '- second point',
              ].join('\n'),
            },
          }],
        }));
      });
    });

    try {
      const port = provider.address().port;
      await page.goto('/');
      await page.getByRole('button', { name: 'Open assistant' }).click({ force: true });
      await page.getByLabel('Provider base URL').fill(`http://127.0.0.1:${port}/v1`);
      await page.getByLabel('Model').fill('fake-model');
      await page.getByLabel('Assistant message').fill('Render markdown please');
      await page.getByRole('button', { name: 'Send', exact: true }).click();

      const reply = page.locator('.assistant-msg.assistant').last();
      await expect(reply.getByRole('heading', { name: 'Summary' })).toBeVisible();
      await expect(reply.locator('strong')).toHaveText('important');
      await expect(reply.locator('code')).toContainText('inline-code');
      await expect(reply.locator('table')).toContainText('200 OK');
      await expect(reply.locator('li')).toHaveCount(2);
      await expect(reply).not.toContainText('# Summary');
      await expect(reply).not.toContainText('| Field | Value |');
    } finally {
      provider.close();
    }
  });
});
