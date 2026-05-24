// @ts-check
const { test, expect } = require('@playwright/test');

const mockRule = () => ({
  id: '',
  name: 'test-mock',
  enabled: true,
  method: null,
  path_pattern: '.*',
  responses: [{ status: 200, headers: {}, body: '{"ok":true}', delay_ms: 0 }],
});

test.describe('Mock rules', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Mock Server', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Mock Server', exact: true })).toBeVisible();
  });

  test('mock view renders', async ({ page }) => {
    await expect(page.getByRole('button', { name: /Add mock/ })).toBeVisible();
  });

  test('GET /admin/mock/rules returns array', async ({ request }) => {
    const res = await request.get('/admin/mock/rules');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(Array.isArray(body)).toBeTruthy();
  });

  test('create mock rule via API, GET returns it', async ({ request }) => {
    const res = await request.post('/admin/mock/rules', { data: mockRule() });
    expect(res.ok()).toBeTruthy();
    // List and find the created rule
    const list = await (await request.get('/admin/mock/rules')).json();
    const created = list.find(r => r.name === 'test-mock');
    expect(created).toBeTruthy();
    // Cleanup
    await request.delete(`/admin/mock/rules/${created.id}`);
  });

  test('delete mock rule removes it', async ({ request }) => {
    await request.post('/admin/mock/rules', { data: { ...mockRule(), name: 'del-mock' } });
    const before = await (await request.get('/admin/mock/rules')).json();
    const created = before.find(r => r.name === 'del-mock');
    expect(created).toBeTruthy();
    await request.delete(`/admin/mock/rules/${created.id}`);
    const after = await (await request.get('/admin/mock/rules')).json();
    expect(after.some(r => r.id === created.id)).toBeFalsy();
  });
});
