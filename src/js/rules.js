// ── Routes ────────────────────────────────────────────────────────────────────
async function fetchRoutes() {
  try { const r=await fetch('/admin/routes'); G.routes=await r.json(); renderRoutes(); }
  catch { toast('Failed to load routes',true); }
}
function renderRoutes() {
  const tb=document.getElementById('rt-tbody'), e=Object.entries(G.routes);
  if (!e.length) { tb.innerHTML=`<tr><td colspan="3" style="text-align:center;color:var(--text4);padding:20px">No route mappings</td></tr>`; return; }
  tb.innerHTML=e.map(([s,d])=>`<tr><td class="mono">${esc(s)}</td><td class="mono">${esc(d)}</td><td><button class="btn btn-sm btn-danger" onclick="delRoute('${esc(s)}')">Remove</button></td></tr>`).join('');
}
async function addRoute() {
  const src=document.getElementById('rt-src').value.trim(), dst=document.getElementById('rt-dst').value.trim();
  if (!src||!dst){toast('Both fields required',true);return;}
  const u={...G.routes,[src]:dst};
  try {
    await fetch('/admin/routes',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(u)});
    G.routes=u; document.getElementById('rt-src').value=''; document.getElementById('rt-dst').value='';
    renderRoutes(); toast('Route added');
  } catch { toast('Failed to save',true); }
}
async function delRoute(src) {
  const u={...G.routes}; delete u[src];
  try {
    await fetch('/admin/routes',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(u)});
    G.routes=u; renderRoutes(); toast('Route removed');
  } catch { toast('Failed to remove',true); }
}

// ── DNS Override ─────────────────────────────────────────────────────────────
async function fetchDns() {
  try { const r=await fetch('/admin/dns'); G.dns=await r.json(); renderDns(); }
  catch { toast('Failed to load DNS overrides',true); }
}
function renderDns() {
  const tb=document.getElementById('dns-tbody'), e=Object.entries(G.dns||{});
  if (!e.length) { tb.innerHTML=`<tr><td colspan="3" style="text-align:center;color:var(--text4);padding:20px">No DNS overrides</td></tr>`; return; }
  tb.innerHTML=e.map(([h,ip])=>`<tr><td class="mono">${esc(h)}</td><td class="mono">${esc(ip)}</td><td><button class="btn btn-sm btn-danger" onclick="delDns('${esc(h)}')">Remove</button></td></tr>`).join('');
}
async function addDns() {
  const host=document.getElementById('dns-host').value.trim(), ip=document.getElementById('dns-ip').value.trim();
  if (!host||!ip){toast('Both fields required',true);return;}
  const u={...G.dns,[host]:ip};
  try {
    await fetch('/admin/dns',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(u)});
    G.dns=u; document.getElementById('dns-host').value=''; document.getElementById('dns-ip').value='';
    renderDns(); toast('DNS override added');
  } catch { toast('Failed to save',true); }
}
async function delDns(host) {
  try {
    await fetch(`/admin/dns/${encodeURIComponent(host)}`,{method:'DELETE'});
    delete G.dns[host]; renderDns(); toast('DNS override removed');
  } catch { toast('Failed to remove',true); }
}

