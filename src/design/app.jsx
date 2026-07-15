import React from 'react';
const {
  useTweaks, TweaksPanel, TweakSection, TweakRadio, TweakSelect,
  Icon, SessionsTable, DetailPanel, RulesSurface, BreakpointsSurface,
  InspectorsSurface, CertSurface, ComposeSurface, MockSurface, LuaSurface,
  WebhooksSurface, DnsSurface, CaptureFilterSurface, SettingsSurface,
  ConnectionsSurface, ProtocolDashboard, AssistantSurface, ShortcutsModal, confirmAction,
} = window;
/* Main app shell — top bar, left rail, master/detail split, status bar, tweaks */

const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
  "theme": "dark",
  "density": "default",
  "accentHue": 215,
  "split": "vertical",
  "showWaterfall": true
}/*EDITMODE-END*/;

const ACCENT_OPTIONS = [
  { label: 'Cyan',     h: 215 },
  { label: 'Lime',     h: 145 },
  { label: 'Amber',    h: 78  },
  { label: 'Magenta',  h: 320 },
];

const METHODS = ['GET','POST','PUT','PATCH','DELETE','CONNECT','OPTIONS','HEAD'];
const STATUS_BUCKETS = ['1','2','3','4','5','-'];
const WORKSPACE_SURFACES = new Set(['sessions','dashboard','connections','compose','rules','breakpoints','mock','lua','inspector','dns','capture','webhooks','ca','settings']);
const SESSION_SORT_KEYS = new Set(['idx','method','status','host','path','type','reqSize','total','ts','protocol']);
const WIRE_BUCKETS = ['h1','h2','h3','socks'];
const PROTO_FILTERS = [
  { key: 'h1', label: '1.1', title: 'HTTP/1.1' },
  { key: 'h2', label: 'H2', title: 'HTTP/2' },
  { key: 'h3', label: 'H3', title: 'HTTP/3' },
  { key: 'socks', label: 'SOCKS', title: 'SOCKS5' },
];
const APP_BUCKETS = ['http','ws','grpc','tunnel'];
const APP_FILTERS = [
  { key: 'http', label: 'HTTP', title: 'HTTP requests' },
  { key: 'ws', label: 'WS', title: 'WebSocket' },
  { key: 'grpc', label: 'gRPC', title: 'gRPC' },
  { key: 'tunnel', label: 'Tunnel', title: 'TCP/SOCKS tunnels' },
];

function protocolToken(value) {
  return String(value || '').toLowerCase().replace(/[^a-z0-9]/g, '');
}

// Maps a session's protocol identity to a filter bucket.
const protoBucketOf = (p, session = null) => {
  const candidates = [
    p,
    session?.proto,
    session?.wireProtocol,
    session?.appProtocol,
    session?.protocolContext?.downstream,
    session?.protocolContext?.application,
    session?.protocol_context?.downstream,
    session?.protocol_context?.application,
    session?.type,
    session?.method,
  ].map(protocolToken);
  if (candidates.some(v => v === 'grpc')) return 'grpc';
  if (candidates.some(v => v === 'websocket' || v === 'ws')) return 'ws';
  if (candidates.some(v => v === 'socks5' || v === 'socks')) return 'socks';
  if (candidates.some(v => v === 'http3' || v === 'h3')) return 'h3';
  if (candidates.some(v => v === 'http2' || v === 'h2')) return 'h2';
  if (candidates.some(v => v === 'http11' || v === 'http10' || v === 'http1' || v === 'h1')) return 'h1';
  return 'h1';
};

const wireBucketOf = (p, session = null) => {
  const candidates = [
    p,
    session?.wireProtocol,
    session?.protocolContext?.downstream,
    session?.protocol_context?.downstream,
    session?.downstream_protocol,
  ].map(protocolToken);
  if (candidates.some(v => v === 'socks5' || v === 'socks')) return 'socks';
  if (candidates.some(v => v === 'http3' || v === 'h3')) return 'h3';
  if (candidates.some(v => v === 'http2' || v === 'h2')) return 'h2';
  return 'h1';
};

const appBucketOf = (p, session = null) => {
  const candidates = [
    p,
    session?.appProtocol,
    session?.protocolContext?.application,
    session?.protocolContext?.body_mode,
    session?.protocol_context?.application,
    session?.protocol_context?.body_mode,
    session?.type,
    session?.method,
  ].map(protocolToken);
  if (candidates.some(v => v === 'grpc')) return 'grpc';
  if (candidates.some(v => v === 'websocket' || v === 'ws' || v === 'frames')) return 'ws';
  if (candidates.some(v => v === 'tunnel' || v === 'socks5' || v === 'socks')) return 'tunnel';
  return 'http';
};

function protocolLabel(value) {
  const token = protocolToken(value);
  if (token === 'admin') return '';
  if (token === 'grpc') return 'gRPC';
  if (token === 'websocket' || token === 'ws') return 'WebSocket';
  if (token === 'socks5' || token === 'socks') return 'SOCKS5';
  if (token === 'http3' || token === 'h3') return 'HTTP/3';
  if (token === 'http2' || token === 'h2') return 'HTTP/2';
  if (token === 'http10') return 'HTTP/1.0';
  if (token === 'http11' || token === 'http1' || token === 'h1') return 'HTTP/1.1';
  return value || '';
}

function applicationLabel(value, fallback = 'HTTP') {
  const token = protocolToken(value);
  if (token === 'grpc') return 'gRPC';
  if (token === 'websocket' || token === 'ws' || token === 'frames') return 'WebSocket';
  if (token === 'tunnel' || token === 'socks5' || token === 'socks' || token === 'raw') return 'Tunnel';
  return fallback;
}

function sourceLabel(value) {
  const token = String(value || 'proxy').toLowerCase().replace(/[^a-z0-9]/g, '');
  if (token === 'adminforward') return 'Compose';
  if (token === 'playback') return 'Replay';
  if (token === 'imported') return 'Imported';
  return 'Proxy';
}

const SESSION_LIST_LIMIT = 10000;
const SESSION_RENDER_PAGE_SIZE = 250;

const STATUS_TEXT = {
  0: 'Pending',
  101: 'Switching Protocols',
  200: 'OK',
  201: 'Created',
  204: 'No Content',
  206: 'Partial Content',
  301: 'Moved Permanently',
  302: 'Found',
  304: 'Not Modified',
  400: 'Bad Request',
  401: 'Unauthorized',
  403: 'Forbidden',
  404: 'Not Found',
  402: 'Payment Required',
  408: 'Timeout',
  409: 'Conflict',
  410: 'Gone',
  413: 'Payload Too Large',
  422: 'Unprocessable Content',
  429: 'Too Many Requests',
  500: 'Internal Server Error',
  502: 'Bad Gateway',
  503: 'Service Unavailable',
  504: 'Gateway Timeout',
};

function inferType(exchange) {
  if (exchange.request?.method === 'WS') return 'ws';
  const proto = exchange.protocol_context || {};
  if (protocolToken(proto.downstream) === 'websocket') return 'ws';
  if (protocolToken(proto.downstream) === 'socks5' || protocolToken(proto.body_mode) === 'tunnel') return 'tunnel';
  if (protocolToken(proto.application) === 'grpc') return 'grpc';
  const headers = exchange.response?.headers || exchange.request?.headers || {};
  const contentType = Object.entries(headers).find(([k]) => k.toLowerCase() === 'content-type')?.[1] || '';
  const mime = String(contentType).split(';')[0].trim().toLowerCase();
  if (exchange.inspector_data?.graphql) return 'graphql';
  if (exchange.inspector_data?.grpc) return 'grpc';
  if (mime.includes('json')) return 'json';
  if (mime.includes('html')) return 'html';
  if (mime.includes('javascript')) return 'js';
  if (mime.includes('css')) return 'css';
  if (mime.startsWith('image/')) return 'image';
  if (mime.includes('event-stream')) return 'sse';
  if (mime.includes('xml')) return 'xml';
  if (mime.startsWith('text/')) return 'text';
  return exchange.response ? 'http' : 'pending';
}

function protocolIdentity(exchange, inferredType, parts) {
  const ctx = exchange.protocol_context || null;
  const req = exchange.request || {};
  const metrics = exchange.metrics || {};
  const app = protocolToken(ctx?.application);
  const downstream = protocolToken(exchange.downstream_protocol) === 'admin' ? null : exchange.downstream_protocol;
  const wire = protocolToken(ctx?.downstream || downstream || metrics.protocol || req.version);
  const bodyMode = protocolToken(ctx?.body_mode);
  const method = String(req.method || '').toUpperCase();
  const type = protocolToken(inferredType);

  const wireProtocol = protocolLabel(ctx?.downstream || downstream || metrics.protocol || req.version || 'HTTP/1.1') || 'HTTP/1.1';
  const appProtocol = app === 'grpc' || type === 'grpc'
    ? 'gRPC'
    : (wire === 'websocket' || bodyMode === 'frames' || method === 'WS')
      ? 'WebSocket'
      : (wire === 'socks5' || bodyMode === 'tunnel' || parts.scheme === 'socks5')
        ? 'Tunnel'
        : applicationLabel(ctx?.application, 'HTTP');

  const bucket = appBucketOf(appProtocol, {
    wireProtocol: ctx?.downstream,
    appProtocol: ctx?.application,
    protocolContext: ctx,
    type: inferredType,
    method,
  });

  return {
    protocolContext: ctx,
    proto: appProtocol,
    protoBucket: bucket,
    appBucket: bucket,
    wireBucket: wireBucketOf(wireProtocol, { protocolContext: ctx, downstream_protocol: downstream }),
    wireProtocol,
    appProtocol,
    bodyMode: ctx?.body_mode || null,
  };
}

function parseUrlParts(uri, host) {
  try {
    const isAbsolute = /^[a-z][a-z0-9+.-]*:\/\//i.test(uri || '');
    const url = new URL(isAbsolute ? uri : `https://${host || 'unknown'}${uri || '/'}`);
    return {
      scheme: url.protocol.replace(':', '') || 'https',
      host: url.host || host || '',
      path: url.pathname || '/',
      query: url.search || '',
      url: url.href,
    };
  } catch {
    return {
      scheme: 'https',
      host: host || '',
      path: uri || '/',
      query: '',
      url: uri || '/',
    };
  }
}

