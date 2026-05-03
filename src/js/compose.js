// ── Compose Workspace ─────────────────────────────────────────────────────────
function cmpMakeId() { return Math.random().toString(36).slice(2,10); }
function cmpSaveState() {
  try {
    localStorage.setItem('oproxy-compose-collections', JSON.stringify(G.compose.collections));
    localStorage.setItem('oproxy-compose-vars', JSON.stringify(G.compose.vars));
  } catch {}
}
function cmpSubstVars(str) {
  return str.replace(/\{\{([^}]+)\}\}/g, (_,k)=>G.compose.vars[k.trim()]??`{{${k}}}`);
}

function cmpNewTab(fromSession=null) {
  const tab = {
    id: cmpMakeId(), name: 'New Request', dirty: false, collectionId: null, savedReqId: null,
    method: fromSession?.request?.method||'GET',
    url: (() => {
      const uri = fromSession?.request?.uri||'';
      if (!fromSession || /^https?:\/\//i.test(uri)) return uri;
      const dest = fromSession.request?.headers?.['x-oproxy-destination'] || fromSession.request?.headers?.['X-Oproxy-Destination'] || '';
      const host = fromSession.request?.host || '';
      const scheme = dest ? dest.split('://')[0] : (host.endsWith(':443') || G.routes[host]?.startsWith('https') ? 'https' : 'https');
      return host ? `${scheme}://${host}${uri}` : uri;
    })(),
    headers: Object.entries(fromSession?.request?.headers||{}).filter(([k])=>!INTERNAL_HDRS.has(k.toLowerCase())).map(([k,v])=>({enabled:true,key:k,val:v})),
    params: [],
    bodyMode: 'raw', bodyRaw: fromSession?.request?.body||'', bodyForm: [],
    contentType: 'application/json',
    history: [],
  };
  if (fromSession?.request?.uri) tab.name = (fromSession.request.method||'GET') + ' ' + getPath(fromSession.request.uri);
  G.compose.tabs.push(tab);
  cmpActivateTab(tab.id);
}
function cmpActivateTab(tabId) {
  G.compose.activeTabId = tabId;
  renderCmpTabs();
  renderCmpEditor();
}
function cmpCloseTab(tabId) {
  const idx = G.compose.tabs.findIndex(t=>t.id===tabId);
  if (idx===-1) return;
  G.compose.tabs.splice(idx,1);
  if (G.compose.activeTabId===tabId) {
    const next = G.compose.tabs[Math.min(idx,G.compose.tabs.length-1)];
    G.compose.activeTabId = next?.id||null;
  }
  renderCmpTabs();
  renderCmpEditor();
}
function cmpDirty() {
  const tab = G.compose.tabs.find(t=>t.id===G.compose.activeTabId);
  if (!tab) return;
  tab.dirty = true;
  cmpSyncTabFromDom(tab);
  renderCmpTabs();
}
function parseCurl(raw) {
  // Normalize multi-line cURL (join continuation lines, collapse whitespace)
  const src = raw.replace(/\\\n/g, ' ').replace(/\r/g, '');
  // Tokenize respecting single/double quotes
  const tokens = [];
  let cur = '', inSq = false, inDq = false;
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    if (c === "'" && !inDq) { inSq = !inSq; continue; }
    if (c === '"' && !inSq) { inDq = !inDq; continue; }
    if (c === ' ' && !inSq && !inDq) { if (cur) { tokens.push(cur); cur = ''; } continue; }
    cur += c;
  }
  if (cur) tokens.push(cur);

  // Must start with 'curl'
  if (!tokens.length || tokens[0].toLowerCase() !== 'curl') return null;

  let method = null, url = null, body = null;
  const headers = [];
  let i = 1;
  while (i < tokens.length) {
    const t = tokens[i];
    if (t === '-X' || t === '--request') { method = tokens[++i]; i++; continue; }
    if (t === '-H' || t === '--header') {
      const hdr = tokens[++i]; i++;
      const ci = hdr.indexOf(':');
      if (ci > 0) headers.push({ key: hdr.slice(0, ci).trim(), val: hdr.slice(ci + 1).trim(), enabled: true });
      continue;
    }
    if (t === '-d' || t === '--data' || t === '--data-raw' || t === '--data-binary') { body = tokens[++i]; i++; continue; }
    if (t === '-u' || t === '--user') {
      const cred = tokens[++i]; i++;
      headers.push({ key: 'Authorization', val: 'Basic ' + btoa(cred), enabled: true });
      continue;
    }
    // Skip boolean flags
    if (t.startsWith('-')) { i++; continue; }
    // Positional URL
    if (!url && (t.startsWith('http://') || t.startsWith('https://'))) { url = t; i++; continue; }
    i++;
  }
  if (!url) return null;
  if (!method) method = body ? 'POST' : 'GET';
  return { method: method.toUpperCase(), url, headers, body: body || '' };
}

function cmpMaybeImportCurl(e) {
  const pasted = (e.clipboardData || window.clipboardData).getData('text').trim();
  if (!pasted.toLowerCase().startsWith('curl ')) return; // let normal paste proceed
  e.preventDefault();
  const parsed = parseCurl(pasted);
  if (!parsed) return;
  const tab = G.compose.tabs.find(t => t.id === G.compose.activeTabId); if (!tab) return;
  tab.method  = parsed.method;
  tab.url     = parsed.url;
  tab.headers = parsed.headers;
  tab.bodyRaw = parsed.body;
  if (parsed.body) {
    tab.bodyMode = 'raw';
    const ctHdr = parsed.headers.find(h => h.key.toLowerCase() === 'content-type');
    if (ctHdr) tab.contentType = ctHdr.val.split(';')[0].trim();
  }
  tab.dirty = true;
  renderCmpEditor();
  renderCmpTabs();
  // Switch to body tab if body present
  if (parsed.body) {
    const bodyTab = document.querySelector('.ctab[data-ctab="body"]');
    if (bodyTab) setCmpBodyTab(bodyTab);
  }
  toast('Imported from cURL');
}

function cmpSyncTabFromDom(tab) {
  tab.method = document.getElementById('cmp-method')?.value||tab.method;
  tab.url = document.getElementById('cmp-url')?.value||tab.url;
  tab.bodyMode = document.getElementById('cmp-body-raw-btn')?.getAttribute('aria-pressed')==='true'?'raw':'form';
  tab.bodyRaw = document.getElementById('cmp-body-raw-ta')?.value||tab.bodyRaw;
  tab.contentType = document.getElementById('cmp-body-ct')?.value||tab.contentType;
  // Sync headers
  const hdrRows=[]; document.querySelectorAll('#cmp-headers-tbody tr').forEach(row=>{
    const cb=row.querySelector('input[type=checkbox]'),k=row.querySelector('.cmp-kv-input:nth-of-type(1)'),v=row.querySelector('.cmp-kv-input:nth-of-type(2)');
    if(k&&v) hdrRows.push({enabled:cb?.checked!==false,key:k.value,val:v.value});
  }); tab.headers=hdrRows;
  // Sync params
  const paramRows=[]; document.querySelectorAll('#cmp-params-tbody tr').forEach(row=>{
    const cb=row.querySelector('input[type=checkbox]'),k=row.querySelector('.cmp-kv-input:nth-of-type(1)'),v=row.querySelector('.cmp-kv-input:nth-of-type(2)');
    if(k&&v) paramRows.push({enabled:cb?.checked!==false,key:k.value,val:v.value});
  }); tab.params=paramRows;
}

