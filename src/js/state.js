// ── State ─────────────────────────────────────────────────────────────────────
const G = {
  sessions: [],
  sel: null,
  view: 'traffic',
  sideFil: 'all',
  rec: true,
  timer: null,
  srch: '',
  regexSearch: false,
  scFil: new Set(['2xx','3xx','4xx','5xx','pending']),
  dtab: 'overview',
  routes: {},
  dns: {},
  rewrites: [],
  throttle: null,
  maxMs: 1,
  bpRules: [],
  bpPending: [],
  sortCol: null,
  sortDir: 1,
  focusHosts: [],
  exportFmt: 'har',
  importMode: 'merge',
  sessionSig: '',
  copyBody: '',
  lastTs: null,    // newest session timestamp seen; used for incremental poll
  sse: null,       // EventSource for server-sent session updates
  bpTimer: null,   // setInterval for breakpoint polling
  treeMode: false,
  treeExpanded: new Set(),  // path sub-folder keys (default closed)
  treeClosed: new Set(),    // host keys explicitly collapsed (default open)
  rwEditIdx: null,          // null=add mode, number=edit mode index
  headerMaps: [],           // Array<HeaderMapRule>
  modifications: [],        // Array<ModificationRule>
  mapLocal: {},             // Record<host, filePath>
  compose: {
    tabs: [],
    activeTabId: null,
    collections: [],
    vars: {},
    _newCollRow: false,
    _renamingReq: null,
    _pendingDelete: null,  // {type:'collection'|'request', collId, reqId?}
  },
  mockRules: [],
  scripts:   [],
  webhooks:  [],
  captureFilter: { mode: 'disabled', hosts: [] },
  mFil: new Set(['GET','POST','PUT','DELETE','PATCH','OTHER']),
  _diffBase: null,
};

// ── Utils ─────────────────────────────────────────────────────────────────────
const esc = s => String(s??'').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
const fmtSz = b => !b&&b!==0?'—':b<1024?b+' B':b<1048576?(b/1024).toFixed(1)+' KB':(b/1048576).toFixed(1)+' MB';
const fmtMs = ms => !ms&&ms!==0?'—':ms<1000?ms+' ms':(ms/1000).toFixed(2)+' s';
const relT = iso => { const d=Date.now()-new Date(iso); return d<2000?'just now':d<60000?Math.floor(d/1000)+'s ago':d<3600000?Math.floor(d/60000)+'m ago':new Date(iso).toLocaleTimeString(); };
const getPath = uri => { try { const u=new URL(uri.startsWith('http')?uri:'http://x'+uri); return u.pathname+u.search; } catch { return uri||'/'; } };
const isProxy = host => host === window.location.host;

function mbadge(m) {
  const cls = {GET:'mGET',POST:'mPOST',PUT:'mPUT',DELETE:'mDELETE',PATCH:'mPATCH',WS:'mWS'}[m]||'mOTHER';
  return `<span class="mbadge ${cls}">${esc(m)}</span>`;
}
function sbadge(code) {
  if (!code) return `<span class="sbadge s0">—</span>`;
  const c = parseInt(code), cls = c<300?'s2':c<400?'s3':c<500?'s4':'s5';
  return `<span class="sbadge ${cls}">${esc(code)}</span>`;
}
function wsIsClosed(s) {
  const frames = s.ws_frames || [];
  return frames.some(f => f.opcode === 8);
}
function wsBadge(s) {
  return wsIsClosed(s)
    ? `<span class="sbadge sWSClosed">Closed</span>`
    : `<span class="sbadge sWSOpen"><span class="ws-dot"></span>Open</span>`;
}
function tlsIcon(on) {
  const c = on?'var(--success)':'var(--text4)';
  return on
    ? `<svg width="13" height="13" viewBox="0 0 12 12" fill="none"><rect x="2" y="5" width="8" height="6" rx="1" fill="${c}" fill-opacity=".12" stroke="${c}" stroke-width="1"/><path d="M4 5V3.5a2 2 0 014 0V5" stroke="${c}" stroke-width="1"/></svg>`
    : `<svg width="13" height="13" viewBox="0 0 12 12" fill="none"><rect x="2" y="5" width="8" height="6" rx="1" fill="${c}" fill-opacity=".07" stroke="${c}" stroke-width="1"/><path d="M4 5V4" stroke="${c}" stroke-width="1" stroke-dasharray="1.5 1.5"/></svg>`;
}
function toast(msg, err=false) {
  const w=document.getElementById('toast-wrap'), t=document.createElement('div');
  t.className='toast'+(err?' err':''); t.textContent=msg; w.appendChild(t);
  requestAnimationFrame(()=>t.classList.add('show'));
  setTimeout(()=>{t.classList.remove('show');setTimeout(()=>t.remove(),300);},2500);
}

// ── HTTP reason phrases ───────────────────────────────────────────────────────
const HTTP_REASONS = {200:'OK',201:'Created',204:'No Content',206:'Partial',301:'Moved',302:'Found',304:'Not Modified',400:'Bad Request',401:'Unauthorized',403:'Forbidden',404:'Not Found',405:'Not Allowed',408:'Timeout',409:'Conflict',410:'Gone',422:'Unprocessable',429:'Too Many',500:'Internal Error',502:'Bad Gateway',503:'Unavailable',504:'Gateway Timeout'};

