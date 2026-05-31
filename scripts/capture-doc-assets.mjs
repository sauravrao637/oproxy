import playwright from '../tests/browser/node_modules/playwright/index.js';
import fs from 'node:fs/promises';
import path from 'node:path';

const { chromium } = playwright;
const baseURL = process.env.OPROXY_BASE_URL || 'http://127.0.0.1:18080';
const outDir = path.resolve('docs/assets');

async function api(pathname, options = {}) {
  const res = await fetch(`${baseURL}${pathname}`, {
    headers: { 'content-type': 'application/json', ...(options.headers || {}) },
    ...options,
  });
  if (!res.ok) {
    throw new Error(`${options.method || 'GET'} ${pathname} failed: ${res.status} ${await res.text()}`);
  }
  return res;
}

function session({
  id,
  host,
  method = 'GET',
  uri,
  status = 200,
  requestHeaders = { accept: 'application/json' },
  requestBody = '',
  responseHeaders = { 'content-type': 'application/json' },
  responseBody = '{"ok":true}',
  tags = [],
  note = null,
  latency = 42,
}) {
  const finalUri = uri || `https://${host}/api`;
  return {
    id,
    timestamp: new Date(Date.now() - Math.floor(Math.random() * 240000)).toISOString(),
    request: {
      method,
      uri: finalUri,
      headers: requestHeaders,
      body: requestBody,
      host,
      body_bytes: null,
    },
    response: {
      status,
      headers: responseHeaders,
      body: responseBody,
      request_uri: finalUri,
      session_id: id,
      ttfb_ms: Math.max(5, Math.floor(latency * 0.65)),
      body_ms: Math.max(2, Math.floor(latency * 0.35)),
      body_bytes: null,
    },
    metrics: {
      latency_ms: latency,
      request_size_bytes: requestBody.length,
      response_size_bytes: responseBody.length,
      status_code: status,
      ttfb_ms: Math.max(5, Math.floor(latency * 0.65)),
      body_ms: Math.max(2, Math.floor(latency * 0.35)),
    },
    source: tags.includes('replay') ? 'admin_forward' : 'proxy',
    ws_frames: [],
    note,
    tags,
  };
}

