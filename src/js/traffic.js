// ── Filter & render ───────────────────────────────────────────────────────────
function applyFilter(all) {
  let r = all.filter(s => !isProxy(s.request?.host));

  if (G.sideFil==='done')    r = r.filter(s=>s.response&&(s.metrics?.status_code||0)<400);
  else if (G.sideFil==='errors')  r = r.filter(s=>s.response&&(s.metrics?.status_code||0)>=400);
  else if (G.sideFil==='pending') r = r.filter(s=>!s.response && s.request?.method !== 'WS');
  else if (G.sideFil==='ws')      r = r.filter(s=>s.request?.method === 'WS');

  if (G.focusHosts.length) {
    r = r.filter(s => G.focusHosts.some(h=>(s.request?.host||'').includes(h)));
  }

  if (G.srch) {
    if (G.regexSearch) {
      try {
        const re = new RegExp(G.srch, 'i');
        r = r.filter(s =>
          re.test(s.request?.host||'') ||
          re.test(s.request?.uri||'') ||
          re.test(s.request?.method||'') ||
          re.test(s.response?.body||'') ||
          re.test(s.request?.body||'')
        );
      } catch { /* invalid regex — show all */ }
    } else {
      const q = G.srch.toLowerCase();
      r = r.filter(s =>
        (s.request?.host||'').toLowerCase().includes(q) ||
        (s.request?.uri||'').toLowerCase().includes(q) ||
        (s.request?.method||'').toLowerCase().includes(q) ||
        (s.response?.body||'').toLowerCase().includes(q) ||
        (s.request?.body||'').toLowerCase().includes(q)
      );
    }
  }

  r = r.filter(s => {
    if (!s.response) return G.scFil.has('pending');
    const c = s.metrics?.status_code || s.response?.status || 0;
    if (c<300) return G.scFil.has('2xx');
    if (c<400) return G.scFil.has('3xx');
    if (c<500) return G.scFil.has('4xx');
    return G.scFil.has('5xx');
  });

  if (G.mFil.size < 6) {
    const KNOWN = new Set(['GET','POST','PUT','DELETE','PATCH']);
    r = r.filter(s => {
      const m = (s.request?.method||'').toUpperCase();
      return KNOWN.has(m) ? G.mFil.has(m) : G.mFil.has('OTHER');
    });
  }

  if (G.sortCol) {
    const d = G.sortDir;
    r.sort((a,b) => {
      let av, bv;
      if (G.sortCol==='method') { av=a.request?.method||''; bv=b.request?.method||''; return d*av.localeCompare(bv); }
      if (G.sortCol==='host')   { av=a.request?.host||'';   bv=b.request?.host||'';   return d*av.localeCompare(bv); }
      if (G.sortCol==='path')   { av=getPath(a.request?.uri); bv=getPath(b.request?.uri); return d*av.localeCompare(bv); }
      if (G.sortCol==='status') { av=a.metrics?.status_code||0; bv=b.metrics?.status_code||0; return d*(av-bv); }
      if (G.sortCol==='size')   { av=a.metrics?.response_size_bytes||0; bv=b.metrics?.response_size_bytes||0; return d*(av-bv); }
      if (G.sortCol==='dur')    { av=a.metrics?.latency_ms||0; bv=b.metrics?.latency_ms||0; return d*(av-bv); }
      return 0;
    });
  } else {
    r.sort((a,b)=>new Date(b.timestamp)-new Date(a.timestamp));
  }

  return r;
}

