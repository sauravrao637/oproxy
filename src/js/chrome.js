// ── Theme ─────────────────────────────────────────────────────────────────────
function toggleTheme() {
  const dark = document.documentElement.getAttribute('data-theme')==='dark';
  const next = dark ? 'light' : 'dark';
  document.documentElement.setAttribute('data-theme', next);
  document.getElementById('ic-moon').style.display = dark ? '' : 'none';
  document.getElementById('ic-sun').style.display  = dark ? 'none' : '';
  localStorage.setItem('oproxy-theme', next);
}

// ── Modals ────────────────────────────────────────────────────────────────────
function openShortcuts() { document.getElementById('modal-shortcuts').classList.add('show'); }
function openExport() {
  document.getElementById('export-count').textContent = G.sessions.filter(s=>!isProxy(s.request?.host)).length;
  document.getElementById('modal-export').classList.add('show');
}
function closeModal(id) { document.getElementById(id).classList.remove('show'); }
function selectExportFmt(fmt) {
  G.exportFmt = fmt;
  selectOpt('eopt-har', 'eopt-json', 'har', fmt);
}
function doExport() {
  const rows = G.sessions.filter(s=>!isProxy(s.request?.host));
  if (!rows.length) { toast('No sessions to export', true); return; }
  let content, filename;
  if (G.exportFmt==='har') {
    const har = { log:{ version:'1.2', creator:{name:'oproxy',version:'1.0'}, entries: rows.map(s=>({
      startedDateTime: s.timestamp,
      time: s.metrics?.latency_ms||0,
      request:{ method:s.request?.method||'GET', url:s.request?.uri||'', httpVersion:'HTTP/1.1',
        headers:Object.entries(s.request?.headers||{}).map(([name,value])=>({name,value})),
        queryString:[], cookies:[], headersSize:-1, bodySize:s.request?.body?.length||0,
        postData: s.request?.body?{mimeType:'application/json',text:s.request.body}:undefined },
      response: s.response ? {status:s.response.status, statusText:'', httpVersion:'HTTP/1.1',
        headers:Object.entries(s.response.headers||{}).map(([name,value])=>({name,value})),
        cookies:[], content:{size:s.metrics?.response_size_bytes||0, mimeType:'', text:s.response.body||''},
        redirectURL:'', headersSize:-1, bodySize:s.metrics?.response_size_bytes||0 }
        : {status:0,statusText:'',httpVersion:'HTTP/1.1',headers:[],cookies:[],content:{size:0,mimeType:'',text:''},redirectURL:'',headersSize:-1,bodySize:-1},
      timings:{send:0, wait:s.metrics?.latency_ms||0, receive:0}, cache:{},
    })) }};
    content = JSON.stringify(har, null, 2);
    filename = 'oproxy-export.har';
  } else {
    content = JSON.stringify(rows, null, 2);
    filename = 'oproxy-export.json';
  }
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([content],{type:'application/json'}));
  a.download = filename; a.click(); URL.revokeObjectURL(a.href);
  closeModal('modal-export');
  toast(`Exported ${rows.length} sessions as ${G.exportFmt.toUpperCase()}`);
}

