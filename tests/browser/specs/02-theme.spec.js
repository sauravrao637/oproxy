// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Theme toggle', () => {
  test('clicking theme button switches theme', async ({ page }) => {
    await page.goto('/');
    const html = page.locator('html');
    const before = await html.getAttribute('data-theme');
    await page.getByTitle('Toggle theme · ⌘D').click();
    await expect(html).toHaveAttribute('data-theme', before === 'dark' ? 'light' : 'dark');
  });

  test('toggling twice returns to original theme', async ({ page }) => {
    await page.goto('/');
    const html = page.locator('html');
    const before = await html.getAttribute('data-theme');
    await page.getByTitle('Toggle theme · ⌘D').click();
    await page.getByTitle('Toggle theme · ⌘D').click();
    await expect(html).toHaveAttribute('data-theme', before || 'dark');
  });

  test('Ctrl+D keyboard shortcut toggles theme', async ({ page }) => {
    await page.goto('/');
    const html = page.locator('html');
    const before = await html.getAttribute('data-theme');
    await page.keyboard.press('Control+d');
    await expect(html).toHaveAttribute('data-theme', before === 'dark' ? 'light' : 'dark');
  });
});