function normalizeInspectorData(data) {
  if (!data) return null;
  if (data.jwt) {
    return {
      kind: 'jwt',
      header: data.jwt.header || {},
      payload: data.jwt.claims || {},
      valid: !data.jwt.alg_none_warning,
      expired: !!data.jwt.expired,
      expAt: data.jwt.expired ? null : (data.jwt.claims?.exp || null),
      expiresIn: (() => {
        if (data.jwt.expired) return 'expired';
        const exp = data.jwt.claims?.exp;
        if (!exp) return 'no expiry';
        const secsLeft = exp - Math.floor(Date.now() / 1000);
        if (secsLeft <= 0) return 'expired';
        if (secsLeft < 60) return `${secsLeft}s`;
        if (secsLeft < 3600) return `${Math.floor(secsLeft / 60)}m`;
        if (secsLeft < 86400) return `${Math.floor(secsLeft / 3600)}h`;
        return `${Math.floor(secsLeft / 86400)}d`;
      })(),
    };
  }
  if (data.graphql) {
    return {
      kind: 'graphql',
      type: data.graphql.operation_type || 'unknown',
      operation: data.graphql.operation_name || '(anonymous)',
      variables: data.graphql.variables || {},
      fields: data.graphql.variables && typeof data.graphql.variables === 'object'
        ? Object.keys(data.graphql.variables).length
        : 0,
    };
  }
  if (data.grpc) {
    const messages = data.grpc.messages || [];
    return {
      kind: 'grpc',
      service: data.grpc.service || '(unknown service)',
      rpc: data.grpc.method || '(unknown method)',
      // Full ordered, direction-aware message stream for the timeline view.
      messages,
      reqCount: messages.filter(m => m.direction === 'request').length,
      resCount: messages.filter(m => m.direction === 'response').length,
      requestMessage: JSON.stringify(messages.filter(m => m.direction === 'request'), null, 2),
      responseMessage: JSON.stringify(messages.filter(m => m.direction === 'response'), null, 2),
    };
  }
  return null;
}

function adaptExchange(exchange, idx) {
  const req = exchange.request || {};
  const res = exchange.response || null;
  const metrics = exchange.metrics || {};
  const parts = parseUrlParts(req.uri || res?.request_uri || '/', req.host);
  const reqHeadersRaw = req.headers || {};
  const resHeadersRaw = res?.headers || {};
  const reqContentType = reqHeadersRaw['content-type'] || reqHeadersRaw['Content-Type'] || '';
  const resContentType = resHeadersRaw['content-type'] || resHeadersRaw['Content-Type'] || '';
  const status = metrics.status_code || res?.status || 0;
  const ttfb = metrics.ttfb_ms || metrics.latency_ms || 0;
  const body = metrics.body_ms || Math.max(0, (metrics.latency_ms || 0) - ttfb);
  const inferredType = inferType(exchange);
  const protocol = protocolIdentity(exchange, inferredType, parts);
  const tags = [
    ...(exchange.tags || []),
    exchange.inspector_data?.jwt ? 'jwt' : null,
    exchange.inspector_data?.graphql ? 'graphql' : null,
    (exchange.inspector_data?.grpc || protocol.protoBucket === 'grpc') ? 'grpc' : null,
    protocol.protoBucket === 'ws' ? 'ws' : null,
    parts.scheme === 'https' ? 'mitm' : null,
  ].filter(Boolean);
  return {
    id: exchange.id || `live_${idx}`,
    idx: idx + 1,
    ts: Date.parse(exchange.timestamp || exchange.updated_at || new Date().toISOString()),
    scheme: parts.scheme,
    url: parts.url,
    method: (req.method || 'GET').toUpperCase(),
    displayMethod: protocol.appBucket === 'ws' ? 'GET' : (req.method || 'GET').toUpperCase(),
    host: parts.host,
    path: parts.path,
    query: parts.query,
    status,
    statusText: STATUS_TEXT[status] || '',
    type: inferredType,
    reqSize: metrics.request_size_bytes || req.body_bytes || (req.body ? String(req.body).length : 0),
    resSize: metrics.response_size_bytes || res?.body_bytes || (res?.body ? String(res.body).length : 0),
    total: metrics.latency_ms || 0,
    ttfb,
    timing: { dns: 0, tcp: 0, tls: 0, ttfb, body },
    tags,
    // paused_at is only set by the breakpoint middleware when a rule is actively holding the request.
    // A request with no response but no paused_at is simply in-flight (pending).
    paused: !!exchange.paused_at,
    pending: !res && !exchange.paused_at && (req.method || 'GET').toUpperCase() !== 'WS',
    note: exchange.note || '',
    // Captured WebSocket frames (empty for non-WS exchanges).
    wsFrames: exchange.ws_frames || [],
    protocolContext: protocol.protocolContext,
    wireProtocol: protocol.wireProtocol,
    appProtocol: protocol.appProtocol,
    bodyMode: protocol.bodyMode,
    appBucket: protocol.appBucket,
    wireBucket: protocol.wireBucket,
    protoBucket: protocol.protoBucket,
    proto: protocol.proto,
    sourceLabel: sourceLabel(exchange.source),
    canCompose: protocol.appBucket !== 'tunnel',
    canReplay: protocol.appBucket !== 'tunnel',
    commandLabel: protocol.appBucket === 'ws' ? 'websocat' : protocol.appBucket === 'tunnel' ? null : 'cURL',
    rawCommandLabel: protocol.appBucket === 'ws' ? 'raw websocat' : protocol.appBucket === 'tunnel' ? null : 'raw cURL',
    remote: req.remote_addr || '',
    cipher: parts.scheme === 'https' ? 'TLS' : '',
    reqHeadersRaw,
    resHeadersRaw,
    reqBodyRaw: req.body || '',
    resBodyRaw: res?.body || '',
    reqHeaders: redactHeaders(reqHeadersRaw),
    resHeaders: redactHeaders(resHeadersRaw),
    reqBody: redactBodyText(req.body || '', reqContentType),
    resBody: redactBodyText(res?.body || '', resContentType),
    inspector: normalizeInspectorData(exchange.inspector_data),
    rewriteApplied: tags.includes('rewrite') ? 'rewrite applied' : '',
  };
}

function headerItems(headers) {
  return Object.entries(headers || {})
    .filter(([k]) => {
      const key = k.toLowerCase();
      return !['host', 'content-length', 'connection', 'proxy-connection'].includes(key) && !key.startsWith('x-oproxy-');
    })
    .map(([key, value], i) => ({ id: `h_${Date.now()}_${i}`, on: true, key, value: String(value) }));
}

function replayableHeaders(headers) {
  return Object.fromEntries(Object.entries(headers || {}).filter(([k]) => isReplayableHeader(k)));
}

function isReplayableHeader(name) {
  const key = String(name || '').toLowerCase();
  return ![
    'host',
    'content-length',
    'connection',
    'keep-alive',
    'proxy-authenticate',
    'proxy-authorization',
    'proxy-connection',
    'te',
    'trailer',
    'transfer-encoding',
    'upgrade',
  ].includes(key) && !key.startsWith('x-oproxy-');
}

function sessionToComposeRequest(s) {
  const bucket = s.appBucket || appBucketOf(s.appProtocol, s);
  if (bucket === 'tunnel') return null;
  const kind = bucket === 'ws' ? 'websocket' : bucket === 'grpc' ? 'grpc' : 'http';
  const headers = s.reqHeadersRaw ?? s.reqHeaders ?? {};
  return {
    importId: `${s.id}_${Date.now()}`,
    name: `${s.method} ${s.host}${s.path || '/'}`,
    kind,
    method: kind === 'grpc' ? 'POST' : s.method,
    url: s.url,
    headers: headerItems(headers),
    params: [],
    body: s.reqBodyRaw ?? s.reqBody ?? '',
    bodyMode: 'raw',
    contentType: headers['content-type'] || headers['Content-Type'] || (kind === 'grpc' ? 'application/grpc+proto' : 'application/json'),
    wsFrames: kind === 'websocket' ? wsFramesForCompose(s) : undefined,
  };
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
}

function buildCurlFromSession(s) {
  const bucket = s.appBucket || appBucketOf(s.appProtocol, s);
  if (bucket === 'ws') return buildWebsocketCommandFromSession(s, false);
  if (bucket === 'tunnel') return '';
  const parts = ['curl'];
  if (bucket === 'grpc') parts.push('--http2');
  const method = bucket === 'grpc' ? 'POST' : s.method;
  if (method && method !== 'GET') parts.push('-X', shellQuote(method));
  Object.entries(s.reqHeaders || {})
    .filter(([k]) => isReplayableHeader(k))
    .forEach(([k, v]) => parts.push('-H', shellQuote(`${k}: ${v}`)));
  if (bucket === 'grpc' && !Object.keys(s.reqHeaders || {}).some(k => k.toLowerCase() === 'content-type')) {
    parts.push('-H', shellQuote('Content-Type: application/grpc+proto'));
  }
  if (s.reqBody !== undefined && s.reqBody !== null && s.reqBody !== '') {
    parts.push('--data-raw', shellQuote(s.reqBody));
  }
  parts.push(shellQuote(s.url));
  return parts.join(' ');
}

function buildRawCurlFromSession(s) {
  const bucket = s.appBucket || appBucketOf(s.appProtocol, s);
  if (bucket === 'ws') return buildWebsocketCommandFromSession(s, true);
  if (bucket === 'tunnel') return '';
  return buildCurlFromSession({
    ...s,
    reqHeaders: s.reqHeadersRaw ?? s.reqHeaders,
    reqBody: s.reqBodyRaw ?? s.reqBody,
  });
}

function buildWebsocketCommandFromSession(s, raw = false) {
  const headers = raw ? (s.reqHeadersRaw ?? s.reqHeaders) : s.reqHeaders;
  const parts = ['websocat'];
  Object.entries(headers || {})
    .filter(([k]) => isReplayableHeader(k))
    .forEach(([k, v]) => parts.push('-H', shellQuote(`${k}: ${v}`)));
  parts.push(shellQuote(s.url));
  return parts.join(' ');
}

function isClientWsFrame(frame) {
  const dir = protocolToken(frame?.direction);
  return dir === 'client' || dir === 'request' || dir === 'clienttoserver' || dir === 'outbound';
}

function wsFramePayload(frame) {
  return frame?.payload ?? frame?.payload_text ?? frame?.text ?? '';
}