async function seedData() {
  await fs.mkdir(outDir, { recursive: true });
  await fs.mkdir('/tmp/oproxy-docs-fixtures', { recursive: true });
  await fs.writeFile('/tmp/oproxy-docs-fixtures/users.json', '[{"id":1,"name":"Ada"},{"id":2,"name":"Lin"}]\n');

  await api('/admin/sessions', { method: 'DELETE', headers: {} });
  for (const route of ['/admin/rule-sets', '/admin/map-remote-rules', '/admin/map-local-rules']) {
    const current = await (await api(route)).json();
    for (const item of current) {
      await api(`${route}/${encodeURIComponent(item.id)}`, { method: 'DELETE', headers: {} });
    }
  }
  for (const route of ['/admin/mock/rules']) {
    const current = await (await api(route)).json();
    for (const item of current) {
      await api(`${route}/${encodeURIComponent(item.id)}`, { method: 'DELETE', headers: {} });
    }
  }
  await api('/admin/sessions/import', {
    method: 'POST',
    body: JSON.stringify({
      merge: false,
      sessions: [
        session({
          id: 'docs-login',
          host: 'api.acme.test',
          method: 'POST',
          uri: 'https://api.acme.test/v1/login',
          status: 200,
          requestHeaders: { 'content-type': 'application/json', authorization: 'Bearer doc-token' },
          requestBody: '{"email":"dev@example.com","password":"secret-password"}',
          responseBody: '{"token":"secret-token","user":{"name":"Dev"}}',
          tags: ['auth'],
          note: 'Default exports redact this request.',
          latency: 86,
        }),
        session({
          id: 'docs-users',
          host: 'api.acme.test',
          uri: 'https://api.acme.test/v1/users?active=true',
          status: 200,
          responseBody: '{"users":[{"id":1,"name":"Ada"},{"id":2,"name":"Lin"}]}',
          tags: ['mock'],
          latency: 31,
        }),
        session({
          id: 'docs-assets',
          host: 'cdn.acme.test',
          uri: 'https://cdn.acme.test/assets/app.js',
          status: 304,
          responseHeaders: { etag: '"docs-1"' },
          responseBody: '',
          latency: 18,
        }),
        session({
          id: 'docs-replay',
          host: 'staging.acme.test',
          method: 'PATCH',
          uri: 'https://staging.acme.test/v2/users/42',
          status: 202,
          requestHeaders: { 'content-type': 'application/json' },
          requestBody: '{"role":"admin"}',
          responseBody: '{"queued":true}',
          tags: ['replay'],
          latency: 119,
        }),
        session({
          id: 'docs-error',
          host: 'api.acme.test',
          uri: 'https://api.acme.test/v1/report',
          status: 502,
          responseHeaders: { 'content-type': 'text/plain' },
          responseBody: 'upstream unavailable',
          note: 'Used to test retry handling.',
          latency: 244,
        }),
      ],
    }),
  });

  await api('/admin/rule-sets', {
    method: 'POST',
    body: JSON.stringify({
      id: '',
      name: 'Redact staging headers',
      enabled: true,
      location: { host: 'staging.acme.test', path: '/v2/*', mode: 'glob' },
      applies_to: 'request',
      actions: [
        { type: 'remove_header', name: 'x-debug-token' },
        { type: 'set_header', name: 'x-oproxy-demo', value: 'true' },
      ],
    }),
  });

  await api('/admin/map-remote-rules', {
    method: 'POST',
    body: JSON.stringify({
      id: '',
      name: 'API to local service',
      enabled: true,
      location: { host: 'api.acme.test', path: '/v1/*', mode: 'glob' },
      destination: 'http://127.0.0.1:3000',
    }),
  });

  await api('/admin/map-local-rules', {
    method: 'POST',
    body: JSON.stringify({
      id: '',
      name: 'Users fixture',
      enabled: true,
      location: { host: 'api.acme.test', path: '/fixtures/users.json', mode: 'glob' },
      file_path: '/tmp/oproxy-docs-fixtures/users.json',
    }),
  });

  await api('/admin/mock/rules', {
    method: 'POST',
    body: JSON.stringify({
      id: '',
      name: 'Mock user profile',
      enabled: true,
      location: { host: 'api.acme.test', path: '^/v1/users/(\\d+)$', mode: 'regex', methods: ['GET'] },
      responses: [
        {
          status: 200,
          headers: { 'content-type': 'application/json' },
          body: '{"id":"${1}","name":"Demo User"}',
          delay_ms: 40,
        },
      ],
      call_count: 7,
    }),
  });

  await api('/admin/dns', {
    method: 'POST',
    body: JSON.stringify({
      'api.acme.test': '127.0.0.1',
      'staging.acme.test': '127.0.0.1',
    }),
  });
}

async function screenshot(page, name) {
  await page.screenshot({ path: path.join(outDir, name), fullPage: false });
}

