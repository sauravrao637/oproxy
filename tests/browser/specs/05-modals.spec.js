// @ts-check
const { test, expect } = require('@playwright/test');

test.describe('Dialogs and downloads', () => {
  test('shortcuts dialog closes with Escape', async ({ page }) => {
    await page.goto('/');
    await page.getByTitle('Keyboard shortcuts · ?').click();
    await expect(page.getByRole('heading', { name: 'Keyboard shortcuts' })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('heading', { name: 'Keyboard shortcuts' })).toHaveCount(0);
  });

  test('shortcuts button opens dialog', async ({ page }) => {
    await page.goto('/');
    await page.getByTitle('Keyboard shortcuts · ?').click();
    await expect(page.getByRole('heading', { name: 'Keyboard shortcuts' })).toBeVisible();
    await expect(page.getByText('Focus search')).toHaveCount(2);
  });

  test('export action downloads HAR directly', async ({ page }) => {
    await page.goto('/');
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByTitle('Export as HAR').click(),
    ]);
    expect(download.suggestedFilename()).toBe('oproxy-session.har');
  });

  test('rule form dialog can open and cancel', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Rules', exact: true }).click();
    await page.getByRole('button', { name: /Add rule/ }).click();
    await expect(page.getByRole('heading', { name: 'Add route' })).toBeVisible();
    await page.getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByRole('heading', { name: 'Add route' })).toHaveCount(0);
  });

  test('confirm dialog closes when cancelled', async ({ page }) => {
    await page.goto('/');
    await page.getByTitle('Clear all sessions').click();
    await expect(page.getByRole('heading', { name: 'Clear all captured sessions?' })).toBeVisible();
    await page.getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByRole('heading', { name: 'Clear all captured sessions?' })).toHaveCount(0);
  });
});