function wsOpcodeLabel(opcode) {
  if (opcode === 1 || opcode === '1') return 'text';
  if (opcode === 2 || opcode === '2') return 'binary';
  if (opcode === 8 || opcode === '8') return 'close';
  if (opcode === 9 || opcode === '9') return 'ping';
  if (opcode === 10 || opcode === '10') return 'pong';
  return opcode || 'text';
}

function wsFramesForCompose(s) {
  const frames = (s.wsFrames || []).filter(isClientWsFrame);
  return (frames.length ? frames : [{ opcode: 'text', payload: '' }]).map((frame, idx) => ({
    id: `wf_${Date.now()}_${idx}`,
    on: true,
    opcode: wsOpcodeLabel(frame.opcode),
    payload: wsFramePayload(frame),
  }));
}

// Client-side sort that mirrors the backend sort_sessions logic (api/mod.rs).
// Used after incremental merges so new sessions land in the correct display position
// without waiting for a full reload.
function clientSortSessions(sessions, sort) {
  const { key, dir } = sort;
  const cmp = (a, b) => {
    switch (key) {
      case 'idx': case 'ts': return a.ts - b.ts;
      case 'method': return a.method.localeCompare(b.method);
      case 'status': return a.status - b.status;
      case 'host':   return a.host.localeCompare(b.host);
      case 'path':   return a.path.localeCompare(b.path);
      case 'type':   return a.type.localeCompare(b.type);
      case 'protocol': return a.proto.localeCompare(b.proto);
      case 'reqSize': return (a.reqSize || 0) - (b.reqSize || 0);
      case 'total':  return a.total - b.total;
      default:       return a.ts - b.ts;
    }
  };
  sessions.sort((a, b) => {
    const result = cmp(a, b);
    return dir === 'desc' ? -result : result;
  });
}

function copyText(text) {
  if (navigator.clipboard?.writeText) navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
  else fallbackCopy(text);
}

function fallbackCopy(text) {
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  document.execCommand('copy');
  ta.remove();
}

async function downloadHar(ids = null, filename = 'oproxy-session.har') {
  const params = new URLSearchParams();
  if (ids?.length) params.set('ids', ids.join(','));
  const suffix = params.toString() ? `?${params}` : '';
  const res = await fetch(`/admin/sessions/export/har${suffix}`);
  if (!res.ok) throw new Error(await res.text());
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function clientProxyAddress(cfg) {
  if (!cfg) return '—';
  if (window.location?.hostname) {
    const port = window.location.port || (window.location.protocol === 'https:' ? '443' : '80');
    return `${window.location.hostname}:${port}`;
  }
  return `127.0.0.1:${cfg.port || 8080}`;
}

function showDownloadError(err) {
  const message = err?.message || String(err);
  if (window.notifyError) window.notifyError(message);
  else window.alert?.(`Export failed: ${message}`);
}

function showToast(message, error = false) {
  const el = document.createElement('div');
  el.className = 'ui-toast' + (error ? ' error' : '');
  el.textContent = String(message || '');
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 4200);
}