// ── Import ────────────────────────────────────────────────────────────────────
function openImport() {
  document.getElementById('modal-import').classList.add('show');
}
function selectOpt(id1, id2, selVal, newVal) {
  document.getElementById(id1).classList.toggle('sel', newVal === selVal);
  document.getElementById(id2).classList.toggle('sel', newVal !== selVal);
}
function selectImportMode(mode) {
  G.importMode = mode;
  selectOpt('iopt-merge', 'iopt-replace', 'merge', mode);
}
function triggerImportFile() {
  document.getElementById('import-file').value = '';
  document.getElementById('import-file').click();
}
function harEntryToExchange(e) {
  const id = (typeof crypto.randomUUID === 'function') ? crypto.randomUUID() : Math.random().toString(36).slice(2);
  let host = '';
  try { host = new URL(e.request.url).host; } catch {}
  return {
    id,
    timestamp: e.startedDateTime || new Date().toISOString(),
    request: {
      method: e.request.method || 'GET',
      uri: e.request.url || '',
      headers: Object.fromEntries((e.request.headers||[]).map(h=>[h.name,h.value])),
      body: e.request.postData?.text || '',
      host,
      body_bytes: null,
    },
    response: (e.response && e.response.status) ? {
      status: e.response.status,
      headers: Object.fromEntries((e.response.headers||[]).map(h=>[h.name,h.value])),
      body: e.response.content?.text || '',
      request_uri: e.request.url || '',
      session_id: null,
      ttfb_ms: 0,
      body_ms: 0,
      body_bytes: null,
    } : null,
    metrics: (e.response && e.response.status) ? {
      latency_ms: Math.round(e.time || 0),
      request_size_bytes: Math.max(0, e.request.bodySize || 0),
      response_size_bytes: Math.max(0, e.response.content?.size || e.response.bodySize || 0),
      status_code: e.response.status,
      ttfb_ms: Math.round(e.timings?.wait || 0),
      body_ms: Math.round(e.timings?.receive || 0),
    } : null,
    ws_frames: [],
  };
}
async function handleImportFile(input) {
  const file = input.files[0];
  if (!file) return;
  closeModal('modal-import');
  let sessions;
  try {
    const text = await file.text();
    const parsed = JSON.parse(text);
    if (parsed?.log?.entries) {
      sessions = parsed.log.entries.map(harEntryToExchange);
    } else if (Array.isArray(parsed)) {
      // oproxy JSON export
      sessions = parsed;
    } else {
      toast('Unrecognised format — expected HAR or oproxy JSON array', true);
      return;
    }
  } catch (e) {
    toast('Failed to parse file: ' + e.message, true);
    return;
  }
  if (!sessions.length) { toast('No sessions found in file', true); return; }
  try {
    const res = await fetch('/admin/sessions/import', {
      method: 'POST',
      headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ sessions, merge: G.importMode === 'merge' }),
    });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    G.lastTs = null; // force full reload
    await fetchSessions();
    toast(`Imported ${data.imported} session${data.imported===1?'':'s'}`);
  } catch (e) {
    toast('Import failed: ' + e.message, true);
  }
}

// ── Navigation ────────────────────────────────────────────────────────────────
function syncChipBar() {
  const f = G.sideFil;
  // Pending chip: only useful in 'all' tab
  const pchip = document.getElementById('chip-pending');
  const psep  = document.getElementById('chip-sep-pending');
  const showPending = f === 'all';
  if (pchip) pchip.style.display = showPending ? '' : 'none';
  if (psep)  psep.style.display  = showPending ? '' : 'none';
  // Status chips: irrelevant in 'pending' tab (nothing has a status code)
  const showStatus = f !== 'pending';
  document.querySelectorAll('.chip[data-sc]:not(#chip-pending)').forEach(c => {
    c.style.display = showStatus ? '' : 'none';
  });
}
function navItem(el) {
  document.querySelectorAll('.nav-item').forEach(i=>i.classList.remove('active'));
  el.classList.add('active');
  G.sideFil = el.dataset.f||'all';
  syncChipBar();
  nav(el.dataset.v||'traffic');
  // Re-render immediately so filter applies to already-loaded sessions
  // (fetchSessions skips renderTable when there are no new sessions since lastTs)
  if ((el.dataset.v||'traffic')==='traffic' && G.sessions.length) {
    if (G.treeMode) renderTree(); else renderTable();
  }
}
function nav(v) {
  document.querySelectorAll('.view').forEach(e=>e.style.display='none');
  const el = document.getElementById('v-'+v);
  if (el) el.style.display = (v==='traffic'||v==='compose')?'flex':'block';
  document.getElementById('detail').className = 'detail'+(v==='traffic'?'':' hide');
  G.view = v;
  if (v==='traffic')      fetchSessions();
  else if (v==='mapping') { fetchRoutes(); fetchHeaderMaps(); }
  else if (v==='dns')     fetchDns();
  else if (v==='rewrites') { fetchRewrites(); fetchModifications(); }
  else if (v==='throttle') fetchThrottle();
  else if (v==='capture')  fetchCaptureFilter();
  else if (v==='breakpoints') fetchBreakpoints();
  else if (v==='compose') renderCmpCollections();
  else if (v==='mock')     fetchMockRules();
  else if (v==='scripts')  fetchScripts();
  else if (v==='webhooks') fetchWebhooks();
  else if (v==='settings') { fetchConfig(); fetchUpstreamProxy(); fetchSocks5Status(); loadMetrics(); }
}

