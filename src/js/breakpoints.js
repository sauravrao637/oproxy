// ── Breakpoints ───────────────────────────────────────────────────────────────
async function fetchBreakpoints() {
  try {
    const [r1, r2] = await Promise.all([fetch('/admin/breakpoints'), fetch('/admin/breakpoints/pending')]);
    G.bpRules = await r1.json();
    G.bpPending = await r2.json();
    renderBpRules();
    renderBpPending();
    document.getElementById('nb-bp').textContent = G.bpRules.filter(r=>r.enabled).length;
  } catch { toast('Failed to load breakpoints', true); }
}
function renderBpRules() {
  const tb = document.getElementById('bp-rules-tbody');
  if (!G.bpRules.length) { tb.innerHTML='<tr><td colspan="4" style="text-align:center;color:var(--text4);padding:20px">No rules — add one above</td></tr>'; return; }
  tb.innerHTML = G.bpRules.map(r=>`<tr>
    <td class="mono">${esc(r.pattern)}</td>
    <td><span class="mbadge ${r.bp_type==='Request'?'mGET':'mPOST'}">${esc(r.bp_type)}</span></td>
    <td style="color:${r.enabled?'var(--success)':'var(--text4)'}">${r.enabled?'On':'Off'}</td>
    <td><button class="btn btn-sm btn-danger" onclick="delBpRule('${r.id}')">✕</button></td>
  </tr>`).join('');
}
function renderBpPending() {
  const el = document.getElementById('bp-pending-list');
  if (!G.bpPending.length) { el.innerHTML='<div class="card-body"><p class="note">No breakpoints are paused right now.</p></div>'; return; }
  el.innerHTML = G.bpPending.map(bp=>renderBpItem(bp)).join('');
}
function renderBpItem(bp) {
  const isReq=!!bp.context.Request, ctx=isReq?bp.context.Request:bp.context.Response;
  const label=isReq?`${ctx.method} ${ctx.uri}`:`${ctx.status} ${ctx.request_uri||ctx.uri||''}`;
  const hdrs=ctx.headers||{};
  const hdrRows=Object.entries(hdrs).map(([k,v],i)=>`<tr>
    <td><input class="cmp-kv-input" id="bpe-hk-${bp.id}-${i}" value="${esc(k)}" placeholder="Header-Name" style="width:100%"></td>
    <td><input class="cmp-kv-input" id="bpe-hv-${bp.id}-${i}" value="${esc(v)}" placeholder="value" style="width:100%"></td>
    <td><button class="cmp-kv-del" onclick="this.closest('tr').remove()">✕</button></td>
  </tr>`).join('');
  const methodOpts=['GET','POST','PUT','DELETE','PATCH','HEAD','OPTIONS'].map(m=>`<option${m===(ctx.method||'')?'selected':''}>${m}</option>`).join('');
  return `<div style="border-bottom:0.5px solid var(--border)">
    <div style="padding:13px 15px;display:flex;align-items:center;gap:8px">
      <span class="mbadge ${isReq?'mGET':'mPOST'}">${isReq?'REQ':'RES'}</span>
      <span class="mono" style="font-size:11.5px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(label)}</span>
      <button class="btn btn-sm btn-primary" onclick="resolveBp('${bp.id}','continue')">Continue</button>
      <button class="btn btn-sm" onclick="toggleBpEdit('${bp.id}')">Edit…</button>
      <button class="btn btn-sm btn-danger" onclick="resolveBp('${bp.id}','drop')">Drop</button>
    </div>
    <div id="bp-edit-${bp.id}" class="bpe-panel" style="display:none">
      <div class="bpe-section-title">Edit ${isReq?'Request':'Response'}</div>
      ${isReq?`
      <div class="form-row" style="margin-bottom:8px">
        <div class="fgroup" style="max-width:90px">
          <label class="flabel">Method</label>
          <select class="finput fselect" id="bpe-method-${bp.id}">${methodOpts}</select>
        </div>
        <div class="fgroup">
          <label class="flabel">URL</label>
          <input class="finput" id="bpe-url-${bp.id}" value="${esc(ctx.uri||'')}" style="font-family:var(--mono)">
        </div>
      </div>`:
      `<div class="form-row" style="margin-bottom:8px">
        <div class="fgroup" style="max-width:90px">
          <label class="flabel">Status</label>
          <input class="finput" type="number" id="bpe-status-${bp.id}" value="${esc(ctx.status||200)}" style="font-family:var(--mono)">
        </div>
      </div>`}
      <div class="fgroup" style="margin-bottom:8px">
        <label class="flabel" style="margin-bottom:4px">Headers</label>
        <table class="cmp-kv-table" style="margin-top:2px">
          <thead><tr><th>Name</th><th>Value</th><th style="width:28px"></th></tr></thead>
          <tbody id="bpe-hdrs-${bp.id}">${hdrRows}</tbody>
        </table>
        <button class="btn btn-sm btn-ghost" style="margin-top:4px" onclick="bpeAddHdrRow('${bp.id}')">+ Add header</button>
      </div>
      <div class="fgroup" style="margin-bottom:10px">
        <label class="flabel" style="margin-bottom:4px">Body</label>
        <textarea class="finput" id="bpe-body-${bp.id}" rows="5" style="font-family:var(--mono);font-size:11px;resize:vertical">${esc(ctx.body||'')}</textarea>
      </div>
      <div style="display:flex;gap:6px;align-items:center">
        <button class="btn btn-primary btn-sm" onclick="resolveBpModify('${bp.id}')">
          <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M10.5 6H1.5M7.5 3l3 3-3 3"/></svg>
          Send Modified
        </button>
        <button class="btn btn-ghost btn-sm" onclick="toggleBpEdit('${bp.id}')">Cancel</button>
      </div>
    </div>
  </div>`;
}
function bpeAddHdrRow(id) {
  const tbody=document.getElementById(`bpe-hdrs-${id}`); if (!tbody) return;
  const i=tbody.rows.length;
  const tr=document.createElement('tr');
  tr.innerHTML=`<td><input class="cmp-kv-input" id="bpe-hk-${id}-${i}" placeholder="Header-Name" style="width:100%"></td><td><input class="cmp-kv-input" id="bpe-hv-${id}-${i}" placeholder="value" style="width:100%"></td><td><button class="cmp-kv-del" onclick="this.closest('tr').remove()">✕</button></td>`;
  tbody.appendChild(tr);
}
function toggleBpForm() { const f=document.getElementById('bp-form'); f.style.display=f.style.display?'':'none'; }
function toggleBpEdit(id) { const el=document.getElementById(`bp-edit-${id}`); el.style.display=el.style.display?'':'none'; }
async function saveBpRule() {
  const pattern=document.getElementById('bp-pattern').value.trim();
  if (!pattern){toast('Pattern is required',true);return;}
  const rule={id:'',pattern,bp_type:document.getElementById('bp-type').value,enabled:true};
  try {
    await fetch('/admin/breakpoints',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(rule)});
    document.getElementById('bp-pattern').value=''; toggleBpForm(); toast('Rule added'); await fetchBreakpoints();
  } catch { toast('Failed to save rule',true); }
}
async function delBpRule(id) {
  try { await fetch(`/admin/breakpoints/${id}`,{method:'DELETE'}); toast('Rule removed'); await fetchBreakpoints(); }
  catch { toast('Failed to remove rule',true); }
}
async function resolveBp(id, action, context=null) {
  const payload={action}; if(context) payload.context=context;
  try {
    await fetch(`/admin/breakpoints/pending/${id}/resolve`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});
    toast(action==='drop'?'Request dropped':'Breakpoint resolved'); await fetchBreakpoints();
  } catch { toast('Failed to resolve',true); }
}
async function resolveBpModify(id) {
  const bp=G.bpPending.find(b=>b.id===id); if (!bp) return;
  const isReq=!!bp.context.Request, orig=isReq?bp.context.Request:bp.context.Response;
  const newBody=document.getElementById(`bpe-body-${id}`)?.value??orig.body??'';
  // Collect headers from edit table
  const newHeaders={};
  const tbody=document.getElementById(`bpe-hdrs-${id}`);
  if (tbody) {
    tbody.querySelectorAll('tr').forEach(row=>{
      const inputs=row.querySelectorAll('input');
      if (inputs.length>=2) { const k=inputs[0].value.trim(),v=inputs[1].value.trim(); if(k) newHeaders[k]=v; }
    });
  }
  let context;
  if (isReq) {
    const method=document.getElementById(`bpe-method-${id}`)?.value||orig.method;
    const uri=document.getElementById(`bpe-url-${id}`)?.value||orig.uri;
    context={Request:{...orig,method,uri,headers:newHeaders,body:newBody}};
  } else {
    const status=parseInt(document.getElementById(`bpe-status-${id}`)?.value)||orig.status;
    context={Response:{...orig,status,headers:newHeaders,body:newBody}};
  }
  await resolveBp(id,'modify',context);
}