// ── Rewrites ──────────────────────────────────────────────────────────────────
async function fetchRewrites() {
  try { const r=await fetch('/admin/rewrites'); G.rewrites=await r.json(); renderRewrites(); }
  catch { toast('Failed to load rewrites',true); }
}
function descCriteria(c) {
  if (!c) return '—';
  const [k,v]=Object.entries(c)[0]||[];
  if (k==='Header') return `Header(${v?.name})=${v?.value}`;
  return `${k}: ${typeof v==='string'?v:JSON.stringify(v)}`;
}
function descAction(a) {
  if (!a) return '—';
  const [k,v]=Object.entries(a)[0]||[];
  if (k==='ReplaceBody') return `Replace /${v?.pattern}/→${v?.replacement}`;
  if (k==='AddHeader') return `Add ${v?.name}: ${v?.value}`;
  if (k==='RemoveHeader') return `Remove ${v?.name}`;
  if (k==='ReplaceHeader') return `Replace hdr ${v?.name}`;
  if (k==='Redirect') return `Redirect ${v?.status} → ${v?.location}`;
  if (k==='Block') return `Block ${v?.status}`;
  return k;
}
function renderRewrites() {
  const tb=document.getElementById('rw-tbody');
  if (!G.rewrites.length) { tb.innerHTML=`<tr><td colspan="7" style="text-align:center;color:var(--text4);padding:20px">No rewrite rules — click + Add Rule</td></tr>`; return; }
  const n=G.rewrites.length;
  tb.innerHTML=G.rewrites.map((r,i)=>`<tr>
    <td>${esc(r.name)}</td>
    <td class="mono" style="color:var(--text3);font-size:11px">${esc(descCriteria(r.criteria))}</td>
    <td style="color:var(--text3);font-size:11px">${esc(r.scope||'Both')}</td>
    <td style="color:var(--text3);font-size:11px">${esc(descAction(r.action))}</td>
    <td>
      <div class="toggle-wrap" onclick="toggleRwEnabled(${i})" role="switch" aria-checked="${!!r.enabled}" aria-label="Enable rule ${esc(r.name)}">
        <div class="toggle${r.enabled!==false?' on':''}"></div>
      </div>
    </td>
    <td>
      <div class="order-btns">
        <div class="order-btn${i===0?' disabled':''}" onclick="${i>0?`moveRw(${i},-1)`:''}" role="button" aria-label="Move up" aria-disabled="${i===0}">
          <svg width="8" height="6" viewBox="0 0 8 6"><path d="M1 5l3-4 3 4" stroke="currentColor" stroke-width="1.5" fill="none"/></svg>
        </div>
        <div class="order-btn${i===n-1?' disabled':''}" onclick="${i<n-1?`moveRw(${i},1)`:''}" role="button" aria-label="Move down" aria-disabled="${i===n-1}">
          <svg width="8" height="6" viewBox="0 0 8 6"><path d="M1 1l3 4 3-4" stroke="currentColor" stroke-width="1.5" fill="none"/></svg>
        </div>
      </div>
    </td>
    <td style="display:flex;gap:4px">
      <button class="btn btn-sm btn-ghost" onclick="editRw(${i})" aria-label="Edit rule">Edit</button>
      <button class="btn btn-sm btn-danger" onclick="delRw(${i})" aria-label="Delete rule">✕</button>
    </td>
  </tr>`).join('');
}
function toggleRwForm() {
  G.rwEditIdx=null;
  const f=document.getElementById('rw-form');
  const open=!f.style.display;
  if (open) { f.style.display='none'; } else {
    // Reset form for add mode
    document.getElementById('rw-name').value='';
    document.getElementById('rw-crit-v').value='';
    document.getElementById('rw-crit-sample').value='';
    document.getElementById('rw-crit-status').className='regex-status';
    document.getElementById('rw-crit-status').textContent='';
    document.getElementById('rw-act-pat').value='';
    document.getElementById('rw-act-rep').value='';
    document.getElementById('rw-act-hdr').value='';
    document.getElementById('rw-hdr-name').value='';
    document.getElementById('rw-scope').value='Both';
    document.getElementById('rw-crit').value='Path';
    document.getElementById('rw-action').value='ReplaceBody';
    document.getElementById('rw-save-btn').textContent='Save';
    onRwCritChange(); onRwActionChange();
    f.style.display='';
    document.getElementById('rw-name').focus();
  }
}
function cancelRwForm() {
  G.rwEditIdx=null;
  document.getElementById('rw-form').style.display='none';
}
function onRwCritChange() {
  const v=document.getElementById('rw-crit').value, isHdr=v==='Header', isHost=v==='Host';
  document.getElementById('rw-crit-a').querySelector('label').textContent=
    isHdr?'Header value pattern':isHost?'Hostname (contains)':v+' pattern (regex)';
  document.getElementById('rw-hdr-name-wrap').style.display=isHdr?'':'none';
  document.getElementById('rw-crit-tester').style.display=isHost?'none':'';
}
function onRwActionChange() {
  const v=document.getElementById('rw-action').value;
  const hasHdr=v==='AddHeader'||v==='RemoveHeader'||v==='ReplaceHeader';
  const hasPat=v!=='RemoveHeader'&&v!=='Redirect'&&v!=='Block';
  const hasRep=v==='ReplaceBody'||v==='ReplaceHeader';
  const hasLoc=v==='Redirect';
  const hasSt =v==='Redirect'||v==='Block';
  document.getElementById('rw-act-pat-wrap').style.display=hasPat?'':'none';
  document.getElementById('rw-act-rep-wrap').style.display=hasRep?'':'none';
  document.getElementById('rw-act-hdr-wrap').style.display=hasHdr?'':'none';
  document.getElementById('rw-act-loc-wrap').style.display=hasLoc?'':'none';
  document.getElementById('rw-act-status-wrap').style.display=hasSt?'':'none';
  if (v==='ReplaceBody') document.getElementById('rw-act-pat').placeholder='regex pattern';
  else if (v==='AddHeader') document.getElementById('rw-act-pat').placeholder='header value';
  if (v==='Redirect') { document.getElementById('rw-act-status').placeholder='301'; document.getElementById('rw-act-status').value=document.getElementById('rw-act-status').value||'301'; }
  if (v==='Block')    { document.getElementById('rw-act-status').placeholder='403'; document.getElementById('rw-act-status').value=document.getElementById('rw-act-status').value||'403'; }
}
function onRegexTest(patId, sampleId, statusId) {
  const pattern=document.getElementById(patId)?.value||'';
  const sample=document.getElementById(sampleId)?.value||'';
  const el=document.getElementById(statusId); if (!el) return;
  if (!pattern) { el.className='regex-status'; el.textContent=''; return; }
  try {
    const re=new RegExp(pattern);
    if (!sample) { el.className='regex-status no-match'; el.textContent='Enter sample to test'; return; }
    if (re.test(sample)) { el.className='regex-status match'; el.textContent='✓ Match'; }
    else { el.className='regex-status no-match'; el.textContent='No match'; }
  } catch(e) { el.className='regex-status error'; el.textContent='Invalid regex'; }
}
function buildRwRule() {
  const crit=document.getElementById('rw-crit').value, critVal=document.getElementById('rw-crit-v').value.trim();
  const act=document.getElementById('rw-action').value, pat=document.getElementById('rw-act-pat').value.trim();
  const rep=document.getElementById('rw-act-rep').value, hdrName=document.getElementById('rw-act-hdr').value.trim();
  const critHdrName=document.getElementById('rw-hdr-name').value.trim();
  const scope=document.getElementById('rw-scope').value;
  let criteria = crit==='Header' ? {Header:{name:critHdrName,value:critVal}} : {[crit]:critVal};
  let action;
  const loc=document.getElementById('rw-act-loc')?.value.trim()||'';
  const st=parseInt(document.getElementById('rw-act-status')?.value)||0;
  if (act==='ReplaceBody') action={ReplaceBody:{pattern:pat,replacement:rep}};
  else if (act==='AddHeader') action={AddHeader:{name:hdrName,value:pat}};
  else if (act==='RemoveHeader') action={RemoveHeader:{name:hdrName}};
  else if (act==='ReplaceHeader') action={ReplaceHeader:{name:hdrName,pattern:pat,replacement:rep}};
  else if (act==='Redirect') action={Redirect:{status:st||301,location:loc}};
  else if (act==='Block') action={Block:{status:st||403}};
  return {name:document.getElementById('rw-name').value.trim()||'Rule '+(G.rewrites.length+1),criteria,action,scope,enabled:true};
}
function editRw(i) {
  G.rwEditIdx=i;
  const r=G.rewrites[i];
  const f=document.getElementById('rw-form'); f.style.display='';
  document.getElementById('rw-name').value=r.name||'';
  document.getElementById('rw-scope').value=r.scope||'Both';
  const crit=Object.keys(r.criteria)[0];
  document.getElementById('rw-crit').value=crit;
  if (crit==='Header') {
    document.getElementById('rw-hdr-name').value=r.criteria.Header.name||'';
    document.getElementById('rw-crit-v').value=r.criteria.Header.value||'';
  } else {
    document.getElementById('rw-crit-v').value=r.criteria[crit]||'';
  }
  const act=Object.keys(r.action)[0];
  document.getElementById('rw-action').value=act;
  if (act==='ReplaceBody')   { document.getElementById('rw-act-pat').value=r.action.ReplaceBody.pattern||''; document.getElementById('rw-act-rep').value=r.action.ReplaceBody.replacement||''; }
  if (act==='AddHeader')     { document.getElementById('rw-act-hdr').value=r.action.AddHeader.name||''; document.getElementById('rw-act-pat').value=r.action.AddHeader.value||''; }
  if (act==='RemoveHeader')  { document.getElementById('rw-act-hdr').value=r.action.RemoveHeader.name||''; }
  if (act==='ReplaceHeader') { document.getElementById('rw-act-hdr').value=r.action.ReplaceHeader.name||''; document.getElementById('rw-act-pat').value=r.action.ReplaceHeader.pattern||''; document.getElementById('rw-act-rep').value=r.action.ReplaceHeader.replacement||''; }
  if (act==='Redirect') { document.getElementById('rw-act-loc').value=r.action.Redirect.location||''; document.getElementById('rw-act-status').value=r.action.Redirect.status||301; }
  if (act==='Block')    { document.getElementById('rw-act-status').value=r.action.Block.status||403; }
  document.getElementById('rw-save-btn').textContent='Update';
  onRwCritChange(); onRwActionChange();
  document.getElementById('rw-name').focus();
}
async function toggleRwEnabled(i) {
  const rule={...G.rewrites[i], enabled:!G.rewrites[i].enabled};
  try {
    await fetch(`/admin/rewrites/${i}`,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(rule)});
    G.rewrites[i]=rule; renderRewrites();
  } catch { toast('Failed to toggle rule',true); }
}
async function moveRw(i,dir) {
  const rules=[...G.rewrites], j=i+dir;
  if (j<0||j>=rules.length) return;
  [rules[i],rules[j]]=[rules[j],rules[i]];
  try {
    await fetch('/admin/rewrites/replace-all',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(rules)});
    G.rewrites=rules; renderRewrites();
  } catch { toast('Failed to reorder',true); }
}
async function saveRw() {
  const rule=buildRwRule();
  if (!rule.criteria||!rule.action){toast('Fill in all fields',true);return;}
  try {
    if (G.rwEditIdx!==null) {
      await fetch(`/admin/rewrites/${G.rwEditIdx}`,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(rule)});
      G.rewrites[G.rwEditIdx]=rule; G.rwEditIdx=null; toast('Rule updated');
    } else {
      await fetch('/admin/rewrites',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(rule)});
      G.rewrites.push(rule); toast('Rule added');
    }
    renderRewrites(); cancelRwForm();
  } catch { toast('Failed to save rule',true); }
}
async function delRw(i) {
  try {
    await fetch(`/admin/rewrites/${i}`,{method:'DELETE'});
    G.rewrites.splice(i,1); renderRewrites(); toast('Rule removed');
  } catch { toast('Failed to remove rule',true); }
}

