// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Capture filter', () => {
  test.afterEach(async ({ request }) => {
    await request.post('/admin/capture-filter', { data: { mode: 'disabled', hosts: [] } });
  });

  test('GET /admin/capture-filter returns config with mode field', async ({ request }) => {
    const res = await request.get('/admin/capture-filter');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body).toHaveProperty('mode');
  });

  test('POST /admin/capture-filter sets denylist mode', async ({ request }) => {
    const res = await request.post('/admin/capture-filter', {
      data: { mode: 'denylist', hosts: ['ads.example.com'] },
    });
    expect(res.ok()).toBeTruthy();
    // Reset to disabled
    await request.post('/admin/capture-filter', {
      data: { mode: 'disabled', hosts: [] },
    });
  });

  test('POST /admin/capture-filter sets allowlist mode', async ({ request }) => {
    const res = await request.post('/admin/capture-filter', {
      data: { mode: 'allowlist', hosts: ['myapp.example.com'] },
    });
    expect(res.ok()).toBeTruthy();
    // Reset
    await request.post('/admin/capture-filter', { data: { mode: 'disabled', hosts: [] } });
  });

  test('UI copy makes clear filtering only affects recording', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Capture Filter' }).click();
    await expect(page.getByRole('heading', { name: 'Capture Filter' })).toBeVisible();

    await page.getByRole('button', { name: 'Allowlist' }).click();
    await expect(page.getByText('Only matching hosts are recorded. Non-matching traffic is still proxied.')).toBeVisible();

    await page.getByRole('button', { name: 'Denylist' }).click();
    await expect(page.getByText('Matching hosts are skipped from recording. Traffic is still proxied.')).toBeVisible();
  });
});
