// @ts-check
const { test, expect } = require('@playwright/test');
const { gotoRail, importSession, sampleSession } = require('./helpers');

function pngSize(buffer) {
  expect(buffer.subarray(0, 8).toString('hex')).toBe('89504e470d0a1a0a');
  return {
    width: buffer.readUInt32BE(16),
    height: buffer.readUInt32BE(20),
  };
}

async function expectScreenshotFrame(page, label) {
  const shot = await page.screenshot({ fullPage: false });
  const size = pngSize(shot);
  expect(size.width, `${label} screenshot width`).toBe(1400);
  expect(size.height, `${label} screenshot height`).toBe(900);
  expect(shot.length, `${label} screenshot should not be tiny/blank`).toBeGreaterThan(25000);
}

async function expectDesktopChrome(page) {
  const boxes = await page.evaluate(() => {
    const rect = (selector) => {
      const el = document.querySelector(selector);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height };
    };
    return {
      topbar: rect('.topbar'),
      rail: rect('.rail'),
      main: rect('.main'),
      statusbar: rect('.statusbar'),
    };
  });

  expect(boxes.topbar?.height).toBeGreaterThan(40);
  expect(boxes.rail?.width).toBeGreaterThan(40);
  expect(boxes.main?.width).toBeGreaterThan(1000);
  expect(boxes.main?.height).toBeGreaterThan(760);
  expect(boxes.statusbar?.height).toBeGreaterThanOrEqual(24);
  expect(boxes.statusbar?.y).toBeGreaterThan(840);
}

test.describe('desktop visual smoke', () => {
  test('sessions workbench screenshot has populated desktop chrome', async ({ page, request }) => {
    await importSession(request, sampleSession({
      id: 'visual-session-detail',
      host: 'visual.example.com',
      method: 'POST',
      requestBody: '{"name":"visual"}',
    }));
    await page.goto('/');
    await expect(page.locator('tbody tr').filter({ hasText: 'visual.example.com' })).toBeVisible();
    await expectDesktopChrome(page);
    await expectScreenshotFrame(page, 'sessions');
  });

  test('key surfaces produce nonblank desktop screenshots', async ({ page }) => {
    for (const surface of [
      ['Compose', 'Compose'],
      ['Rules', 'Rules'],
      ['Settings', 'Settings'],
    ]) {
      await gotoRail(page, surface[0], surface[1]);
      await expectDesktopChrome(page);
      await expect(page.locator('.surface')).toBeVisible();
      await expectScreenshotFrame(page, surface[0]);
    }
  });
});