// ── Header Map ────────────────────────────────────────────────────────────────
function setMappingTab(el) {
  document.querySelectorAll('.chip[data-mv]').forEach(c=>c.classList.remove('on'));
  el.classList.add('on');
  const mv=el.dataset.mv;
  document.getElementById('mv-host-remap').style.display=mv==='host-remap'?'':'none';
  document.getElementById('mv-header-map').style.display=mv==='header-map'?'':'none';
  document.getElementById('mv-map-local').style.display=mv==='map-local'?'':'none';
  if (mv==='map-local') fetchMapLocal();
}
function toggleHdrMapForm() {
  const f=document.getElementById('hdrmap-form'); f.style.display=f.style.display?'':'none';
}
function onHmScopeChange() {
  const v=document.getElementById('hm-scope').value;
  const wrap=document.getElementById('hm-match-wrap');
  wrap.style.display=v==='all'?'none':'';
  document.getElementById('hm-match-label').textContent=v==='host'?'Host pattern':'Path pattern (regex)';
}
function onHmActionChange() {
  const v=document.getElementById('hm-action').value;
  document.getElementById('hm-value-wrap').style.display=v==='Remove'?'none':'';
}
async function fetchHeaderMaps() {
  try {
    const r=await fetch('/admin/header-maps');
    if (!r.ok) { G.headerMaps=[]; return; }
    G.headerMaps=await r.json(); renderHdrMap();
  } catch { G.headerMaps=[]; }
}
function renderHdrMap() {
  const tb=document.getElementById('hdrmap-tbody');
  if (!tb) return;
  if (!G.headerMaps.length) { tb.innerHTML=`<tr><td colspan="6" style="text-align:center;color:var(--text4);padding:20px">No header map rules</td></tr>`; return; }
  tb.innerHTML=G.headerMaps.map(r=>`<tr>
    <td><span class="hdrmap-scope-chip">${esc(r.scope==='all'?'All':r.scope==='host'?'Host':'Path')}</span>${r.match?` <span style="font-size:11px;color:var(--text3)">${esc(r.match)}</span>`:''}</td>
    <td style="font-size:11px;font-weight:600;color:var(--${r.action==='Remove'?'danger':r.action==='Append'?'warning':'accent'})">${esc(r.action)}</td>
    <td class="mono" style="font-size:11px">${esc(r.name)}</td>
    <td style="font-size:11px;color:var(--text3)">${esc(r.value||'—')}</td>
    <td><div class="toggle-wrap" onclick="toggleHdrMapEnabled('${esc(r.id)}')" role="switch" aria-checked="${!!r.enabled}"><div class="toggle${r.enabled!==false?' on':''}"></div></div></td>
    <td><button class="btn btn-sm btn-danger" onclick="delHdrMapRule('${esc(r.id)}')">✕</button></td>
  </tr>`).join('');
}
async function saveHdrMapRule() {
  const scope=document.getElementById('hm-scope').value;
  const match=document.getElementById('hm-match').value.trim();
  const action=document.getElementById('hm-action').value;
  const name=document.getElementById('hm-name').value.trim();
  const value=document.getElementById('hm-value').value.trim();
  if (!name){toast('Header name required',true);return;}
  const rule={id:'',scope,match,action,name,value,enabled:true};
  try {
    const r=await fetch('/admin/header-maps',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(rule)});
    const saved=await r.json(); G.headerMaps.push(saved); renderHdrMap();
    document.getElementById('hm-name').value=''; document.getElementById('hm-value').value='';
    toggleHdrMapForm(); toast('Header map rule added');
  } catch { toast('Failed to save rule',true); }
}
async function delHdrMapRule(id) {
  try {
    await fetch(`/admin/header-maps/${id}`,{method:'DELETE'});
    G.headerMaps=G.headerMaps.filter(r=>r.id!==id); renderHdrMap(); toast('Rule removed');
  } catch { toast('Failed to remove rule',true); }
}
async function toggleHdrMapEnabled(id) {
  const r=G.headerMaps.find(x=>x.id===id); if (!r) return;
  const updated={...r,enabled:!r.enabled};
  try {
    await fetch(`/admin/header-maps/${id}`,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(updated)});
    Object.assign(r,updated); renderHdrMap();
  } catch { toast('Failed to toggle',true); }
}

// ── Modifications ─────────────────────────────────────────────────────────────
async function fetchModifications() {
  try {
    const r = await fetch('/admin/modifications');
    G.modifications = await r.json();
    renderModifications();
  } catch { toast('Failed to load modifications', true); }
}
function toggleModForm() {
  const f = document.getElementById('mod-form');
  const btn = document.getElementById('mod-add-btn');
  const open = f.style.display === 'none' || f.style.display === '';
  if (open) {
    f.style.display = 'block'; btn.textContent = '✕ Cancel';
    document.getElementById('mod-pattern').value = '';
    document.getElementById('mod-body').value = '';
    document.getElementById('mod-hdrs-tbody').innerHTML = '';
    modAddHdrRow();
  } else { f.style.display = 'none'; btn.textContent = '+ Add Rule'; }
}
function modAddHdrRow() {
  const tb = document.getElementById('mod-hdrs-tbody');
  const i = tb.rows.length;
  const tr = document.createElement('tr');
  tr.innerHTML = `<td><input class="cmp-kv-input" id="mod-hk-${i}" placeholder="Header-Name" style="width:100%"></td><td><input class="cmp-kv-input" id="mod-hv-${i}" placeholder="value" style="width:100%"></td><td><button class="cmp-kv-del" onclick="this.closest('tr').remove()">✕</button></td>`;
  tb.appendChild(tr);
}
async function saveMod() {
  const pattern = document.getElementById('mod-pattern').value.trim();
  if (!pattern) { toast('URI pattern is required', true); return; }
  const header_replacements = {};
  document.getElementById('mod-hdrs-tbody').querySelectorAll('tr').forEach(row => {
    const inputs = row.querySelectorAll('input');
    if (inputs.length >= 2) { const k = inputs[0].value.trim(), v = inputs[1].value.trim(); if (k) header_replacements[k] = v; }
  });
  const body = document.getElementById('mod-body').value.trim();
  const rule = { request_uri_pattern: pattern, header_replacements, body_replacement: body || null };
  try {
    await fetch('/admin/modifications', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(rule) });
    toggleModForm();
    toast('Modification rule added');
    await fetchModifications();
  } catch { toast('Failed to save rule', true); }
}
async function delMod(i) {
  try {
    await fetch(`/admin/modifications/${i}`, { method: 'DELETE' });
    toast('Rule removed');
    await fetchModifications();
  } catch { toast('Failed to remove rule', true); }
}
function renderModifications() {
  const tb = document.getElementById('mod-tbody'); if (!tb) return;
  if (!G.modifications.length) {
    tb.innerHTML = `<tr><td colspan="4" style="text-align:center;color:var(--text4);padding:20px">No modification rules — click + Add Rule</td></tr>`;
    return;
  }
  tb.innerHTML = G.modifications.map((r, i) => {
    const hdrs = Object.entries(r.header_replacements || {}).map(([k, v]) => `<span class="hdrmap-scope-chip" style="margin-right:3px">${esc(k)}: ${esc(v)}</span>`).join('');
    const body = r.body_replacement ? `<span style="font-family:var(--mono);font-size:10px;color:var(--text3)">${esc(r.body_replacement.slice(0, 40))}${r.body_replacement.length > 40 ? '…' : ''}</span>` : `<span style="color:var(--text4)">—</span>`;
    return `<tr>
      <td class="trunc" style="font-family:var(--mono);font-size:11px">${esc(r.request_uri_pattern)}</td>
      <td style="max-width:240px">${hdrs || '<span style="color:var(--text4)">—</span>'}</td>
      <td>${body}</td>
      <td><button class="btn btn-sm btn-danger" onclick="delMod(${i})">Delete</button></td>
    </tr>`;
  }).join('');
}

