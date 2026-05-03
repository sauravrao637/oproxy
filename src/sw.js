const CACHE = 'oproxy-shell-v3';
const SHELL = [
  '/', '/manifest.json', '/icons/icon.svg',
  '/app.css',
  '/js/state.js', '/js/traffic.js', '/js/compose.js',
  '/js/rules.js', '/js/breakpoints.js', '/js/chrome.js',
];

self.addEventListener('install', e => {
  e.waitUntil(
    caches.open(CACHE).then(c => c.addAll(SHELL)).then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', e => {
  e.waitUntil(
    caches.keys()
      .then(keys => Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', e => {
  const url = new URL(e.request.url);

  // Always go to network for API and admin routes
  if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/admin/')) {
    return;
  }

  e.respondWith(
    caches.match(e.request).then(cached => {
      if (cached) return cached;
      return fetch(e.request).then(res => {
        if (res.ok && SHELL.includes(url.pathname)) {
          const clone = res.clone();
          caches.open(CACHE).then(c => c.put(e.request, clone));
        }
        return res;
      }).catch(() => {
        // Offline fallback for navigation requests
        if (e.request.mode === 'navigate') {
          return caches.match('/').then(r => r || new Response(
            '<!DOCTYPE html><html><head><meta charset="UTF-8"><title>oproxy — offline</title>' +
            '<meta name="viewport" content="width=device-width,initial-scale=1">' +
            '<style>body{font-family:-apple-system,sans-serif;display:flex;align-items:center;' +
            'justify-content:center;height:100vh;margin:0;background:#F2F2F7;color:#1C1C1E}' +
            '.box{text-align:center;padding:40px}h1{font-size:24px;font-weight:600;margin-bottom:8px}' +
            'p{color:#636366;font-size:14px}</style></head><body>' +
            '<div class="box"><h1>oproxy is not running</h1>' +
            '<p>Start the proxy server, then reload this page.</p></div></body></html>',
            { headers: { 'Content-Type': 'text/html' } }
          ));
        }
      });
    })
  );
});