async function makeDemoVideo(browser) {
  const frames = [
    {
      file: 'sessions-screenshot.png',
      label: 'Capture traffic',
      detail: 'Requests, responses, status, timing, and bodies in one view.',
    },
    {
      file: 'sessions-screenshot.png',
      label: 'Inspect a session',
      detail: 'Open a captured request and check the headers or payload.',
    },
    {
      file: 'compose-screenshot.png',
      label: 'Replay in Compose',
      detail: 'Use saved requests, variables, headers, auth, and raw bodies.',
    },
    {
      file: 'compose-screenshot.png',
      label: 'Paste cURL',
      detail: 'Import method, URL, headers, and body into the active tab.',
    },
    {
      file: 'rules-screenshot.png',
      label: 'Shape proxy behavior',
      detail: 'Add rewrites, map-local fixtures, mocks, throttling, and filters.',
    },
    {
      file: 'sessions-screenshot.png',
      label: 'Export or replay',
      detail: 'Copy redacted cURL, export HAR, or send the request again.',
    },
  ];
  const payload = [];
  for (const frame of frames) {
    const bytes = await fs.readFile(path.join(outDir, frame.file));
    payload.push({
      ...frame,
      dataUrl: `data:image/png;base64,${bytes.toString('base64')}`,
    });
  }

  const page = await browser.newPage({ viewport: { width: 1440, height: 920 } });
  const base64 = await page.evaluate(async ({ frames, width, height }) => {
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    document.body.style.margin = '0';
    document.body.appendChild(canvas);
    const ctx = canvas.getContext('2d');
    const images = await Promise.all(frames.map(frame => new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => resolve({ img, label: frame.label, detail: frame.detail });
      img.onerror = reject;
      img.src = frame.dataUrl;
    })));

    const stream = canvas.captureStream(24);
    const mimeType = MediaRecorder.isTypeSupported('video/webm;codecs=vp8')
      ? 'video/webm;codecs=vp8'
      : 'video/webm';
    const recorder = new MediaRecorder(stream, { mimeType });
    const chunks = [];
    recorder.ondataavailable = event => {
      if (event.data.size > 0) chunks.push(event.data);
    };
    const stopped = new Promise(resolve => {
      recorder.onstop = resolve;
    });

    function roundedRect(x, y, w, h, r) {
      ctx.beginPath();
      ctx.moveTo(x + r, y);
      ctx.arcTo(x + w, y, x + w, y + h, r);
      ctx.arcTo(x + w, y + h, x, y + h, r);
      ctx.arcTo(x, y + h, x, y, r);
      ctx.arcTo(x, y, x + w, y, r);
      ctx.closePath();
    }

    function drawFrame(image, progress, frameIndex, totalFrames) {
      ctx.fillStyle = '#0b0f14';
      ctx.fillRect(0, 0, width, height);
      const scale = 1 + progress * 0.018;
      const drawW = width * scale;
      const drawH = height * scale;
      ctx.drawImage(image.img, (width - drawW) / 2, (height - drawH) / 2, drawW, drawH);

      ctx.fillStyle = 'rgba(5, 8, 12, 0.18)';
      ctx.fillRect(0, 0, width, height);

      const grad = ctx.createLinearGradient(0, height - 230, 0, height);
      grad.addColorStop(0, 'rgba(8, 12, 18, 0)');
      grad.addColorStop(1, 'rgba(8, 12, 18, 0.88)');
      ctx.fillStyle = grad;
      ctx.fillRect(0, height - 230, width, 230);

      roundedRect(48, 48, 132, 38, 8);
      ctx.fillStyle = 'rgba(10, 15, 22, 0.72)';
      ctx.fill();
      ctx.strokeStyle = 'rgba(255,255,255,0.14)';
      ctx.stroke();
      ctx.fillStyle = 'rgba(255,255,255,0.88)';
      ctx.font = '600 15px Inter, system-ui, sans-serif';
      ctx.fillText('oproxy', 68, 73);

      ctx.fillStyle = 'rgba(255,255,255,0.92)';
      ctx.font = '600 36px Inter, system-ui, sans-serif';
      ctx.fillText(image.label, 56, height - 96);

      ctx.fillStyle = 'rgba(223, 232, 242, 0.82)';
      ctx.font = '400 18px Inter, system-ui, sans-serif';
      ctx.fillText(image.detail, 58, height - 62);

      const lineX = 56;
      const lineY = height - 28;
      const lineW = width - 112;
      ctx.strokeStyle = 'rgba(255,255,255,0.22)';
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(lineX, lineY);
      ctx.lineTo(lineX + lineW, lineY);
      ctx.stroke();

      ctx.fillStyle = 'rgba(117, 210, 255, 0.92)';
      const absoluteProgress = (frameIndex + progress) / totalFrames;
      roundedRect(lineX, lineY - 1.5, lineW * absoluteProgress, 3, 2);
      ctx.fill();
    }

    recorder.start();
    const frameDurationMs = 5000;
    for (let index = 0; index < images.length; index += 1) {
      const image = images[index];
      const start = performance.now();
      await new Promise(resolve => {
        function tick(now) {
          const elapsed = now - start;
          const progress = Math.min(elapsed / frameDurationMs, 1);
          drawFrame(image, progress, index, images.length);
          if (progress < 1) requestAnimationFrame(tick);
          else resolve();
        }
        requestAnimationFrame(tick);
      });
    }
    recorder.stop();
    await stopped;

    const blob = new Blob(chunks, { type: mimeType });
    const buffer = await blob.arrayBuffer();
    let binary = '';
    const bytes = new Uint8Array(buffer);
    for (let i = 0; i < bytes.length; i += 1) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
  }, { frames: payload, width: 1440, height: 920 });

  await page.close();
  await fs.writeFile(path.join(outDir, 'demo.webm'), Buffer.from(base64, 'base64'));
}