// ── Settings ──────────────────────────────────────────────────────────────────
async function fetchConfig() {
  try {
    const [cfgR, hlthR] = await Promise.all([fetch('/admin/config'), fetch('/health')]);
    const cfg = await cfgR.json();
    const set = (id, val) => { const el = document.getElementById(id); if (el) el.textContent = val; };
    set('cfg-port', cfg.port);
    set('cfg-bind', cfg.bind_host);
    set('cfg-mitm', cfg.mitm_enabled ? 'Enabled' : 'Disabled');
    set('cfg-uptime', fmtDuration(cfg.uptime_secs));
    set('cfg-storage', cfg.storage_path);
    set('cfg-max-sessions', cfg.max_sessions.toLocaleString());
    set('cfg-timeout', cfg.timeout_secs + 's');
    set('cfg-ws', cfg.inspect_ws_frames ? 'On' : 'Off');
    const inp = document.getElementById('cfg-max-body');
    if (inp && !inp._dirty) inp.value = cfg.max_body_bytes;
  } catch { toast('Failed to load config', true); }
}
function fmtDuration(secs) {
  if (secs < 60) return secs + 's';
  if (secs < 3600) return Math.floor(secs/60) + 'm ' + (secs%60) + 's';
  return Math.floor(secs/3600) + 'h ' + Math.floor((secs%3600)/60) + 'm';
}
async function applyMaxBody() {
  const v = parseInt(document.getElementById('cfg-max-body').value);
  if (!v || v < 1024) { toast('Must be ≥ 1024 bytes', true); return; }
  try {
    const r = await fetch('/admin/config/reload', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ max_body_bytes: v }) });
    const d = await r.json();
    toast('max_body_bytes → ' + d.max_body_bytes.toLocaleString());
    document.getElementById('cfg-max-body')._dirty = false;
    await fetchConfig();
  } catch { toast('Failed to apply', true); }
}

// ── Throttle ──────────────────────────────────────────────────────────────────
const THROTTLE_PRESETS = {
  slow3g: {enabled:true, latency_ms:400, bandwidth_limit_kbps:400},
  fast3g: {enabled:true, latency_ms:150, bandwidth_limit_kbps:1500},
  '4g':   {enabled:true, latency_ms:50,  bandwidth_limit_kbps:20000},
};
function applyThrottlePreset(key) {
  const p = THROTTLE_PRESETS[key];
  if (!p) return;
  document.getElementById('th-en').value = 'true';
  document.getElementById('th-lat').value = p.latency_ms;
  document.getElementById('th-bw').value = p.bandwidth_limit_kbps;
}
async function fetchThrottle() {
  try {
    const r=await fetch('/admin/throttling'); const d=await r.json();
    G.throttle=d;
    document.getElementById('th-en').value=d.enabled?'true':'false';
    document.getElementById('th-lat').value=d.latency_ms||0;
    document.getElementById('th-bw').value=d.bandwidth_limit_kbps||0;
  } catch {}
}
async function saveThrottle() {
  const cfg={
    enabled:document.getElementById('th-en').value==='true',
    latency_ms:parseInt(document.getElementById('th-lat').value)||0,
    bandwidth_limit_kbps:parseInt(document.getElementById('th-bw').value)||0,
  };
  try {
    await fetch('/admin/throttling',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(cfg)});
    toast('Throttle applied');
  } catch { toast('Failed to apply throttle',true); }
}

// ── Capture Filter ────────────────────────────────────────────────────────────
async function fetchCaptureFilter() {
  try {
    const r = await fetch('/admin/capture-filter');
    const d = await r.json();
    G.captureFilter = d;
    document.getElementById('cf-mode').value = d.mode || 'disabled';
    renderCfHosts();
    onCfModeChange();
  } catch { toast('Failed to load capture filter', true); }
}
function renderCfHosts() {
  const hosts = (G.captureFilter && G.captureFilter.hosts) || [];
  const tbody = document.getElementById('cf-tbody');
  const table = document.getElementById('cf-hosts-table');
  if (!tbody) return;
  if (!hosts.length) {
    tbody.innerHTML = `<tr><td colspan="2" style="text-align:center;color:var(--text4);padding:14px">No hostname patterns</td></tr>`;
  } else {
    tbody.innerHTML = hosts.map(h => `<tr><td class="mono">${esc(h)}</td><td><button class="btn btn-sm btn-danger" onclick="removeCfHost('${esc(h)}')">Remove</button></td></tr>`).join('');
  }
  if (table) table.style.display = '';
}
function onCfModeChange() {
  const mode = document.getElementById('cf-mode')?.value;
  const wrap = document.getElementById('cf-hosts-wrap');
  const table = document.getElementById('cf-hosts-table');
  const note = document.getElementById('cf-note');
  const isActive = mode === 'allowlist' || mode === 'denylist';
  if (wrap) wrap.style.display = isActive ? '' : 'none';
  if (table) table.style.display = isActive ? '' : 'none';
  if (note) note.style.display = isActive ? '' : 'none';
}
async function saveCaptureFilter() {
  const mode = document.getElementById('cf-mode').value;
  const hosts = (G.captureFilter && G.captureFilter.hosts) || [];
  const cfg = { mode, hosts };
  try {
    await fetch('/admin/capture-filter', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(cfg) });
    G.captureFilter = cfg;
    onCfModeChange();
    renderCfHosts();
    toast('Capture filter saved');
  } catch { toast('Failed to save', true); }
}
async function addCfHost() {
  const host = document.getElementById('cf-host').value.trim();
  if (!host) { toast('Enter a hostname pattern', true); return; }
  const hosts = [...((G.captureFilter && G.captureFilter.hosts) || [])];
  if (hosts.includes(host)) { toast('Already in list', true); return; }
  hosts.push(host);
  const cfg = { mode: document.getElementById('cf-mode').value, hosts };
  try {
    await fetch('/admin/capture-filter', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(cfg) });
    G.captureFilter = cfg;
    document.getElementById('cf-host').value = '';
    renderCfHosts();
    toast('Host added');
  } catch { toast('Failed to add host', true); }
}
async function removeCfHost(host) {
  const hosts = ((G.captureFilter && G.captureFilter.hosts) || []).filter(h => h !== host);
  const cfg = { mode: document.getElementById('cf-mode').value, hosts };
  try {
    await fetch('/admin/capture-filter', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(cfg) });
    G.captureFilter = cfg;
    renderCfHosts();
    toast('Host removed');
  } catch { toast('Failed to remove host', true); }
}

