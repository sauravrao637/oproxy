// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail } = require('./helpers');

test.describe('DNS overrides', () => {
  test.afterEach(async ({ request }) => {
    await request.delete('/admin/dns/dns-test.example.com');
    await request.delete('/admin/dns/del-dns.example.com');
  });

  test('DNS view opens add override dialog', async ({ page }) => {
    await gotoRail(page, 'DNS Override');
    await page.getByRole('button', { name: /Add override/ }).click();
    await expect(page.getByRole('heading', { name: 'Add DNS override' })).toBeVisible();
    await expect(page.locator('[name="host"]')).toBeVisible();
    await expect(page.locator('[name="ip"]')).toBeVisible();
  });

  test('add DNS override via form', async ({ page, request }) => {
    await gotoRail(page, 'DNS Override');
    await page.getByRole('button', { name: /Add override/ }).click();
    await page.locator('[name="host"]').fill('dns-test.example.com');
    await page.locator('[name="ip"]').fill('1.2.3.4');
    await page.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByText('dns-test.example.com')).toBeVisible();

    const dns = await (await request.get('/admin/dns')).json();
    expect(dns['dns-test.example.com']).toBe('1.2.3.4');
  });

  test('delete DNS override via API removes it', async ({ request }) => {
    const existing = await (await request.get('/admin/dns')).json();
    await request.post('/admin/dns', { data: { ...existing, 'del-dns.example.com': '9.9.9.9' } });
    await request.delete('/admin/dns/del-dns.example.com');
    const after = await (await request.get('/admin/dns')).json();
    expect(after['del-dns.example.com']).toBeUndefined();
  });
});
