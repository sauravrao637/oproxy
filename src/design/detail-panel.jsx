import React from 'react';
const { Icon, fmtBytes, fmtMs, statusBucket } = window;
/* Detail panel — tabs: Overview, Headers, Request, Response, Timing, Inspector, Cookies */

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

function syntaxJson(value, indent = 0) {
  // Recursive JSON tokenizer rendering colored spans.
  if (value === null) return <span className="null">null</span>;
  if (typeof value === 'boolean') return <span className="bool">{String(value)}</span>;
  if (typeof value === 'number') return <span className="num">{value}</span>;
  if (typeof value === 'string') return <span className="str">"{value}"</span>;
  if (Array.isArray(value)) {
    if (value.length === 0) return <span className="punct">[]</span>;
    return (
      <>
        <span className="punct">[</span>
        {value.map((v, i) => (
          <span key={i}>
            {'\n' + '  '.repeat(indent + 1)}
            {syntaxJson(v, indent + 1)}
            {i < value.length - 1 ? <span className="punct">,</span> : null}
          </span>
        ))}
        {'\n' + '  '.repeat(indent)}<span className="punct">]</span>
      </>
    );
  }
  if (typeof value === 'object') {
    const keys = Object.keys(value);
    if (keys.length === 0) return <span className="punct">{'{}'}</span>;
    return (
      <>
        <span className="punct">{'{'}</span>
        {keys.map((k, i) => (
          <span key={k}>
            {'\n' + '  '.repeat(indent + 1)}
            <span className="key">"{k}"</span>
            <span className="punct">: </span>
            {syntaxJson(value[k], indent + 1)}
            {i < keys.length - 1 ? <span className="punct">,</span> : null}
          </span>
        ))}
        {'\n' + '  '.repeat(indent)}<span className="punct">{'}'}</span>
      </>
    );
  }
  return <span>{String(value)}</span>;
}

function CodeBlock({ title, lang, content }) {
  const isObj = content && typeof content === 'object';
  const text = isObj ? JSON.stringify(content, null, 2) : (content || '');
  const lines = text.split('\n');
  const copy = () => copyText(text);
  const download = () => downloadText(text, `${title.toLowerCase().replace(/[^a-z0-9]+/g, '-') || 'body'}.txt`, lang || 'text/plain');
  return (
    <div className="code">
      <div className="code-head">
        <div className="crumbs">
          <span className="hi">{title}</span>
          <span className="mute">{lang}</span>
          <span className="mute">{lines.length} lines · {text.length} chars</span>
        </div>
        <div className="actions">
          <button className="copy-btn" onClick={copy} aria-label={`Copy ${title}`}><Icon name="copy" size={11} stroke={1.8} /> Copy</button>
          <button className="copy-btn" onClick={download} aria-label={`Download ${title}`}><Icon name="download" size={11} stroke={1.8} /></button>
        </div>
      </div>
      <div className="code-body">
        <div className="ln">
          {lines.map((_, i) => <div key={i}>{i + 1}</div>)}
        </div>
        <div className="src">
          {isObj ? syntaxJson(content) : text}
        </div>
      </div>
    </div>
  );
}