function renderCmpTabs() {
  const strip = document.getElementById('cmp-tab-strip'); if (!strip) return;
  // Remove old tabs (leave the + button)
  strip.querySelectorAll('.cmp-tab').forEach(el=>el.remove());
  const btn = strip.querySelector('.cmp-tab-new');
  G.compose.tabs.forEach(tab=>{
    const el=document.createElement('div');
    el.className='cmp-tab'+(tab.id===G.compose.activeTabId?' active':'');
    el.setAttribute('role','tab'); el.setAttribute('aria-selected',tab.id===G.compose.activeTabId);
    el.innerHTML=`<span class="cmp-tab-name">${esc(tab.name)}</span>${tab.dirty?'<span class="cmp-tab-dirty">●</span>':''}<span class="cmp-tab-x" onclick="event.stopPropagation();cmpCloseTab('${tab.id}')" aria-label="Close tab">✕</span>`;
    el.addEventListener('click', ()=>cmpActivateTab(tab.id));
    strip.insertBefore(el, btn);
  });
}
function renderCmpEditor() {
  const noTab=document.getElementById('cmp-no-tab'), editor=document.getElementById('cmp-editor');
  if (!noTab||!editor) return;
  const tab=G.compose.tabs.find(t=>t.id===G.compose.activeTabId);
  if (!tab) { noTab.style.display=''; editor.style.display='none'; document.getElementById('cmp-response').style.display='none'; return; }
  noTab.style.display='none'; editor.style.display='';
  document.getElementById('cmp-method').value=tab.method;
  document.getElementById('cmp-url').value=tab.url;
  document.getElementById('cmp-body-ct').value=tab.contentType;
  document.getElementById('cmp-body-raw-ta').value=tab.bodyRaw;
  renderCmpKvTable('headers', tab.headers, 'cmp-headers-tbody');
  renderCmpKvTable('params', tab.params, 'cmp-params-tbody');
  renderCmpKvTable('form', tab.bodyForm, 'cmp-form-tbody');
  renderCmpHistory(tab);
  cmpResolveVars();
}
function renderCmpKvTable(type, rows, tbodyId) {
  const tb=document.getElementById(tbodyId); if (!tb) return;
  tb.innerHTML=(rows||[]).map((r,i)=>`<tr>
    <td><input type="checkbox" ${r.enabled!==false?'checked':''} onchange="cmpDirty()"></td>
    <td><input class="cmp-kv-input" placeholder="${type==='headers'?'Header-Name':'key'}" value="${esc(r.key||'')}" oninput="cmpDirty()"></td>
    <td><input class="cmp-kv-input" placeholder="value" value="${esc(r.val||'')}" oninput="cmpDirty()"></td>
    <td><button class="cmp-kv-del" onclick="this.closest('tr').remove();cmpDirty()">✕</button></td>
  </tr>`).join('');
}
function renderCmpHistory(tab) {
  const el=document.getElementById('cmp-history-list'); if (!el) return;
  const count=document.getElementById('cmp-history-count'); if(count) count.textContent=tab.history.length;
  if (!tab.history.length) { el.innerHTML='<div style="padding:10px 14px;color:var(--text4);font-size:11px">No history</div>'; return; }
  el.innerHTML=[...tab.history].reverse().map(h=>`<div class="cmp-history-row" onclick="cmpRestoreHistory(${JSON.stringify(h)})">
    ${mbadge(h.method)} <span class="mono" style="flex:1;overflow:hidden;text-overflow:ellipsis">${esc(getPath(h.url))}</span>
    ${h.status?sbadge(h.status):'<span style="color:var(--text4)">—</span>'}
    <span style="color:var(--text4)">${fmtMs(h.ms)}</span>
  </div>`).join('');
}
function cmpRestoreHistory(h) {
  const tab=G.compose.tabs.find(t=>t.id===G.compose.activeTabId); if (!tab) return;
  tab.url=h.url; tab.method=h.method;
  document.getElementById('cmp-url').value=h.url;
  document.getElementById('cmp-method').value=h.method;
  cmpResolveVars();
}

function cmpAddKvRow(type) {
  const tbodyId=type==='headers'?'cmp-headers-tbody':type==='params'?'cmp-params-tbody':'cmp-form-tbody';
  const tb=document.getElementById(tbodyId); if (!tb) return;
  const ph=type==='headers'?'Header-Name':'key';
  const row=document.createElement('tr');
  row.innerHTML=`<td><input type="checkbox" checked onchange="cmpDirty()"></td><td><input class="cmp-kv-input" placeholder="${ph}" oninput="cmpDirty()"></td><td><input class="cmp-kv-input" placeholder="value" oninput="cmpDirty()"></td><td><button class="cmp-kv-del" onclick="this.closest('tr').remove();cmpDirty()">✕</button></td>`;
  tb.appendChild(row); row.querySelector('input:nth-of-type(2)')?.focus();
}
function cmpToggleAllKv(type, checked) {
  const tbodyId=type==='headers'?'cmp-headers-tbody':'cmp-params-tbody';
  document.querySelectorAll(`#${tbodyId} input[type=checkbox]`).forEach(cb=>cb.checked=checked);
  cmpDirty();
}
function setCmpBodyTab(el) {
  document.querySelectorAll('.ctab[data-ctab]').forEach(t=>t.classList.remove('on'));
  el.classList.add('on');
  const tab=el.dataset.ctab;
  document.getElementById('cmp-pane-headers').style.display=tab==='headers'?'':'none';
  document.getElementById('cmp-pane-params').style.display=tab==='params'?'':'none';
  document.getElementById('cmp-pane-body').style.display=tab==='body'?'':'none';
}
function setCmpBodyMode(mode) {
  const isRaw=mode==='raw';
  document.getElementById('cmp-body-raw-btn').setAttribute('aria-pressed',isRaw);
  document.getElementById('cmp-body-form-btn').setAttribute('aria-pressed',!isRaw);
  document.getElementById('cmp-body-raw-btn').className='vmode-btn'+(isRaw?' on':'');
  document.getElementById('cmp-body-form-btn').className='vmode-btn'+(!isRaw?' on':'');
  document.getElementById('cmp-body-raw-ta').style.display=isRaw?'':'none';
  document.getElementById('cmp-body-form-editor').style.display=isRaw?'none':'';
}
function setCmpResTab(el) {
  document.querySelectorAll('#cmp-res-tabs .ctab').forEach(t=>t.classList.remove('on'));
  el.classList.add('on');
  const t=el.dataset.ctab;
  document.getElementById('cmp-res-body-pane').style.display=t==='body'?'':'none';
  document.getElementById('cmp-res-headers-pane').style.display=t==='headers'?'':'none';
}
function cmpResolveVars() {
  const raw=document.getElementById('cmp-url')?.value||'';
  const resolved=cmpSubstVars(raw);
  const el=document.getElementById('cmp-resolved-url'); if (!el) return;
  if (resolved!==raw && raw.includes('{{')) {
    el.style.display=''; el.innerHTML='→ '+raw.replace(/\{\{([^}]+)\}\}/g,(_,k)=>{
      const v=G.compose.vars[k.trim()];
      return v?`<span class="var-resolved">${esc(v)}</span>`:`<span style="color:var(--warning)">{{${esc(k)}}}</span>`;
    });
  } else { el.style.display='none'; }
}
async function sendCmpRequest() {
  const tab=G.compose.tabs.find(t=>t.id===G.compose.activeTabId); if (!tab) return;
  cmpSyncTabFromDom(tab);
  let url=cmpSubstVars(tab.url);
  if (!url){toast('URL is required',true);return;}
  // Append enabled params
  const ps=tab.params.filter(p=>p.enabled&&p.key);
  if (ps.length) {
    const q=new URLSearchParams(ps.map(p=>[p.key,cmpSubstVars(p.val)]));
    url+=( url.includes('?')?'&':'?')+q.toString();
  }
  const headers={};
  tab.headers.filter(h=>h.enabled&&h.key&&!INTERNAL_HDRS.has(h.key.toLowerCase())).forEach(h=>{ headers[cmpSubstVars(h.key)]=cmpSubstVars(h.val); });
  const method=tab.method;
  let body=undefined;
  if (method!=='GET'&&method!=='HEAD') {
    if (tab.bodyMode==='form') {
      const fd=new URLSearchParams(); tab.bodyForm.filter(f=>f.key).forEach(f=>fd.append(cmpSubstVars(f.key),cmpSubstVars(f.val)));
      body=fd.toString(); if (!headers['Content-Type']) headers['Content-Type']='application/x-www-form-urlencoded';
    } else if (tab.bodyRaw) {
      body=tab.bodyRaw; if (!headers['Content-Type']) headers['Content-Type']=tab.contentType;
    }
  }
  const t0=Date.now();
  document.getElementById('cmp-send-btn').disabled=true;
  try {
    const fwdRes=await fetch('/admin/forward',{
      method:'POST',
      headers:{'Content-Type':'application/json'},
      body:JSON.stringify({method,url,headers,body:body||null})
    });
    const ms=Date.now()-t0;
    if (!fwdRes.ok && fwdRes.status!==502) throw new Error('Forward endpoint error '+fwdRes.status);
    const data=await fwdRes.json().catch(()=>null);
    if (!data) throw new Error('Invalid response from forward endpoint');
    const resHdrs=data.headers||{};
    const resBody=data.body||'';
    const resSize=new TextEncoder().encode(resBody).length;
    // Show response panel
    const rp=document.getElementById('cmp-response'); rp.style.display='';
    document.getElementById('cmp-res-status').innerHTML=sbadge(data.status)+` ${esc(HTTP_REASONS[data.status]||'')}`;
    document.getElementById('cmp-res-time').textContent=fmtMs(ms);
    document.getElementById('cmp-res-size').textContent=fmtSz(resSize);
    document.getElementById('cmp-res-body-pane').innerHTML=renderBody(resBody,resHdrs);
    document.getElementById('cmp-res-headers-pane').innerHTML=Object.entries(resHdrs).map(([k,v])=>`<div class="hdr-row"><span class="hdr-name">${esc(k)}</span><span class="hdr-val">${esc(v)}</span></div>`).join('');
    // Record in history
    tab.history.push({ts:Date.now(),method,url:tab.url,status:data.status,ms});
    tab.dirty=true; renderCmpHistory(tab); renderCmpTabs();
    toast('Response received');
  } catch(e) { toast('Request failed — '+e.message,true); }
  finally { document.getElementById('cmp-send-btn').disabled=false; }
}