async function importSessionsFile(file, merge = true) {
  const text = await file.text();
  const parsed = JSON.parse(text);
  const isHar = !!parsed?.log?.entries;
  const url = isHar
    ? `/admin/sessions/import/har?merge=${merge ? 'true' : 'false'}`
    : '/admin/sessions/import';
  const body = isHar
    ? parsed
    : {
        sessions: Array.isArray(parsed) ? parsed : parsed?.sessions,
        merge,
      };
  if (!isHar && !Array.isArray(body.sessions)) {
    throw new Error('expected a HAR file, a JSON session array, or {"sessions": [...]}');
  }
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

async function loadRuntimePart(label, url, parse) {
  try {
    const res = await fetch(url);
    if (res.status === 401) {
      // Admin token is required but not present — redirect to the login page.
      // Preserve the current URL so the user lands back here after signing in.
      const next = encodeURIComponent(window.location.pathname + window.location.search);
      window.location.href = `/login?next=${next}`;
      return { label, value: null, error: null };
    }
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const value = parse ? await parse(res) : await res.json();
    return { label, value, error: null };
  } catch (err) {
    return { label, value: null, error: err?.message || 'unavailable' };
  }
}

// Parse search query into structured terms, mirroring Rust parse_search_query.
// Supported prefixes: tag:, host:, method:, status:  — else plain text substring.
function parseSearch(query) {
  return query.trim().split(/\s+/).filter(Boolean).map(token => {
    const lower = token.toLowerCase();
    if (lower.startsWith('tag:'))    return { kind: 'tag',    val: lower.slice(4) };
    if (lower.startsWith('host:'))   return { kind: 'host',   val: lower.slice(5) };
    if (lower.startsWith('method:')) return { kind: 'method', val: lower.slice(7) };
    if (lower.startsWith('status:')) {
      const n = parseInt(lower.slice(7), 10);
      return isNaN(n) ? { kind: 'text', val: lower.slice(7) } : { kind: 'status', val: n };
    }
    return { kind: 'text', val: lower };
  });
}

function sessionMatchesTerms(s, terms) {
  return terms.every(({ kind, val }) => {
    switch (kind) {
      case 'tag':    return s.tags.some(t => t.toLowerCase().includes(val));
      case 'host':   return s.host.toLowerCase().includes(val);
      case 'method': return s.method.toLowerCase() === val;
      case 'status': return s.status === val;
      case 'text':
      default: {
        const hay = (s.url + ' ' + s.method + ' ' + s.host + ' ' + s.type + ' ' + s.tags.join(' ')).toLowerCase();
        return hay.includes(val);
      }
    }
  });
}

function normalizeWorkspaceMethods(methods) {
  const allowed = new Set(METHODS);
  const picked = new Set((methods || []).map(m => String(m).toUpperCase()).filter(m => allowed.has(m)));
  return METHODS.filter(m => picked.has(m));
}

function normalizeWorkspaceStatusBuckets(buckets) {
  const allowed = new Set(STATUS_BUCKETS);
  const picked = new Set((buckets || []).map(b => String(b)).filter(b => allowed.has(b)));
  return STATUS_BUCKETS.filter(b => picked.has(b));
}

function activeSelection(values, allValues) {
  return values.length > 0 && values.length < allValues.length ? values : [];
}

function normalizeWorkspaceSort(sort) {
  // 'idx' is no longer a user-visible sort (SEQ column removed). Treat it as 'ts'.
  const rawKey = SESSION_SORT_KEYS.has(sort?.key) ? sort.key : 'ts';
  const key = rawKey === 'idx' ? 'ts' : rawKey;
  const dir = sort?.dir === 'asc' ? 'asc' : 'desc';
  return { key, dir };
}

function workspaceViewSnapshot({ activeRail, search, regexMode, methodFilter, statusFilter, wireFilter, appFilter, hostFocus, hostFilter, sort, viewMode, selectedId }) {
  return {
    active_surface: WORKSPACE_SURFACES.has(activeRail) ? activeRail : 'sessions',
    sessions_view: {
      query: search || '',
      regex: !!regexMode,
      methods: methodFilter.size === 0 ? METHODS : METHODS.filter(m => methodFilter.has(m)),
      status_buckets: statusFilter.size === 0 ? STATUS_BUCKETS : STATUS_BUCKETS.filter(b => statusFilter.has(b)),
      wire_filter: wireFilter ? [...wireFilter] : [],
      app_filter: appFilter ? [...appFilter] : [],
      host_focus: Array.isArray(hostFocus) ? hostFocus : [],
      host_filter: hostFilter || null,
      sort: normalizeWorkspaceSort(sort),
      view_mode: viewMode === 'structure' ? 'structure' : 'sequence',
      selected_session_id: selectedId || null,
    },
  };
}

function workspaceSnapshotFromState(workspace) {
  const view = workspace?.sessions_view || {};
  const methods = normalizeWorkspaceMethods(view.methods);
  const statusBuckets = normalizeWorkspaceStatusBuckets(view.status_buckets);
  return {
    active_surface: WORKSPACE_SURFACES.has(workspace?.active_surface) ? workspace.active_surface : 'sessions',
    sessions_view: {
      query: view.query || '',
      regex: !!view.regex,
      methods: activeSelection(methods, METHODS),
      status_buckets: activeSelection(statusBuckets, STATUS_BUCKETS),
      wire_filter: Array.isArray(view.wire_filter) ? view.wire_filter : [],
      app_filter: Array.isArray(view.app_filter) ? view.app_filter : [],
      host_focus: Array.isArray(view.host_focus) ? view.host_focus : [],
      host_filter: view.host_filter || null,
      sort: normalizeWorkspaceSort(view.sort),
      view_mode: view.view_mode === 'structure' ? 'structure' : 'sequence',
      selected_session_id: view.selected_session_id || null,
    },
  };
}

function App() {
  const [t, setTweak] = useTweaks(TWEAK_DEFAULTS);

  // Apply theme + density + accent at root
  React.useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = t.theme;
    root.dataset.density = t.density;
    root.style.setProperty('--accent-h', String(t.accentHue));
  }, [t.theme, t.density, t.accentHue]);

  const [sessions, setSessions] = React.useState([]);
  const [selectedId, setSelectedId] = React.useState(null);
  const [search, setSearch] = React.useState('');
  const [methodFilter, setMethodFilter] = React.useState(new Set());
  const [statusFilter, setStatusFilter] = React.useState(new Set());
  const [wireFilter, setWireFilter] = React.useState(new Set());
  const [appFilter, setAppFilter] = React.useState(new Set());
  const [hostFilter, setHostFilter] = React.useState(null);
  const [hostFocus, setHostFocus] = React.useState([]); // pinned hosts shown as chips
  const [liveRefresh, setLiveRefresh] = React.useState(true);
  const [sseConnected, setSseConnected] = React.useState(false);
  // proxyDown becomes true ~4 s after SSE goes offline, to avoid flicker on startup/reconnect.
  const [proxyDown, setProxyDown] = React.useState(false);
  const [sort, setSort] = React.useState({ key: 'ts', dir: 'desc' });
  const [activeRail, setActiveRail] = React.useState('sessions');
  const [rulesTab, setRulesTab] = React.useState(null);
  const [assistantOpen, setAssistantOpen] = React.useState(false);
  const [regexMode, setRegexMode] = React.useState(false);
  const [showShortcuts, setShowShortcuts] = React.useState(false);
  const [tinyViewport, setTinyViewport] = React.useState(false);
  const [viewMode, setViewMode] = React.useState('sequence'); // sequence | structure
  const [bulkSel, setBulkSel] = React.useState(new Set());
  const [composeRequest, setComposeRequest] = React.useState(null);
  const [createFromTarget, setCreateFromTarget] = React.useState(null); // { action, prefill }
  const [runtime, setRuntime] = React.useState({ config: null, throttle: null, socks5: null, caBytes: 0, breakpointHeld: 0, errors: {} });
  const [sessionsError, setSessionsError] = React.useState(null);
  const [sessionMeta, setSessionMeta] = React.useState({ total: 0, filtered_total: 0, facets: null });
  const [renderLimit, setRenderLimit] = React.useState(SESSION_RENDER_PAGE_SIZE);
  const [detailById, setDetailById] = React.useState({});
  const [workspaceHydrated, setWorkspaceHydrated] = React.useState(false);
  const mainRef = React.useRef(null);
  const [splitSize, setSplitSize] = React.useState({ detailW: 560, detailH: 360 });
  const lastFetchRef = React.useRef(null);    // Date of last full or incremental session fetch
  const sessionsRef = React.useRef([]);        // always-current sessions array (no stale closure)
  const selectedIdRef = React.useRef(null);    // always-current selectedId
  const sortRef = React.useRef(sort);          // always-current sort (used inside fetchIncremental callback)
  const workspaceVersionRef = React.useRef(null);
  const workspaceSnapshotRef = React.useRef(null);
  const [detailVersion, setDetailVersion] = React.useState(0); // bumped to force detail re-fetch
  // Keep refs in sync on every render (standard "latest-value ref" pattern).
  sessionsRef.current = sessions;
  sortRef.current = sort;
  selectedIdRef.current = selectedId;

  const applyWorkspaceState = React.useCallback((workspace) => {
    const snapshot = workspaceSnapshotFromState(workspace);
    workspaceVersionRef.current = Number.isFinite(workspace?.version) ? workspace.version : null;
    // Store the snapshot ref using the SERVER's raw sort key so that any client-side
    // key migration (e.g. idx→ts) produces a diff and triggers a workspace PATCH to
    // persist the migrated value back to the server.
    const rawSortKey = workspace?.sessions_view?.sort?.key;
    const serializedSessionsView = {
      ...snapshot.sessions_view,
      methods: snapshot.sessions_view.methods.length === 0 ? METHODS : snapshot.sessions_view.methods,
      status_buckets: snapshot.sessions_view.status_buckets.length === 0 ? STATUS_BUCKETS : snapshot.sessions_view.status_buckets,
    };
    const snapshotForRef = {
      ...snapshot,
      sessions_view: {
        ...serializedSessionsView,
        sort: rawSortKey && rawSortKey !== snapshot.sessions_view.sort.key
          ? { key: rawSortKey, dir: workspace.sessions_view.sort.dir }
          : snapshot.sessions_view.sort,
      },
    };
    workspaceSnapshotRef.current = JSON.stringify(snapshotForRef);
    setActiveRail(snapshot.active_surface);
    const rulesTabHint = workspace?.feature_views?.rules?.tab;
    if (rulesTabHint) setRulesTab(rulesTabHint);
    setSearch(snapshot.sessions_view.query);
    setRegexMode(snapshot.sessions_view.regex);
    setMethodFilter(new Set(snapshot.sessions_view.methods));
    setStatusFilter(new Set(snapshot.sessions_view.status_buckets));
    setHostFocus(snapshot.sessions_view.host_focus);
    setHostFilter(snapshot.sessions_view.host_filter);
    if (Array.isArray(snapshot.sessions_view.wire_filter)) setWireFilter(new Set(snapshot.sessions_view.wire_filter));
    if (Array.isArray(snapshot.sessions_view.app_filter)) setAppFilter(new Set(snapshot.sessions_view.app_filter));
    setSort(snapshot.sessions_view.sort);
    setViewMode(snapshot.sessions_view.view_mode);
    setSelectedId(snapshot.sessions_view.selected_session_id);
    setWorkspaceHydrated(true);
  }, []);

  const loadWorkspace = React.useCallback(async () => {
    try {
      const res = await fetch('/admin/workspace');
      if (res.status === 401) {
        const next = encodeURIComponent(window.location.pathname + window.location.search);
        window.location.href = `/login?next=${next}`;
        return;
      }
      if (!res.ok) throw new Error(await res.text());
      const data = await res.json();
      applyWorkspaceState(data.workspace);
    } catch (err) {
      console.warn('Failed to load workspace state', err);
      setWorkspaceHydrated(true);
    }
  }, [applyWorkspaceState]);

  React.useEffect(() => {
    loadWorkspace();
  }, [loadWorkspace]);

  const loadSessions = React.useCallback(async () => {
    try {
      const params = new URLSearchParams({ workspace_view: 'current', limit: String(SESSION_LIST_LIMIT) });
      const res = await fetch(`/api/sessions?${params}`);
      if (!res.ok) throw new Error(await res.text());
      const data = await res.json();
      const live = (data.sessions || []).map((s, i) => adaptExchange(s, i));
      lastFetchRef.current = new Date();
      setSessionsError(null);
      setSessionMeta({
        total: data.total ?? live.length,
        filtered_total: data.filtered_total ?? live.length,
        facets: data.facets || null,
      });
      setSessions(live);
      setSelectedId(prev => prev && live.some(s => s.id === prev) ? prev : live[0]?.id || null);
    } catch (err) {
      console.warn('Failed to load live sessions', err);
      setSessionsError(err);
      setSessionMeta({ total: 0, filtered_total: 0, facets: null });
      setSessions([]);
      setSelectedId(null);
    }
  }, []);

  const fetchIncremental = React.useCallback(async () => {
    if (!lastFetchRef.current) { loadSessions(); return; }
    const since = new Date(lastFetchRef.current.getTime() - 2000);
    try {
      const params = new URLSearchParams({ workspace_view: 'current', since: since.toISOString(), limit: String(SESSION_LIST_LIMIT) });
      const res = await fetch(`/api/sessions?${params}`);
      if (!res.ok) throw new Error(await res.text());
      const data = await res.json();
      lastFetchRef.current = new Date();
      const updated = data.sessions || [];
      if (updated.length === 0) return;
      setSessions(prev => {
        const idxMap = new Map(prev.map((s, i) => [s.id, i]));
        const next = [...prev];
        let hadNew = false;
        for (const ex of updated) {
          const existingIdx = idxMap.get(ex.id) ?? -1;
          const adapted = adaptExchange(ex, existingIdx >= 0 ? existingIdx : next.length);
          if (existingIdx >= 0) {
            next[existingIdx] = adapted;
          } else {
            if (next.length >= SESSION_LIST_LIMIT) next.shift();
            next.push(adapted);
            hadNew = true;
          }
        }
        // Re-sort after any insertion so new sessions appear in the correct position
        // (e.g. newest-first with ts desc) without requiring a full reload.
        if (hadNew) clientSortSessions(next, sortRef.current);
        return next;
      });
      // Invalidate detail cache for updated sessions so the panel gets fresh data.
      setDetailById(prev => {
        const next = { ...prev };
        for (const ex of updated) delete next[ex.id];
        return next;
      });
      // If the currently-selected session was updated, bump version to re-fetch its detail.
      if (updated.some(ex => ex.id === selectedIdRef.current)) {
        setDetailVersion(v => v + 1);
      }
    } catch (err) {
      console.warn('Incremental fetch failed, falling back to full reload', err);
      loadSessions();
    }
  }, [loadSessions]);

  React.useEffect(() => {
    loadSessions();
  }, [loadSessions]);

  React.useEffect(() => {
    if (!workspaceHydrated) return undefined;
    const patch = workspaceViewSnapshot({
      activeRail,
      search,
      regexMode,
      methodFilter,
      statusFilter,
      wireFilter,
      appFilter,
      hostFocus,
      hostFilter,
      sort,
      viewMode,
      selectedId,
    });
    const signature = JSON.stringify(patch);
    if (signature === workspaceSnapshotRef.current) return undefined;

    const timer = setTimeout(async () => {
      try {
        const body = {
          base_version: workspaceVersionRef.current,
          patch,
        };
        if (body.base_version == null) delete body.base_version;
        const res = await fetch('/admin/workspace', {
          method: 'PATCH',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(body),
        });
        if (res.status === 409) {
          await loadWorkspace();
          return;
        }
        if (!res.ok) throw new Error(await res.text());
        const data = await res.json();
        if (data.workspace) {
          applyWorkspaceState(data.workspace);
          loadSessions();
        } else {
          workspaceSnapshotRef.current = signature;
        }
      } catch (err) {
        console.warn('Failed to patch workspace state', err);
      }
    }, 250);

    return () => clearTimeout(timer);
  }, [workspaceHydrated, activeRail, search, regexMode, methodFilter, statusFilter, wireFilter, appFilter, hostFocus, hostFilter, sort, viewMode, selectedId, loadWorkspace, loadSessions, applyWorkspaceState]);

  React.useEffect(() => {
    if (!selectedId) return;
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch(`/api/sessions/${encodeURIComponent(selectedId)}`);
        if (!res.ok) throw new Error(await res.text());
        const data = await res.json();
        const summary = sessionsRef.current.find(s => s.id === selectedId);
        const detail = adaptExchange(data.exchange, Math.max(0, (summary?.idx || 1) - 1));
        if (!cancelled) {
          setDetailById(prev => ({
            ...prev,
            [selectedId]: { ...detail, idx: summary?.idx || detail.idx },
          }));
        }
      } catch (err) {
        console.warn('Failed to load session detail', err);
      }
    })();
    return () => { cancelled = true; };
  }, [selectedId, detailVersion]);

  const loadRuntime = React.useCallback(async () => {
    const [config, throttle, socks5, caText, pendingBreakpoints] = await Promise.all([
      loadRuntimePart('config', '/admin/config'),
      loadRuntimePart('throttling', '/admin/throttling'),
      loadRuntimePart('socks5', '/admin/socks5/status'),
      loadRuntimePart('ca', '/admin/ca', res => res.text()),
      loadRuntimePart('breakpoints_pending', '/admin/breakpoints/pending'),
    ]);
    setRuntime({
      config: config.value,
      throttle: throttle.value,
      socks5: socks5.value,
      caBytes: caText.value?.length || 0,
      breakpointHeld: Array.isArray(pendingBreakpoints.value) ? pendingBreakpoints.value.length : 0,
      errors: Object.fromEntries(
        [config, throttle, socks5, caText, pendingBreakpoints]
          .filter(part => part.error)
          .map(part => [part.label, part.error]),
      ),
    });
  }, []);

  React.useEffect(() => {
    loadRuntime();
    const id = setInterval(loadRuntime, 5000);
    return () => clearInterval(id);
  }, [loadRuntime]);

  React.useEffect(() => {
    if (!liveRefresh) return;
    let es = null;
    let debounceTimer = null;
    let watchdogTimer = null;

    const scheduleIncremental = () => {
      clearTimeout(debounceTimer);
      debounceTimer = setTimeout(fetchIncremental, 150);
    };

    function connect() {
      if (es) { es.close(); es = null; }
      setSseConnected(false);
      es = new EventSource('/api/sessions/stream');
      clearTimeout(watchdogTimer);
      // Server sends keep-alive pings every 15 s; reconnect if silent for 35 s.
      watchdogTimer = setTimeout(() => { setSseConnected(false); connect(); }, 35000);

      es.onopen = () => setSseConnected(true);

      es.onmessage = (e) => {
        setSseConnected(true);
        clearTimeout(watchdogTimer);
        watchdogTimer = setTimeout(() => { setSseConnected(false); connect(); }, 35000);
        try {
          const ev = JSON.parse(e.data);
          if (ev.kind === 'sessions_cleared') {
            clearTimeout(debounceTimer);
            setSessions([]);
            setDetailById({});
            setSelectedId(null);
            lastFetchRef.current = new Date();
          } else if (ev.kind === 'sessions_imported' || ev.kind === 'reload') {
            clearTimeout(debounceTimer);
            loadSessions();
          } else {
            scheduleIncremental();
          }
        } catch {
          scheduleIncremental();
        }
      };

      es.onerror = () => {
        setSseConnected(false);
        clearTimeout(watchdogTimer);
        if (es) { es.close(); es = null; }
        watchdogTimer = setTimeout(connect, 3000);
      };
    }

    connect();

    return () => {
      clearTimeout(debounceTimer);
      clearTimeout(watchdogTimer);
      if (es) { es.close(); es = null; }
    };
  }, [liveRefresh, fetchIncremental, loadSessions]);

  // Delay showing the proxy-down banner to avoid flicker during startup / brief reconnect.
  React.useEffect(() => {
    if (sseConnected || !liveRefresh) {
      setProxyDown(false);
      return;
    }
    const t = setTimeout(() => setProxyDown(true), 4000);
    return () => clearTimeout(t);
  }, [sseConnected, liveRefresh]);

  const toggleFacet = (value, setter) => {
    setter(prev => {
      if (prev.size === 0) return new Set([value]);
      const next = new Set(prev);
      next.has(value) ? next.delete(value) : next.add(value);
      return next;
    });
  };
  const toggleMethod = (m) => toggleFacet(m, setMethodFilter);
  const toggleStatus = (s) => toggleFacet(s, setStatusFilter);
  const toggleWire = (p) => toggleFacet(p, setWireFilter);
  const toggleApp = (p) => toggleFacet(p, setAppFilter);
  const DEFAULT_SORT = { key: 'ts', dir: 'desc' };
  const onSort = (key) => setSort(prev => {
    if (prev.key !== key) {
      // Switching to a different column: start in the most useful direction.
      // For timestamp (the default key) that is desc (newest first); for
      // every other column ascending is the natural starting direction.
      return { key, dir: key === DEFAULT_SORT.key ? DEFAULT_SORT.dir : 'asc' };
    }
    if (prev.dir === 'asc') return { key, dir: 'desc' };
    // desc → reset. For the default column wrap back to asc so the user can
    // cycle; for any other column reset to the default (ts desc).
    if (key === DEFAULT_SORT.key) return { key, dir: 'asc' };
    return { ...DEFAULT_SORT };
  });

  // host counts (for filter chip)
  const hostCounts = React.useMemo(() => {
    const m = new Map();
    sessions.forEach(s => m.set(s.host, (m.get(s.host) || 0) + 1));
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  }, [sessions]);

  // Backend owns search/method/status filtering and sorting; wire/app facets
  // are lightweight client-side refinements over the rows already fetched.
  const filtered = React.useMemo(() => {
    return sessions.filter(s =>
      (wireFilter.size === 0 || wireFilter.has(s.wireBucket || wireBucketOf(s.wireProtocol, s)))
      && (appFilter.size === 0 || appFilter.has(s.appBucket || appBucketOf(s.appProtocol, s)))
    );
  }, [sessions, wireFilter, appFilter]);

  // Value-based keys, not the Set objects themselves — those get rebuilt with
  // new references on every workspace sync even when unchanged, which used to
  // spuriously reset pagination below.
  const methodFilterKey = [...methodFilter].sort().join(',');
  const statusFilterKey = [...statusFilter].sort().join(',');
  const wireFilterKey = [...wireFilter].sort().join(',');
  const appFilterKey = [...appFilter].sort().join(',');
  const sortKey = `${sort.key}:${sort.dir}`;

  React.useEffect(() => {
    setRenderLimit(SESSION_RENDER_PAGE_SIZE);
  }, [search, methodFilterKey, statusFilterKey, wireFilterKey, appFilterKey, hostFilter, hostFocus, sortKey, regexMode, viewMode]);

  const renderedSessions = React.useMemo(
    () => filtered.slice(0, renderLimit),
    [filtered, renderLimit],
  );
  const hiddenSessionCount = Math.max(0, filtered.length - renderedSessions.length);

  const selected = selectedId ? (detailById[selectedId] || sessions.find(s => s.id === selectedId)) : null;
  const hasActiveFilter =
    search.trim().length > 0 ||
    !!hostFilter ||
    (Array.isArray(hostFocus) ? hostFocus.length > 0 : !!hostFocus) ||
    methodFilter.size > 0 ||
    statusFilter.size > 0 ||
    wireFilter.size > 0 ||
    appFilter.size > 0;
  const emptyState = sessionsError
    ? {
        title: 'Session API unavailable.',
        hint: 'Check that oproxy is running, then reload this page.',
      }
    : sessions.length === 0 && hasActiveFilter
      ? {
          title: 'No sessions match the current filters.',
          hint: 'Try clearing search or method filters.',
        }
      : sessions.length === 0
        ? {
            title: 'No sessions captured yet.',
            hint: 'Send traffic through oproxy to populate this table.',
          }
        : {
            title: 'No sessions match the current filters.',
            hint: 'Try clearing search or method filters.',
          };

  const RAIL_ORDER_LOCAL = ['sessions','compose','rules','breakpoints','mock','lua','inspector','dns','capture','webhooks','ca','settings'];

  // keyboard nav
  React.useEffect(() => {
    const onKey = (e) => {
      const isField = e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.isContentEditable;
      const mod = e.metaKey || e.ctrlKey;

      if (mod && /^[1-9]$/.test(e.key)) {
        const target = RAIL_ORDER_LOCAL[+e.key - 1];
        if (target) { e.preventDefault(); setActiveRail(target); return; }
      }
      if (mod && e.key.toLowerCase() === 'd') {
        e.preventDefault();
        setTweak('theme', t.theme === 'dark' ? 'light' : 'dark');
        return;
      }
      if (mod && (e.key.toLowerCase() === 'k' || e.key.toLowerCase() === 'f')) {
        e.preventDefault();
        document.querySelector('.tb-search input')?.focus();
        return;
      }
      if (mod && e.key.toLowerCase() === 'b') {
        e.preventDefault();
        setActiveRail('ca');
        return;
      }
      if (mod && e.key === '/') {
        e.preventDefault();
        setRegexMode(v => !v);
        return;
      }
      if (mod && e.key.toLowerCase() === 'j') {
        e.preventDefault();
        setAssistantOpen(v => !v);
        return;
      }

      if (isField) return;
      const idx = renderedSessions.findIndex(s => s.id === selectedId);
      if (e.key === 'ArrowDown' && idx < renderedSessions.length - 1) { e.preventDefault(); setSelectedId(renderedSessions[idx + 1].id); }
      if (e.key === 'ArrowUp' && idx > 0) { e.preventDefault(); setSelectedId(renderedSessions[idx - 1].id); }
      if (e.key === 'Escape') {
        if (assistantOpen) setAssistantOpen(false);
        else if (showShortcuts) setShowShortcuts(false);
        else setSelectedId(null);
      }
      if (e.key === ' ' && activeRail === 'sessions') {
        e.preventDefault();
        setLiveRefresh(v => !v);
      }
      if (e.key === '?' && !mod) {
        setShowShortcuts(v => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [renderedSessions, selectedId, t.theme, showShortcuts, activeRail, assistantOpen]);

  // Create-from-session: listen for context menu actions from sessions table
  React.useEffect(() => {
    const handler = (e) => {
      const { action, prefill } = e.detail;
      setCreateFromTarget({ action, prefill });
      if (action === 'mock')       setActiveRail('mock');
      else if (action === 'rule')  setActiveRail('rules');
      else if (action === 'breakpoint') setActiveRail('breakpoints');
      // Clear after a tick so surfaces see the update then reset
      setTimeout(() => setCreateFromTarget(null), 500);
    };
    window.addEventListener('oproxy:create-from-session', handler);
    return () => window.removeEventListener('oproxy:create-from-session', handler);
  }, []);

  // Counts for status bar
  const counts = React.useMemo(() => {
    const c = { total: sessions.length, ok: 0, redirect: 0, client: 0, server: 0, paused: 0, pending: 0, bytes: 0 };
    sessions.forEach(s => {
      const b = statusBucket(s.status);
      if (b === '2') c.ok++;
      else if (b === '3') c.redirect++;
      else if (b === '4') c.client++;
      else if (b === '5') c.server++;
      else if (s.paused) c.paused++;
      else if (s.pending) c.pending++;
      c.bytes += (s.reqSize || 0) + (s.resSize || 0);
    });
    return c;
  }, [sessions]);
  const displayCounts = React.useMemo(() => ({
    ...counts,
    total: sessionMeta.total ?? counts.total,
  }), [counts, sessionMeta.total]);

  const resume = async (id) => {
    const targetId = id || selectedId;
    if (!targetId) return;
    // Optimistically clear paused state; server response will confirm or update.
    setSessions(prev => prev.map(s => s.id === targetId ? {
      ...s, paused: false, tags: s.tags ? s.tags.filter(t => t !== 'bp') : [],
    } : s));
    try {
      await fetch(`/admin/breakpoints/pending/${encodeURIComponent(targetId)}/resolve`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ action: 'resume' }),
      });
    } catch {}
  };
  const abort = async (id) => {
    const targetId = id || selectedId;
    if (!targetId) return;
    // Optimistically clear paused state; server response will confirm or update.
    setSessions(prev => prev.map(s => s.id === targetId ? {
      ...s, paused: false, tags: s.tags ? s.tags.filter(t => t !== 'bp') : [],
    } : s));
    try {
      await fetch(`/admin/breakpoints/pending/${encodeURIComponent(targetId)}/resolve`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ action: 'abort' }),
      });
    } catch {}
  };

  const replaySession = async (s) => {
    if (!s) return;
    let source = s;
    try {
      const res = await fetch(`/api/sessions/${encodeURIComponent(s.id)}`);
      if (res.ok) {
        const detail = await res.json();
        if (detail.exchange) source = adaptExchange(detail.exchange, Math.max(0, (s.idx || 1) - 1));
      }
    } catch {}
    const bucket = source.appBucket || appBucketOf(source.appProtocol, source);
    if (bucket === 'tunnel') {
      showToast('Tunnel sessions are metadata-only and cannot be replayed from the session page.', true);
      return;
    }
    if (bucket === 'ws') {
      await fetch('/admin/forward/websocket', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          url: source.url,
          headers: replayableHeaders(source.reqHeadersRaw ?? source.reqHeaders),
          frames: wsFramesForCompose(source).filter(f => f.payload !== '').map(f => ({
            opcode: f.opcode || 'text',
            payload: f.payload || '',
          })),
          note: `Replay of ${s.id}`,
          tags: ['replay'],
        }),
      }).catch(() => {});
      await loadSessions();
      return;
    }
    const wireBucket = source.wireBucket || wireBucketOf(source.wireProtocol, source);
    const replayKind = bucket === 'grpc' ? 'grpc'
      : wireBucket === 'h3' ? 'http3'
      : wireBucket === 'h2' ? 'http2'
      : 'http';
    await fetch('/admin/forward', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        kind: replayKind,
        method: bucket === 'grpc' ? 'POST' : source.method,
        url: source.url,
        headers: replayableHeaders(source.reqHeadersRaw ?? source.reqHeaders),
        body: source.reqBodyRaw ?? source.reqBody ?? null,
        note: `Replay of ${s.id}`,
        tags: ['replay'],
      }),
    }).catch(() => {});
    await loadSessions();
  };

  const openSessionInCompose = (s) => {
    if (!s) return;
    const request = sessionToComposeRequest(s);
    if (!request) {
      showToast('Tunnel sessions are metadata-only and cannot be opened in Compose.', true);
      return;
    }
    setComposeRequest(request);
    setActiveRail('compose');
  };
  const handleImportFile = async (file) => {
    if (!file) return;
    try {
      const result = await importSessionsFile(file, true);
      await loadSessions();
      setActiveRail('sessions');
      showToast(`Imported ${result.imported || 0} session${result.imported === 1 ? '' : 's'}`);
    } catch (err) {
      showToast(`Import failed: ${err?.message || err}`, true);
    }
  };

  const selectedSessions = () => sessions.filter(s => bulkSel.has(s.id));
  const replaySelected = async () => {
    for (const s of selectedSessions()) await replaySession(s);
    setBulkSel(new Set());
  };
  const startSplitResize = React.useCallback((event) => {
    if (!mainRef.current || activeRail !== 'sessions') return;
    event.preventDefault();
    const rect = mainRef.current.getBoundingClientRect();
    const mode = t.split;
    document.body.classList.add('resizing-split');
    const clamp = (value, min, max) => Math.min(Math.max(value, min), Math.max(min, max));
    const onMove = (moveEvent) => {
      if (mode === 'vertical') {
        const next = clamp(rect.right - moveEvent.clientX, 360, rect.width - 420);
        setSplitSize(prev => ({ ...prev, detailW: Math.round(next) }));
      } else {
        const next = clamp(rect.bottom - moveEvent.clientY, 260, rect.height - 240);
        setSplitSize(prev => ({ ...prev, detailH: Math.round(next) }));
      }
    };
    const onUp = () => {
      document.body.classList.remove('resizing-split');
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp, { once: true });
    onMove(event);
  }, [activeRail, t.split]);

  React.useEffect(() => {
    // Defer the initial height check until the viewport settles, debounce resize
    // events, and observe `visualViewport` so transient startup and mobile
    // viewport changes do not trigger the small-window warning.
    // the banner, just a little later.
    let raf1 = 0;
    let raf2 = 0;
    let debounceTimer;
    const measure = () => setTinyViewport(window.innerHeight < 420);
    const debouncedMeasure = () => {
      clearTimeout(debounceTimer);
      debounceTimer = setTimeout(measure, 150);
    };
    raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(measure);
    });
    window.addEventListener('resize', debouncedMeasure);
    window.visualViewport?.addEventListener('resize', debouncedMeasure);
    return () => {
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
      clearTimeout(debounceTimer);
      window.removeEventListener('resize', debouncedMeasure);
      window.visualViewport?.removeEventListener('resize', debouncedMeasure);
    };
  }, []);

  return (
    <div className="app">
      <TopBar
        liveRefresh={liveRefresh} setLiveRefresh={setLiveRefresh}
        search={search} setSearch={setSearch}
        regexMode={regexMode} setRegexMode={setRegexMode}
        theme={t.theme} setTheme={(v) => setTweak('theme', v)}
        onClear={async () => {
          if (!await confirmAction('Clear all captured sessions?', 'Clear', 'danger')) return;
          await fetch('/admin/sessions', { method: 'DELETE' }).catch(() => {});
          setSessions([]);
          setDetailById({});
          setSelectedId(null);
        }}
        onShortcuts={() => setShowShortcuts(true)}
        setActiveRail={setActiveRail}
        activeRail={activeRail}
        sessions={sessions}
        onImportFile={handleImportFile}
      />

      <div className="body">
        {tinyViewport && (
          <div className="warn-strip" style={{ margin: 8, gridColumn: '1 / -1', alignSelf: 'start' }}>
            Window height is very small. Enlarge the app window and press <code>Ctrl+0</code> to reset zoom.
          </div>
        )}
        {proxyDown && (
          <div className="proxy-down-banner" style={{ gridColumn: '1 / -1', alignSelf: 'start' }}>
            <span className="dot" />
            Proxy unreachable — reconnecting…
          </div>
        )}
        <LeftRail active={activeRail} onChange={setActiveRail} />

        <div
          ref={mainRef}
          className="main"
          data-split={t.split}
          style={{
            '--detail-w': `${splitSize.detailW}px`,
            '--detail-h': `${splitSize.detailH}px`,
          }}
        >
          {activeRail === 'sessions' && (
            <>
              <div className="list-panel">
                <FilterBar
                  methodFilter={methodFilter} toggleMethod={toggleMethod}
                  statusFilter={statusFilter} toggleStatus={toggleStatus}
                  wireFilter={wireFilter} toggleWire={toggleWire}
                  appFilter={appFilter} toggleApp={toggleApp}
                  hostFilter={hostFilter} setHostFilter={setHostFilter}
                  hostFocus={hostFocus} setHostFocus={setHostFocus}
                  hostCounts={hostCounts}
                  counts={displayCounts}
                  total={sessionMeta.filtered_total ?? filtered.length}
                  viewMode={viewMode} setViewMode={setViewMode}
                  sort={sort} onResetSort={() => setSort({ key: 'ts', dir: 'desc' })}
                />
                {bulkSel.size > 0 && (
                  <div className="bulk-bar">
                    <span><b>{bulkSel.size}</b> selected</span>
                    <button
                      className="btn sm"
                      onClick={() => downloadHar(selectedSessions().map(s => s.id), 'oproxy-selected.har').catch(showDownloadError)}
                    >
                      Export HAR
                    </button>
                    <button className="btn sm" onClick={replaySelected}>Replay all</button>
                    <button className="btn sm ghost" onClick={() => setBulkSel(new Set())}>Clear</button>
                  </div>
                )}
                {viewMode === 'sequence' ? (
                  <SessionsTable
                    sessions={renderedSessions}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                    sort={sort}
                    onSort={onSort}
                    bulkSel={bulkSel}
                    emptyState={emptyState}
                    onBulkToggle={(id) => setBulkSel(prev => {
                      const n = new Set(prev);
                      n.has(id) ? n.delete(id) : n.add(id);
                      return n;
                    })}
                    onBulkToggleAll={(on) => setBulkSel(on ? new Set(renderedSessions.map(s => s.id)) : new Set())}
                  />
                ) : (
                  <StructureView
                    sessions={renderedSessions}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                    emptyState={emptyState}
                  />
                )}
                {hiddenSessionCount > 0 && (
                  <div className="page-more">
                    <span>
                      Showing {renderedSessions.length.toLocaleString()} of {filtered.length.toLocaleString()} matching sessions
                    </span>
                    <button className="btn sm" onClick={() => setRenderLimit(v => v + SESSION_RENDER_PAGE_SIZE)}>
                      Show next {Math.min(SESSION_RENDER_PAGE_SIZE, hiddenSessionCount).toLocaleString()}
                    </button>
                  </div>
                )}
              </div>
              <div
                className="divider"
                role="separator"
                aria-orientation={t.split === 'vertical' ? 'vertical' : 'horizontal'}
                title="Drag to resize request details"
                onPointerDown={startSplitResize}
                onDoubleClick={() => setSplitSize({ detailW: 560, detailH: 360 })}
              />
              <DetailPanel
                session={selected}
                onClose={() => setSelectedId(null)}
                onResume={() => resume(selectedId)}
                onAbort={() => abort(selectedId)}
                onCopyCurl={(s) => {
                  const command = buildCurlFromSession(s);
                  if (!command) {
                    showToast('SOCKS tunnel sessions do not have a replayable shell command.', true);
                    return;
                  }
                  copyText(command);
                }}
                onCopyRawCurl={async (s) => {
                  if (await confirmAction('Copy unredacted request data to the clipboard?', 'Copy')) {
                    const command = buildRawCurlFromSession(s);
                    if (!command) {
                      showToast('SOCKS tunnel sessions do not have a replayable shell command.', true);
                      return;
                    }
                    copyText(command);
                  }
                }}
                onReplay={replaySession}
                onOpenInCompose={openSessionInCompose}
              />
            </>
          )}
          {activeRail === 'dashboard' && <ProtocolDashboard />}
          {activeRail === 'connections' && <ConnectionsSurface />}
          {activeRail === 'rules' && <RulesSurface createFrom={createFromTarget?.action === 'rule' ? createFromTarget.prefill : null} initialTab={rulesTab} />}
          {activeRail === 'breakpoints' && (
            <BreakpointsSurface
              sessions={sessions}
              onResume={(id) => { setSelectedId(id); resume(id); }}
              onAbort={(id) => { setSelectedId(id); abort(id); }}
              createFrom={createFromTarget?.action === 'breakpoint' ? createFromTarget.prefill : null}
            />
          )}
          {activeRail === 'inspector' && <InspectorsSurface />}
          {activeRail === 'ca' && <CertSurface />}
          {activeRail === 'compose' && <ComposeSurface incomingRequest={composeRequest} />}
          {activeRail === 'mock' && <MockSurface createFrom={createFromTarget?.action === 'mock' ? createFromTarget.prefill : null} />}
          {activeRail === 'lua' && <LuaSurface />}
          {activeRail === 'webhooks' && <WebhooksSurface />}
          {activeRail === 'dns' && <DnsSurface />}
          {activeRail === 'capture' && <CaptureFilterSurface />}
          {activeRail === 'settings' && <SettingsSurface />}
        </div>
      </div>

      <StatusBar counts={counts} liveRefresh={liveRefresh} sseConnected={sseConnected} t={t} runtime={runtime} setActiveRail={setActiveRail} />

      <button
        className="assistant-fab"
        type="button"
        aria-label="Open assistant"
        aria-expanded={assistantOpen}
        onClick={() => setAssistantOpen(true)}
      >
        <Icon name="bolt" size={18} stroke={1.8} />
        <span>Assistant</span>
      </button>

      {assistantOpen && (
        <div className="assistant-drawer-backdrop" onMouseDown={() => setAssistantOpen(false)}>
          <div className="assistant-drawer" role="dialog" aria-label="Assistant" onMouseDown={e => e.stopPropagation()}>
            <AssistantSurface
              mode="drawer"
              onClose={() => setAssistantOpen(false)}
              onRefresh={() => loadSessions()}
              activeSurface={activeRail}
              uiState={{
                sessions_count: sessions.length,
                filtered_sessions_count: filtered.length,
                search,
                regex_mode: regexMode,
                method_filter: [...methodFilter],
                status_filter: [...statusFilter],
                wire_filter: [...wireFilter],
                app_filter: [...appFilter],
                host_focus: hostFocus,
                view_mode: viewMode,
              }}
              onWorkspaceChanged={() => {
                loadWorkspace();
                loadSessions();
              }}
            />
          </div>
        </div>
      )}

      <TweaksPanel title="Tweaks">
        <TweakSection title="Appearance">
          <TweakRadio
            label="Theme" value={t.theme}
            options={[{label: 'Dark', value: 'dark'}, {label: 'Light', value: 'light'}]}
            onChange={v => setTweak('theme', v)}
          />
          <TweakRadio
            label="Density" value={t.density}
            options={[
              {label: 'Compact', value: 'compact'},
              {label: 'Default', value: 'default'},
              {label: 'Cozy',    value: 'comfortable'},
            ]}
            onChange={v => setTweak('density', v)}
          />
        </TweakSection>
        <TweakSection title="Accent">
          <div style={{ display: 'flex', gap: 6, padding: '4px 0' }}>
            {ACCENT_OPTIONS.map(a => (
              <button key={a.h}
                onClick={() => setTweak('accentHue', a.h)}
                title={a.label}
                style={{
                  width: 32, height: 32, borderRadius: 6,
                  border: t.accentHue === a.h ? '2px solid var(--text-hi)' : '1px solid var(--border)',
                  background: `oklch(0.78 0.13 ${a.h})`,
                  cursor: 'pointer'
                }}
              />
            ))}
          </div>
        </TweakSection>
        <TweakSection title="Layout">
          <TweakRadio
            label="Split" value={t.split}
            options={[
              {label: 'Side by side', value: 'vertical'},
              {label: 'Top/Bottom',   value: 'horizontal'},
            ]}
            onChange={v => setTweak('split', v)}
          />
        </TweakSection>
      </TweaksPanel>

      {showShortcuts && <ShortcutsModal onClose={() => setShowShortcuts(false)} />}
    </div>
  );
}