function renderTable() {
  const rows = applyFilter(G.sessions);
  G.maxMs = Math.max(1,...rows.map(s=>s.metrics?.latency_ms||0));
  const tbody = document.getElementById('tbody');
  const fc = document.getElementById('filter-count');
  if (fc) fc.textContent = rows.length + ' request' + (rows.length!==1?'s':'');

  if (!rows.length) {
    tbody.innerHTML = `<tr class="empty"><td colspan="9">No traffic captured.<br>Set your system HTTP proxy to <strong>127.0.0.1:8080</strong> and browse.</td></tr>`;
    return;
  }

  tbody.innerHTML = rows.map((s,i)=>{
    const status = s.metrics?.status_code || s.response?.status || null;
    const lat = s.metrics?.latency_ms;
    const sz = s.metrics?.response_size_bytes;
    const pathFull = getPath(s.request?.uri);
    const qIdx = pathFull.indexOf('?');
    const pathDisplay = qIdx >= 0
      ? esc(pathFull.slice(0, qIdx)) + `<span style="color:var(--text4)">${esc(pathFull.slice(qIdx))}</span>`
      : esc(pathFull);
    const tls = !!(G.routes[s.request?.host]?.startsWith('https://'));
    const pct = lat ? Math.round(lat/G.maxMs*100) : 0;
    const sel = G.sel?.id===s.id;
    const pending = !s.response && s.request?.method !== 'WS';
    const durCls = !lat ? '' : lat < 200 ? 'dur-fast' : lat >= 1000 ? 'dur-slow' : '';
    const checked = G._bulkSel?.has(s.id) ? 'checked' : '';
    const pills = [
      s.inspector_data?.jwt     && '<span class="proto-pill pill-jwt">JWT</span>',
      s.inspector_data?.graphql && '<span class="proto-pill pill-gql">GQL</span>',
      s.inspector_data?.grpc    && '<span class="proto-pill pill-grpc">gRPC</span>',
    ].filter(Boolean).join('');
    return `<tr ${sel?'class="sel"':''} onclick="clickRow('${s.id}')">
      <td style="padding:0 4px" onclick="event.stopPropagation()"><input type="checkbox" class="row-chk" data-id="${s.id}" ${checked} onchange="toggleBulkRow('${s.id}',this.checked)" style="cursor:pointer"></td>
      <td style="color:var(--text4);font-size:11px">${i+1}</td>
      <td>${mbadge(s.request?.method||'?')}</td>
      <td class="trunc" style="color:var(--text3)">${esc(s.request?.host||'')}</td>
      <td class="trunc">${pathDisplay}${pills}</td>
      <td>${s.request?.method==='WS' ? wsBadge(s) : pending ? (G.sideFil==='all' ? '<span class="pending-spinner"></span>' : '<span class="sbadge s0">—</span>') : sbadge(status)}</td>
      <td style="color:var(--text3)">${esc(fmtSz(sz))}</td>
      <td><div class="dur-wrap"><span class="dur-txt ${durCls}">${pending ? '<span style="color:var(--text4)">—</span>' : esc(fmtMs(lat))}</span>${lat?`<div class="dur-track"><div class="dur-fill" style="width:${pct}%"></div></div>`:''}</div></td>
      <td>${tlsIcon(tls)}</td>
    </tr>`;
  }).join('');
}

// ── View mode ─────────────────────────────────────────────────────────────────
function setViewMode(mode) {
  G.treeMode = mode === 'structure';
  document.getElementById('vmode-seq').classList.toggle('on', !G.treeMode);
  document.getElementById('vmode-tree').classList.toggle('on', G.treeMode);
  document.getElementById('seq-wrap').style.display = G.treeMode ? 'none' : '';
  document.getElementById('tree-wrap').style.display = G.treeMode ? '' : 'none';
  if (G.treeMode) renderTree(); else renderTable();
}

// ── Path trie ─────────────────────────────────────────────────────────────────
// Each node: { children: Map<seg, Node>, requests: Session[] }
function buildTrie(sessions) {
  const hosts = new Map();
  for (const s of sessions) {
    const host = s.request?.host || '(unknown)';
    if (!hosts.has(host)) hosts.set(host, { children: new Map(), requests: [] });
    const pathOnly = (getPath(s.request?.uri) || '/').split('?')[0];
    const segs = pathOnly.split('/').filter(Boolean);
    let node = hosts.get(host);
    for (const seg of segs) {
      if (!node.children.has(seg)) node.children.set(seg, { children: new Map(), requests: [] });
      node = node.children.get(seg);
    }
    node.requests.push(s);
  }
  return hosts;
}