function copyText(text) {
  if (navigator.clipboard?.writeText) {
    navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
  } else {
    fallbackCopy(text);
  }
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

function downloadText(text, filename, type = 'text/plain') {
  const blob = new Blob([text], { type });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function HeaderList({ obj }) {
  const entries = Object.entries(obj || {});
  if (entries.length === 0) return <div className="mute" style={{ fontSize: 11.5, padding: '6px 0' }}>(no headers)</div>;
  return (
    <div className="kv">
      {entries.map(([k, v]) => (
        <React.Fragment key={k}>
          <div className="k">{k}</div>
          <div className="v">{String(v)}</div>
        </React.Fragment>
      ))}
    </div>
  );
}

function OverviewTab({ s }) {
  const m = (label, value, tone, unit) => (
    <div className={'metric' + (tone ? ' ' + tone : '')}>
      <div className="label">{label}</div>
      <div className="value">{value}{unit && <span className="unit">{unit}</span>}</div>
    </div>
  );
  const tone = s.status >= 500 ? 'bad' : s.status >= 400 ? 'warn' : s.status >= 200 ? 'ok' : '';
  return (
    <>
      <div className="overview-grid">
        {m('Status', s.paused ? 'PAUSED' : s.status, tone)}
        {m('Latency', s.paused ? '—' : (s.total < 1000 ? s.total : (s.total / 1000).toFixed(2)), '', s.total < 1000 ? ' ms' : ' s')}
        {m('TTFB', s.paused ? '—' : s.ttfb, '', ' ms')}
        {m('Request',  fmtBytes(s.reqSize), '')}
        {m('Response', fmtBytes(s.resSize), '')}
        {m('Protocol', s.proto, '')}
      </div>

      <div className="section">
        <h4>General</h4>
        <div className="sec-body">
          <div className="kv" style={{ gridTemplateColumns: '140px 1fr' }}>
            <div className="k">Request URL</div><div className="v">{s.url}</div>
            <div className="k">Request Method</div><div className="v"><span className="cell-method" data-m={s.method}>{s.method}</span></div>
            <div className="k">Status Code</div><div className="v"><span className="cell-status" data-c={statusBucket(s.status)}>{s.status || '—'}</span> {STATUS_TEXT[s.status]}</div>
            <div className="k">Remote Address</div><div className="v">{s.remote}</div>
            <div className="k">Referrer Policy</div><div className="v">strict-origin-when-cross-origin</div>
            <div className="k">TLS</div><div className="v">{s.cipher || '—'}</div>
            <div className="k">Started</div><div className="v">{new Date(s.ts).toISOString()}</div>
            {s.note && <><div className="k">Note</div><div className="v" style={{ color: 'var(--c-paused)' }}>{s.note}</div></>}
            {s.rewriteApplied && <><div className="k">Rewrite</div><div className="v" style={{ color: 'var(--c-4xx)' }}>{s.rewriteApplied}</div></>}
          </div>
        </div>
      </div>

      <div className="section">
        <h4>Tags</h4>
        <div className="sec-body">
          {s.tags.length === 0 && <span className="mute">— no tags applied</span>}
          {s.tags.map(t => <span key={t} className={'tag-badge ' + t}>{t.toUpperCase()}</span>)}
        </div>
      </div>
    </>
  );
}

function HeadersTab({ s, raw }) {
  const reqHeaders = raw ? (s.reqHeadersRaw || s.reqHeaders) : s.reqHeaders;
  const resHeaders = raw ? (s.resHeadersRaw || s.resHeaders) : s.resHeaders;
  return (
    <>
      <div className="section">
        <h4>Request Headers <span className="meta">{Object.keys(reqHeaders || {}).length} entries · {raw ? 'raw' : 'redacted'}</span></h4>
        <div className="sec-body"><HeaderList obj={reqHeaders} /></div>
      </div>
      <div className="section">
        <h4>Response Headers <span className="meta">{Object.keys(resHeaders || {}).length} entries · {raw ? 'raw' : 'redacted'}</span></h4>
        <div className="sec-body"><HeaderList obj={resHeaders} /></div>
      </div>
    </>
  );
}

function RequestTab({ s, raw }) {
  const body = raw ? (s.reqBodyRaw || s.reqBody) : s.reqBody;
  if (!body) return <div className="mute" style={{ padding: 14 }}>(no request body)</div>;
  const lang = typeof body === 'object' ? 'application/json' : 'text/plain';
  return <CodeBlock title={`Request Body (${raw ? 'raw' : 'redacted'})`} lang={lang} content={body} />;
}

function ResponseTab({ s, raw }) {
  const body = raw ? (s.resBodyRaw || s.resBody) : s.resBody;
  if (s.resSize === 0 || !body) {
    return (
      <div className="section">
        <h4>Response Body <span className="meta">{fmtBytes(s.resSize)}</span></h4>
        <div className="sec-body"><span className="mute">(empty body)</span></div>
      </div>
    );
  }
  const lang = typeof body === 'object' ? 'application/json' : (s.type === 'sse' ? 'text/event-stream' : 'text/plain');
  if (['image'].includes(s.type)) {
    return (
      <div className="section">
        <h4>Response Body <span className="meta">{fmtBytes(s.resSize)} · binary/base64</span></h4>
        <div className="sec-body">
          <span className="mute">Binary response body is stored as base64 for export and replay-safe inspection.</span>
          <div style={{ marginTop: 10 }}>
            <CodeBlock title={`Response Body (${raw ? 'raw' : 'redacted'})`} lang="base64" content={body} />
          </div>
        </div>
      </div>
    );
  }
  return <CodeBlock title={`Response Body (${raw ? 'raw' : 'redacted'})`} lang={lang} content={body} />;
}

function TimingTab({ s }) {
  const t = s.timing;
  const total = t.dns + t.tcp + t.tls + t.ttfb + t.body || 1;
  const max = total;
  const rows = [
    ['DNS lookup',       t.dns, 'dns',  'oklch(0.7 0.12 270)'],
    ['TCP handshake',    t.tcp, 'tcp',  'oklch(0.74 0.12 250)'],
    ['TLS negotiation',  t.tls, 'tls',  'oklch(0.74 0.12 220)'],
    ['Waiting (TTFB)',   t.ttfb,'ttfb', 'oklch(0.82 0.14 78)'],
    ['Content download', t.body,'body', 'oklch(0.78 0.15 148)'],
  ];
  let offset = 0;
  return (
    <>
      <div className="section">
        <h4>Timing Breakdown <span className="meta">total {fmtMs(s.total)}</span></h4>
        <div className="sec-body">
          <div className="timing">
            {rows.map(([label, ms, _, color]) => {
              const left = offset / max * 100;
              const w = Math.max(1, ms / max * 100);
              offset += ms;
              return (
                <div className="timing-row" key={label}>
                  <div className="label">{label}</div>
                  <div className="bar">
                    <span className="fill" style={{ left: left + '%', width: w + '%', background: color }} />
                  </div>
                  <div className="val">{ms ? ms + ' ms' : '—'}</div>
                </div>
              );
            })}
            <div className="timing-row" style={{ marginTop: 6, paddingTop: 6, borderTop: '1px solid var(--border-soft)' }}>
              <div className="label hi">Total</div>
              <div className="bar" style={{ background: 'transparent' }} />
              <div className="val hi">{fmtMs(s.total)}</div>
            </div>
          </div>
        </div>
      </div>

      <div className="section">
        <h4>Connection Reuse</h4>
        <div className="sec-body">
          <div className="kv">
            <div className="k">Connection</div><div className="v">{(t.dns + t.tcp + t.tls) === 0 ? 'reused (keep-alive)' : 'new'}</div>
            <div className="k">Protocol</div><div className="v">{s.proto}</div>
            <div className="k">Cipher</div><div className="v">{s.cipher}</div>
            <div className="k">ALPN</div><div className="v">{s.proto.includes('2') ? 'h2' : 'http/1.1'}</div>
          </div>
        </div>
      </div>
    </>
  );
}

function InspectorTab({ s }) {
  const i = s.inspector;
  if (!i) {
    return (
      <div className="empty" style={{ flexDirection: 'column', gap: 6, padding: 32 }}>
        <Icon name="inspector" size={28} stroke={1.2} />
        <div>No inspector data for this exchange.</div>
        <div className="mute" style={{ fontSize: 11 }}>JWT, GraphQL & gRPC plugins auto-populate this view when matching content is detected.</div>
      </div>
    );
  }
  if (i.kind === 'jwt') {
    return (
      <>
        <div className="section">
          <h4>Decoded JWT <span className="meta">{i.valid ? 'signature ok' : 'signature mismatch'} · expires in {i.expiresIn}</span></h4>
          <div className="sec-body">
            <div className="jwt-segments">
              <div className="jwt-seg" data-part="h">
                <h5>Header — alg</h5>
                <pre>{JSON.stringify(i.header, null, 2)}</pre>
              </div>
              <div className="jwt-seg" data-part="p">
                <h5>Payload — claims</h5>
                <pre>{JSON.stringify(i.payload, null, 2)}</pre>
              </div>
              <div className="jwt-seg" data-part="s">
                <h5>Signature</h5>
                <pre>{i.valid ? 'verified against JWKS\n076f3fb11' : 'invalid signature'}</pre>
              </div>
            </div>
          </div>
        </div>
        <div className="section">
          <h4>Claim Details</h4>
          <div className="sec-body">
            <div className="kv">
              {Object.entries(i.payload).map(([k, v]) => (
                <React.Fragment key={k}>
                  <div className="k">{k}</div>
                  <div className="v">{typeof v === 'object' ? JSON.stringify(v) : String(v)}</div>
                </React.Fragment>
              ))}
            </div>
          </div>
        </div>
      </>
    );
  }
  if (i.kind === 'graphql') {
    return (
      <div className="section">
        <h4>GraphQL Operation <span className="meta">{i.type} · {i.fields} fields</span></h4>
        <div className="sec-body">
          <div className="kv" style={{ gridTemplateColumns: '120px 1fr', marginBottom: 12 }}>
            <div className="k">Operation</div><div className="v">{i.operation}</div>
            <div className="k">Type</div><div className="v">{i.type}</div>
            <div className="k">Variables</div><div className="v">{JSON.stringify(i.variables)}</div>
          </div>
          <CodeBlock title="Query" lang="graphql" content={s.reqBody} />
        </div>
      </div>
    );
  }
  if (i.kind === 'grpc') {
    return (
      <>
        <div className="section">
          <h4>gRPC Call <span className="meta">{i.service} / {i.rpc}</span></h4>
          <div className="sec-body">
            <div className="kv" style={{ gridTemplateColumns: '120px 1fr' }}>
              <div className="k">Service</div><div className="v">{i.service}</div>
              <div className="k">Method</div><div className="v">{i.rpc}</div>
              <div className="k">Encoding</div><div className="v">application/grpc+proto</div>
            </div>
          </div>
        </div>
        <CodeBlock title="Request message" lang="protobuf" content={i.requestMessage} />
        <div style={{ height: 12 }} />
        <CodeBlock title="Response message" lang="protobuf" content={i.responseMessage} />
      </>
    );
  }
  return null;
}

function CookiesTab({ s, raw }) {
  const headerValue = (headers, name) => {
    const entry = Object.entries(headers || {}).find(([k]) => k.toLowerCase() === name);
    return entry ? String(entry[1]) : '';
  };
  const splitCookiePair = (value) => {
    const text = String(value || '').trim();
    if (!text) return null;
    const idx = text.indexOf('=');
    if (idx < 0) return { name: text, value: '', malformed: true };
    return {
      name: text.slice(0, idx).trim() || '(unnamed)',
      value: text.slice(idx + 1).trim(),
      malformed: false,
    };
  };
  const headers = raw ? (s.reqHeadersRaw || s.reqHeaders) : s.reqHeaders;
  const resHeaders = raw ? (s.resHeadersRaw || s.resHeaders) : s.resHeaders;
  const cookieHeader = headerValue(headers, 'cookie');
  const rawSetCookie = headerValue(resHeaders, 'set-cookie');
  const setCookies = rawSetCookie
    ? (Array.isArray(rawSetCookie) ? rawSetCookie : String(rawSetCookie).split(/,(?=\s*[^;,]+=)/))
    : [];
  const redactedCookie = cookieHeader === '••••••';
  const redactedSetCookie = rawSetCookie === '••••••';
  return (
    <>
      <div className="section">
        <h4>Request Cookies</h4>
        <div className="sec-body">
          {!cookieHeader && <span className="mute">(no request cookies)</span>}
          {redactedCookie && <span className="mute">(request cookies redacted)</span>}
          <div className="kv">
            {cookieHeader && !redactedCookie && cookieHeader.split(';').map((c, i) => {
              const pair = splitCookiePair(c);
              if (!pair) return null;
              return (
                <React.Fragment key={i}>
                  <div className="k">{pair.name}</div>
                  <div className="v">
                    {pair.value || <span className="mute">{pair.malformed ? '(redacted or malformed)' : '(empty)'}</span>}
                  </div>
                </React.Fragment>
              );
            })}
          </div>
        </div>
      </div>
      <div className="section">
        <h4>Response Set-Cookie</h4>
        <div className="sec-body">
          {setCookies.length === 0 && <span className="mute">(no response cookies)</span>}
          {redactedSetCookie && <span className="mute">(response cookies redacted)</span>}
          <div className="kv">
            {!redactedSetCookie && setCookies.map((c, i) => {
              const [first, ...attrs] = c.split(';');
              const pair = splitCookiePair(first);
              if (!pair) return null;
              return (
                <React.Fragment key={i}>
                  <div className="k">{pair.name}</div>
                  <div className="v">
                    {pair.value || <span className="mute">{pair.malformed ? '(redacted or malformed)' : '(empty)'}</span>}
                    {attrs.length > 0 && <span className="mute"> · {attrs.join(';').trim()}</span>}
                  </div>
                </React.Fragment>
              );
            })}
          </div>
        </div>
      </div>
    </>
  );
}

function DetailPanel({ session: s, onClose, onResume, onAbort, onCopyCurl, onCopyRawCurl, onReplay, onOpenInCompose }) {
  const [tab, setTab] = React.useState('overview');
  const [rawView, setRawView] = React.useState(false);
  React.useEffect(() => { setTab('overview'); }, [s?.id]);

  if (!s) {
    return (
      <div className="detail-panel">
        <div className="empty">
          Select a session to inspect headers, body, timing, and decoded payloads.
          <br /><br />
          <span className="mute">
            Keys: <span className="key">↑↓</span> navigate · <span className="key">⌘F</span> / <span className="key">⌘K</span> search
          </span>
        </div>
      </div>
    );
  }

  // Parse URL for highlighted breakdown
  let u;
  try { u = new URL(s.url); } catch { u = new URL(`http://${s.host || 'unknown'}${s.path || '/'}`); }

  const setBodyView = async (nextRaw) => {
    if (nextRaw && !rawView) {
      const ok = await window.confirmAction('Show unredacted local request and response data?', 'Show raw');
      if (!ok) return;
    }
    setRawView(nextRaw);
  };

  const tabs = [
    { key: 'overview',  label: 'Overview' },
    { key: 'headers',   label: 'Headers',   count: Object.keys(s.reqHeaders || {}).length + Object.keys(s.resHeaders || {}).length },
    { key: 'request',   label: 'Request',   count: s.reqBody ? 1 : null },
    { key: 'response',  label: 'Response',  count: s.resBody ? 1 : null },
    { key: 'timing',    label: 'Timing' },
    { key: 'inspector', label: 'Inspector', count: s.inspector ? 1 : null },
    { key: 'cookies',   label: 'Cookies' },
  ];

  return (
    <div className="detail-panel">
      <div className="detail-header">
        <div className="detail-title">
          <span className="cell-method" data-m={s.method} style={{ fontSize: 12, padding: '3px 7px', border: '1px solid currentColor', borderRadius: 4 }}>
            {s.method}
          </span>
          <span className="cell-status" data-c={statusBucket(s.status)} style={{ fontSize: 12, fontFamily: 'var(--font-mono)' }}>
            {s.status || 'PAUSED'} {STATUS_TEXT[s.status] && <span className="dim">{STATUS_TEXT[s.status]}</span>}
          </span>
          <span className="url" title={s.url}>
            <span className="scheme">{u.protocol}//</span>
            <span className="host">{u.host}</span>
            <span className="path">{u.pathname}</span>
            {u.search && <span className="query">{u.search}</span>}
          </span>
          <div className="actions">
            <button className="copy-btn" onClick={() => onCopyCurl?.(s)} title="Copy as cURL"><Icon name="copy" size={11} stroke={1.8} /> cURL</button>
            <button className="copy-btn" onClick={() => onCopyRawCurl?.(s)} title="Copy raw cURL with unredacted local data">raw cURL</button>
            <button className="copy-btn" onClick={() => onReplay?.(s)} title="Replay this request"><Icon name="replay" size={11} stroke={1.8} /> Replay</button>
            <button className="copy-btn" onClick={() => onOpenInCompose?.(s)} title="Send to builder" aria-label="Send to builder"><Icon name="open" size={11} stroke={1.8} /></button>
            <button className="icon-btn" onClick={onClose} title="Close panel" aria-label="Close detail panel" style={{ marginLeft: 2 }}>
              <Icon name="x" size={14} stroke={1.6} />
            </button>
          </div>
        </div>
        <div className="detail-sub">
          <div className="item"><span className="k">ID</span><span className="v">{s.id}</span></div>
          <div className="item"><span className="k">PROTO</span><span className="v">{s.proto}</span></div>
          <div className="item"><span className="k">REMOTE</span><span className="v">{s.remote}</span></div>
          <div className="item"><span className="k">STARTED</span><span className="v">{fmtTime(s.ts)}</span></div>
          <div className="item"><span className="k">TOTAL</span><span className="v">{s.paused ? '—' : fmtMs(s.total)}</span></div>
        </div>
        <div className="detail-tabs">
          {tabs.map(t => (
            <button key={t.key} className={'tab' + (tab === t.key ? ' on' : '')} onClick={() => setTab(t.key)}>
              {t.label}
              {t.count != null && t.count > 0 && <span className="count">{t.count}</span>}
            </button>
          ))}
          <div className="spacer" />
          <div className="segctl" style={{ marginLeft: 8 }}>
            <button className={!rawView ? 'on' : ''} onClick={() => setBodyView(false)}>Redacted</button>
            <button className={rawView ? 'on' : ''} onClick={() => setBodyView(true)}>Raw</button>
          </div>
        </div>
      </div>

      {s.paused && (
        <div className="paused-banner">
          <Icon name="pauseRail" size={16} stroke={1.6} />
          <div>
            <div className="label">Request pending</div>
            <div className="mute" style={{ fontSize: 11 }}>
              {s.note || 'No response captured yet. If a breakpoint is active, use the Breakpoints panel to resume or abort.'}
            </div>
          </div>
        </div>
      )}

      <div className="detail-body">
        {tab === 'overview'  && <OverviewTab s={s} />}
        {tab === 'headers'   && <HeadersTab s={s} raw={rawView} />}
        {tab === 'request'   && <RequestTab s={s} raw={rawView} />}
        {tab === 'response'  && <ResponseTab s={s} raw={rawView} />}
        {tab === 'timing'    && <TimingTab s={s} />}
        {tab === 'inspector' && <InspectorTab s={s} />}
        {tab === 'cookies'   && <CookiesTab s={s} raw={rawView} />}
      </div>
    </div>
  );
}

window.DetailPanel = DetailPanel;
window.statusBucket = statusBucket;