// ── Compose collections & variables ──────────────────────────────────────────
function renderCmpCollections() {
  const el=document.getElementById('cmp-collections-list'); if (!el) return;
  let html='';
  if (G.compose._newCollRow) {
    html+=`<div class="cmp-coll-new-row"><input id="cmp-new-coll-input" placeholder="Collection name" onkeydown="cmpCommitNewColl(event)" onblur="cmpCommitNewColl({key:'Blur'})"></div>`;
  }
  if (!G.compose.collections.length && !G.compose._newCollRow) {
    el.innerHTML='<div style="padding:12px;color:var(--text4);font-size:11px">No collections yet.<br>Click + to create one.</div>'; renderCmpVars(); return;
  }
  const pd = G.compose._pendingDelete;
  html+=G.compose.collections.map(coll=>{
    const collDelPending = pd?.type==='collection' && pd.collId===coll.id;
    let rows = `<div class="cmp-coll-item">
      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M1 3.5h10M3 1.5h6L10 3.5H2L3 1.5zM1 3.5v7h10v-7"/></svg>
      <span style="flex:1;cursor:pointer" ondblclick="cmpStartRenameCollection('${coll.id}')" onclick="cmpToggleColl('${coll.id}')">${esc(coll.name)}</span>
      <button class="cmp-kv-del" onclick="event.stopPropagation();cmpAskDelCollection('${coll.id}')" title="Delete collection" style="font-size:11px">✕</button>
    </div>`;
    if (collDelPending) {
      rows += `<div class="cmp-del-confirm">
        <span>Delete "${esc(coll.name)}" and all its requests?</span>
        <button class="btn btn-sm btn-danger" onclick="cmpConfirmDelCollection('${coll.id}')">Delete</button>
        <button class="btn btn-sm btn-ghost" onclick="G.compose._pendingDelete=null;renderCmpCollections()">Cancel</button>
      </div>`;
    }
    if (coll.expanded) {
      rows += coll.requests.map(req => {
        const isActive = G.compose.tabs.find(t=>t.savedReqId===req.id);
        const reqDelPending = pd?.type==='request' && pd.reqId===req.id;
        if (G.compose._renamingReq===req.id) {
          return `<div class="cmp-req-rename-row">
            <input id="cmp-rename-req-input" value="${esc(req.name||getPath(req.url))}" onkeydown="cmpCommitRenameReq(event,'${coll.id}','${req.id}')" onblur="cmpCommitRenameReq({key:'Blur'},'${coll.id}','${req.id}')">
            <button class="btn btn-sm btn-ghost" onclick="G.compose._renamingReq=null;renderCmpCollections()">✕</button>
          </div>`;
        }
        let reqRow = `<div class="cmp-saved-req${isActive?' active':''}" onclick="cmpLoadRequest('${coll.id}','${req.id}')">
          <span class="cmp-saved-method" style="color:var(--accent)">${esc(req.method)}</span>
          <span class="cmp-saved-name">${esc(req.name||getPath(req.url))}</span>
          <button class="cmp-req-edit-btn" onclick="event.stopPropagation();cmpStartRenameReq('${req.id}')" title="Rename">✎</button>
          <button class="cmp-kv-del" onclick="event.stopPropagation();cmpAskDelRequest('${coll.id}','${req.id}')" title="Delete" style="font-size:11px">✕</button>
        </div>`;
        if (reqDelPending) {
          reqRow += `<div class="cmp-del-confirm" style="padding-left:22px">
            <span>Delete "${esc(req.name||getPath(req.url))}"?</span>
            <button class="btn btn-sm btn-danger" onclick="cmpConfirmDelRequest('${coll.id}','${req.id}')">Delete</button>
            <button class="btn btn-sm btn-ghost" onclick="G.compose._pendingDelete=null;renderCmpCollections()">Cancel</button>
          </div>`;
        }
        return reqRow;
      }).join('');
    }
    return rows;
  }).join('');
  el.innerHTML=html;
  if (G.compose._newCollRow) { const i=document.getElementById('cmp-new-coll-input'); if(i) i.focus(); }
  if (G.compose._renamingReq) { const i=document.getElementById('cmp-rename-req-input'); if(i){i.focus();i.select();} }
  renderCmpVars();
}
function renderCmpVars() {
  const el=document.getElementById('cmp-vars-list'); if (!el) return;
  el.innerHTML=Object.entries(G.compose.vars).map(([k,v])=>`<div class="cmp-var-row">
    <input class="cmp-var-key" value="${esc(k)}" placeholder="name" onchange="cmpUpdateVar('${esc(k)}',this.value,document.querySelector('.cmp-var-row:has(input.cmp-var-key[value=\\'${esc(k)}\\']) .cmp-var-val')?.value||'${esc(v)}')">
    <input class="cmp-var-val" value="${esc(v)}" placeholder="value" oninput="G.compose.vars['${esc(k)}']=this.value;cmpSaveState();">
    <button class="cmp-kv-del" onclick="cmpDelVar('${esc(k)}')">✕</button>
  </div>`).join('');
}
function cmpAddVar() {
  const key=prompt('Variable name (used as {{key}} in URLs/values):'); if (!key) return;
  G.compose.vars[key]=''; cmpSaveState(); renderCmpVars();
}
function cmpDelVar(key) { delete G.compose.vars[key]; cmpSaveState(); renderCmpVars(); }
function cmpUpdateVar(oldKey,newKey,val) {
  if (oldKey!==newKey) delete G.compose.vars[oldKey];
  G.compose.vars[newKey]=val; cmpSaveState(); renderCmpVars();
}
function cmpNewCollection() {
  G.compose._newCollRow = true;
  renderCmpCollections();
}
function cmpCommitNewColl(e) {
  if (e.key !== 'Enter' && e.key !== 'Blur') return;
  if (!G.compose._newCollRow) return;   // blur re-fires when DOM removes input; ignore
  const inp = document.getElementById('cmp-new-coll-input'); if (!inp) return;
  const name = inp.value.trim();
  G.compose._newCollRow = false;
  if (name) G.compose.collections.push({id:cmpMakeId(), name, expanded:true, requests:[]});
  cmpSaveState(); renderCmpCollections();
  // Re-populate collection select in save form if open
  cmpPopulateSaveCollSelect();
}
function cmpStartRenameCollection(id) {
  const c = G.compose.collections.find(x=>x.id===id); if (!c) return;
  const name = prompt('Rename collection:', c.name); // keep simple for collection-level rename
  if (name && name.trim()) { c.name = name.trim(); cmpSaveState(); renderCmpCollections(); }
}
function cmpStartRenameReq(reqId) {
  G.compose._renamingReq = reqId;
  G.compose._pendingDelete = null;
  renderCmpCollections();
}
function cmpCommitRenameReq(e, collId, reqId) {
  if (e.key !== 'Enter' && e.key !== 'Blur') return;
  if (!G.compose._renamingReq) return;
  const inp = document.getElementById('cmp-rename-req-input'); if (!inp) return;
  const name = inp.value.trim();
  if (name) {
    const coll = G.compose.collections.find(c=>c.id===collId); if (!coll) return;
    const req = coll.requests.find(r=>r.id===reqId); if (!req) return;
    req.name = name;
    // Also update tab name if open
    const tab = G.compose.tabs.find(t=>t.savedReqId===reqId);
    if (tab) { tab.name = name; renderCmpTabs(); }
    cmpSaveState();
  }
  G.compose._renamingReq = null;
  renderCmpCollections();
}
function cmpAskDelCollection(id) {
  G.compose._pendingDelete = {type:'collection', collId:id};
  G.compose._renamingReq = null;
  renderCmpCollections();
}
function cmpConfirmDelCollection(id) {
  G.compose.collections = G.compose.collections.filter(c=>c.id!==id);
  G.compose._pendingDelete = null;
  cmpSaveState(); renderCmpCollections();
}
function cmpToggleColl(id) {
  const c=G.compose.collections.find(x=>x.id===id); if(c) c.expanded=!c.expanded; cmpSaveState(); renderCmpCollections();
}
function cmpLoadRequest(collId, reqId) {
  const coll=G.compose.collections.find(c=>c.id===collId); if (!coll) return;
  const req=coll.requests.find(r=>r.id===reqId); if (!req) return;
  const existing=G.compose.tabs.find(t=>t.collectionId===collId&&t.savedReqId===reqId);
  if (existing) { cmpActivateTab(existing.id); return; }
  const tab={id:cmpMakeId(),name:req.name||getPath(req.url),dirty:false,collectionId:collId,savedReqId:reqId,
    method:req.method||'GET',url:req.url||'',
    headers:[...(req.headers||[])],params:[...(req.params||[])],
    bodyMode:req.bodyMode||'raw',bodyRaw:req.bodyRaw||'',bodyForm:[...(req.bodyForm||[])],
    contentType:req.contentType||'application/json',history:[]};
  G.compose.tabs.push(tab); cmpActivateTab(tab.id);
}
function cmpAskDelRequest(collId, reqId) {
  G.compose._pendingDelete = {type:'request', collId, reqId};
  G.compose._renamingReq = null;
  renderCmpCollections();
}
function cmpConfirmDelRequest(collId, reqId) {
  const coll = G.compose.collections.find(c=>c.id===collId); if (!coll) return;
  coll.requests = coll.requests.filter(r=>r.id!==reqId);
  G.compose._pendingDelete = null;
  cmpSaveState(); renderCmpCollections();
}
function cmpToggleSaveForm() {
  const form = document.getElementById('cmp-save-form'); if (!form) return;
  if (form.style.display !== 'none') { form.style.display = 'none'; return; }
  const tab = G.compose.tabs.find(t=>t.id===G.compose.activeTabId); if (!tab) return;
  cmpSyncTabFromDom(tab);
  cmpPopulateSaveCollSelect();
  document.getElementById('cmp-save-name').value = tab.name || '';
  form.style.display = '';
  document.getElementById('cmp-save-name').focus();
  document.getElementById('cmp-save-name').select();
}
function cmpPopulateSaveCollSelect() {
  const sel = document.getElementById('cmp-save-coll'); if (!sel) return;
  const tab = G.compose.tabs.find(t=>t.id===G.compose.activeTabId);
  sel.innerHTML = G.compose.collections.map(c =>
    `<option value="${esc(c.id)}"${tab?.collectionId===c.id?' selected':''}>${esc(c.name)}</option>`
  ).join('');
  if (!G.compose.collections.length) sel.innerHTML = '<option disabled>No collections</option>';
}
function cmpConfirmSave() {
  const tab = G.compose.tabs.find(t=>t.id===G.compose.activeTabId); if (!tab) return;
  cmpSyncTabFromDom(tab);
  const name = (document.getElementById('cmp-save-name')?.value||'').trim() || tab.url || 'Untitled';
  const collId = document.getElementById('cmp-save-coll')?.value;
  if (!collId) { toast('Create a collection first', true); return; }
  const coll = G.compose.collections.find(c=>c.id===collId);
  if (!coll) { toast('Collection not found', true); return; }
  const reqData = {name, method:tab.method, url:tab.url, headers:tab.headers, params:tab.params, bodyMode:tab.bodyMode, bodyRaw:tab.bodyRaw, bodyForm:tab.bodyForm, contentType:tab.contentType};
  if (tab.collectionId===collId && tab.savedReqId) {
    const idx = coll.requests.findIndex(r=>r.id===tab.savedReqId);
    if (idx>=0) { coll.requests[idx]={...reqData, id:tab.savedReqId}; }
    else { const rid=cmpMakeId(); coll.requests.push({...reqData, id:rid}); tab.savedReqId=rid; }
  } else {
    const rid = cmpMakeId();
    coll.requests.push({...reqData, id:rid});
    tab.collectionId=coll.id; tab.savedReqId=rid;
  }
  tab.name=name; tab.dirty=false;
  document.getElementById('cmp-save-form').style.display='none';
  cmpSaveState(); renderCmpTabs(); renderCmpCollections(); toast('Saved to '+coll.name);
}