// ── Map Local ─────────────────────────────────────────────────────────────────
async function fetchMapLocal() {
  try {
    const r = await fetch('/admin/map-local');
    G.mapLocal = await r.json();
    renderMapLocal();
  } catch { toast('Failed to load map-local rules', true); }
}
function renderMapLocal() {
  const tb = document.getElementById('ml-tbody');
  if (!tb) return;
  const entries = Object.entries(G.mapLocal || {});
  if (!entries.length) { tb.innerHTML = `<tr><td colspan="3" style="text-align:center;color:var(--text4);padding:14px">No rules</td></tr>`; return; }
  tb.innerHTML = entries.map(([host, fp]) => `<tr>
    <td><code>${esc(host)}</code></td>
    <td><code style="color:var(--text3)">${esc(fp)}</code></td>
    <td><button class="btn btn-sm btn-danger" onclick="delMapLocal('${esc(host)}')">✕</button></td>
  </tr>`).join('');
}
async function addMapLocal() {
  const host = document.getElementById('ml-host').value.trim();
  const fp   = document.getElementById('ml-path').value.trim();
  if (!host || !fp) { toast('Fill host and file path', true); return; }
  try {
    await fetch('/admin/map-local', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({host, file_path: fp})});
    document.getElementById('ml-host').value = '';
    document.getElementById('ml-path').value = '';
    await fetchMapLocal();
    toast('Rule added');
  } catch { toast('Failed to add rule', true); }
}
async function delMapLocal(host) {
  try {
    await fetch(`/admin/map-local/${encodeURIComponent(host)}`, {method:'DELETE'});
    await fetchMapLocal();
    toast('Rule removed');
  } catch { toast('Failed to remove rule', true); }
}

// ── Mock Server ────────────────────────────────────────────────────────────────
let _mockFormOpen = false;
let _mockEditId   = null;

async function fetchMockRules() {
  try {
    const r = await fetch('/admin/mock/rules');
    G.mockRules = await r.json();
    renderMockRules();
  } catch { toast('Failed to load mock rules', true); }
}

function renderMockRules() {
  const tb = document.getElementById('mock-tbody');
  if (!tb) return;
  if (!G.mockRules.length) {
    tb.innerHTML = `<tr><td colspan="8" style="text-align:center;color:var(--text4);padding:20px">No mock rules — click + Add Rule</td></tr>`;
    return;
  }
  tb.innerHTML = G.mockRules.map(r => `<tr>
    <td>${esc(r.name)}</td>
    <td class="mono">${r.method ? esc(r.method) : '<span style="color:var(--text4)">any</span>'}</td>
    <td class="mono" style="color:var(--text3);font-size:11px;max-width:100px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(r.host||'')}">${r.host ? esc(r.host) : '<span style="color:var(--text4)">any</span>'}</td>
    <td class="mono" style="color:var(--text3);font-size:11px;max-width:120px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(r.path_pattern)}</td>
    <td style="color:var(--text3);font-size:11.5px">${(r.responses||[]).length}</td>
    <td style="color:var(--text3);font-size:11.5px">${r.call_count||0} <button class="btn btn-sm btn-ghost" style="padding:1px 5px;font-size:10px" onclick="resetMockCallCount('${esc(r.id)}')">↺</button></td>
    <td>
      <div class="toggle-wrap" onclick="toggleMockEnabled('${esc(r.id)}',${!r.enabled})">
        <div class="toggle${r.enabled?' on':''}"></div>
      </div>
    </td>
    <td style="display:flex;gap:4px">
      <button class="btn btn-sm btn-ghost" onclick="editMockRule('${esc(r.id)}')">Edit</button>
      <button class="btn btn-sm btn-danger" onclick="deleteMockRule('${esc(r.id)}')">Delete</button>
    </td>
  </tr>`).join('');
}

function openMockForm() {
  _mockFormOpen = true; _mockEditId = null;
  clearMockForm();
  document.getElementById('mock-form').style.display = '';
  document.getElementById('mock-add-btn').textContent = '✕ Cancel';
}

function closeMockForm() {
  _mockFormOpen = false; _mockEditId = null;
  document.getElementById('mock-form').style.display = 'none';
  document.getElementById('mock-add-btn').textContent = '+ Add Rule';
}

function clearMockForm() {
  ['mock-name','mock-method','mock-path','mock-host'].forEach(id => { const el=document.getElementById(id); if(el) el.value=''; });
  renderMockResponses([{status:200,delay_ms:0,body:'',headers:{}}]);
}

// ── F14: path regex tester ────────────────────────────────────────────────────
function onRegexTest(patternId, sampleId, statusId) {
  const pattern = document.getElementById(patternId)?.value || '';
  const sample  = document.getElementById(sampleId)?.value  || '';
  const out     = document.getElementById(statusId);
  if (!out) return;
  if (!pattern) { out.textContent=''; return; }
  try {
    const re = new RegExp(pattern);
    const match = re.test(sample);
    out.textContent = sample ? (match ? '✓ Match' : '✗ No match') : 'enter sample to test';
    out.style.color = sample ? (match ? 'var(--success)' : 'var(--danger)') : 'var(--text4)';
  } catch(e) {
    out.textContent = '⚠ ' + (e.message.split(':')[0] || 'Invalid regex');
    out.style.color = 'var(--warning)';
  }
}