/* ===== Top bar ===== */
function TopBar({ liveRefresh, setLiveRefresh, search, setSearch, regexMode, setRegexMode, theme, setTheme, onClear, onShortcuts, setActiveRail, activeRail, sessions, onImportFile }) {
  const exportHar = () => downloadHar(null, 'oproxy-session.har').catch(showDownloadError);
  const importInputRef = React.useRef(null);
  return (
    <div className="topbar">
      <div className="brand">
        <img src="/icons/icon.svg" className="brand-mark" alt="oproxy" draggable="false" />
        <div className="brand-name">oproxy <span className="dim">/ traffic</span></div>
      </div>

      <div className="tb-controls">
        <button
          className={'icon-btn' + (liveRefresh ? ' live-refresh' : '')}
          onClick={() => setLiveRefresh(v => !v)}
          title={liveRefresh ? 'Live refresh on (click to pause) · Space' : 'Live refresh paused (click to resume) · Space'}
          aria-label={liveRefresh ? 'Pause live refresh' : 'Resume live refresh'}
          aria-pressed={liveRefresh}
          style={{ position: 'relative' }}>
          {liveRefresh ? <Icon name="replay" size={14} /> : <Icon name="pause" size={14} />}
        </button>
        <button className="icon-btn" onClick={onClear} title="Clear all sessions" aria-label="Clear all sessions"><Icon name="trash" size={14} /></button>
        <div className="sep" />
        <button
          className="icon-btn"
          onClick={() => importInputRef.current?.click()}
          title="Import HAR or JSON"
          aria-label="Import HAR or JSON"
        >
          <Icon name="upload" size={14} />
        </button>
        <input
          ref={importInputRef}
          type="file"
          accept=".har,.json,application/json"
          aria-label="Import HAR or JSON file"
          style={{ display: 'none' }}
          onChange={async (event) => {
            const file = event.currentTarget.files?.[0];
            event.currentTarget.value = '';
            await onImportFile?.(file);
          }}
        />
        <button className="icon-btn" onClick={exportHar} title="Export as HAR" aria-label="Export as HAR"><Icon name="download" size={14} /></button>
      </div>

      {activeRail === 'sessions' && (
        <div className="tb-search">
          <span className="ico-left"><Icon name="search" size={14} stroke={1.6} /></span>
          <input
            aria-label={regexMode ? 'Regex filter requests' : 'Filter requests'}
            placeholder={regexMode ? 'Regex filter' : 'Filter requests by method, host, path, status, or tag'}
            value={search}
            onChange={e => setSearch(e.target.value)}
          />
          <button className={'regex-toggle' + (regexMode ? ' on' : '')}
                  onClick={() => setRegexMode(v => !v)}
                  title="Toggle regex search · ⌘/"
                  aria-label="Toggle regex search"
                  aria-pressed={regexMode}>.*</button>
          <span className="ico-right">⌘F</span>
        </div>
      )}

      <div className="tb-right">
        <button className="icon-btn" onClick={() => setActiveRail('rules')} title="Active rules · ⌘3" aria-label="Open active rules">
          <Icon name="rules" size={14} />
        </button>
        <button className="icon-btn" onClick={() => setActiveRail('breakpoints')} title="Breakpoints · ⌘4" aria-label="Open breakpoints">
          <Icon name="pauseRail" size={14} />
        </button>
        <button className="icon-btn" onClick={() => setActiveRail('ca')} title="Root CA · ⌘B" aria-label="Open Root CA">
          <Icon name="cert" size={14} />
        </button>
        <div className="sep" />
        <button className="icon-btn" onClick={onShortcuts} title="Keyboard shortcuts · ?" aria-label="Open keyboard shortcuts">
          <Icon name="layout" size={14} />
        </button>
        <button className="icon-btn" onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')} title="Toggle theme · ⌘D" aria-label="Toggle theme">
          {theme === 'dark' ? <Icon name="sun" size={14} /> : <Icon name="moon" size={14} />}
        </button>
      </div>
    </div>
  );
}