// ── Record ────────────────────────────────────────────────────────────────────
function toggleRec() {
  G.rec=!G.rec;
  const btn=document.getElementById('rec-btn'), lbl=document.getElementById('rec-lbl');
  if (G.rec) { btn.className='btn btn-stop'; lbl.textContent='Stop'; startPoll(); }
  else       { btn.className='btn btn-primary'; lbl.textContent='Record'; stopPoll(); }
}
function startPoll() {
  // Use SSE for session updates; keep a short poll only for breakpoints.
  if (!G.sse) {
    G.sse = new EventSource('/api/sessions/stream');
    let _sseTick; G.sse.onmessage = () => { if (G.view!=='traffic') return; clearTimeout(_sseTick); _sseTick = setTimeout(fetchSessions, 80); };
    G.sse.onerror = () => {
      // SSE lost — fall back to polling until reconnected.
      if (!G.timer) G.timer = setInterval(()=>{ if (G.view==='traffic') fetchSessions(); }, 2000);
    };
    G.sse.onopen = () => { if (G.timer) { clearInterval(G.timer); G.timer=null; } };
  }
  // Breakpoints still need polling (no SSE channel for them).
  if (!G.bpTimer) G.bpTimer = setInterval(()=>{ if (G.view==='breakpoints') fetchBreakpoints(); }, 2000);
}
function stopPoll() {
  if (G.sse) { G.sse.close(); G.sse=null; }
  if (G.timer) { clearInterval(G.timer); G.timer=null; }
  if (G.bpTimer) { clearInterval(G.bpTimer); G.bpTimer=null; }
}

// ── Search & chips ────────────────────────────────────────────────────────────
function renderCurrent() { if (G.treeMode) renderTree(); else renderTable(); }
function onSearch(q) { G.srch=q; renderCurrent(); }
function toggleRegexSearch() {
  G.regexSearch = !G.regexSearch;
  const btn = document.getElementById('regex-btn');
  if (btn) btn.classList.toggle('on', G.regexSearch);
  renderCurrent();
}
function updateChipReset() {
  const scOff = ['2xx','3xx','4xx','5xx','pending'].some(s=>!G.scFil.has(s));
  const mOff  = ['GET','POST','PUT','DELETE','PATCH','OTHER'].some(m=>!G.mFil.has(m));
  document.getElementById('chip-reset').style.display = (scOff||mOff) ? '' : 'none';
}
function toggleChip(el) {
  const sc=el.dataset.sc;
  G.scFil.has(sc) ? (G.scFil.delete(sc), el.classList.remove('on')) : (G.scFil.add(sc), el.classList.add('on'));
  updateChipReset();
  renderCurrent();
}
function resetChips() {
  ['2xx','3xx','4xx','5xx','pending'].forEach(sc => {
    G.scFil.add(sc);
    const el = document.querySelector(`.chip[data-sc="${sc}"]`);
    if (el) el.classList.add('on');
  });
  ['GET','POST','PUT','DELETE','PATCH','OTHER'].forEach(m => {
    G.mFil.add(m);
    const el = document.querySelector(`.chip[data-mc="${m}"]`);
    if (el) el.classList.add('on');
  });
  updateChipReset();
  renderCurrent();
}
function toggleMethodChip(el) {
  const m = el.dataset.mc;
  G.mFil.has(m) ? (G.mFil.delete(m), el.classList.remove('on')) : (G.mFil.add(m), el.classList.add('on'));
  updateChipReset();
  renderCurrent();
}

// ── Clear ─────────────────────────────────────────────────────────────────────
async function clearSessions() {
  if (!confirm('Clear all captured traffic?')) return;
  try {
    await fetch('/admin/sessions',{method:'DELETE'});
    G.sessions=[]; G.sel=null; G.sessionSig=''; G.lastTs=null; G.treeExpanded.clear(); G.treeClosed.clear();
    renderCurrent(); updateBadges(); renderDetail();
    document.getElementById('detail-topbar').style.display='none';
    toast('Sessions cleared');
  } catch { toast('Failed to clear',true); }
}


// ── Resize handle ─────────────────────────────────────────────────────────────
(()=>{
  const h=document.getElementById('resize-handle'), p=document.getElementById('detail');
  let drag=false, sx=0, sw=0;
  h.addEventListener('mousedown', e=>{drag=true;sx=e.clientX;sw=p.offsetWidth;h.classList.add('drag');e.preventDefault()});
  document.addEventListener('mousemove', e=>{if(!drag)return; p.style.width=Math.max(240,Math.min(640,sw-(e.clientX-sx)))+'px'});
  document.addEventListener('mouseup', ()=>{drag=false;h.classList.remove('drag')});
})();