// ── F3: multi-response UI ─────────────────────────────────────────────────────
function renderMockResponses(responses) {
  const wrap = document.getElementById('mock-responses-wrap');
  if (!wrap) return;
  wrap.innerHTML = '';
  (responses||[{status:200,delay_ms:0,body:'',headers:{}}]).forEach((res, i) => {
    const card = document.createElement('div');
    card.className = 'mock-res-card';
    card.dataset.idx = i;
    card.innerHTML = `
      <div style="display:flex;align-items:center;gap:6px;margin-bottom:6px">
        <span style="font-size:11px;color:var(--text3);font-weight:600">Response ${i+1}</span>
        <button class="btn btn-sm btn-danger" style="padding:1px 6px;margin-left:auto" onclick="removeMockResponse(${i})">✕</button>
      </div>
      <div class="form-row">
        <div class="fgroup" style="max-width:80px">
          <label class="flabel">Status</label>
          <input class="finput mock-res-status" type="number" value="${res.status||200}" min="100" max="599">
        </div>
        <div class="fgroup" style="max-width:80px">
          <label class="flabel">Delay (ms)</label>
          <input class="finput mock-res-delay" type="number" value="${res.delay_ms||0}" min="0">
        </div>
        <div class="fgroup">
          <label class="flabel">Body</label>
          <input class="finput mock-res-body" value="${esc(res.body||'')}" placeholder='{"message":"ok"}'>
        </div>
      </div>
      <div class="fgroup">
        <label class="flabel">Headers <button class="btn btn-ghost btn-sm" style="margin-left:6px;padding:1px 6px" onclick="addMockResHeaderRow(${i})">+ Add</button></label>
        <div class="mock-res-headers" style="display:flex;flex-direction:column;gap:4px;margin-top:4px">${
          Object.entries(res.headers||{}).map(([k,v])=>`<div style="display:flex;gap:4px;align-items:center"><input class="finput" style="flex:1" placeholder="Header-Name" value="${esc(k)}"><input class="finput" style="flex:2" placeholder="value" value="${esc(v)}"><button class="btn btn-sm btn-danger" onclick="this.parentElement.remove()">✕</button></div>`).join('')
        }</div>
      </div>`;
    wrap.appendChild(card);
  });
}
function addMockResponse() {
  const cards = document.querySelectorAll('#mock-responses-wrap .mock-res-card');
  const res = getMockResponses();
  res.push({status:200,delay_ms:0,body:'',headers:{}});
  renderMockResponses(res);
}
function removeMockResponse(i) {
  const res = getMockResponses();
  if (res.length <= 1) { toast('At least one response required', true); return; }
  res.splice(i, 1);
  renderMockResponses(res);
}
function addMockResHeaderRow(cardIdx) {
  const cards = document.querySelectorAll('#mock-responses-wrap .mock-res-card');
  const wrap = cards[cardIdx]?.querySelector('.mock-res-headers');
  if (!wrap) return;
  const row = document.createElement('div');
  row.style.cssText = 'display:flex;gap:4px;align-items:center';
  row.innerHTML = `<input class="finput" style="flex:1" placeholder="Header-Name"><input class="finput" style="flex:2" placeholder="value"><button class="btn btn-sm btn-danger" onclick="this.parentElement.remove()">✕</button>`;
  wrap.appendChild(row);
}
function getMockResponses() {
  const cards = document.querySelectorAll('#mock-responses-wrap .mock-res-card');
  return Array.from(cards).map(card => {
    const status  = parseInt(card.querySelector('.mock-res-status')?.value) || 200;
    const delay_ms= parseInt(card.querySelector('.mock-res-delay')?.value)  || 0;
    const body    = card.querySelector('.mock-res-body')?.value || '';
    const hrows   = card.querySelectorAll('.mock-res-headers > div');
    const headers = {};
    hrows.forEach(row => {
      const [k,v] = row.querySelectorAll('input');
      if (k?.value.trim()) headers[k.value.trim()] = v?.value || '';
    });
    return { status, delay_ms, body, headers };
  });
}
function addMockHeaderRow(name='', value='') {
  const wrap = document.getElementById('mock-headers-wrap');
  if (!wrap) return;
  const row = document.createElement('div');
  row.style.cssText = 'display:flex;gap:4px;align-items:center';
  row.innerHTML = `<input class="finput" style="flex:1" placeholder="Header-Name" value="${esc(name)}"><input class="finput" style="flex:2" placeholder="value" value="${esc(value)}"><button class="btn btn-sm btn-danger" onclick="this.parentElement.remove()">✕</button>`;
  wrap.appendChild(row);
}
function getMockHeaders() {
  const rows = document.querySelectorAll('#mock-headers-wrap > div');
  const h = {};
  rows.forEach(row => {
    const [k,v] = row.querySelectorAll('input');
    if (k?.value.trim()) h[k.value.trim()] = v?.value || '';
  });
  return h;
}

function editMockRule(id) {
  const r = G.mockRules.find(r => r.id === id);
  if (!r) return;
  _mockEditId = id;
  document.getElementById('mock-name').value = r.name || '';
  document.getElementById('mock-method').value = r.method || '';
  document.getElementById('mock-path').value = r.path_pattern || '';
  document.getElementById('mock-host').value = r.host || '';
  renderMockResponses(r.responses && r.responses.length ? r.responses : [{status:200,delay_ms:0,body:'',headers:{}}]);
  document.getElementById('mock-form').style.display = '';
  document.getElementById('mock-add-btn').textContent = '✕ Cancel';
  _mockFormOpen = true;
}

async function saveMockRule() {
  const name         = document.getElementById('mock-name').value.trim();
  const method       = document.getElementById('mock-method').value.trim() || null;
  const path_pattern = document.getElementById('mock-path').value.trim();
  const host         = document.getElementById('mock-host')?.value.trim() || null;
  if (!name || !path_pattern) { toast('Name and path required', true); return; }
  const responses = getMockResponses();
  const existing = _mockEditId ? G.mockRules.find(r => r.id === _mockEditId) : null;
  const rule = {
    id: _mockEditId || '',
    name, method, path_pattern, host,
    enabled: existing ? existing.enabled : true,
    call_count: existing ? existing.call_count : 0,
    responses,
  };
  try {
    if (_mockEditId) {
      await fetch(`/admin/mock/rules/${encodeURIComponent(_mockEditId)}`, {
        method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify(rule),
      });
      toast('Rule updated');
    } else {
      await fetch('/admin/mock/rules', {
        method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(rule),
      });
      toast('Rule created');
    }
    closeMockForm();
    await fetchMockRules();
  } catch { toast('Failed to save rule', true); }
}

async function deleteMockRule(id) {
  if (!confirm('Delete this mock rule?')) return;
  try {
    await fetch(`/admin/mock/rules/${encodeURIComponent(id)}`, { method: 'DELETE' });
    toast('Rule deleted');
    await fetchMockRules();
  } catch { toast('Failed to delete', true); }
}

async function resetMockCallCount(id) {
  try {
    await fetch(`/admin/mock/rules/${encodeURIComponent(id)}/reset`, { method: 'POST' });
    const r = G.mockRules.find(r => r.id === id);
    if (r) r.call_count = 0;
    renderMockRules();
  } catch { toast('Failed to reset', true); }
}

async function toggleMockEnabled(id, enabled) {
  const r = G.mockRules.find(r => r.id === id);
  if (!r) return;
  const updated = { ...r, enabled };
  try {
    await fetch(`/admin/mock/rules/${encodeURIComponent(id)}`, {
      method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify(updated),
    });
    r.enabled = enabled;
    renderMockRules();
  } catch { toast('Failed to update', true); }
}

// ── Lua Scripts ────────────────────────────────────────────────────────────────
let _scriptFormOpen = false;
let _scriptEditId   = null;

async function fetchScripts() {
  try {
    const r = await fetch('/admin/scripts');
    G.scripts = await r.json();
    renderScripts();
  } catch { toast('Failed to load scripts', true); }
}

function renderScripts() {
  const tb = document.getElementById('scripts-tbody');
  if (!tb) return;
  if (!G.scripts.length) {
    tb.innerHTML = `<tr><td colspan="3" style="text-align:center;color:var(--text4);padding:20px">No scripts — click + Add Script</td></tr>`;
    return;
  }
  tb.innerHTML = G.scripts.map(s => `<tr>
    <td>${esc(s.name)}</td>
    <td>
      <div class="toggle-wrap" onclick="toggleScriptEnabled('${esc(s.id)}',${!s.enabled})">
        <div class="toggle${s.enabled?' on':''}"></div>
      </div>
    </td>
    <td style="display:flex;gap:4px">
      <button class="btn btn-sm btn-ghost" onclick="editScript('${esc(s.id)}')">Edit</button>
      <button class="btn btn-sm btn-danger" onclick="deleteScript('${esc(s.id)}')">Delete</button>
    </td>
  </tr>`).join('');
}

function luaTabKey(e) {
  if (e.key !== 'Tab') return;
  e.preventDefault();
  const ta = e.target;
  const s = ta.selectionStart, end = ta.selectionEnd;
  ta.value = ta.value.slice(0,s) + '  ' + ta.value.slice(end);
  ta.selectionStart = ta.selectionEnd = s + 2;
  onLuaInput(ta);
}