/* ===== Left rail ===== */
function LeftRail({ active, onChange }) {
  const items = [
    { key: 'sessions',    icon: 'list',       label: 'Sessions' },
    { key: 'dashboard',   icon: 'record',     label: 'Dashboard' },
    { key: 'connections', icon: 'layout',     label: 'Connections' },
    { key: 'compose',     icon: 'open',       label: 'Compose' },
    { key: 'rules',       icon: 'rules',      label: 'Rules' },
    { key: 'breakpoints', icon: 'pauseRail',  label: 'Breakpoints' },
    { key: 'mock',        icon: 'shield',     label: 'Mock Server' },
    { key: 'lua',         icon: 'bolt',       label: 'Lua Scripts' },
    { key: 'inspector',   icon: 'inspector',  label: 'Inspectors' },
    { key: 'dns',         icon: 'wifi',       label: 'DNS Override' },
    { key: 'capture',     icon: 'filter',     label: 'Capture Filter' },
    { key: 'webhooks',    icon: 'replay',     label: 'Webhooks' },
    { key: 'ca',          icon: 'cert',       label: 'Root CA' },
  ];
  return (
    <div className="rail">
      {items.map(it => (
        <button key={it.key}
                className={'rail-btn' + (active === it.key ? ' active' : '')}
                onClick={() => onChange(it.key)}
                title={it.label}
                aria-label={it.label}>
          <Icon name={it.icon} size={18} stroke={1.5} />
          <span className="label">{it.label}</span>
        </button>
      ))}
      <div className="rail-spacer" />
      <button className={'rail-btn' + (active === 'settings' ? ' active' : '')}
              onClick={() => onChange('settings')}
              title="Settings"
              aria-label="Settings">
        <Icon name="cog" size={18} stroke={1.5} />
        <span className="label">Settings</span>
      </button>
    </div>
  );
}