async function clickRail(page, name, heading = name) {
  await page.getByRole('button', { name, exact: true }).click();
  await page.getByText(heading, { exact: true }).first().waitFor({ state: 'visible' });
  await page.waitForTimeout(350);
}

async function main() {
  await seedData();

  const preferredExecutable = process.env.CHROMIUM_EXECUTABLE_PATH || '/usr/bin/brave-browser';
  const executablePath = await fs.access(preferredExecutable)
    .then(() => preferredExecutable)
    .catch(() => undefined);
  const browser = await chromium.launch({
    headless: true,
    executablePath,
    args: ['--no-sandbox'],
  });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 920 },
    deviceScaleFactor: 1,
  });
  const page = await context.newPage();

  await page.goto(baseURL, { waitUntil: 'domcontentloaded' });
  await page.getByText('Sessions', { exact: true }).first().waitFor({ state: 'visible' });
  await page.waitForTimeout(800);
  await screenshot(page, 'sessions-screenshot.png');

  await clickRail(page, 'Compose');
  await page.evaluate(() => {
    localStorage.setItem('oproxy.compose.workspace.v1', JSON.stringify({
      collections: [{
        id: 'c_docs',
        name: 'Acme API',
        open: true,
        requests: [{
          id: 'r_docs_login',
          name: 'Login smoke',
          method: 'POST',
          url: 'https://{{base}}/v1/login',
          headers: [{ id: 'h1', on: true, key: 'content-type', value: 'application/json' }],
          params: [],
          body: '{"email":"dev@example.com","password":"{{password}}"}',
          bodyMode: 'raw',
          contentType: 'application/json',
        }],
      }],
      variables: [
        { id: 'v_base', enabled: true, key: 'base', value: 'api.acme.test' },
        { id: 'v_password', enabled: true, key: 'password', value: 'secret-password' },
      ],
    }));
  });
  await page.reload();
  await clickRail(page, 'Compose');
  await page.getByText('Login smoke').click();
  await page.waitForTimeout(500);
  await screenshot(page, 'compose-screenshot.png');

  await clickRail(page, 'Rules');
  await screenshot(page, 'rules-screenshot.png');

  await clickRail(page, 'Mock Server');
  await page.waitForTimeout(650);
  await clickRail(page, 'DNS Override');
  await page.waitForTimeout(650);
  await clickRail(page, 'Sessions');
  await page.waitForTimeout(650);

  await context.close();
  await makeDemoVideo(browser);
  await browser.close();
}

main().catch(error => {
  console.error(error);
  process.exit(1);
});