function luaHighlight(raw) {
  const spans = [];
  const PH = i => `\x01p${i}p\x01`;
  let s = raw;
  s = s.replace(/--[^\n]*/g, m => { const i=spans.length; spans.push(`<span class="lc-cmt">${esc(m)}</span>`); return PH(i); });
  s = s.replace(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/g, m => { const i=spans.length; spans.push(`<span class="lc-str">${esc(m)}</span>`); return PH(i); });
  s = s.replace(/[&<>]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
  s = s.replace(/\b(and|break|do|else|elseif|end|false|for|function|goto|if|in|local|nil|not|or|repeat|return|then|true|until|while)\b/g, '<span class="lc-kw">$1</span>');
  s = s.replace(/\b(\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|0x[\da-fA-F]+)\b/g, '<span class="lc-num">$1</span>');
  s = s.replace(/\x01p(\d+)p\x01/g, (_, i) => spans[+i]);
  return s + '\n';
}

function onLuaInput(ta) {
  const pre = document.getElementById('script-pre');
  const gutter = document.getElementById('script-gutter');
  if (pre) { pre.innerHTML = luaHighlight(ta.value); pre.scrollTop = ta.scrollTop; }
  if (gutter) gutter.textContent = Array.from({length: ta.value.split('\n').length}, (_, i) => i + 1).join('\n');
}

function onLuaScroll(ta) {
  const pre = document.getElementById('script-pre');
  if (pre) pre.scrollTop = ta.scrollTop;
}

function luaInitEditor(value) {
  const ta = document.getElementById('script-code');
  if (!ta) return;
  if (!ta._tabWired) { ta.addEventListener('keydown', luaTabKey); ta._tabWired = true; }
  ta.value = value;
  onLuaInput(ta);
}
function openScriptForm() {
  _scriptFormOpen = true; _scriptEditId = null;
  clearScriptForm();
  document.getElementById('script-form').style.display = '';
  document.getElementById('script-add-btn').textContent = '✕ Cancel';
}

function closeScriptForm() {
  _scriptFormOpen = false; _scriptEditId = null;
  document.getElementById('script-form').style.display = 'none';
  document.getElementById('script-add-btn').textContent = '+ Add Script';
}

function clearScriptForm() {
  const n = document.getElementById('script-name'); if(n) n.value = '';
  const err = document.getElementById('script-err'); if(err) { err.style.display='none'; err.textContent=''; }
  luaInitEditor('-- Lua 5.4\n-- request.headers["x-custom"] = "value"\n-- abort(403, "forbidden")\n-- log("message")\n');
}

function editScript(id) {
  const s = G.scripts.find(s => s.id === id);
  if (!s) return;
  _scriptEditId = id;
  document.getElementById('script-name').value = s.name || '';
  const err = document.getElementById('script-err'); if(err) { err.style.display='none'; err.textContent=''; }
  luaInitEditor(s.code || '');
  document.getElementById('script-form').style.display = '';
  document.getElementById('script-add-btn').textContent = '✕ Cancel';
  _scriptFormOpen = true;
}

function setLuaError(msg) {
  const el = document.getElementById('script-err');
  if (!el) return;
  if (msg) { el.textContent = msg; el.style.display = ''; }
  else { el.textContent = ''; el.style.display = 'none'; }
}

async function saveScript() {
  const name = document.getElementById('script-name').value.trim();
  const code = document.getElementById('script-code').value;
  if (!name) { toast('Name required', true); return; }
  setLuaError(null);
  const existing = _scriptEditId ? G.scripts.find(s => s.id === _scriptEditId) : null;
  const script = { id: _scriptEditId || '', name, code, enabled: existing ? existing.enabled : true };
  try {
    let res;
    if (_scriptEditId) {
      res = await fetch(`/admin/scripts/${encodeURIComponent(_scriptEditId)}`, {
        method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify(script),
      });
      if (!res.ok) throw new Error((await res.json().catch(()=>({}))).error || `HTTP ${res.status}`);
      toast('Script updated');
    } else {
      res = await fetch('/admin/scripts', {
        method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(script),
      });
      if (!res.ok) throw new Error((await res.json().catch(()=>({}))).error || `HTTP ${res.status}`);
      toast('Script created');
    }
    closeScriptForm();
    await fetchScripts();
  } catch(e) { setLuaError(e.message); toast('Failed to save script', true); }
}

async function deleteScript(id) {
  if (!confirm('Delete this script?')) return;
  try {
    await fetch(`/admin/scripts/${encodeURIComponent(id)}`, { method: 'DELETE' });
    toast('Script deleted');
    await fetchScripts();
  } catch { toast('Failed to delete', true); }
}

async function toggleScriptEnabled(id, enabled) {
  const s = G.scripts.find(s => s.id === id);
  if (!s) return;
  const updated = { ...s, enabled };
  try {
    await fetch(`/admin/scripts/${encodeURIComponent(id)}`, {
      method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify(updated),
    });
    s.enabled = enabled;
    renderScripts();
  } catch { toast('Failed to update', true); }
}

// ── Webhooks ──────────────────────────────────────────────────────────────────
let _webhookFormOpen = false;
let _webhookEditId   = null;

const WH_EV_LABELS = {
  request_captured:  'Request',
  response_captured: 'Response',
  breakpoint_hit:    'Breakpoint',
  error:             'Error',
};

async function fetchWebhooks() {
  try {
    const r = await fetch('/admin/webhooks');
    G.webhooks = await r.json();
    renderWebhooks();
  } catch { toast('Failed to load webhooks', true); }
}

function renderWebhooks() {
  const tb = document.getElementById('webhooks-tbody');
  if (!tb) return;
  if (!G.webhooks.length) {
    tb.innerHTML = `<tr><td colspan="4" style="text-align:center;color:var(--text4);padding:20px">No webhooks — click + Add Webhook</td></tr>`;
    return;
  }
  tb.innerHTML = G.webhooks.map(h => {
    const evs = (h.events||[]).map(e => WH_EV_LABELS[e]||e).join(', ');
    return `<tr>
      <td class="mono" style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(h.url)}">${esc(h.url)}</td>
      <td style="font-size:11px;color:var(--text3)">${esc(evs)}</td>
      <td>
        <div class="toggle-wrap" onclick="toggleWebhookEnabled('${esc(h.id)}',${!h.enabled})">
          <div class="toggle${h.enabled?' on':''}"></div>
        </div>
      </td>
      <td style="display:flex;gap:4px">
        <button class="btn btn-sm btn-ghost" onclick="editWebhook('${esc(h.id)}')">Edit</button>
        <button class="btn btn-sm btn-danger" onclick="deleteWebhook('${esc(h.id)}')">Delete</button>
      </td>
    </tr>`;
  }).join('');
}

function openWebhookForm() {
  _webhookFormOpen = true; _webhookEditId = null;
  clearWebhookForm();
  document.getElementById('webhook-form').style.display = '';
  document.getElementById('webhook-add-btn').textContent = '✕ Cancel';
}

function closeWebhookForm() {
  _webhookFormOpen = false; _webhookEditId = null;
  document.getElementById('webhook-form').style.display = 'none';
  document.getElementById('webhook-add-btn').textContent = '+ Add Webhook';
}

function clearWebhookForm() {
  const u = document.getElementById('webhook-url'); if(u) u.value = '';
  const s = document.getElementById('webhook-secret'); if(s) s.value = '';
  ['wh-ev-request','wh-ev-response'].forEach(id => { const el=document.getElementById(id); if(el) el.checked=true; });
  ['wh-ev-breakpoint','wh-ev-error'].forEach(id => { const el=document.getElementById(id); if(el) el.checked=false; });
}

function editWebhook(id) {
  const h = G.webhooks.find(h => h.id === id);
  if (!h) return;
  _webhookEditId = id;
  document.getElementById('webhook-url').value = h.url || '';
  document.getElementById('webhook-secret').value = h.secret || '';
  const evMap = {
    request_captured:'wh-ev-request', response_captured:'wh-ev-response',
    breakpoint_hit:'wh-ev-breakpoint', error:'wh-ev-error',
  };
  Object.entries(evMap).forEach(([ev,eid]) => {
    const el = document.getElementById(eid);
    if (el) el.checked = (h.events||[]).includes(ev);
  });
  document.getElementById('webhook-form').style.display = '';
  document.getElementById('webhook-add-btn').textContent = '✕ Cancel';
  _webhookFormOpen = true;
}

async function saveWebhook() {
  const url    = document.getElementById('webhook-url').value.trim();
  const secret = document.getElementById('webhook-secret').value.trim() || null;
  if (!url) { toast('URL required', true); return; }
  const events = [];
  if (document.getElementById('wh-ev-request')?.checked)    events.push('request_captured');
  if (document.getElementById('wh-ev-response')?.checked)   events.push('response_captured');
  if (document.getElementById('wh-ev-breakpoint')?.checked) events.push('breakpoint_hit');
  if (document.getElementById('wh-ev-error')?.checked)      events.push('error');
  if (!events.length) { toast('Select at least one event', true); return; }
  const existing = _webhookEditId ? G.webhooks.find(h => h.id === _webhookEditId) : null;
  const hook = { id: _webhookEditId || '', url, events, enabled: existing ? existing.enabled : true, secret };
  try {
    if (_webhookEditId) {
      await fetch(`/admin/webhooks/${encodeURIComponent(_webhookEditId)}`, {
        method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify(hook),
      });
      toast('Webhook updated');
    } else {
      await fetch('/admin/webhooks', {
        method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(hook),
      });
      toast('Webhook created');
    }
    closeWebhookForm();
    await fetchWebhooks();
  } catch { toast('Failed to save webhook', true); }
}

async function deleteWebhook(id) {
  if (!confirm('Delete this webhook?')) return;
  try {
    await fetch(`/admin/webhooks/${encodeURIComponent(id)}`, { method: 'DELETE' });
    toast('Webhook deleted');
    await fetchWebhooks();
  } catch { toast('Failed to delete', true); }
}

async function toggleWebhookEnabled(id, enabled) {
  const h = G.webhooks.find(h => h.id === id);
  if (!h) return;
  const updated = { ...h, enabled };
  try {
    await fetch(`/admin/webhooks/${encodeURIComponent(id)}`, {
      method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify(updated),
    });
    h.enabled = enabled;
    renderWebhooks();
  } catch { toast('Failed to update', true); }
}

// ── Session Annotations ───────────────────────────────────────────────────────
async function saveAnnotation() {
  if (!G.sel) return;
  const note = document.getElementById('ann-note')?.value ?? '';
  const tags = G.sel.tags || [];
  try {
    await fetch(`/api/sessions/${encodeURIComponent(G.sel.id)}/annotation`, {
      method: 'PATCH', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ note, tags }),
    });
    G.sel.note = note;
  } catch { toast('Failed to save annotation', true); }
}
async function addTag() {
  if (!G.sel) return;
  const input = document.getElementById('ann-tag-input');
  const tag = input?.value.trim();
  if (!tag) return;
  const tags = [...(G.sel.tags || [])];
  if (tags.includes(tag)) { input.value=''; return; }
  tags.push(tag);
  try {
    await fetch(`/api/sessions/${encodeURIComponent(G.sel.id)}/annotation`, {
      method: 'PATCH', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ note: G.sel.note || '', tags }),
    });
    G.sel.tags = tags;
    if (input) input.value = '';
    const wrap = document.getElementById('ann-tags-wrap');
    if (wrap) wrap.innerHTML = tags.map(t=>`<span class="chip on" style="cursor:pointer" onclick="removeTag('${esc(t)}')">${esc(t)} ✕</span>`).join('');
  } catch { toast('Failed to save tag', true); }
}
async function removeTag(tag) {
  if (!G.sel) return;
  const tags = (G.sel.tags || []).filter(t => t !== tag);
  try {
    await fetch(`/api/sessions/${encodeURIComponent(G.sel.id)}/annotation`, {
      method: 'PATCH', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ note: G.sel.note || '', tags }),
    });
    G.sel.tags = tags;
    const wrap = document.getElementById('ann-tags-wrap');
    if (wrap) wrap.innerHTML = tags.map(t=>`<span class="chip on" style="cursor:pointer" onclick="removeTag('${esc(t)}')">${esc(t)} ✕</span>`).join('');
  } catch { toast('Failed to remove tag', true); }
}