/* ===== Filter bar ===== */
function FilterBar({ methodFilter, toggleMethod, statusFilter, toggleStatus, wireFilter, toggleWire, appFilter, toggleApp, hostFilter, setHostFilter, hostFocus, setHostFocus, hostCounts, counts, total, viewMode, setViewMode, sort, onResetSort }) {
  const [hostMenuOpen, setHostMenuOpen] = React.useState(false);
  const [hostMenuPos, setHostMenuPos] = React.useState({ top: 0, left: 0 });
  const hostButtonRef = React.useRef(null);
  const addFocus = (h) => setHostFocus(prev => prev.includes(h) ? prev : [...prev, h]);
  const removeFocus = (h) => setHostFocus(prev => prev.filter(x => x !== h));
  const openHostMenu = (event) => {
    event.stopPropagation();
    const rect = hostButtonRef.current?.getBoundingClientRect();
    if (rect) {
      const width = Math.min(360, Math.max(260, window.innerWidth - 24));
      setHostMenuPos({
        top: Math.min(rect.bottom + 6, window.innerHeight - 320),
        left: Math.min(rect.left, window.innerWidth - width - 12),
        width,
      });
    }
    setHostMenuOpen(v => !v);
  };
  React.useEffect(() => {
    if (!hostMenuOpen) return undefined;
    const close = () => setHostMenuOpen(false);
    const onKey = (event) => {
      if (event.key === 'Escape') close();
    };
    window.addEventListener('click', close);
    window.addEventListener('resize', close);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('click', close);
      window.removeEventListener('resize', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [hostMenuOpen]);
  const sortActive = sort && !(sort.key === 'ts' && sort.dir === 'desc');
  return (
    <div className="filter-bar">
      <span className="filter-label">Method</span>
      <div className="chip-group">
        {METHODS.map(m => (
          <button key={m} className={'chip' + (methodFilter.has(m) ? ' on' : '')} onClick={() => toggleMethod(m)} aria-pressed={methodFilter.has(m)}>
            {m}
          </button>
        ))}
      </div>
      <span className="filter-label" style={{ marginLeft: 8 }}>Status</span>
      <div className="chip-group">
        <button className={'chip' + (statusFilter.has('1') ? ' on' : '')} data-tone="1xx" onClick={() => toggleStatus('1')} aria-pressed={statusFilter.has('1')}>1xx</button>
        <button className={'chip' + (statusFilter.has('2') ? ' on' : '')} data-tone="2xx" onClick={() => toggleStatus('2')} aria-pressed={statusFilter.has('2')}>2xx</button>
        <button className={'chip' + (statusFilter.has('3') ? ' on' : '')} data-tone="3xx" onClick={() => toggleStatus('3')} aria-pressed={statusFilter.has('3')}>3xx</button>
        <button className={'chip' + (statusFilter.has('4') ? ' on' : '')} data-tone="4xx" onClick={() => toggleStatus('4')} aria-pressed={statusFilter.has('4')}>4xx</button>
        <button className={'chip' + (statusFilter.has('5') ? ' on' : '')} data-tone="5xx" onClick={() => toggleStatus('5')} aria-pressed={statusFilter.has('5')}>5xx</button>
        <button className={'chip' + (statusFilter.has('-') ? ' on' : '')} onClick={() => toggleStatus('-')} aria-pressed={statusFilter.has('-')}>pending</button>
      </div>
      <span className="filter-label" style={{ marginLeft: 8 }}>Wire</span>
      <div className="chip-group">
        {PROTO_FILTERS.map(p => (
          <button
            key={p.key}
            className={'chip' + (wireFilter.has(p.key) ? ' on' : '')}
            onClick={() => toggleWire(p.key)}
            aria-pressed={wireFilter.has(p.key)}
            title={p.title}
          >
            {p.label}
          </button>
        ))}
      </div>
      <span className="filter-label" style={{ marginLeft: 8 }}>App</span>
      <div className="chip-group">
        {APP_FILTERS.map(p => (
          <button
            key={p.key}
            className={'chip' + (appFilter.has(p.key) ? ' on' : '')}
            onClick={() => toggleApp(p.key)}
            aria-pressed={appFilter.has(p.key)}
            title={p.title}
          >
            {p.label}
          </button>
        ))}
      </div>

      <div className="host-filter" style={{ marginLeft: 8 }}>
        <button ref={hostButtonRef} onClick={openHostMenu} aria-expanded={hostMenuOpen} aria-label="Open focus host menu">
          <Icon name="filter" size={11} stroke={1.8} />
          <span>focus host</span>
          {hostFocus && hostFocus.length > 0 && <span className="count">{hostFocus.length}</span>}
        </button>
        {hostMenuOpen && (
          <div
            className="menu host-menu"
            onClick={(event) => event.stopPropagation()}
            style={{ top: hostMenuPos.top, left: hostMenuPos.left, width: hostMenuPos.width }}
          >
            <div className="item" onClick={() => { setHostFocus([]); setHostFilter(null); setHostMenuOpen(false); }}>
              <span className="menu-label">Show all hosts</span><span className="shortcut">{counts.total}</span>
            </div>
            {hostCounts.length > 0 && <hr />}
            {hostCounts.length === 0 && (
              <div className="item disabled">
                <span className="menu-label">No hosts captured</span>
              </div>
            )}
            {hostCounts.map(([h, n]) => (
              <div key={h} className="item" onClick={() => { addFocus(h); setHostFilter(null); setHostMenuOpen(false); }}>
                <span className="menu-label">{h}</span><span className="shortcut">{n}</span>
              </div>
            ))}
          </div>
        )}
      </div>

      {hostFocus && hostFocus.length > 0 && (
        <div style={{ display: 'inline-flex', gap: 4, marginLeft: 4, flexWrap: 'wrap' }}>
          {hostFocus.map(h => (
            <span key={h} className="focus-chip">
              <span style={{ color: 'var(--text-faint)', marginRight: 2 }}>host:</span>{h}
              <button onClick={() => removeFocus(h)} aria-label={`Remove focus host ${h}`}>×</button>
            </span>
          ))}
        </div>
      )}

      <div className="spacer" />
      {sortActive && (
        <button onClick={onResetSort}
                title="Reset sort to chronological"
                aria-label="Reset sort to chronological"
                style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--accent)', padding: '2px 8px', borderRadius: 4 }}>
          sort: {sort.key} {sort.dir === 'asc' ? '↑' : '↓'} ✕
        </button>
      )}
      {viewMode && (
        <div className="segctl" style={{ marginRight: 8 }}>
          <button className={viewMode === 'sequence' ? 'on' : ''} onClick={() => setViewMode('sequence')}>Sequence</button>
          <button className={viewMode === 'structure' ? 'on' : ''} onClick={() => setViewMode('structure')}>Structure</button>
        </div>
      )}
      <span className="filter-label" style={{ fontFamily: 'var(--font-mono)', textTransform: 'none', letterSpacing: 0, color: 'var(--text-mid)' }}>
        {total} / {counts.total}
      </span>
    </div>
  );
}

/* ===== Status bar ===== */
function StatusBar({ counts, liveRefresh, sseConnected, t, runtime, setActiveRail }) {
  const cfg = runtime?.config;
  const throttle = runtime?.throttle;
  const runtimeErrors = Object.entries(runtime?.errors || {});
  const bind = cfg ? `${cfg.bind_host || '127.0.0.1'}:${cfg.port || 8080}` : '—';
  const clientProxy = clientProxyAddress(cfg);
  const mitm = cfg ? (cfg.mitm_enabled ? 'on' : 'off') : '—';
  const ca = runtime?.caBytes ? fmtBytes(runtime.caBytes) : 'unavailable';
  const throttleText = throttle?.enabled
    ? `${throttle.latency_ms || 0} ms · ${throttle.bandwidth_limit_kbps || '∞'} kbps`
    : 'off';
  const copyProxy = () => {
    if (clientProxy !== '—') copyText(clientProxy);
  };
  return (
    <div className="statusbar">
      <div className="group">
        <span className={'dot ' + (!liveRefresh ? 'warn' : sseConnected ? 'ok' : 'err')} />
        <span className="k">LIVE</span>
        <span className="v">{!liveRefresh ? 'paused' : sseConnected ? 'live' : 'offline'}</span>
      </div>
      <button className="group status-action" onClick={copyProxy} title={`Copy client proxy address. Listener bind: ${bind}`}>
        <span className="k">PROXY</span><span className="v">{clientProxy}</span>
      </button>
      <button className="group status-action" onClick={() => setActiveRail?.('ca')} title="Open Root CA">
        <span className="k">MITM</span><span className="v" style={{ color: mitm === 'on' ? 'var(--c-2xx)' : 'var(--text-mid)' }}>{mitm}</span>
      </button>
      <button className="group status-action" onClick={() => setActiveRail?.('ca')} title="Open Root CA">
        <span className="k">CA</span><span className="v">{ca}</span>
      </button>
      <button className="group status-action" onClick={() => setActiveRail?.('rules')} title="Open traffic rules">
        <span className="k">THROTTLE</span><span className="v">{throttleText}</span>
      </button>
      {runtimeErrors.length > 0 && (
        <button
          className="group status-action"
          onClick={() => setActiveRail?.('settings')}
          title={`Runtime API degraded: ${runtimeErrors.map(([name, err]) => `${name}: ${err}`).join('; ')}`}
        >
          <span className="k">RUNTIME</span>
          <span className="v" style={{ color: 'var(--c-4xx)' }}>degraded</span>
        </button>
      )}

      <div className="right">
        <div className="group"><span className="k">2xx</span><span className="v" style={{ color: 'var(--c-2xx)' }}>{counts.ok}</span></div>
        <div className="group"><span className="k">3xx</span><span className="v" style={{ color: 'var(--c-3xx)' }}>{counts.redirect}</span></div>
        <div className="group"><span className="k">4xx</span><span className="v" style={{ color: 'var(--c-4xx)' }}>{counts.client}</span></div>
        <div className="group"><span className="k">5xx</span><span className="v" style={{ color: 'var(--c-5xx)' }}>{counts.server}</span></div>
        <div className="group"><span className="k">HELD</span><span className="v" style={{ color: 'var(--c-paused)' }}>{runtime?.breakpointHeld || 0}</span></div>
        {counts.paused > 0 && <div className="group"><span className="k">PAUSED</span><span className="v" style={{ color: 'var(--c-paused)' }}>{counts.paused}</span></div>}
        {counts.pending > 0 && <div className="group"><span className="k">PENDING</span><span className="v" style={{ color: 'var(--text-low)' }}>{counts.pending}</span></div>}
        <div className="group"><span className="k">BYTES</span><span className="v">{fmtBytes(counts.bytes)}</span></div>
      </div>
    </div>
  );
}

window.App = App;