function trieCount(node) {
  return node.requests.length + [...node.children.values()].reduce((s,c)=>s+trieCount(c),0);
}
function trieHasErr(node) {
  return node.requests.some(s=>(s.metrics?.status_code||0)>=400) ||
    [...node.children.values()].some(trieHasErr);
}
function trieHasPend(node) {
  return node.requests.some(s=>!s.response && s.request?.method !== 'WS') ||
    [...node.children.values()].some(trieHasPend);
}

// Walk down while exactly 1 child and no direct requests — merge into single label.
function compressNode(seg, node) {
  let label = seg, cur = node;
  while (cur.children.size === 1 && cur.requests.length === 0) {
    const [[nextSeg, nextNode]] = [...cur.children.entries()];
    label += '/' + nextSeg;
    cur = nextNode;
  }
  return { label, node: cur };
}

const CARET = `<svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><path d="M2 1.5l3.5 2.5L2 6.5"/></svg>`;

// nodeKeys: index → nodeKey string (avoids escaping issues in onclick attrs)
let _nodeKeys = [];
function _nk(key) { const i = _nodeKeys.length; _nodeKeys.push(key); return i; }

function reqRow(s, indent, compact=false) {
  const status = s.metrics?.status_code || s.response?.status || null;
  const lat = s.metrics?.latency_ms;
  const sz = s.metrics?.response_size_bytes;
  const sel = G.sel?.id === s.id;
  let pathLabel = '';
  if (compact) {
    const uri = s.request?.uri || '';
    const q = uri.indexOf('?');
    if (q >= 0) {
      const qs = uri.slice(q + 1);
      const first = qs.split('&')[0];
      pathLabel = `<span class="tree-req-qs">(${esc(first)}${qs.includes('&') ? '…' : ''})</span>`;
    }
  } else {
    const path = getPath(s.request?.uri);
    const qIdx = path.indexOf('?');
    pathLabel = `<span class="tree-req-path" title="${esc(path)}">${qIdx >= 0 ? esc(path.slice(0,qIdx)) + `<span style="color:var(--text4)">${esc(path.slice(qIdx))}</span>` : esc(path)}</span>`;
  }
  return `<div class="tree-req-row${sel?' sel':''}" onclick="clickRow('${s.id}')">
    <span class="tree-indent" style="width:${indent}px"></span>
    <span class="tree-req-method">${mbadge(s.request?.method||'?')}</span>
    ${pathLabel}
    <span class="tree-req-status">${sbadge(status)}</span>
    <span class="tree-req-size">${esc(fmtSz(sz))}</span>
    <span class="tree-req-dur">${esc(fmtMs(lat))}</span>
  </div>`;
}

function renderNodeChildren(node, depth, hostKey, pathSoFar) {
  let html = '';
  const indent = depth * 14 + 10;

  // Direct requests at this path level (e.g. GET /v1 when /v1/sub also exists)
  for (const s of node.requests) html += reqRow(s, indent, true);

  // Children, sorted alphabetically, with path compression applied
  for (const seg of [...node.children.keys()].sort()) {
    const { label, node: child } = compressNode(seg, node.children.get(seg));
    const fullPath = pathSoFar + '/' + label;
    const cnt = trieCount(child);
    const hasErr = trieHasErr(child);

    if (child.children.size === 0) {
      // Leaf folder: always expanded, no toggle caret
      html += `<div>
        <div class="tree-folder-hdr" style="cursor:default">
          <span class="tree-indent" style="width:${indent}px"></span>
          <span class="tree-seg">${esc(label)}</span>
          <span class="tree-seg-count">${cnt}</span>
          ${hasErr?'<span class="tree-err-dot"></span>':''}
        </div>
        ${child.requests.map(s => reqRow(s, indent + 14, true)).join('')}
      </div>`;
    } else {
      // Has sub-children: collapsible folder
      const nodeKey = hostKey + '\x00' + fullPath;
      const ki = _nk(nodeKey);
      const open = G.treeExpanded.has(nodeKey);
      html += `<div>
        <div class="tree-folder-hdr" onclick="toggleTreeNode(${ki})">
          <span class="tree-indent" style="width:${indent}px"></span>
          <span class="tree-caret${open?' open':''}">${CARET}</span>
          <span class="tree-seg">${esc(label)}</span>
          <span class="tree-seg-count">${cnt}</span>
          ${hasErr?'<span class="tree-err-dot"></span>':''}
        </div>
        ${open ? renderNodeChildren(child, depth+1, hostKey, fullPath) : ''}
      </div>`;
    }
  }
  return html;
}

