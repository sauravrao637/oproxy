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

  test('assistant can apply a sessions filter without model credentials', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Open assistant' }).click({ force: true });
    await page.getByLabel('Assistant message').fill('show failed requests');
    await page.getByRole('button', { name: /Send/ }).click();

    await expect(page.getByText(/Applied Sessions filter/).first()).toBeVisible();
    await expect(page.getByText(/I updated the UI/)).toBeVisible();
    await expect(page.getByRole('button', { name: '2xx' })).not.toHaveClass(/ on/);
    await expect(page.getByRole('button', { name: '4xx' })).toHaveClass(/ on/);
    await expect(page.getByRole('button', { name: '5xx' })).toHaveClass(/ on/);
  });
});