// ── JSON pretty-print ─────────────────────────────────────────────────────────
function isJsonCt(headers) {
  const ct = Object.entries(headers||{}).find(([k])=>k.toLowerCase()==='content-type');
  return ct && String(ct[1]).includes('json');
}
function prettyJson(raw) {
  try {
    // Escape HTML first so JSON string values cannot inject tags, then apply
    // span-based syntax highlighting on the safe, entity-encoded output.
    // [^<\n] matches through &amp; &quot; etc. (entities have no literal <)
    // so the lazy *? always finds the correct closing &quot; delimiter.
    const safe = esc(JSON.stringify(JSON.parse(raw), null, 2));
    return safe
      .replace(/(&quot;[^<\n]*?&quot;)(\s*:)/g, '<span class="json-k">$1</span>$2')
      .replace(/:\s*(&quot;[^<\n]*?&quot;)/g, ': <span class="json-s">$1</span>')
      .replace(/:\s*(-?\d+\.?\d*(?:[eE][+-]?\d+)?)\b/g, ': <span class="json-n">$1</span>')
      .replace(/:\s*(true|false)\b/g, ': <span class="json-b">$1</span>')
      .replace(/:\s*(null)\b/g, ': <span class="json-null">$1</span>');
  } catch { return esc(raw); }
}
function isImageCt(headers) {
  const ct = Object.entries(headers||{}).find(([k])=>k.toLowerCase()==='content-type');
  return ct && (ct[1]||'').split(';')[0].trim().startsWith('image/');
}
function getCtMime(headers) {
  const ct = Object.entries(headers||{}).find(([k])=>k.toLowerCase()==='content-type');
  return ct ? (ct[1]||'').split(';')[0].trim() : '';
}
function isXmlCt(headers) {
  const m = getCtMime(headers);
  return m === 'text/xml' || m === 'application/xml' || m.endsWith('+xml');
}
function isHtmlCt(headers) {
  return getCtMime(headers) === 'text/html';
}
function prettyXml(raw) {
  try {
    const doc = new DOMParser().parseFromString(raw, 'text/xml');
    if (doc.querySelector('parsererror')) return esc(raw);
    function indent(node, depth) {
      const pad = '  '.repeat(depth);
      if (node.nodeType === Node.TEXT_NODE) {
        const t = node.textContent.trim();
        return t ? esc(t) : '';
      }
      if (node.nodeType === Node.COMMENT_NODE) return `${pad}<span class="json-null">&lt;!--${esc(node.textContent)}--&gt;</span>`;
      if (node.nodeType !== Node.ELEMENT_NODE) return '';
      const tag = esc(node.tagName);
      const attrs = Array.from(node.attributes).map(a=>`<span class="json-k"> ${esc(a.name)}</span>=<span class="json-s">&quot;${esc(a.value)}&quot;</span>`).join('');
      const children = Array.from(node.childNodes).map(c=>indent(c,depth+1)).filter(Boolean);
      if (!children.length) return `${pad}&lt;<span style="color:var(--accent)">${tag}</span>${attrs}/&gt;`;
      if (children.length===1 && !children[0].includes('\n'))
        return `${pad}&lt;<span style="color:var(--accent)">${tag}</span>${attrs}&gt;${children[0]}&lt;/<span style="color:var(--accent)">${tag}</span>&gt;`;
      return `${pad}&lt;<span style="color:var(--accent)">${tag}</span>${attrs}&gt;\n${children.join('\n')}\n${pad}&lt;/<span style="color:var(--accent)">${tag}</span>&gt;`;
    }
    return Array.from(doc.childNodes).map(n=>indent(n,0)).filter(Boolean).join('\n');
  } catch { return esc(raw); }
}
function prettyHtml(raw) {
  try {
    const doc = new DOMParser().parseFromString(raw, 'text/html');
    const s = new XMLSerializer().serializeToString(doc);
    return prettyXml(s);
  } catch { return esc(raw); }
}
function renderBody(body, headers) {
  if (!body) return '<span style="color:var(--text4);font-size:12px">Empty body</span>';
  if (isImageCt(headers)) {
    const mime = getCtMime(headers);
    return `<div style="padding:12px;text-align:center">
      <img src="data:${esc(mime)};base64,${body}" style="max-width:100%;max-height:480px;border-radius:4px;border:0.5px solid var(--border)" onerror="this.style.display='none';this.nextSibling.style.display=''">
      <span style="display:none;color:var(--text4);font-size:12px">Image could not be decoded</span>
    </div>`;
  }
  const highlighted = isJsonCt(headers) ? prettyJson(body)
    : isXmlCt(headers) ? prettyXml(body)
    : isHtmlCt(headers) ? prettyHtml(body)
    : esc(body);
  G.copyBody = body;
  return `<pre class="body-pre">${highlighted}</pre>
    <div style="margin-top:8px;display:flex;gap:6px">
      <button class="btn btn-sm btn-ghost" onclick="navigator.clipboard.writeText(G.copyBody).then(()=>toast('Copied'))">Copy</button>
    </div>`;
}

