// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail, importSession, sampleSession } = require('./helpers');

test.describe('Critical control semantics', () => {
  test.afterEach(async ({ request }) => {
    await request.delete('/admin/sessions');
  });

  test('inspectors surface does not render fake middleware toggles', async ({ page }) => {
    await gotoRail(page, 'Inspectors');
    await expect(page.getByLabel(/Inspector .* enabled/)).toHaveCount(0);
    await expect(page.getByText('managed by runtime configuration').first()).toBeVisible();
    await expect(page.locator('.insp-card button')).toHaveCount(0);
  });

  test('primary navigation and session details affordances are discoverable', async ({ page, request }) => {
    const id = `ui-affordance-${Date.now()}`;
    await importSession(request, sampleSession({
      id,
      host: 'ui.example.com',
      uri: 'https://ui.example.com/v1/check',
    }));

    await page.goto('/');
    await expect(page.locator('tbody tr').first()).toContainText('ui.example.com');
    await expect(page.locator('.rail-btn .label', { hasText: 'Sessions' })).toBeVisible();
    await expect(page.locator('.rail-btn .label', { hasText: 'Compose' })).toBeVisible();

    await page.getByRole('button', { name: 'Open focus host menu' }).click();
    await expect(page.locator('.host-menu')).toBeVisible();
    await expect(page.locator('.host-menu .item', { hasText: 'ui.example.com' })).toBeVisible();
    await page.locator('.host-menu .item', { hasText: 'ui.example.com' }).click();
    await expect(page.locator('.focus-chip', { hasText: 'ui.example.com' })).toBeVisible();

    await page.locator('tbody tr').first().click();
    const panel = page.locator('.detail-panel');
    await expect(panel).toBeVisible();
    const before = await panel.boundingBox();
    const divider = await page.locator('.divider').boundingBox();
    expect(before).toBeTruthy();
    expect(divider).toBeTruthy();

    await page.mouse.move(divider.x + divider.width / 2, divider.y + divider.height / 2);
    await page.mouse.down();
    await page.mouse.move(divider.x - 140, divider.y + divider.height / 2, { steps: 6 });
    await page.mouse.up();

    const after = await panel.boundingBox();
    expect(after.width).toBeGreaterThan(before.width + 80);
  });
});