function renderDetail() {
  const body = document.getElementById('dbody');
  if (!G.sel) {
    body.innerHTML=`<div class="dempty"><svg width="30" height="30" viewBox="0 0 30 30" fill="none" stroke="currentColor" stroke-width="1.3" opacity="0.25"><rect x="3" y="3" width="24" height="24" rx="4"/><path d="M9 11h12M9 15h12M9 19h8"/></svg><span>Select a request to inspect</span></div>`;
    return;
  }
  const isWs = G.sel?.request?.method === 'WS';
  const framesTab   = document.getElementById('dtab-frames');
  const composeTab  = document.getElementById('dtab-compose');
  const inspectorTab= document.getElementById('dtab-inspector');
  const hasFrames   = G.sel?.ws_frames?.length > 0;
  const id          = G.sel?.inspector_data;
  const hasInspector= !!(id?.jwt || id?.graphql || id?.grpc);
  if (framesTab)    framesTab.style.display    = hasFrames    ? '' : 'none';
  if (composeTab)   composeTab.style.display   = isWs         ? 'none' : '';
  if (inspectorTab) inspectorTab.style.display = hasInspector ? '' : 'none';
  if (!hasFrames    && G.dtab === 'frames')    G.dtab = 'overview';
  if (isWs          && G.dtab === 'compose')   G.dtab = 'overview';
  if (!hasInspector && G.dtab === 'inspector') G.dtab = 'overview';
  const fn = {overview:renderOverview,request:renderRequest,response:renderResponse,timing:renderTiming,frames:renderFrames,inspector:renderInspector}[G.dtab]||renderOverview;
  body.innerHTML = fn();
}

