// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('SSL / CA cert', () => {
  test('Root CA nav item opens the certificate view', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Root CA', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Root CA', exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'oproxy Root CA' })).toBeVisible();
  });

  test('/admin/ca returns a PEM certificate', async ({ request }) => {
    const res = await request.get('/admin/ca');
    expect(res.ok()).toBeTruthy();
    const text = await res.text();
    expect(text).toContain('BEGIN CERTIFICATE');
  });

  test('Root CA view exposes certificate download without fake trust status', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Root CA', exact: true }).click();
    await expect(page.getByRole('link', { name: 'Download certificate' })).toHaveAttribute('href', '/admin/ca');
    await expect(page.getByRole('link', { name: 'Open install guide' })).toHaveAttribute('href', '/setup/mobile');
    await expect(page.getByText('Trust status')).toHaveCount(0);
    await expect(page.getByText('oproxy probes the OS keychain')).toHaveCount(0);
  });

  test('advertised Root CA shortcut opens Root CA', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByTitle('Root CA · ⌘B')).toBeVisible();
    await page.keyboard.press('Control+B');
    await expect(page.getByRole('heading', { name: 'Root CA', exact: true })).toBeVisible();
  });

  test('/setup and /setup/mobile serve the install guide', async ({ request }) => {
    for (const path of ['/setup', '/setup/mobile']) {
      const res = await request.get(path);
      expect(res.ok()).toBeTruthy();
      const text = await res.text();
      expect(text).toContain('Mobile Device Setup');
      expect(text).toContain('/admin/setup/network-info');
    }
  });
});