// ── Keyboard shortcuts ────────────────────────────────────────────────────────
document.addEventListener('keydown', e => {
  const inInput = e.target.tagName==='INPUT'||e.target.tagName==='TEXTAREA'||e.target.isContentEditable;
  if (e.key==='Escape') {
    document.querySelectorAll('.overlay.show').forEach(m=>m.classList.remove('show'));
    if (!inInput) { G.sel=null; renderTable(); renderDetail(); document.getElementById('sb-sel').textContent='No selection'; document.getElementById('detail-topbar').style.display='none'; }
    return;
  }
  if (inInput) return;
  if (e.key==='?') { openShortcuts(); return; }
  if ((e.metaKey||e.ctrlKey) && e.key==='d') { e.preventDefault(); toggleTheme(); }
  if ((e.metaKey||e.ctrlKey) && e.key==='f') { e.preventDefault(); document.getElementById('srch').focus(); }
  if ((e.metaKey||e.ctrlKey) && e.key==='k') { e.preventDefault(); clearSessions(); }
  if ((e.metaKey||e.ctrlKey) && e.key==='r') { e.preventDefault(); if (G.sel) goCompose(); }
  if ((e.metaKey||e.ctrlKey) && e.key==='c' && G.sel) { e.preventDefault(); copyCurl(); }
  if (e.key===' ') { e.preventDefault(); toggleRec(); }
});

// ── Boot ──────────────────────────────────────────────────────────────────────
(async () => {
  const saved = localStorage.getItem('oproxy-theme');
  if (saved === 'dark') {
    document.documentElement.setAttribute('data-theme','dark');
    document.getElementById('ic-moon').style.display='none';
    document.getElementById('ic-sun').style.display='';
  }

  // Restore compose collections and vars
  try { const sc=localStorage.getItem('oproxy-compose-collections'); if(sc) G.compose.collections=JSON.parse(sc); } catch {}
  try { const sv=localStorage.getItem('oproxy-compose-vars'); if(sv) G.compose.vars=JSON.parse(sv); } catch {}

  // Detect proxy not running
  try {
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), 3000);
    await fetch('/health', { signal: ctrl.signal });
    clearTimeout(t);
  } catch {
    const el = document.getElementById('proxy-offline');
    if (el) el.style.display = 'flex';
    return;
  }

  // Hydrate MITM indicators from /health
  try {
    const d = await fetch('/health').then(r=>r.json());
    const on = !!d.mitm_enabled;
    const badge = document.getElementById('tb-mitm');
    badge.className = 'mitm-badge ' + (on ? 'mitm-on' : 'mitm-off');
    document.getElementById('tb-mitm-lbl').textContent = on ? 'MITM on' : 'MITM off';
    const mv = document.getElementById('mitm-val');
    if (mv) mv.textContent = on ? 'Intercept (MITM)' : 'Passthrough (tunnelled)';
  } catch {}

  try { const r=await fetch('/admin/routes'); G.routes=await r.json(); } catch {}
  await fetchSessions();
  startPoll();
})();

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(()=>{});
}

// ── Metrics (M5) ─────────────────────────────────────────────────────────────
async function loadMetrics() {
  try {
    const d = await fetch('/admin/metrics').then(r=>r.json());
    const el = document.getElementById('metrics-body');
    if (!el) return;
    const latencies = (d.latency_samples||[]).slice().sort((a,b)=>a-b);
    const pct = (p) => {
      if (!latencies.length) return '—';
      const i = Math.ceil(p/100*latencies.length)-1;
      return fmtMs(latencies[Math.max(0,i)]);
    };
    const total = d.total_requests || 0;
    const errors = d.error_count || 0;
    const errRate = total ? ((errors/total)*100).toFixed(1)+'%' : '—';
    el.innerHTML = `
      <div class="kv"><span class="kk">Total Requests</span><span class="kv-val kv-mono">${total}</span></div>
      <div class="kv"><span class="kk">Error Count</span><span class="kv-val kv-mono" style="color:${errors?'var(--danger)':'inherit'}">${errors}</span></div>
      <div class="kv"><span class="kk">Error Rate</span><span class="kv-val kv-mono">${errRate}</span></div>
      <div class="kv"><span class="kk">Avg Latency</span><span class="kv-val kv-mono">${fmtMs(d.avg_latency_ms)}</span></div>
      <div class="kv"><span class="kk">p50</span><span class="kv-val kv-mono">${pct(50)}</span></div>
      <div class="kv"><span class="kk">p95</span><span class="kv-val kv-mono">${pct(95)}</span></div>
      <div class="kv"><span class="kk">p99</span><span class="kv-val kv-mono">${pct(99)}</span></div>
      <div class="kv"><span class="kk">Avg Req Size</span><span class="kv-val kv-mono">${fmtSz(d.avg_request_size_bytes)}</span></div>
      <div class="kv"><span class="kk">Avg Res Size</span><span class="kv-val kv-mono">${fmtSz(d.avg_response_size_bytes)}</span></div>`;
  } catch { toast('Failed to load metrics', true); }
}