function renderOverview() {
  const s=G.sel, st=s.metrics?.status_code||s.response?.status, tls=!!(G.routes[s.request?.host]?.startsWith('https://'));
  const reason = st ? HTTP_REASONS[st] || '' : '';
  const latMs = s.metrics?.latency_ms;
  const latColor = !latMs ? '' : latMs < 200 ? 'color:var(--success)' : latMs >= 1000 ? 'color:var(--danger)' : '';
  const isWs = s.request?.method === 'WS';
  const connSection = isWs ? `
    <div class="dsec">
      <div class="dsec-title">Connection</div>
      <div class="kv"><span class="kk">Status</span><span class="kv-val">${wsBadge(s)}</span></div>
      <div class="kv"><span class="kk">Frames</span><span class="kv-val kv-mono">${(s.ws_frames||[]).length}</span></div>
      <div class="kv"><span class="kk">TLS</span><span class="kv-val" style="display:flex;align-items:center;gap:6px">${tlsIcon(tls)}<span>${tls?'Secured':'Unsecured'}</span></span></div>
    </div>` : `
    <div class="dsec">
      <div class="dsec-title">Response</div>
      <div class="kv"><span class="kk">Status</span><span class="kv-val" style="display:flex;align-items:center;gap:6px">${sbadge(st)}<span style="font-size:11px;color:var(--text3)">${reason||(!st?'no response':'')}</span></span></div>
      <div class="kv"><span class="kk">Latency</span><span class="kv-val kv-mono" style="${latColor}">${esc(fmtMs(latMs))}</span></div>
      <div class="kv"><span class="kk">Res size</span><span class="kv-val kv-mono">${esc(fmtSz(s.metrics?.response_size_bytes))}</span></div>
      <div class="kv"><span class="kk">Req size</span><span class="kv-val kv-mono">${esc(fmtSz(s.metrics?.request_size_bytes))}</span></div>
      <div class="kv"><span class="kk">TLS</span><span class="kv-val" style="display:flex;align-items:center;gap:6px">${tlsIcon(tls)}<span>${tls?'Secured':'Unsecured'}</span></span></div>
    </div>`;
  return `
    <div class="dsec">
      <div class="dsec-title">Request</div>
      <div class="kv" style="align-items:flex-start">
        <span class="kk" style="padding-top:1px">URL</span>
        <div style="flex:1;min-width:0">
          <div class="kv-val kv-accent" style="font-size:11.5px;word-break:break-all;line-height:1.5">${esc(s.request?.uri||'')}</div>
          <button class="btn btn-sm btn-ghost" style="margin-top:4px;padding:2px 7px;font-size:10px" onclick="navigator.clipboard.writeText(G.sel?.request?.uri||'').then(()=>toast('Copied'))">Copy URL</button>
        </div>
      </div>
      <div class="kv"><span class="kk">Method</span><span class="kv-val">${mbadge(s.request?.method||'')}</span></div>
      <div class="kv"><span class="kk">Host</span><span class="kv-val kv-mono">${esc(s.request?.host||'')}</span></div>
      <div class="kv"><span class="kk">Path</span><span class="kv-val kv-mono trunc">${esc(getPath(s.request?.uri))}</span></div>
    </div>
    ${connSection}
    <div class="dsec">
      <div class="dsec-title">Time</div>
      <div class="kv"><span class="kk">Captured</span><span class="kv-val kv-mono">${esc(new Date(s.timestamp).toLocaleTimeString())}</span></div>
      <div class="kv"><span class="kk">Ago</span><span class="kv-val kv-mono">${esc(relT(s.timestamp))}</span></div>
    </div>
    <div class="dsec">
      <div class="dsec-title">Annotations</div>
      <div class="kv" style="align-items:flex-start">
        <span class="kk" style="padding-top:5px">Note</span>
        <div style="flex:1;min-width:0">
          <textarea id="ann-note" rows="2" style="width:100%;box-sizing:border-box;font-size:12px;font-family:inherit;background:var(--bg2);color:var(--text);border:0.5px solid var(--border);border-radius:5px;padding:5px 7px;resize:vertical" placeholder="Add a note…" onchange="saveAnnotation()">${esc(s.note||'')}</textarea>
        </div>
      </div>
      <div class="kv" style="align-items:flex-start;margin-top:4px">
        <span class="kk" style="padding-top:5px">Tags</span>
        <div style="flex:1;min-width:0">
          <div id="ann-tags-wrap" style="display:flex;flex-wrap:wrap;gap:4px;margin-bottom:4px">${(s.tags||[]).map(t=>`<span class="chip on" style="cursor:pointer" onclick="removeTag('${esc(t)}')">${esc(t)} ✕</span>`).join('')}</div>
          <div style="display:flex;gap:4px">
            <input id="ann-tag-input" class="finput" style="font-size:11px;padding:3px 7px;height:24px" placeholder="add tag…" onkeydown="if(event.key==='Enter'){addTag();event.preventDefault()}">
            <button class="btn btn-ghost btn-sm" onclick="addTag()">+</button>
          </div>
        </div>
      </div>
    </div>`;
}

const INTERNAL_HDRS = new Set(['x-oproxy-session-id', 'x-oproxy-destination', 'x-oproxy-map-local-file']);
function renderHeaders(hdrs) {
  const entries = Object.entries(hdrs||{}).filter(([k]) => !INTERNAL_HDRS.has(k.toLowerCase()));
  if (!entries.length) return '<span style="color:var(--text4);font-size:12px">No headers</span>';
  return entries.map(([k,v])=>`<div class="hrow"><span class="hk" title="${esc(k)}">${esc(k)}</span><span class="hv" title="${esc(v)}">${esc(v)}</span></div>`).join('');
}

