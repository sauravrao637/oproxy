// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Scripts (Lua)', () => {
  test('scripts view renders', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Lua Scripts', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Lua scripts', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /New script/ })).toBeVisible();
  });

  test('GET /admin/scripts returns array', async ({ request }) => {
    const res = await request.get('/admin/scripts');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(Array.isArray(body)).toBeTruthy();
  });

  test('create script via API and find it in list', async ({ request }) => {
    const res = await request.post('/admin/scripts', {
      data: { id: '', name: 'test-script', enabled: true, code: '-- noop' },
    });
    expect(res.ok()).toBeTruthy();
    const list = await (await request.get('/admin/scripts')).json();
    const created = list.find(s => s.name === 'test-script');
    expect(created).toBeTruthy();
    // Cleanup
    await request.delete(`/admin/scripts/${created.id}`);
  });

  test('delete script removes it', async ({ request }) => {
    await request.post('/admin/scripts', {
      data: { id: '', name: 'del-script', enabled: false, code: '-- del' },
    });
    const before = await (await request.get('/admin/scripts')).json();
    const created = before.find(s => s.name === 'del-script');
    expect(created).toBeTruthy();
    await request.delete(`/admin/scripts/${created.id}`);
    const after = await (await request.get('/admin/scripts')).json();
    expect(after.some(s => s.id === created.id)).toBeFalsy();
  });
});