// ── Session Log Save/Load ─────────────────────────────────────────────────────
async function saveSessionLog() {
  const path = document.getElementById('sess-file-path')?.value.trim();
  if (!path) { toast('Enter a file path', true); return; }
  try {
    const r = await fetch('/admin/sessions/save', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ path }) });
    if (!r.ok) { toast('Save failed: ' + (await r.text()), true); return; }
    toast('Session log saved → ' + path);
  } catch { toast('Failed to save session log', true); }
}
async function loadSessionLog() {
  const path = document.getElementById('sess-file-path')?.value.trim();
  if (!path) { toast('Enter a file path', true); return; }
  try {
    const r = await fetch('/admin/sessions/load', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ path }) });
    if (!r.ok) { toast('Load failed: ' + (await r.text()), true); return; }
    toast('Session log loaded ← ' + path);
  } catch { toast('Failed to load session log', true); }
}

// ── Config Reload ─────────────────────────────────────────────────────────────
async function reloadConfig() {
  try {
    const r = await fetch('/admin/config/reload', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({}) });
    const d = await r.json();
    toast('Config reloaded — max_body: ' + d.max_body_bytes?.toLocaleString());
    await fetchConfig();
  } catch { toast('Failed to reload config', true); }
}

// ── Upstream Proxy ─────────────────────────────────────────────────────────────
async function fetchUpstreamProxy() {
  try {
    const r = await fetch('/admin/upstream-proxy');
    const d = await r.json();
    const el = document.getElementById('upstream-proxy-url');
    if (el) el.value = d.upstream_proxy || '';
  } catch {}
}

async function saveUpstreamProxy() {
  const url = document.getElementById('upstream-proxy-url')?.value.trim() || '';
  try {
    const r = await fetch('/admin/upstream-proxy', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ upstream_proxy: url || null }),
    });
    if (!r.ok) { const d=await r.json(); toast(d.error||'Invalid proxy URL',true); return; }
    toast(url ? `Upstream proxy → ${url}` : 'Upstream proxy disabled');
  } catch { toast('Failed to save proxy settings', true); }
}

// ── SOCKS5 Status ─────────────────────────────────────────────────────────────
async function fetchSocks5Status() {
  try {
    const r = await fetch('/admin/socks5/status');
    const d = await r.json();
    const statusEl = document.getElementById('socks5-status-val');
    const portEl   = document.getElementById('socks5-port-val');
    if (statusEl) statusEl.textContent = d.enabled ? 'Enabled' : 'Disabled';
    if (portEl)   portEl.textContent   = d.port || '—';
  } catch {}
}

