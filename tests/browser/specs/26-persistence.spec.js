// @ts-check
const { test, expect } = require('@playwright/test');
const { importSession, sampleSession } = require('./helpers');

test.describe('Persistence guarantees', () => {
  test.afterEach(async ({ request }) => {
    await request.delete('/admin/sessions');
  });

  test('manual session save/load preserves annotations', async ({ request }) => {
    const id = `persist-session-${Date.now()}`;
    const path = `browser-${id}.json`;
    await importSession(request, sampleSession({ id, host: 'persist.example.com' }));
    const annotate = await request.patch(`/api/sessions/${id}/annotation`, {
      data: { note: 'keep this note', tags: ['persisted', 'beta'] },
    });
    expect(annotate.ok()).toBeTruthy();

    const save = await request.post('/admin/sessions/save', { data: { path } });
    expect(save.ok()).toBeTruthy();
    await request.delete('/admin/sessions');
    await expect.poll(async () => {
      const body = await (await request.get('/api/sessions')).json();
      return body.sessions.length;
    }).toBe(0);

    const load = await request.post('/admin/sessions/load', { data: { path } });
    expect(load.ok()).toBeTruthy();
    const detail = await (await request.get(`/api/sessions/${id}`)).json();
    expect(detail.exchange.note).toBe('keep this note');
    expect(detail.exchange.tags).toEqual(['persisted', 'beta']);
  });
});