function renderTree() {
  const rows = applyFilter(G.sessions);
  const tb = document.getElementById('tree-body');
  const fc = document.getElementById('filter-count');
  if (fc) fc.textContent = rows.length + ' request' + (rows.length!==1?'s':'');

  if (!rows.length) {
    tb.innerHTML = `<div style="padding:40px;text-align:center;color:var(--text4);line-height:1.6">No traffic captured.<br>Set your system HTTP proxy to <strong>127.0.0.1:8080</strong> and browse.</div>`;
    return;
  }

  _nodeKeys = []; // reset index before each render
  const hostTrie = buildTrie(rows);

  // Sort hosts: error hosts first, then alphabetical
  const hosts = [...hostTrie.keys()].sort((a, b) => {
    const ae = trieHasErr(hostTrie.get(a)), be = trieHasErr(hostTrie.get(b));
    if (ae !== be) return ae ? -1 : 1;
    return a.localeCompare(b);
  });

  tb.innerHTML = hosts.map(host => {
    const root = hostTrie.get(host);
    const cnt = trieCount(root);
    const hasErr = trieHasErr(root);
    const hasPend = trieHasPend(root);
    const hostKey = host;
    const ki = _nk(hostKey);
    const open = !G.treeClosed.has(hostKey);
    return `<div class="tree-host-section">
      <div class="tree-host-hdr" onclick="toggleTreeNode(${ki})">
        <span class="tree-caret${open?' open':''}">${CARET}</span>
        <span class="tree-hostname">${esc(host)}</span>
        <span class="tree-host-count">${cnt}</span>
        ${hasErr?'<span class="tree-err-dot"></span>':''}
        ${hasPend?'<span style="width:5px;height:5px;border-radius:50%;background:var(--warning);flex-shrink:0;margin-left:3px;display:inline-block"></span>':''}
      </div>
      ${open ? renderNodeChildren(root, 1, hostKey, '') : ''}
    </div>`;
  }).join('');
}

function toggleTreeNode(ki) {
  const key = _nodeKeys[ki];
  if (!key) return;
  if (key.includes('\x00')) {
    // path sub-folder: default closed, toggle treeExpanded
    if (G.treeExpanded.has(key)) G.treeExpanded.delete(key);
    else G.treeExpanded.add(key);
  } else {
    // host node: default open, toggle treeClosed
    if (G.treeClosed.has(key)) G.treeClosed.delete(key);
    else G.treeClosed.add(key);
  }
  renderTree();
}

function updateBadges() {
  const all = G.sessions.filter(s=>!isProxy(s.request?.host));
  const done = all.filter(s=>s.response&&(s.metrics?.status_code||0)<400);
  const err  = all.filter(s=>s.response&&(s.metrics?.status_code||0)>=400);
  const pend = all.filter(s=>!s.response && s.request?.method !== 'WS');
  const ws   = all.filter(s=>s.request?.method === 'WS');
  document.getElementById('nb-all').textContent  = all.length;
  document.getElementById('nb-done').textContent = done.length;
  document.getElementById('nb-err').textContent  = err.length;
  document.getElementById('nb-pend').textContent = pend.length;
  document.getElementById('nb-ws').textContent   = ws.length;
  document.getElementById('sb-count').textContent = all.length+' request'+(all.length!==1?'s':'');
}

