// @ts-check
const { test, expect } = require('@playwright/test');
const { importSession, sampleSession } = require('./helpers');

async function expectNoUnnamedInteractive(page, context) {
  const offenders = await page.evaluate(() => {
    const isVisible = (el) => {
      const style = window.getComputedStyle(el);
      return style.visibility !== 'hidden' &&
        style.display !== 'none' &&
        el.getClientRects().length > 0 &&
        !el.closest('[aria-hidden="true"]');
    };
    const labelText = (el) => {
      if (el.getAttribute('aria-label')?.trim()) return el.getAttribute('aria-label').trim();
      const ids = (el.getAttribute('aria-labelledby') || '').trim().split(/\s+/).filter(Boolean);
      const byId = ids.map(id => document.getElementById(id)?.textContent?.trim()).filter(Boolean).join(' ');
      if (byId) return byId;
      if (el.getAttribute('title')?.trim()) return el.getAttribute('title').trim();
      if (el instanceof HTMLInputElement && el.type === 'button' && el.value.trim()) return el.value.trim();
      if ((el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) && el.labels?.length) {
        const text = Array.from(el.labels).map(label => label.textContent?.trim()).filter(Boolean).join(' ');
        if (text) return text;
      }
      return (el.textContent || '').replace(/\s+/g, ' ').trim();
    };
    return Array.from(document.querySelectorAll('button, input:not([type="hidden"]), select, textarea, [role="button"], [role="checkbox"], [role="radio"]'))
      .filter(isVisible)
      .filter(el => !labelText(el))
      .map(el => ({
        tag: el.tagName.toLowerCase(),
        type: el.getAttribute('type') || '',
        className: el.getAttribute('class') || '',
        html: el.outerHTML.slice(0, 220),
      }));
  });
  expect(offenders, `${context} has unnamed interactive controls`).toEqual([]);
}

test.describe('desktop accessibility smoke', () => {
  test.beforeEach(async ({ request }) => {
    await request.delete('/admin/sessions');
    await request.post('/admin/capture-filter', { data: { mode: 'disabled', hosts: [] } });
  });

  test.afterEach(async ({ request }) => {
    await request.delete('/admin/sessions');
    await request.post('/admin/capture-filter', { data: { mode: 'disabled', hosts: [] } });
  });

  test('main rails expose names for visible controls', async ({ page, request }) => {
    await importSession(request, sampleSession({ id: 'a11y-detail', host: 'a11y.example.com' }));

    await page.goto('/');
    await expectNoUnnamedInteractive(page, 'Sessions');
    await page.locator('tbody tr').first().click();
    await page.locator('.detail-tabs button', { hasText: 'Response' }).click();
    await expectNoUnnamedInteractive(page, 'Session detail');

    const rails = [
      ['Rules', 'Rules'],
      ['Breakpoints', 'Breakpoints'],
      ['Mock Server', 'Mock Server'],
      ['Lua Scripts', 'Lua scripts'],
      ['Inspectors', 'Inspectors'],
      ['DNS Override', 'DNS Override'],
      ['Capture Filter', 'Capture Filter'],
      ['Webhooks', 'Webhooks'],
      ['Root CA', 'Root CA'],
      ['Settings', 'Settings'],
    ];
    for (const [rail, heading] of rails) {
      await page.getByRole('button', { name: rail, exact: true }).click();
      await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible();
      await expectNoUnnamedInteractive(page, rail);
    }
  });

  test('compose and capture filter dynamic controls expose names', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Compose', exact: true }).click();
    await page.locator('.cmp-tab-new').click();
    await expectNoUnnamedInteractive(page, 'Compose headers');

    await page.getByRole('button', { name: /Body/ }).click();
    await expectNoUnnamedInteractive(page, 'Compose body');

    await page.getByRole('button', { name: /Auth/ }).click();
    await page.locator('.cmp-pane select').selectOption('bearer');
    await expectNoUnnamedInteractive(page, 'Compose auth');

    await page.getByRole('button', { name: 'Capture Filter', exact: true }).click();
    await page.getByRole('button', { name: 'Allowlist' }).click();
    await expectNoUnnamedInteractive(page, 'Capture filter allowlist');
  });
});
