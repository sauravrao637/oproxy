// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Webhooks', () => {
  test('webhooks view renders', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Webhooks', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Webhooks', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /Add webhook/ })).toBeVisible();
  });

  test('GET /admin/webhooks returns array', async ({ request }) => {
    const res = await request.get('/admin/webhooks');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(Array.isArray(body)).toBeTruthy();
  });

  test('create webhook via API and find it in list', async ({ request }) => {
    // Events serialized as snake_case: request_captured, response_captured
    const res = await request.post('/admin/webhooks', {
      data: {
        id: '',
        url: 'https://webhook.example.com/hook',
        events: ['request_captured'],
        enabled: true,
        secret: null,
      },
    });
    expect(res.ok()).toBeTruthy();
    const list = await (await request.get('/admin/webhooks')).json();
    const created = list.find(w => w.url === 'https://webhook.example.com/hook');
    expect(created).toBeTruthy();
    // Cleanup
    await request.delete(`/admin/webhooks/${created.id}`);
  });

  test('delete webhook removes it', async ({ request }) => {
    await request.post('/admin/webhooks', {
      data: { id: '', url: 'https://del.example.com/hook', events: ['response_captured'], enabled: false, secret: null },
    });
    const before = await (await request.get('/admin/webhooks')).json();
    const created = before.find(w => w.url === 'https://del.example.com/hook');
    expect(created).toBeTruthy();
    await request.delete(`/admin/webhooks/${created.id}`);
    const after = await (await request.get('/admin/webhooks')).json();
    expect(after.some(w => w.id === created.id)).toBeFalsy();
  });
});