function renderRequest() {
  const s=G.sel;
  const hdrCount = Object.entries(s.request?.headers||{}).filter(([k])=>!INTERNAL_HDRS.has(k.toLowerCase())).length;
  return `
    <div class="dsec"><div class="dsec-title" style="display:flex;align-items:center;justify-content:space-between">Request Headers <span style="font-weight:400;color:var(--text4);letter-spacing:0;text-transform:none;font-size:10px">${hdrCount} headers</span></div>${renderHeaders(s.request?.headers)}</div>
    <div class="dsec"><div class="dsec-title">Request Body</div>${renderBody(s.request?.body, s.request?.headers)}</div>`;
}

function renderResponse() {
  const s=G.sel;
  if (!s.response) return `<div class="dempty"><span>No response recorded</span><span style="font-size:11px;margin-top:4px;color:var(--text4)">The request may have failed or the outbound connection was refused.</span></div>`;
  const hdrCount = Object.keys(s.response?.headers||{}).length;
  return `
    <div class="dsec"><div class="dsec-title" style="display:flex;align-items:center;justify-content:space-between">Response Headers <span style="font-weight:400;color:var(--text4);letter-spacing:0;text-transform:none;font-size:10px">${hdrCount} headers</span></div>${renderHeaders(s.response.headers)}</div>
    <div class="dsec"><div class="dsec-title">Response Body</div>${renderBody(s.response.body, s.response.headers)}</div>`;
}

function renderTiming() {
  const s=G.sel, ms=s.metrics?.latency_ms;
  if (!ms) return `<div class="dempty"><span>No timing data available</span></div>`;

  const dns  = s.metrics?.dns_time_ms     || 0;
  const tcp  = s.metrics?.tcp_connect_ms  || 0;
  const tls  = s.metrics?.tls_handshake_ms|| 0;
  const ttfb = s.metrics?.ttfb_ms || 0;
  const body = s.metrics?.body_ms || 0;
  const hasDetailed = dns || tcp || tls;

  let phases;
  if (hasDetailed) {
    const wait = Math.max(0, ttfb - dns - tcp - tls);
    const overhead = Math.max(0, ms - ttfb - body);
    phases = [
      {label:'DNS',      cls:'wf-dns',  ms:dns},
      {label:'TCP',      cls:'wf-tcp',  ms:tcp},
      {label:'TLS',      cls:'wf-tls',  ms:tls},
      {label:'Wait',     cls:'wf-wait', ms:wait},
      {label:'Download', cls:'wf-recv', ms:body},
      {label:'Overhead', cls:'wf-dns',  ms:overhead},
    ].filter(p=>p.ms>0);
  } else {
    const overhead = Math.max(0, ms - ttfb - body);
    phases = [
      {label:'TTFB',     cls:'wf-wait', ms:ttfb},
      {label:'Download', cls:'wf-recv', ms:body},
      {label:'Overhead', cls:'wf-dns',  ms:overhead},
    ].filter(p=>p.ms>0);
  }

  const total = phases.reduce((a,p)=>a+p.ms,0) || 1;
  let off=0;
  const bars = phases.map(p=>{
    const w = (p.ms/total*100).toFixed(1);
    const row = `<div class="wf-row">
      <div class="wf-lbl">${p.label}</div>
      <div class="wf-track"><div class="wf-bar ${p.cls}" style="left:${off.toFixed(1)}%;width:${w}%"></div></div>
      <div class="wf-ms">${p.ms}ms</div>
    </div>`;
    off += p.ms/total*100;
    return row;
  }).join('');

  const legend = phases.map(p=>`<div class="wf-leg"><div class="wf-dot ${p.cls}"></div>${p.label}</div>`).join('');

  const detailedRows = hasDetailed ? `
    <div class="kv"><span class="kk">DNS</span><span class="kv-val kv-mono">${esc(fmtMs(dns))}</span></div>
    <div class="kv"><span class="kk">TCP</span><span class="kv-val kv-mono">${esc(fmtMs(tcp))}</span></div>
    <div class="kv"><span class="kk">TLS</span><span class="kv-val kv-mono">${esc(fmtMs(tls))}</span></div>` : '';

  return `<div class="dsec">
    <div class="dsec-title">Waterfall</div>
    <div class="wf">${bars}</div>
    <div class="wf-legend">${legend}</div>
  </div>
  <div class="dsec">
    <div class="dsec-title">Summary</div>
    <div class="kv"><span class="kk">Total</span><span class="kv-val kv-mono">${esc(fmtMs(ms))}</span></div>
    ${detailedRows}
    <div class="kv"><span class="kk">TTFB</span><span class="kv-val kv-mono">${esc(fmtMs(ttfb))}</span></div>
    <div class="kv"><span class="kk">Download</span><span class="kv-val kv-mono">${esc(fmtMs(body))}</span></div>
    <div class="kv"><span class="kk">Req size</span><span class="kv-val kv-mono">${esc(fmtSz(s.metrics?.request_size_bytes))}</span></div>
    <div class="kv"><span class="kk">Res size</span><span class="kv-val kv-mono">${esc(fmtSz(s.metrics?.response_size_bytes))}</span></div>
  </div>`;
}

function renderFrames() {
  const frames = G.sel.ws_frames || [];
  if (!frames.length) return `<div class="dempty"><span>No frames recorded</span></div>`;
  const DIRS = {ClientToServer:'↑',ServerToClient:'↓'};
  const OPCODES = {1:'Text',2:'Binary',8:'Close',9:'Ping',10:'Pong'};
  const rows = frames.map((f,i) => {
    const dir = DIRS[f.direction] || '?';
    const op = OPCODES[f.opcode] || `0x${f.opcode.toString(16)}`;
    const ts = new Date(f.timestamp).toLocaleTimeString([], {hour12:false,hour:'2-digit',minute:'2-digit',second:'2-digit',fractionalSecondDigits:3});
    const payload = f.payload_text != null
      ? `<span style="color:var(--text)">${esc(f.payload_text.slice(0,120))}${f.payload_text.length>120?'…':''}</span>`
      : f.payload_hex
        ? `<span style="color:var(--text3);font-family:monospace;font-size:10px">${esc(f.payload_hex)}</span>`
        : `<span style="color:var(--text3)">—</span>`;
    const dirColor = f.direction==='ClientToServer' ? 'var(--accent)' : 'var(--success)';
    return `<tr>
      <td style="color:var(--text3);font-size:10px;white-space:nowrap">${ts}</td>
      <td style="color:${dirColor};font-weight:600;text-align:center">${dir}</td>
      <td style="color:var(--text2)">${op}</td>
      <td style="color:var(--text3)">${f.payload_len}</td>
      <td style="max-width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${payload}</td>
    </tr>`;
  }).join('');
  return `<div class="dsec" style="overflow:auto">
    <div class="dsec-title">WebSocket Frames (${frames.length})</div>
    <table style="width:100%;border-collapse:collapse;font-size:11px">
      <thead><tr style="color:var(--text3);border-bottom:0.5px solid var(--border)">
        <th style="text-align:left;padding:4px 6px;font-weight:500">Time</th>
        <th style="padding:4px 6px;font-weight:500">Dir</th>
        <th style="text-align:left;padding:4px 6px;font-weight:500">Type</th>
        <th style="text-align:right;padding:4px 6px;font-weight:500">Bytes</th>
        <th style="text-align:left;padding:4px 6px;font-weight:500">Payload</th>
      </tr></thead>
      <tbody>${rows}</tbody>
    </table>
  </div>`;
}