// ── Fetch sessions ─────────────────────────────────────────────────────────────
async function fetchSessions() {
  try {
    const url = G.lastTs
      ? `/api/sessions?since=${encodeURIComponent(G.lastTs)}`
      : '/api/sessions';
    const r = await fetch(url);
    const d = await r.json();
    const incoming = d.sessions || [];

    if (G.lastTs && incoming.length > 0) {
      // Merge incoming (new + pending-updated) into existing list by ID
      const byId = new Map(G.sessions.map(s => [s.id, s]));
      for (const s of incoming) byId.set(s.id, s);
      G.sessions = [...byId.values()].sort((a, b) =>
        new Date(b.timestamp) - new Date(a.timestamp));
    } else if (!G.lastTs) {
      G.sessions = incoming;
    }

    // Advance cursor to newest session timestamp
    if (G.sessions.length > 0) G.lastTs = G.sessions[0].timestamp;

    // Refresh selected session if it was updated (keeps Frames tab live)
    if (G.sel && incoming.length > 0) {
      const updated = incoming.find(s => s.id === G.sel.id);
      if (updated) { G.sel = updated; renderDetail(); }
    }

    updateBadges();
    if (!G.lastTs || incoming.length > 0) {
      if (G.treeMode) renderTree(); else renderTable();
    }
    document.getElementById('sb-dot').style.background = 'var(--success)';
  } catch {
    document.getElementById('sb-dot').style.background = 'var(--danger)';
  }
}

// ── Click row ─────────────────────────────────────────────────────────────────
async function clickRow(id) {
  try {
    const r = await fetch(`/api/sessions/${id}`);
    const d = await r.json();
    G.sel = d.exchange;
    // Keep G.sessions in sync so the row status reflects the fetched state
    const si = G.sessions.findIndex(s => s.id === G.sel.id);
    if (si >= 0) G.sessions[si] = G.sel;
    if (G.treeMode) renderTree(); else renderTable();
    renderDetail();
    const label = (G.sel.request?.method||'') + ' ' + getPath(G.sel.request?.uri);
    document.getElementById('sb-sel').textContent = label;
    document.getElementById('detail-title').textContent = label;
    document.getElementById('detail-topbar').style.display = '';
  } catch { toast('Failed to load session', true); }
}

// ── Detail panel ───────────────────────────────────────────────────────────────
function switchDTab(el) {
  document.querySelectorAll('.dtab').forEach(t=>t.classList.remove('on'));
  el.classList.add('on');
  G.dtab = el.dataset.tab;
  renderDetail();
}

function goCompose() {
  // Create a compose tab pre-filled from the selected session, then navigate there
  if (G.sel) cmpNewTab(G.sel);
  const navEl=document.querySelector('.nav-item[data-v="compose"]');
  if (navEl) navItem(navEl);
}


// ── Sort ──────────────────────────────────────────────────────────────────────
function updateSortReset() {
  document.getElementById('sort-reset').style.display = G.sortCol ? '' : 'none';
}

function sortBy(th) {
  const col = th.dataset.col;
  document.querySelectorAll('th[data-col]').forEach(t=>t.classList.remove('asc','desc'));
  if (G.sortCol===col) { G.sortDir *= -1; } else { G.sortCol=col; G.sortDir=1; }
  th.classList.add(G.sortDir>0?'asc':'desc');
  updateSortReset();
  renderCurrent();
}

function resetSort() {
  G.sortCol = null;
  G.sortDir = 1;
  document.querySelectorAll('th[data-col]').forEach(t=>t.classList.remove('asc','desc'));
  updateSortReset();
  renderCurrent();
}

// ── Host Focus ────────────────────────────────────────────────────────────────
function onFocusKey(e) {
  if (e.key!=='Enter') return;
  const v = document.getElementById('focus-in').value.trim();
  if (v && !G.focusHosts.includes(v)) {
    G.focusHosts.push(v);
    document.getElementById('focus-in').value = '';
    renderFocusChips();
    renderCurrent();
  }
}
function removeFocus(h) {
  G.focusHosts = G.focusHosts.filter(x=>x!==h);
  renderFocusChips();
  renderCurrent();
}
function renderFocusChips() {
  document.getElementById('focus-chips').innerHTML =
    G.focusHosts.map(h=>`<span class="focus-chip">${esc(h)}<span class="focus-chip-x" onclick="removeFocus('${esc(h)}')">✕</span></span>`).join('');
}