// ── Playback (M6) ─────────────────────────────────────────────────────────────
async function startPlayback() {
  const file = document.getElementById('playback-file')?.value.trim();
  const target = document.getElementById('playback-target')?.value.trim();
  if (!file||!target) { toast('File and target URL required', true); return; }
  try {
    const r = await fetch('/admin/playback', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ file, target }),
    });
    if (!r.ok) throw new Error(await r.text());
    toast('Playback started');
  } catch(e) { toast('Playback failed: '+(e.message||e), true); }
}

// ── Bulk select (F9) ──────────────────────────────────────────────────────────
G._bulkSel = new Set();
function toggleBulkRow(id, chk) {
  chk ? G._bulkSel.add(id) : G._bulkSel.delete(id);
  const bar = document.getElementById('bulk-bar');
  if (bar) {
    bar.style.display = G._bulkSel.size ? 'flex' : 'none';
    const lbl = document.getElementById('bulk-count');
    if (lbl) lbl.textContent = G._bulkSel.size + ' selected';
  }
}
function clearBulkSel() {
  G._bulkSel.clear();
  document.querySelectorAll('.row-chk').forEach(c=>c.checked=false);
  const bar = document.getElementById('bulk-bar');
  if (bar) bar.style.display = 'none';
}
async function bulkDelete() {
  if (!G._bulkSel.size) return;
  if (!confirm(`Delete ${G._bulkSel.size} session(s)?`)) return;
  try {
    await Promise.all([...G._bulkSel].map(id =>
      fetch(`/api/sessions/${id}`, { method: 'DELETE' })
    ));
    G.sessions = G.sessions.filter(s => !G._bulkSel.has(s.id));
    if (G.sel && G._bulkSel.has(G.sel.id)) { G.sel = null; renderDetail(); }
    G._bulkSel.clear();
    const bar = document.getElementById('bulk-bar');
    if (bar) bar.style.display = 'none';
    renderCurrent(); updateBadges();
    toast('Deleted');
  } catch { toast('Delete failed', true); }
}

// ── Body find (F10) ───────────────────────────────────────────────────────────
let _bodyFindOpen = false;
function openBodyFind() {
  _bodyFindOpen = true;
  const bar = document.getElementById('body-find-bar');
  if (bar) { bar.style.display = 'flex'; document.getElementById('body-find-input').focus(); }
}
function closeBodyFind() {
  _bodyFindOpen = false;
  const bar = document.getElementById('body-find-bar');
  if (bar) bar.style.display = 'none';
  clearBodyFindHighlights();
}
function onBodyFind(q) {
  clearBodyFindHighlights();
  if (!q.trim()) return;
  const pre = document.querySelector('#dbody .body-pre');
  if (!pre) return;
  const raw = pre.textContent;
  if (!raw.includes(q)) return;
  const re = new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g,'\\$&'), 'gi');
  pre.innerHTML = raw.replace(re, m => `<mark class="body-hl">${esc(m)}</mark>`);
  const first = pre.querySelector('.body-hl');
  if (first) first.scrollIntoView({ block: 'nearest' });
}
function clearBodyFindHighlights() {
  const pre = document.querySelector('#dbody .body-pre');
  if (!pre) return;
  pre.innerHTML = pre.textContent; // strip marks; re-highlight if needed via renderDetail
}

// Ctrl+F inside detail panel → open body find
document.addEventListener('keydown', e => {
  const inDetail = document.getElementById('detail')?.contains(e.target) ||
                   (!e.target.matches('input,textarea,select'));
  if ((e.metaKey||e.ctrlKey) && e.key==='f' && G.sel && (G.dtab==='response'||G.dtab==='request')) {
    e.preventDefault();
    openBodyFind();
  }
  if (e.key==='Escape' && _bodyFindOpen) {
    e.stopPropagation();
    closeBodyFind();
  }
});