function renderCompose() {
  const s=G.sel;
  const method = s?.request?.method||'GET';
  const url = esc(s?.request?.uri||'');
  const hdrsText = esc(Object.entries(s?.request?.headers||{}).map(([k,v])=>`${k}: ${v}`).join('\n'));
  const bodyText = esc(s?.request?.body||'');
  const methods = ['GET','POST','PUT','DELETE','PATCH','HEAD','OPTIONS'];
  const opts = methods.map(m=>`<option${m===method?' selected':''}>${m}</option>`).join('');
  return `<div class="compose-wrap">
    <div class="compose-top">
      <select class="compose-method finput" id="cmp-method" style="width:90px;font-weight:700;color:var(--accent)">${opts}</select>
      <input class="compose-url" id="cmp-url" value="${url}" spellcheck="false" placeholder="https://…">
    </div>
    <div class="ctab-bar" style="margin-top:4px">
      <div class="ctab on" onclick="setCTab(this,'headers')">Headers</div>
      <div class="ctab" onclick="setCTab(this,'body')">Body</div>
    </div>
    <textarea class="compose-ta" id="cmp-headers" placeholder="Header-Name: value\nAnother: value">${hdrsText}</textarea>
    <textarea class="compose-ta" id="cmp-body" style="display:none" placeholder="Request body…">${bodyText}</textarea>
    <div style="display:flex;gap:6px;flex-wrap:wrap">
      <button class="btn btn-primary btn-sm" onclick="sendCompose()">
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M10.5 6H1.5M7.5 3l3 3-3 3"/></svg>
        Send
      </button>
      <button class="btn btn-ghost btn-sm" onclick="copyCurlCompose()">Copy as cURL</button>
    </div>
    <p class="note" style="font-size:11px">Sends the request through your configured proxy. The response will appear in the traffic table.</p>
  </div>`;
}

function setCTab(el, tab) {
  document.querySelectorAll('.ctab').forEach(t=>t.classList.remove('on'));
  el.classList.add('on');
  const hEl=document.getElementById('cmp-headers'), bEl=document.getElementById('cmp-body');
  if (hEl) hEl.style.display = tab==='headers' ? '' : 'none';
  if (bEl) bEl.style.display = tab==='body' ? '' : 'none';
}

async function sendCompose() {
  const url = document.getElementById('cmp-url')?.value?.trim();
  const method = document.getElementById('cmp-method')?.value||'GET';
  const rawHdrs = document.getElementById('cmp-headers')?.value||'';
  const body = document.getElementById('cmp-body')?.value||'';
  if (!url) { toast('URL is required', true); return; }
  const headers = {};
  rawHdrs.split('\n').forEach(line => {
    const i = line.indexOf(':');
    if (i>0) headers[line.slice(0,i).trim()] = line.slice(i+1).trim();
  });
  try {
    const opts = { method, headers };
    if (body && method!=='GET' && method!=='HEAD') opts.body = body;
    await fetch(url, opts);
    toast('Request sent — check the traffic table');
  } catch(e) {
    toast('Request failed (CORS or network error) — use cURL instead', true);
  }
}

// ── Copy as cURL ──────────────────────────────────────────────────────────────
function copyCurl() {
  const s = G.sel;
  if (!s) { toast('No request selected', true); return; }
  const hdrLines = Object.entries(s.request?.headers||{})
    .filter(([k]) => !INTERNAL_HDRS.has(k.toLowerCase()))
    .map(([k,v]) => `  -H '${k}: ${v.replace(/'/g,"'\\''")}' \\`)
    .join('\n');
  const bodyLine = s.request?.body ? `  -d '${s.request.body.replace(/'/g,"'\\''")}' \\` : '';
  const parts = [`curl -X ${s.request.method} \\`];
  if (hdrLines) parts.push(hdrLines);
  if (bodyLine) parts.push(bodyLine);
  parts.push(`  '${s.request.uri}'`);
  navigator.clipboard.writeText(parts.join('\n')).then(()=>toast('Copied as cURL'));
}

function copyCurlCompose() {
  const tab = G.compose.tabs.find(t => t.id === G.compose.activeTabId);
  if (!tab) { toast('No active Compose tab', true); return; }
  cmpSyncTabFromDom(tab);
  let url = cmpSubstVars(tab.url);
  if (!url) { toast('No URL in Compose tab', true); return; }
  const ps = tab.params.filter(p => p.enabled && p.key);
  if (ps.length) {
    const q = new URLSearchParams(ps.map(p => [p.key, cmpSubstVars(p.val)]));
    url += (url.includes('?') ? '&' : '?') + q.toString();
  }
  const hdrs = {};
  tab.headers.filter(h => h.enabled && h.key).forEach(h => { hdrs[cmpSubstVars(h.key)] = cmpSubstVars(h.val); });
  const method = tab.method;
  let body = '';
  if (method !== 'GET' && method !== 'HEAD') {
    if (tab.bodyMode === 'form') {
      const fd = new URLSearchParams();
      tab.bodyForm.filter(f => f.key).forEach(f => fd.append(cmpSubstVars(f.key), cmpSubstVars(f.val)));
      body = fd.toString();
      if (!hdrs['Content-Type']) hdrs['Content-Type'] = 'application/x-www-form-urlencoded';
    } else if (tab.bodyRaw) {
      body = tab.bodyRaw;
      if (!hdrs['Content-Type']) hdrs['Content-Type'] = tab.contentType;
    }
  }
  const hdrLines = Object.entries(hdrs)
    .map(([k,v]) => `  -H '${k}: ${v.replace(/'/g,"'\\''")}' \\`)
    .join('\n');
  const bodyLine = body ? `  -d '${body.replace(/'/g,"'\\''")}' \\` : '';
  const parts = [`curl -X ${method} \\`];
  if (hdrLines) parts.push(hdrLines);
  if (bodyLine) parts.push(bodyLine);
  parts.push(`  '${url}'`);
  navigator.clipboard.writeText(parts.join('\n')).then(() => toast('Copied as cURL'));
}

