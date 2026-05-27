// @ts-check
const { test, expect } = require('@playwright/test');

const RAIL_VIEWS = [
  { label: 'Compose', heading: 'Compose' },
  { label: 'Rules', heading: 'Rules' },
  { label: 'Breakpoints', heading: 'Breakpoints' },
  { label: 'Mock Server', heading: 'Mock Server' },
  { label: 'Lua Scripts', heading: 'Lua scripts' },
  { label: 'Inspectors', heading: 'Inspectors' },
  { label: 'DNS Override', heading: 'DNS Override' },
  { label: 'Capture Filter', heading: 'Capture Filter' },
  { label: 'Webhooks', heading: 'Webhooks' },
  { label: 'Root CA', heading: 'Root CA' },
  { label: 'Settings', heading: 'Settings' },
];

test.describe('Sidebar navigation', () => {
  test('sessions view is active by default', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('button', { name: 'Sessions', exact: true })).toHaveClass(/active/);
    await expect(page.locator('table')).toBeVisible();
  });

  for (const { label, heading } of RAIL_VIEWS) {
    test(`navigates to ${label}`, async ({ page }) => {
      await page.goto('/');
      await page.getByRole('button', { name: label, exact: true }).click();
      await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: label, exact: true })).toHaveClass(/active/);
    });
  }

  test('clicking Sessions returns to traffic list', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Breakpoints', exact: true }).click();
    await page.getByRole('button', { name: 'Sessions', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Sessions', exact: true })).toHaveClass(/active/);
    await expect(page.locator('table')).toBeVisible();
  });
});