// ── Inspector tab ─────────────────────────────────────────────────────────────
function renderInspector() {
  const id = G.sel?.inspector_data;
  if (!id) return `<div class="dempty"><span>No inspector data for this request</span></div>`;

  let html = '';

  if (id.jwt) {
    const j = id.jwt;
    const expBadge = j.expired ? `<span class="sbadge s4">Expired</span>` : `<span class="sbadge s2">Valid</span>`;
    const algWarn = j.alg_none_warning
      ? `<div class="note warn" style="margin-top:8px">Algorithm &ldquo;none&rdquo; — token has no signature</div>` : '';
    html += `<div class="dsec">
      <div class="dsec-title">JWT</div>
      <div class="kv"><span class="kk">Expiry</span><span class="kv-val">${expBadge}</span></div>
      ${algWarn}
      <div class="dsec-title" style="margin-top:10px;margin-bottom:6px">Header</div>
      <pre class="body-pre" style="max-height:140px">${prettyJson(JSON.stringify(j.header))}</pre>
      <div class="dsec-title" style="margin-top:10px;margin-bottom:6px">Claims</div>
      <pre class="body-pre" style="max-height:200px">${prettyJson(JSON.stringify(j.claims))}</pre>
    </div>`;
  }

  if (id.graphql) {
    const g = id.graphql;
    const typeCls = {query:'s2',mutation:'s4',subscription:'s3'}[g.operation_type]||'s0';
    html += `<div class="dsec">
      <div class="dsec-title">GraphQL</div>
      <div class="kv"><span class="kk">Type</span><span class="kv-val"><span class="sbadge ${typeCls}">${esc(g.operation_type)}</span></span></div>
      ${g.operation_name ? `<div class="kv"><span class="kk">Name</span><span class="kv-val kv-mono">${esc(g.operation_name)}</span></div>` : ''}
      ${g.variables ? `<div class="dsec-title" style="margin-top:10px;margin-bottom:6px">Variables</div><pre class="body-pre" style="max-height:180px">${prettyJson(JSON.stringify(g.variables))}</pre>` : ''}
    </div>`;
  }

  if (id.grpc) {
    const g = id.grpc;
    html += `<div class="dsec">
      <div class="dsec-title">gRPC</div>
      ${g.service ? `<div class="kv"><span class="kk">Service</span><span class="kv-val kv-mono">${esc(g.service)}</span></div>` : ''}
      ${g.method  ? `<div class="kv"><span class="kk">Method</span><span class="kv-val kv-mono">${esc(g.method)}</span></div>`  : ''}
      <div class="kv"><span class="kk">Messages</span><span class="kv-val kv-mono">${(g.messages||[]).length}</span></div>
      ${(g.messages||[]).map((msg,i) => `
        <div style="margin-top:10px">
          <div class="dsec-title">Message ${i+1} <span style="text-transform:none;letter-spacing:0;font-weight:400">(${esc(msg.direction)}${msg.compressed?' · compressed':''})</span></div>
          ${(msg.fields||[]).map(f => `<div class="kv" style="margin-bottom:2px"><span class="kk" style="min-width:54px;font-family:var(--mono);font-size:10.5px">field ${f.field_number}</span><span class="kv-val kv-mono" style="word-break:break-all">${esc(JSON.stringify(f.value))}</span></div>`).join('')}
        </div>`).join('')}
    </div>`;
  }

  return html || `<div class="dempty"><span>No inspector data available</span></div>`;
}

// ── Session diff modal ────────────────────────────────────────────────────────
function openDiffModal() {
  if (!G.sel) { toast('Select a base session first', true); return; }
  G._diffBase = G.sel.id;
  document.getElementById('diff-base-label').textContent =
    (G.sel.request?.method||'') + ' ' + getPath(G.sel.request?.uri);
  const sel = document.getElementById('diff-target-select');
  sel.innerHTML = G.sessions
    .filter(s => s.id !== G.sel.id && !isProxy(s.request?.host))
    .map(s => {
      const label = (s.request?.method||'') + ' ' + getPath(s.request?.uri) +
        (s.metrics?.status_code ? ' · ' + s.metrics.status_code : '');
      return `<option value="${esc(s.id)}">${esc(label)}</option>`;
    }).join('');
  document.getElementById('diff-result').innerHTML = '';
  document.getElementById('modal-diff').classList.add('show');
}

async function runDiff() {
  const target = document.getElementById('diff-target-select').value;
  if (!target) { toast('Select a target session', true); return; }
  try {
    const r = await fetch(`/api/sessions/diff?a=${encodeURIComponent(G._diffBase)}&b=${encodeURIComponent(target)}`);
    if (!r.ok) { toast('Diff failed: ' + await r.text(), true); return; }
    document.getElementById('diff-result').innerHTML = buildDiffHtml(await r.json());
  } catch { toast('Diff failed', true); }
}

function buildDiffHtml(diff) {
  let html = '';
  const rd = diff.request_diff;
  if (rd) {
    html += `<div class="dsec-title" style="margin:10px 0 6px">Request</div>`;
    if (rd.method_changed) html += `<div class="note" style="margin-bottom:6px">Method changed</div>`;
    if (rd.uri_diff?.hunks?.length)
      html += `<div style="font-size:10px;color:var(--text4);margin:4px 0 2px">URI</div>` + _renderHunks(rd.uri_diff.hunks);
    if (rd.headers_added?.length)   html += `<div class="note" style="margin:4px 0">+ Headers: ${rd.headers_added.map(esc).join(', ')}</div>`;
    if (rd.headers_removed?.length) html += `<div class="note" style="margin:4px 0">- Headers: ${rd.headers_removed.map(esc).join(', ')}</div>`;
    if (rd.headers_changed?.length) html += `<div class="note" style="margin:4px 0">~ Changed: ${rd.headers_changed.map(c=>esc(c.name)).join(', ')}</div>`;
    if (rd.body_diff?.hunks?.length)
      html += `<div style="font-size:10px;color:var(--text4);margin:8px 0 2px">Request Body</div>` + _renderHunks(rd.body_diff.hunks);
  }
  const resd = diff.response_diff;
  if (resd) {
    html += `<div class="dsec-title" style="margin:14px 0 6px">Response</div>`;
    if (resd.status_changed)
      html += `<div class="note" style="margin-bottom:6px">Status: ${resd.status_changed[0]} → ${resd.status_changed[1]}</div>`;
    if (resd.headers_added?.length)   html += `<div class="note" style="margin:4px 0">+ Headers: ${resd.headers_added.map(esc).join(', ')}</div>`;
    if (resd.headers_removed?.length) html += `<div class="note" style="margin:4px 0">- Headers: ${resd.headers_removed.map(esc).join(', ')}</div>`;
    if (resd.body_diff?.hunks?.length)
      html += `<div style="font-size:10px;color:var(--text4);margin:8px 0 2px">Response Body</div>` + _renderHunks(resd.body_diff.hunks);
  }
  const t = diff.timing_delta;
  if (t && (t.latency_delta_ms || t.ttfb_delta_ms || t.size_delta_bytes)) {
    const sign = n => n > 0 ? '+' : '';
    html += `<div class="dsec-title" style="margin:14px 0 6px">Timing Delta</div>`;
    html += `<div class="kv"><span class="kk">Latency</span><span class="kv-val kv-mono" style="color:${t.latency_delta_ms>0?'var(--danger)':'var(--success)'}">${sign(t.latency_delta_ms)}${t.latency_delta_ms}ms</span></div>`;
    html += `<div class="kv"><span class="kk">TTFB</span><span class="kv-val kv-mono">${sign(t.ttfb_delta_ms)}${t.ttfb_delta_ms}ms</span></div>`;
    html += `<div class="kv"><span class="kk">Size</span><span class="kv-val kv-mono">${sign(t.size_delta_bytes)}${t.size_delta_bytes}B</span></div>`;
  }
  if (!html) html = `<p style="color:var(--text4);font-size:12px;padding:10px 0">Sessions are identical — no differences found.</p>`;
  return html;
}

function _renderHunks(hunks) {
  const lines = hunks.join('\n').split('\n').map(line => {
    const safe = esc(line);
    if (line.startsWith('+')) return `<span class="diff-add">${safe}</span>`;
    if (line.startsWith('-')) return `<span class="diff-del">${safe}</span>`;
    if (line.startsWith('@@')) return `<span class="diff-ctx">${safe}</span>`;
    return safe;
  });
  return `<pre class="diff-hunk">${lines.join('\n')}</pre>`;
}

