import React from 'react';
const { Icon } = window;
/* Surfaces — Rules / Breakpoints / Inspectors / Root CA
   Activated via the left rail; each renders inside <main> instead of the
   sessions list/detail split. */

// ─── small primitives ──────────────────────────────────────────────────
function Toggle({ on, onChange, label = 'Toggle' }) {
  return <button className={'toggle' + (on ? ' on' : '')} onClick={() => onChange && onChange(!on)} aria-pressed={on} aria-label={label} />;
}

// Redirect to the login page, preserving the current URL as the post-login destination.
function redirectToLogin() {
  const next = encodeURIComponent(window.location.pathname + window.location.search);
  window.location.href = `/login?next=${next}`;
}

async function fetchJson(url, fallback) {
  try {
    // no-store: admin JSON has no Cache-Control, so without this the browser may
    // serve a stale cached response on the refetch right after a save/delete.
    const res = await fetch(url, { cache: 'no-store' });
    if (res.status === 401) { redirectToLogin(); return fallback; }
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return await res.json();
  } catch {
    return fallback;
  }
}

async function sendJson(url, method, body) {
  const res = await fetch(url, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body == null ? undefined : JSON.stringify(body),
  });
  if (res.status === 401) { redirectToLogin(); return res; }
  if (!res.ok) throw new Error(await res.text().catch(() => `HTTP ${res.status}`));
  return res;
}

function notifyError(message) {
  const el = document.createElement('div');
  el.className = 'ui-toast error';
  el.textContent = String(message || 'Action failed');
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 4500);
}

function notifyOk(message) {
  const el = document.createElement('div');
  el.className = 'ui-toast';
  el.textContent = String(message || 'Done');
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 3000);
}

function ask(label, value = '') {
  return new Promise(resolve => {
    const overlay = document.createElement('div');
    overlay.className = 'ui-dialog-backdrop';
    overlay.innerHTML = `
      <form class="ui-dialog">
        <h3>${label}</h3>
        <input class="cmp-input" value="${String(value || '').replace(/"/g, '&quot;')}" />
        <div class="ui-dialog-actions">
          <button type="button" class="btn ghost" data-cancel>Cancel</button>
          <button type="submit" class="btn primary">Save</button>
        </div>
      </form>`;
    document.body.appendChild(overlay);
    const input = overlay.querySelector('input');
    const close = (result) => { overlay.remove(); resolve(result); };
    overlay.querySelector('[data-cancel]').addEventListener('click', () => close(null));
    overlay.addEventListener('click', e => { if (e.target === overlay) close(null); });
    overlay.querySelector('form').addEventListener('submit', e => { e.preventDefault(); close(input.value.trim()); });
    input.focus(); input.select();
  });
}

function formDialog(title, fields) {
  return new Promise(resolve => {
    const overlay = document.createElement('div');
    overlay.className = 'ui-dialog-backdrop';
    const fieldHtml = fields.map(f => {
      const value = String(f.value || '').replace(/"/g, '&quot;');
      const label = String(f.label || f.name);
      if (f.type === 'select') {
        const options = (f.options || []).map(opt => {
          const selected = String(opt.value) === String(f.value || '') ? ' selected' : '';
          return `<option value="${String(opt.value).replace(/"/g, '&quot;')}"${selected}>${String(opt.label || opt.value)}</option>`;
        }).join('');
        return `<label class="ui-field"><span>${label}</span><select class="cmp-input" name="${f.name}">${options}</select></label>`;
      }
      if (f.type === 'textarea') {
        return `<label class="ui-field"><span>${label}</span><textarea class="cmp-input" name="${f.name}" rows="${f.rows || 4}">${String(f.value || '')}</textarea></label>`;
      }
      if (f.type === 'checkboxGroup') {
        const checks = (f.options || []).map(opt => {
          const checked = (f.value || []).includes(opt.value) ? ' checked' : '';
          return `<label class="ui-cb-row"><input type="checkbox" name="${f.name}" value="${String(opt.value).replace(/"/g, '&quot;')}"${checked} />${String(opt.label || opt.value)}</label>`;
        }).join('');
        return `<div class="ui-field"><span>${label}</span><div class="ui-cb-group">${checks}</div></div>`;
      }
      return `<label class="ui-field"><span>${label}</span><input class="cmp-input" name="${f.name}" value="${value}" placeholder="${String(f.placeholder || '').replace(/"/g, '&quot;')}" /></label>`;
    }).join('');
    overlay.innerHTML = `
      <form class="ui-dialog ui-form-dialog">
        <h3>${title}</h3>${fieldHtml}
        <div class="ui-dialog-actions">
          <button type="button" class="btn ghost" data-cancel>Cancel</button>
          <button type="submit" class="btn primary">Save</button>
        </div>
      </form>`;
    document.body.appendChild(overlay);
    const close = (result) => { overlay.remove(); resolve(result); };
    overlay.querySelector('[data-cancel]').addEventListener('click', () => close(null));
    overlay.addEventListener('click', e => { if (e.target === overlay) close(null); });
    overlay.querySelector('form').addEventListener('submit', e => {
      e.preventDefault();
      const data = {};
      fields.forEach(f => {
        if (f.type === 'checkboxGroup') {
          data[f.name] = Array.from(overlay.querySelectorAll(`[name="${f.name}"]:checked`)).map(el => el.value);
        } else {
          const el = overlay.querySelector(`[name="${f.name}"]`);
          data[f.name] = el ? el.value.trim() : '';
        }
      });
      close(data);
    });
    const first = overlay.querySelector('input, textarea, select');
    first?.focus(); first?.select?.();
  });
}

function confirmAction(message, confirmLabel = 'Confirm', tone = 'primary') {
  return new Promise(resolve => {
    const overlay = document.createElement('div');
    overlay.className = 'ui-dialog-backdrop';
    const buttonClass = tone === 'danger' ? 'btn danger' : 'btn primary';
    overlay.innerHTML = `
      <form class="ui-dialog">
        <h3>${message}</h3>
        <div class="ui-dialog-actions">
          <button type="button" class="btn ghost" data-cancel>Cancel</button>
          <button type="submit" class="${buttonClass}">${confirmLabel}</button>
        </div>
      </form>`;
    document.body.appendChild(overlay);
    const close = (result) => { overlay.remove(); resolve(result); };
    overlay.querySelector('[data-cancel]').addEventListener('click', () => close(false));
    overlay.addEventListener('click', e => { if (e.target === overlay) close(false); });
    overlay.querySelector('form').addEventListener('submit', e => { e.preventDefault(); close(true); });
  });
}

Object.assign(window, { Toggle, SurfaceShell, fetchJson, sendJson, notifyError, ask, formDialog, confirmAction, nonEmpty, Modal, LocationEditor });

function nonEmpty(v) {
  return v != null && String(v).trim() !== '';
}

function SurfaceShell({ title, sub, tabs, activeTab, onTab, actions, children }) {
  return (
    <div className="surface">
      <div className="surface-head">
        <div>
          <h2>{title}</h2>
          {sub && <div className="sub">{sub}</div>}
        </div>
        <div className="right">{actions}</div>
      </div>
      {tabs && (
        <div className="surface-tabs">
          {tabs.map(t => (
            <button key={t.key} className={'tab' + (activeTab === t.key ? ' on' : '')} aria-label={t.ariaLabel || t.label} onClick={() => onTab(t.key)}>
              {t.label}
              {!!t.count && <span className="pill">{t.count}</span>}
            </button>
          ))}
        </div>
      )}
      <div className="surface-body">{children}</div>
    </div>
  );
}

function RuleBadge({ kind, variant }) {
  return <span className={`rule-badge rb-${variant || 'match'}`}>{kind}</span>;
}

function RuleTable({ rows, onToggle, onEdit, onDelete, emptyTitle, emptyDesc }) {
  return (
    <div className="rule-list">
      <div className="rule-head rule-head-rich">
        <div></div>
        <div>Name / source</div>
        <div>Match</div>
        <div>Action</div>
        <div></div>
      </div>
      {rows.length === 0 && (
        <div className="empty" style={{ padding: '40px 24px', textAlign: 'left', maxWidth: 480 }}>
          <div style={{ fontWeight: 600, marginBottom: 6, color: 'var(--text)' }}>{emptyTitle || 'No rules yet'}</div>
          {emptyDesc && <div style={{ fontSize: 12, color: 'var(--text-mid)', lineHeight: 1.6 }}>{emptyDesc}</div>}
          <div style={{ marginTop: 10, fontSize: 12, color: 'var(--text-faint)' }}>Press <span className="key">+</span> to add one.</div>
        </div>
      )}
      {rows.map((r, i) => (
        <div key={i} className={'rule-row rule-row-rich' + (r.on ? '' : ' off')}>
          <div className="col-toggle">
            {r.toggle === false
              ? <span className="mute" style={{ fontSize: 13 }}>●</span>
              : <Toggle label={`Toggle rule ${r.name || i + 1}`} on={r.on} onChange={v => onToggle && onToggle(i, v)} />}
          </div>
          <div className="col-name" title={r.name}>{r.name || <span className="mute">—</span>}</div>
          <div className="col-match-rich col-match">
            <RuleBadge kind={r.matchKind || 'ANY'} variant={r.matchKind ? r.matchKind.toLowerCase().replace(/\s+/g,'') : 'any'} />
            {r.match && r.match !== r.name
              ? <code className="rule-pattern" title={r.match}>{r.match}</code>
              : <span className="mute" style={{ fontSize: 11 }}>all requests</span>}
          </div>
          <div className="col-action-rich">
            {r.actionKind && <RuleBadge kind={r.actionKind} variant="action" />}
            <span className="rule-action-text" title={r.action}>{r.action}</span>
          </div>
          <div className="col-act">
            {onEdit && <button className="copy-btn" onClick={() => onEdit(i, r)} aria-label={`Edit rule ${r.name || i + 1}`}>edit</button>}
            {onDelete && <button className="copy-btn" onClick={() => onDelete(i, r)} aria-label={`Delete rule ${r.name || i + 1}`}>×</button>}
          </div>
        </div>
      ))}
    </div>
  );
}

// ─── Rules surface ──────────────────────────────────────────────────────

const EMPTY_LOCATION = { host: null, path: null, port: null, protocol: null, query: null, methods: [], mode: 'glob' };

function summarizeLocation(loc) {
  if (!loc) return null;
  const parts = [];
  if (loc.host) parts.push(loc.host);
  if (loc.path) parts.push(loc.path);
  if (loc.port) parts.push(`:${loc.port}`);
  if (loc.protocol) parts.push(loc.protocol);
  if (loc.methods && loc.methods.length) parts.push(loc.methods.join(' '));
  if (loc.query) parts.push(`?${loc.query}`);
  return parts.join(' · ') || null;
}

// ── React modal overlay ─────────────────────────────────────────────────

function Modal({ title, onClose, onSave, saveLabel = 'Save', children }) {
  React.useEffect(() => {
    const h = (e) => { if (e.key === 'Escape') onClose(); };
    document.addEventListener('keydown', h);
    return () => document.removeEventListener('keydown', h);
  }, [onClose]);
  return (
    <div className="ui-dialog-backdrop" onClick={e => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="ui-dialog ui-form-dialog"
           style={{ maxWidth: 660, width: '92vw', maxHeight: '92vh', overflowY: 'auto', display: 'flex', flexDirection: 'column' }}
           onClick={e => e.stopPropagation()}>
        <h3 style={{ margin: '0 0 12px', flexShrink: 0 }}>{title}</h3>
        <div style={{ flex: 1, minHeight: 0 }}>{children}</div>
        <div className="ui-dialog-actions" style={{ flexShrink: 0 }}>
          <button className="btn ghost" onClick={onClose}>Cancel</button>
          <button className="btn primary" onClick={onSave}>{saveLabel}</button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, hint, children, row }) {
  return (
    <label className="ui-field" style={row ? { flexDirection: 'row', alignItems: 'center', gap: 8 } : {}}>
      <span style={row ? { flexShrink: 0, minWidth: 80 } : {}}>{label}</span>
      {children}
      {hint && <span style={{ fontSize: 11, color: 'var(--text-faint)', marginTop: 2, gridColumn: '1 / -1' }}>{hint}</span>}
    </label>
  );
}

// ── Location editor ─────────────────────────────────────────────────────

const METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS'];

function LocationEditor({ loc, onChange }) {
  const set = (k, v) => onChange({ ...loc, [k]: v || null });
  const toggleMethod = (m) => {
    const cur = loc.methods || [];
    onChange({ ...loc, methods: cur.includes(m) ? cur.filter(x => x !== m) : [...cur, m] });
  };
  const lbl = { fontSize: 12, color: 'var(--text-faint)', whiteSpace: 'nowrap' };

  return (
    <div style={{ background: 'rgba(0,0,0,0.04)', borderRadius: 6, padding: '10px 12px', marginBottom: 10 }}>
      <div style={{ fontSize: 10.5, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.07em', color: 'var(--text-faint)', marginBottom: 8 }}>
        Location — leave blank to match all
      </div>
      {/* 4-column inline grid: label | input | label | input */}
      <div style={{ display: 'grid', gridTemplateColumns: 'max-content 1fr max-content 1fr', gap: '5px 10px', alignItems: 'center', marginBottom: 6 }}>
        <span style={lbl}>Host</span>
        <input className="cmp-input" value={loc.host || ''} onChange={e => set('host', e.target.value)} placeholder="api.example.com" />
        <span style={lbl}>Path</span>
        <input className="cmp-input" value={loc.path || ''} onChange={e => set('path', e.target.value)} placeholder="/api/*" />

        <span style={lbl}>Query</span>
        <input className="cmp-input" value={loc.query || ''} onChange={e => set('query', e.target.value)} placeholder="key=value*" />
        <span style={lbl}>Port</span>
        <input className="cmp-input" type="number" min="1" max="65535" value={loc.port || ''} onChange={e => set('port', e.target.value ? Number(e.target.value) : null)} placeholder="any" />

        <span style={lbl}>Protocol</span>
        <select className="cmp-input" value={loc.protocol || ''} onChange={e => set('protocol', e.target.value)}>
          <option value="">any</option>
          <option value="http">http</option>
          <option value="https">https</option>
        </select>
        <span style={lbl}>Match mode</span>
        <select className="cmp-input" value={loc.mode || 'glob'} onChange={e => onChange({ ...loc, mode: e.target.value })}>
          <option value="glob">Glob (* ? wildcards)</option>
          <option value="regex">Regex (unanchored)</option>
        </select>
      </div>
      {/* Methods on a single row */}
      <div style={{ display: 'flex', alignItems: 'center', gap: '4px 10px', flexWrap: 'wrap' }}>
        <span style={{ ...lbl, marginRight: 2 }}>Methods</span>
        {METHODS.map(m => (
          <label key={m} style={{ display: 'flex', alignItems: 'center', gap: 3, fontSize: 12, cursor: 'pointer', whiteSpace: 'nowrap' }}>
            <input type="checkbox" checked={(loc.methods || []).includes(m)} onChange={() => toggleMethod(m)} />
            {m}
          </label>
        ))}
        <span style={{ fontSize: 11, color: 'var(--text-faint)', marginLeft: 2 }}>(blank = any)</span>
      </div>
    </div>
  );
}

// ── Actions editor (for RewriteRuleSet) ────────────────────────────────

const ACTION_TYPES = [
  { value: 'set_header',        label: 'Set header' },
  { value: 'append_header',     label: 'Append header' },
  { value: 'remove_header',     label: 'Remove header' },
  { value: 'set_query_param',   label: 'Set query param' },
  { value: 'remove_query_param',label: 'Remove query param' },
  { value: 'set_host',          label: 'Set host' },
  { value: 'set_path',          label: 'Set path' },
  { value: 'set_status',        label: 'Set status code' },
  { value: 'replace_body',      label: 'Replace body' },
  { value: 'redirect',          label: 'Redirect' },
  { value: 'block',             label: 'Block' },
];

function defaultAction(type) {
  switch (type) {
    case 'set_header':         return { type, name: '', value: '' };
    case 'append_header':      return { type, name: '', value: '' };
    case 'remove_header':      return { type, name: '' };
    case 'set_query_param':    return { type, name: '', value: '' };
    case 'remove_query_param': return { type, name: '' };
    case 'set_host':           return { type, value: '' };
    case 'set_path':           return { type, pattern: '', replacement: '' };
    case 'set_status':         return { type, code: 200 };
    case 'replace_body':       return { type, pattern: '', replacement: '' };
    case 'redirect':           return { type, status: 302, location: '' };
    case 'block':              return { type, status: 403 };
    default:                   return { type: 'set_header', name: '', value: '' };
  }
}

function summarizeActions(actions) {
  if (!actions || !actions.length) return 'no actions';
  return actions.map(a => {
    switch (a.type) {
      case 'set_header':         return `set ${a.name}`;
      case 'append_header':      return `append ${a.name}`;
      case 'remove_header':      return `rm ${a.name}`;
      case 'set_query_param':    return `?${a.name}=…`;
      case 'remove_query_param': return `rm ?${a.name}`;
      case 'set_host':           return `host→${a.value}`;
      case 'set_path':           return `path ${a.pattern}→${a.replacement}`;
      case 'set_status':         return `${a.code}`;
      case 'replace_body':       return `body ${a.pattern}→${a.replacement}`;
      case 'redirect':           return `→${a.location || a.status}`;
      case 'block':              return `block ${a.status}`;
      default:                   return a.type;
    }
  }).join(', ');
}

function ActionRow({ action, onChange, onRemove }) {
  const set = (k, v) => onChange({ ...action, [k]: v });
  const inp = (props) => <input className="cmp-input" style={{ flex: 1, minWidth: 60 }} {...props} />;
  return (
    <div style={{ display: 'flex', gap: 6, alignItems: 'center', marginBottom: 6 }}>
      <select className="cmp-input" style={{ flexShrink: 0, width: 160 }}
              value={action.type} onChange={e => onChange(defaultAction(e.target.value))}>
        {ACTION_TYPES.map(t => <option key={t.value} value={t.value}>{t.label}</option>)}
      </select>
      {(action.type === 'set_header' || action.type === 'append_header') && <>
        {inp({ value: action.name || '', onChange: e => set('name', e.target.value), placeholder: 'header-name', style: { flex: 1, minWidth: 60 } })}
        {inp({ value: action.value || '', onChange: e => set('value', e.target.value), placeholder: 'value' })}
      </>}
      {action.type === 'remove_header' &&
        inp({ value: action.name || '', onChange: e => set('name', e.target.value), placeholder: 'header-name' })}
      {(action.type === 'set_query_param') && <>
        {inp({ value: action.name || '', onChange: e => set('name', e.target.value), placeholder: 'param' })}
        {inp({ value: action.value || '', onChange: e => set('value', e.target.value), placeholder: 'value' })}
      </>}
      {action.type === 'remove_query_param' &&
        inp({ value: action.name || '', onChange: e => set('name', e.target.value), placeholder: 'param' })}
      {action.type === 'set_host' &&
        inp({ value: action.value || '', onChange: e => set('value', e.target.value), placeholder: 'staging.example.com' })}
      {action.type === 'set_path' && <>
        {inp({ value: action.pattern || '', onChange: e => set('pattern', e.target.value), placeholder: '^/api/v1' })}
        {inp({ value: action.replacement || '', onChange: e => set('replacement', e.target.value), placeholder: '/api/v2' })}
      </>}
      {action.type === 'set_status' &&
        <input className="cmp-input" type="number" min="100" max="599" style={{ width: 80 }}
               value={action.code || 200} onChange={e => set('code', Number(e.target.value))} />}
      {action.type === 'replace_body' && <>
        {inp({ value: action.pattern || '', onChange: e => set('pattern', e.target.value), placeholder: 'find regex' })}
        <textarea className="cmp-input" rows={2} style={{ flex: 1, fontFamily: 'var(--font-mono)', fontSize: 12 }}
                  value={action.replacement || ''} onChange={e => set('replacement', e.target.value)} placeholder="replacement" />
      </>}
      {action.type === 'redirect' && <>
        <input className="cmp-input" type="number" min="300" max="399" style={{ width: 70 }}
               value={action.status || 302} onChange={e => set('status', Number(e.target.value))} />
        {inp({ value: action.location || '', onChange: e => set('location', e.target.value), placeholder: 'https://…' })}
      </>}
      {action.type === 'block' &&
        <input className="cmp-input" type="number" min="400" max="599" style={{ width: 80 }}
               value={action.status || 403} onChange={e => set('status', Number(e.target.value))} />}
      <button className="copy-btn" onClick={onRemove} title="Remove action" style={{ flexShrink: 0 }}>×</button>
    </div>
  );
}

function ActionsEditor({ actions, onChange }) {
  const add = () => onChange([...actions, defaultAction('set_header')]);
  const update = (i, a) => onChange(actions.map((x, j) => j === i ? a : x));
  const remove = (i) => onChange(actions.filter((_, j) => j !== i));
  return (
    <div>
      <div style={{ fontSize: 10.5, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.07em', color: 'var(--text-faint)', marginBottom: 8 }}>
        Actions  <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0, fontSize: 11 }}>— applied in order</span>
      </div>
      {actions.map((a, i) => (
        <ActionRow key={i} action={a} onChange={a2 => update(i, a2)} onRemove={() => remove(i)} />
      ))}
      {actions.length === 0 && (
        <div style={{ fontSize: 12, color: 'var(--text-faint)', marginBottom: 8 }}>No actions yet — add at least one.</div>
      )}
      <button className="btn ghost" style={{ fontSize: 12, marginTop: 4 }} onClick={add}>+ Add action</button>
    </div>
  );
}

// ── RuleSet modal ───────────────────────────────────────────────────────

function RuleSetModal({ rule, onClose, onSave }) {
  const isNew = !rule;
  const [name, setName] = React.useState(rule?.name || '');
  const [enabled, setEnabled] = React.useState(rule ? rule.enabled : true);
  const [appliesTo, setAppliesTo] = React.useState((rule?.applies_to || 'both').toLowerCase());
  const [loc, setLoc] = React.useState(rule?.location ? { ...EMPTY_LOCATION, ...rule.location } : { ...EMPTY_LOCATION });
  const [actions, setActions] = React.useState(rule?.actions || []);

  const save = async () => {
    if (!name.trim()) { notifyError('Name is required'); return; }
    const body = { id: rule?.id || '', name: name.trim(), enabled, applies_to: appliesTo, location: loc, actions };
    try {
      if (isNew) {
        await sendJson('/admin/rule-sets', 'POST', body);
      } else {
        await sendJson(`/admin/rule-sets/${rule.id}`, 'PUT', body);
      }
      onSave();
    } catch (e) { notifyError(e.message || e); }
  };

  return (
    <Modal title={isNew ? 'New rule set' : `Edit — ${rule.name}`} onClose={onClose} onSave={save}>
      {/* Name + Applies-to + Enabled on one row */}
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 10 }}>
        <input className="cmp-input" style={{ flex: 1 }} value={name} onChange={e => setName(e.target.value)} placeholder="e.g. Add CORS headers" autoFocus />
        <select className="cmp-input" style={{ flexShrink: 0, width: 'auto' }} value={appliesTo} onChange={e => setAppliesTo(e.target.value)}>
          <option value="both">Req &amp; Resp</option>
          <option value="request">Request</option>
          <option value="response">Response</option>
        </select>
        <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, cursor: 'pointer', flexShrink: 0 }}>
          <Toggle on={enabled} onChange={setEnabled} label="Enabled" />
          Enabled
        </label>
      </div>
      <LocationEditor loc={loc} onChange={setLoc} />
      <ActionsEditor actions={actions} onChange={setActions} />
    </Modal>
  );
}

// ── Map Local path field: type a path, upload a file, or paste content ────
// Uploaded/pasted fixtures are stored server-side in storage/map-local/ and
// referenced by name — no restart needed, served on the next matching request.

function MapLocalPathField({ value, onChange, onInline, inlineBody, hint, placeholder }) {
  const fileRef = React.useRef(null);
  const [busy, setBusy] = React.useState(false);
  const pasteOpen = inlineBody != null;

  // Binary/large files: upload to the managed fixtures dir and reference by name.
  const onFile = async (e) => {
    const f = e.target.files?.[0];
    if (!f) { return; }
    setBusy(true);
    try {
      const res = await fetch(`/admin/map-local-rules/fixtures/${encodeURIComponent(f.name)}`, { method: 'POST', body: f });
      if (!res.ok) {
        const j = await res.json().catch(() => ({}));
        throw new Error(j.error || `Upload failed (${res.status})`);
      }
      const j = await res.json();
      onChange(j.name);
      notifyOk(`Uploaded "${j.name}" — referenced by this rule.`);
    } catch (err) { notifyError(err.message || err); }
    finally { setBusy(false); e.target.value = ''; }
  };

  // Paste: stash the content on the rule as inline_body; it is written to
  // storage/map-local/ atomically when the rule is saved. No separate request.
  // When opening on an existing managed fixture, load its current content so
  // editing shows what was previously pasted/uploaded.
  const togglePaste = async () => {
    if (pasteOpen) { onInline(null); return; }
    const name = value.trim();
    const isManagedName = name && !name.startsWith('/') && !name.includes('/');
    if (!isManagedName) {
      onInline('');
      if (!name) { onChange('pasted.json'); }
      return;
    }
    setBusy(true);
    try {
      const res = await fetch(`/admin/map-local-rules/fixtures/${encodeURIComponent(name)}`, { cache: 'no-store' });
      onInline(res.ok ? await res.text() : '');
    } catch { onInline(''); }
    finally { setBusy(false); }
  };

  return (
    <div style={{ gridColumn: '2' }}>
      <div style={{ display: 'flex', gap: 6 }}>
        <input className="cmp-input" style={{ flex: 1 }} value={value} onChange={e => onChange(e.target.value)} placeholder={placeholder} />
        <button type="button" className="copy-btn" disabled={busy} onClick={() => fileRef.current?.click()}>{busy ? '…' : 'upload'}</button>
        <button type="button" className={'copy-btn' + (pasteOpen ? ' on' : '')} onClick={togglePaste}>paste</button>
        <input ref={fileRef} type="file" style={{ display: 'none' }} onChange={onFile} />
      </div>
      {pasteOpen && (
        <div style={{ marginTop: 6, display: 'grid', gap: 4 }}>
          <div style={{ fontSize: 11, color: 'var(--text-faint)' }}>
            Paste content below and set the file name above (e.g. <code>users.json</code>). It's saved to storage/map-local/ when you click Save.
          </div>
          <textarea className="cmp-input" rows={5} style={{ fontFamily: 'monospace', fontSize: 12 }} value={inlineBody} onChange={e => onInline(e.target.value)} placeholder="paste file content…" autoFocus />
        </div>
      )}
      {hint && <div style={{ fontSize: 11, color: 'var(--text-faint)', marginTop: 4, lineHeight: 1.5 }}>{hint}</div>}
    </div>
  );
}

// ── Generic simple rule modal (Map Remote / Map Local / Access) ──────────

function SimpleRuleModal({ title, rule, extraFields, onClose, onSave }) {
  const isNew = !rule;
  const [name, setName] = React.useState(rule?.name || '');
  const [enabled, setEnabled] = React.useState(rule ? rule.enabled : true);
  const [loc, setLoc] = React.useState(rule?.location ? { ...EMPTY_LOCATION, ...rule.location } : { ...EMPTY_LOCATION });
  const [extra, setExtra] = React.useState(() => {
    const init = {};
    (extraFields || []).forEach(f => { init[f.key] = rule?.[f.key] ?? f.default ?? ''; });
    return init;
  });
  const setE = (k, v) => setExtra(p => ({ ...p, [k]: v }));

  const save = async () => {
    if (!name.trim()) { notifyError('Name is required'); return; }
    try {
      await onSave({ name: name.trim(), enabled, location: loc, ...extra });
    } catch (e) { notifyError(e.message || e); }
  };

  return (
    <Modal title={title} onClose={onClose} onSave={save}>
      {/* Name + Enabled on one row */}
      <div style={{ display: 'flex', gap: 10, alignItems: 'center', marginBottom: 10 }}>
        <input className="cmp-input" style={{ flex: 1 }} value={name} onChange={e => setName(e.target.value)} placeholder="Name" autoFocus />
        <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, cursor: 'pointer', flexShrink: 0 }}>
          <Toggle on={enabled} onChange={setEnabled} label="Enabled" />
          Enabled
        </label>
      </div>
      <LocationEditor loc={loc} onChange={setLoc} />
      {(extraFields || []).map(f => (
        <div key={f.key} style={{ display: 'grid', gridTemplateColumns: 'max-content 1fr', gap: '5px 10px', alignItems: f.type === 'file' ? 'start' : 'center' }}>
          <span style={{ fontSize: 12, color: 'var(--text-faint)', whiteSpace: 'nowrap', marginTop: f.type === 'file' ? 7 : 0 }}>{f.label}</span>
          {f.type === 'file'
            ? <MapLocalPathField
                value={extra[f.key] ?? ''}
                onChange={v => setE(f.key, v)}
                onInline={b => setExtra(p => ({ ...p, inline_body: b == null ? undefined : b }))}
                inlineBody={extra.inline_body}
                hint={f.hint} placeholder={f.placeholder} />
            : f.type === 'select'
            ? <select className="cmp-input" value={extra[f.key] ?? ''} onChange={e => setE(f.key, e.target.value)}>
                {(f.options || []).map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
            : <input className="cmp-input" value={extra[f.key] ?? ''} onChange={e => setE(f.key, e.target.value)} placeholder={f.placeholder} />
          }
          {f.hint && f.type !== 'file' && <span style={{ fontSize: 11, color: 'var(--text-faint)', gridColumn: '2', marginTop: -2 }}>{f.hint}</span>}
        </div>
      ))}
    </Modal>
  );
}

// ── Generic rule list ───────────────────────────────────────────────────

function GenericRuleList({ rules, renderExtra, onToggle, onEdit, onDelete, emptyTitle, emptyDesc }) {
  return (
    <div className="rule-list">
      {rules.length === 0 && (
        <div className="empty" style={{ padding: '40px 24px', textAlign: 'left', maxWidth: 520 }}>
          <div style={{ fontWeight: 600, marginBottom: 6, color: 'var(--text)' }}>{emptyTitle || 'No rules yet'}</div>
          {emptyDesc && <div style={{ fontSize: 12, color: 'var(--text-mid)', lineHeight: 1.6 }}>{emptyDesc}</div>}
          <div style={{ marginTop: 10, fontSize: 12, color: 'var(--text-faint)' }}>Press <span className="key">+</span> to add one.</div>
        </div>
      )}
      {rules.map((r, i) => {
        const locStr = summarizeLocation(r.location);
        return (
          <div key={r.id || i} className={'rule-row rule-row-rich' + (r.enabled ? '' : ' off')}>
            <div className="col-toggle">
              <Toggle label={`Toggle ${r.name || i + 1}`} on={!!r.enabled} onChange={v => onToggle(r, v)} />
            </div>
            <div className="col-name" title={r.name}>{r.name || <span className="mute">—</span>}</div>
            <div className="col-match-rich col-match">
              {locStr
                ? <code className="rule-pattern" title={locStr}>{locStr}</code>
                : <span className="mute" style={{ fontSize: 11 }}>all requests</span>}
            </div>
            <div className="col-action-rich" style={{ fontSize: 12 }}>
              {renderExtra(r)}
            </div>
            <div className="col-act">
              <button className="copy-btn" onClick={() => onEdit(r)}>edit</button>
              <button className="copy-btn" onClick={() => onDelete(r)}>×</button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ── Rules tab (RewriteRuleSet) ──────────────────────────────────────────

function RuleSetTab({ rules, onReload, onAdd, editTarget, setEditTarget }) {
  const toggle = async (r, v) => {
    try { await sendJson(`/admin/rule-sets/${r.id}`, 'PUT', { ...r, enabled: v }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const del = async (r) => {
    if (!await confirmAction(`Delete "${r.name}"?`, 'Delete', 'danger')) return;
    try { await fetch(`/admin/rule-sets/${r.id}`, { method: 'DELETE' }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };

  return (
    <>
      <div className="rule-list">
        {rules.length === 0 && (
          <div className="empty" style={{ padding: '40px 24px', textAlign: 'left', maxWidth: 520 }}>
            <div style={{ fontWeight: 600, marginBottom: 6, color: 'var(--text)' }}>No rule sets</div>
            <div style={{ fontSize: 12, color: 'var(--text-mid)', lineHeight: 1.6 }}>
              Rule sets match by location and run an ordered list of actions — set headers, redirect, block, rewrite paths, and more.
            </div>
            <div style={{ marginTop: 10, fontSize: 12, color: 'var(--text-faint)' }}>Press <span className="key">+</span> to add one.</div>
          </div>
        )}
        {rules.map((r, i) => {
          const locStr = summarizeLocation(r.location);
          const actStr = summarizeActions(r.actions);
          return (
            <div key={r.id || i} className={'rule-row rule-row-rich' + (r.enabled ? '' : ' off')}>
              <div className="col-toggle">
                <Toggle label={`Toggle ${r.name}`} on={!!r.enabled} onChange={v => toggle(r, v)} />
              </div>
              <div className="col-name" title={r.name}>{r.name || <span className="mute">—</span>}</div>
              <div className="col-match-rich col-match">
                {locStr
                  ? <code className="rule-pattern" title={locStr}>{locStr}</code>
                  : <span className="mute" style={{ fontSize: 11 }}>all requests</span>}
              </div>
              <div className="col-action-rich" style={{ fontSize: 12 }}>
                <RuleBadge kind={r.applies_to || 'Both'} variant="action" />
                <span className="rule-action-text" title={actStr}>{actStr}</span>
              </div>
              <div className="col-act">
                <button className="copy-btn" onClick={() => setEditTarget(r)}>edit</button>
                <button className="copy-btn" onClick={() => del(r)}>×</button>
              </div>
            </div>
          );
        })}
      </div>
      {(editTarget !== undefined) && (
        <RuleSetModal
          rule={editTarget}
          onClose={() => setEditTarget(undefined)}
          onSave={async () => { setEditTarget(undefined); await onReload(); }}
        />
      )}
    </>
  );
}

// ── Map Remote tab ──────────────────────────────────────────────────────

function MapRemoteTab({ rules, onReload, editTarget, setEditTarget }) {
  const toggle = async (r, v) => {
    try { await sendJson(`/admin/map-remote-rules/${r.id}`, 'PUT', { ...r, enabled: v }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const del = async (r) => {
    if (!await confirmAction(`Delete "${r.name}"?`, 'Delete', 'danger')) return;
    try { await fetch(`/admin/map-remote-rules/${r.id}`, { method: 'DELETE' }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const saveRule = async (data) => {
    if (editTarget && editTarget.id) {
      await sendJson(`/admin/map-remote-rules/${editTarget.id}`, 'PUT', { id: editTarget.id, ...data });
    } else {
      await sendJson('/admin/map-remote-rules', 'POST', { id: '', ...data });
    }
    setEditTarget(undefined);
    await onReload();
  };

  const fields = [{ key: 'destination', label: 'Destination URL', placeholder: 'http://10.0.0.1:3000', hint: 'Origin is replaced; path and query are preserved.' }];

  return (
    <>
      <GenericRuleList
        rules={rules}
        renderExtra={r => <code className="rule-pattern" style={{ color: 'var(--c-2xx)' }}>{r.destination}</code>}
        onToggle={toggle} onEdit={setEditTarget} onDelete={del}
        emptyTitle="No Map Remote rules"
        emptyDesc="Map Remote routes matching requests to a different upstream origin. The path and query string are preserved — only the origin changes." />
      {editTarget !== undefined && (
        <SimpleRuleModal
          title={editTarget && editTarget.id ? `Edit — ${editTarget.name}` : 'New Map Remote rule'}
          rule={editTarget || null}
          extraFields={fields}
          onClose={() => setEditTarget(undefined)}
          onSave={saveRule}
        />
      )}
    </>
  );
}

// ── Map Local tab ───────────────────────────────────────────────────────

function MapLocalTab({ rules, onReload, editTarget, setEditTarget }) {
  const toggle = async (r, v) => {
    try { await sendJson(`/admin/map-local-rules/${r.id}`, 'PUT', { ...r, enabled: v }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const del = async (r) => {
    if (!await confirmAction(`Delete "${r.name}"?`, 'Delete', 'danger')) return;
    try { await fetch(`/admin/map-local-rules/${r.id}`, { method: 'DELETE' }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const saveRule = async (data) => {
    if (editTarget && editTarget.id) {
      await sendJson(`/admin/map-local-rules/${editTarget.id}`, 'PUT', { id: editTarget.id, ...data });
    } else {
      await sendJson('/admin/map-local-rules', 'POST', { id: '', ...data });
    }
    setEditTarget(undefined);
    await onReload();
  };

  const fields = [{ key: 'file_path', label: 'Local path', type: 'file', placeholder: 'users.json or /absolute/path/or/dir', hint: 'Upload or paste to store a fixture in storage/map-local/, or type any path. A file is returned for every matching request; a directory maps the request path to a file inside it.' }];

  return (
    <>
      <GenericRuleList
        rules={rules}
        renderExtra={r => <code className="rule-pattern" style={{ fontSize: 11, wordBreak: 'break-all' }}>{r.file_path}</code>}
        onToggle={toggle} onEdit={setEditTarget} onDelete={del}
        emptyTitle="No Map Local rules"
        emptyDesc="Map Local serves a local file (or directory) as the response for matching requests, bypassing the upstream entirely. Great for mocking API endpoints from fixture files." />
      {editTarget !== undefined && (
        <SimpleRuleModal
          title={editTarget && editTarget.id ? `Edit — ${editTarget.name}` : 'New Map Local rule'}
          rule={editTarget || null}
          extraFields={fields}
          onClose={() => setEditTarget(undefined)}
          onSave={saveRule}
        />
      )}
    </>
  );
}

// ── Access tab ──────────────────────────────────────────────────────────

function AccessTab({ rules, onReload, editTarget, setEditTarget }) {
  const toggle = async (r, v) => {
    try { await sendJson(`/admin/access-rules/${r.id}`, 'PUT', { ...r, enabled: v }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const del = async (r) => {
    if (!await confirmAction(`Delete "${r.name}"?`, 'Delete', 'danger')) return;
    try { await fetch(`/admin/access-rules/${r.id}`, { method: 'DELETE' }); await onReload(); }
    catch (e) { notifyError(e.message || e); }
  };
  const saveRule = async (data) => {
    if (editTarget && editTarget.id) {
      await sendJson(`/admin/access-rules/${editTarget.id}`, 'PUT', { id: editTarget.id, ...data });
    } else {
      await sendJson('/admin/access-rules', 'POST', { id: '', ...data });
    }
    setEditTarget(undefined);
    await onReload();
  };

  const fields = [{
    key: 'action', label: 'Action', type: 'select', default: 'block',
    options: [{ value: 'block', label: 'Block — deny matching requests (403)' }, { value: 'allow', label: 'Allow — only allow matching (block everything else)' }],
  }];

  const accessBadge = (r) => (
    <RuleBadge
      kind={r.action === 'allow' ? 'ALLOW' : 'BLOCK'}
      variant={r.action === 'allow' ? 'any' : 'action'}
    />
  );

  return (
    <>
      <div style={{ padding: '10px 16px', fontSize: 12, color: 'var(--text-mid)', lineHeight: 1.6 }}>
        <strong>Block</strong> rules 403 matching requests. <strong>Allow</strong> rules create an allowlist — if any exist, only matching requests pass. Block takes priority over Allow.
      </div>
      <GenericRuleList
        rules={rules}
        renderExtra={accessBadge}
        onToggle={toggle} onEdit={setEditTarget} onDelete={del}
        emptyTitle="No access rules"
        emptyDesc="Block specific hosts or paths, or build an allowlist to restrict which requests the proxy forwards." />
      {editTarget !== undefined && (
        <SimpleRuleModal
          title={editTarget && editTarget.id ? `Edit — ${editTarget.name}` : 'New access rule'}
          rule={editTarget || null}
          extraFields={fields}
          onClose={() => setEditTarget(undefined)}
          onSave={saveRule}
        />
      )}
    </>
  );
}

// ── Main RulesSurface ───────────────────────────────────────────────────

function RulesSurface() {
  const [tab, setTab] = React.useState('rules');
  const [ruleSets, setRuleSets] = React.useState([]);
  const [mapRemote, setMapRemote] = React.useState([]);
  const [mapLocal, setMapLocal] = React.useState([]);
  const [access, setAccess] = React.useState([]);
  const [throttle, setThrottle] = React.useState({ enabled: false, preset: 'off', latency: 0, downKbps: 0 });
  // editTarget: undefined = modal closed, null = new rule, object = editing that rule
  const [rsEdit, setRsEdit] = React.useState(undefined);
  const [mrEdit, setMrEdit] = React.useState(undefined);
  const [mlEdit, setMlEdit] = React.useState(undefined);
  const [acEdit, setAcEdit] = React.useState(undefined);

  const load = React.useCallback(async () => {
    const [rs, mr, ml, ac, th] = await Promise.all([
      fetchJson('/admin/rule-sets', []),
      fetchJson('/admin/map-remote-rules', []),
      fetchJson('/admin/map-local-rules', []),
      fetchJson('/admin/access-rules', []),
      fetchJson('/admin/throttling', {}),
    ]);
    setRuleSets(rs || []);
    setMapRemote(mr || []);
    setMapLocal(ml || []);
    setAccess(ac || []);
    setThrottle({
      enabled: !!th?.enabled,
      preset: th?.enabled ? 'custom' : 'off',
      latency: th?.latency_ms || 0,
      downKbps: th?.bandwidth_limit_kbps || 0,
    });
  }, []);

  React.useEffect(() => { load(); }, [load]);

  const saveThrottle = async (cfg) => {
    try {
      await sendJson('/admin/throttling', 'POST', {
        enabled: !!cfg.enabled,
        latency_ms: Number(cfg.latency) || 0,
        bandwidth_limit_kbps: Number(cfg.downKbps) || 0,
      });
      await load();
    } catch (e) { notifyError(`Failed to save throttling: ${e.message || e}`); }
  };

  const openAdd = () => {
    if (tab === 'rules')     setRsEdit(null);
    if (tab === 'mapremote') setMrEdit(null);
    if (tab === 'maplocal')  setMlEdit(null);
    if (tab === 'access')    setAcEdit(null);
  };

  const tabs = [
    { key: 'rules',     label: 'Rules',      ariaLabel: 'Rule sets', count: ruleSets.length },
    { key: 'mapremote', label: 'Map Remote',  count: mapRemote.length },
    { key: 'maplocal',  label: 'Map Local',   count: mapLocal.length },
    { key: 'access',    label: 'Access',      count: access.length },
    { key: 'throttle',  label: 'Throttling',  count: throttle.enabled ? '●' : null },
  ];

  const actions = tab !== 'throttle' && (
    <button className="btn primary" onClick={openAdd}>
      <span style={{ fontSize: 14, lineHeight: 0 }}>＋</span> Add rule
    </button>
  );

  return (
    <SurfaceShell
      title="Rules"
      sub="location-based rules evaluated in order on every proxied request"
      tabs={tabs} activeTab={tab} onTab={setTab}
      actions={actions}>
      {tab === 'rules'     && <RuleSetTab     rules={ruleSets} onReload={load} editTarget={rsEdit} setEditTarget={setRsEdit} />}
      {tab === 'mapremote' && <MapRemoteTab   rules={mapRemote} onReload={load} editTarget={mrEdit} setEditTarget={setMrEdit} />}
      {tab === 'maplocal'  && <MapLocalTab    rules={mapLocal}  onReload={load} editTarget={mlEdit} setEditTarget={setMlEdit} />}
      {tab === 'access'    && <AccessTab      rules={access}    onReload={load} editTarget={acEdit} setEditTarget={setAcEdit} />}
      {tab === 'throttle'  && <ThrottleControls cfg={throttle} onChange={setThrottle} onSave={saveThrottle} />}
    </SurfaceShell>
  );
}

// ─── Throttle ───────────────────────────────────────────────────────────

function ThrottleControls({ cfg, onChange, onSave }) {
  const PRESETS = [
    { id: 'wifi',    name: 'Wifi',      latency: 2,   down: 30000 },
    { id: '3g-fast', name: '3G fast',   latency: 80,  down: 1600 },
    { id: '3g-slow', name: '3G slow',   latency: 200, down: 400 },
    { id: 'edge',    name: 'Edge / 2G', latency: 800, down: 240 },
  ];
  const applyPreset = (p) => onChange({ ...cfg, enabled: true, preset: p.id, latency: p.latency, downKbps: p.down });
  return (
    <div className="throttle-card">
      <div className="head">
        <div className="row" style={{ alignItems: 'flex-start' }}>
          <div>
            <h3>Network throttling</h3>
            <div className="desc">Inject latency and clamp response bandwidth for proxied traffic.</div>
          </div>
          <div className="spacer" />
          <div className="row" style={{ gap: 8 }}>
            <span className="mute" style={{ fontSize: 11 }}>Enabled</span>
            <Toggle label="Enable network throttling" on={cfg.enabled} onChange={v => onChange({ ...cfg, enabled: v })} />
          </div>
        </div>
        <div className="preset-row">
          {PRESETS.map(p => (
            <button key={p.id} className={'preset' + (cfg.preset === p.id ? ' on' : '')} onClick={() => applyPreset(p)}>{p.name}</button>
          ))}
        </div>
      </div>
      <div className="body">
        <div className="throttle-row">
          <div className="label">latency</div>
          <input type="range" aria-label="Throttle latency milliseconds" min={0} max={1000} value={cfg.latency} onChange={e => onChange({ ...cfg, latency: +e.target.value, preset: 'custom' })} />
          <div className="val">{cfg.latency} ms</div>
        </div>
        <div className="throttle-row">
          <div className="label">download</div>
          <input type="range" aria-label="Throttle download kilobits per second" min={0} max={30000} value={cfg.downKbps} onChange={e => onChange({ ...cfg, downKbps: +e.target.value, preset: 'custom' })} />
          <div className="val">{cfg.downKbps ? cfg.downKbps + ' kbps' : '∞'}</div>
        </div>
      </div>
      <div style={{ padding: '0 16px 12px', display: 'flex', justifyContent: 'flex-end' }}>
        <button className="btn primary" onClick={() => onSave(cfg)}>Apply throttling</button>
      </div>
    </div>
  );
}

// ─── Breakpoints surface ───────────────────────────────────────────────
const BP_INITIAL = [];

// ── Breakpoint modal ────────────────────────────────────────────────────

const EMPTY_BP_LOC = { host: null, path: null, port: null, protocol: null, query: null, methods: [], mode: 'glob' };

function BreakpointModal({ rule, onClose, onSave }) {
  const isNew = !rule?.id;
  const [loc, setLoc] = React.useState(rule?.location || EMPTY_BP_LOC);
  const [bpType, setBpType] = React.useState(rule?.bp_type || 'Request');
  const lbl = { fontSize: 12, color: 'var(--text-faint)', whiteSpace: 'nowrap' };
  const save = async () => {
    try { await onSave({ location: loc, bp_type: bpType }); }
    catch (e) { notifyError(e.message || e); }
  };
  return (
    <Modal title={isNew ? 'Add breakpoint' : 'Edit breakpoint'} onClose={onClose} onSave={save}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
        <span style={lbl}>Pause</span>
        <select className="cmp-input" style={{ width: 'auto' }} value={bpType} onChange={e => setBpType(e.target.value)} autoFocus>
          <option value="Request">Requests</option>
          <option value="Response">Responses</option>
        </select>
        <span style={{ ...lbl, marginLeft: 6 }}>matching</span>
      </div>
      <LocationEditor loc={loc} onChange={setLoc} />
    </Modal>
  );
}

function BreakpointsSurface({ sessions, onResume, onAbort }) {
  const [bps, setBps] = React.useState(BP_INITIAL);
  const [pending, setPending] = React.useState([]);
  const [bpEdit, setBpEdit] = React.useState(undefined); // undefined=closed, null=new, obj=editing
  const load = React.useCallback(async () => {
    const [rules, held] = await Promise.all([
      fetchJson('/admin/breakpoints', []),
      fetchJson('/admin/breakpoints/pending', []),
    ]);
    setBps((rules || []).map(r => {
      const loc = r.location || {};
      const parts = [loc.host, loc.path, loc.query].filter(Boolean);
      const match = parts.length ? parts.join(' · ') : 'any';
      return { name: match, match, action: r.bp_type, meta: loc.mode || 'glob', on: !!r.enabled, raw: r };
    }));
    setPending(held || []);
  }, []);
  React.useEffect(() => { load(); const id = setInterval(load, 2000); return () => clearInterval(id); }, [load]);
  const paused = pending;

  const saveBp = async (data) => {
    if (bpEdit?.id) {
      await sendJson(`/admin/breakpoints/${encodeURIComponent(bpEdit.id)}`, 'PUT', { ...bpEdit, ...data }).catch(e => notifyError(e.message || e));
    } else {
      await sendJson('/admin/breakpoints', 'POST', { id: '', ...data, enabled: true }).catch(e => notifyError(e.message || e));
    }
    setBpEdit(undefined);
    await load();
  };
  const deleteBreakpoint = async (_i, row) => {
    if (!await confirmAction('Delete this breakpoint?', 'Delete', 'danger')) return;
    await fetch(`/admin/breakpoints/${encodeURIComponent(row.raw.id)}`, { method: 'DELETE' }).catch(e => notifyError(e.message || e));
    await load();
  };
  const toggleBreakpoint = async (_i, enabled) => {
    const row = bps[_i];
    if (!row?.raw?.id) return;
    await sendJson(`/admin/breakpoints/${encodeURIComponent(row.raw.id)}`, 'PUT', { ...row.raw, enabled }).catch(e => notifyError(e.message || e));
    await load();
  };
  const disableAll = async () => {
    for (const row of bps) {
      if (row.on) {
        await sendJson(`/admin/breakpoints/${encodeURIComponent(row.raw.id)}`, 'PUT', { ...row.raw, enabled: false }).catch(e => notifyError(e.message || e));
      }
    }
    const heldNow = await fetchJson('/admin/breakpoints/pending', pending);
    for (const held of heldNow || []) {
      await sendJson(`/admin/breakpoints/pending/${encodeURIComponent(held.id)}/resolve`, 'POST', { action: 'continue' }).catch(e => notifyError(e.message || e));
    }
    await load();
  };
  const resolvePending = async (id, action) => {
    await sendJson(`/admin/breakpoints/pending/${encodeURIComponent(id)}/resolve`, 'POST', { action }).catch(e => notifyError(e.message || e));
    await load();
  };

  const actions = (
    <>
      <button className="btn ghost" onClick={disableAll}>Disable all</button>
      <button className="btn primary" onClick={() => setBpEdit(null)}><span style={{fontSize:14, lineHeight:0}}>＋</span> Add breakpoint</button>
    </>
  );

  return (
    <SurfaceShell
      title="Breakpoints"
      sub={`${paused.length} request${paused.length === 1 ? '' : 's'} currently held · ${bps.filter(b=>b.on).length} of ${bps.length} rules active`}
      actions={actions}>
      <div style={{ padding: '16px 16px 8px' }}>
        <div style={{ fontSize: 10.5, color: 'var(--text-faint)', textTransform: 'uppercase', letterSpacing: '0.08em', marginBottom: 8 }}>
          Live queue · paused requests
        </div>
      </div>
      <div className="queue">
        {paused.length === 0 && <div className="empty-q">No requests are paused. Triggering rules will hold them here.</div>}
        {paused.map(s => {
          const ctx = s.context?.Request || s.context?.Response || s;
          return (
            <div key={s.id} className="qrow">
              <span className="cell-method" data-m={ctx.method || 'GET'} style={{ fontSize: 11 }}>{ctx.method || 'RESP'}</span>
              <span className="tag-badge bp">BP</span>
              <div>
                <div className="url">
                  <span className="host">{ctx.host || ''}</span><span className="path">{ctx.uri || ctx.request_uri || ''}</span>
                </div>
                <div className="when">held by breakpoint until resumed · {s.bp_type || s.note || ''}</div>
              </div>
              <div className="acts">
                <button className="btn sm" onClick={() => pending.length ? resolvePending(s.id, 'drop') : onAbort(s.id)}>Abort</button>
                <button className="btn sm primary" onClick={() => pending.length ? resolvePending(s.id, 'continue') : onResume(s.id)}><Icon name="resume" size={10} /> Resume</button>
              </div>
            </div>
          );
        })}
      </div>
      <div style={{ padding: '20px 16px 8px' }}>
        <div style={{ fontSize: 10.5, color: 'var(--text-faint)', textTransform: 'uppercase', letterSpacing: '0.08em', marginBottom: 8 }}>
          Breakpoint rules
        </div>
      </div>
      <RuleTable
        rows={bps}
        onToggle={toggleBreakpoint}
        onDelete={deleteBreakpoint}
      />
      {bpEdit !== undefined && (
        <BreakpointModal
          rule={bpEdit}
          onClose={() => setBpEdit(undefined)}
          onSave={saveBp}
        />
      )}
    </SurfaceShell>
  );
}

// ─── Inspectors surface ────────────────────────────────────────────────
const PLUGIN_META = {
  AccessControlMiddleware:   { icon: 'shield',    label: 'Access Control',     desc: 'Blocks or allows requests based on Location rules (host, path, method). Block rules 403 on match; Allow rules create an allowlist.', config: '/rules' },
  CaptureFilterMiddleware:   { icon: 'filter',    label: 'Capture Filter',     desc: 'Controls which hosts are recorded into the session log. Configure in the Capture Filter surface.',         config: '/capture-filter' },
  DnsOverrideMiddleware:     { icon: 'globe',     label: 'DNS Override',       desc: 'Resolves specific hostnames to fixed IPs before forwarding. Configure in the DNS Override surface.',         config: '/dns' },
  MapRemoteMiddleware:       { icon: 'route',     label: 'Map Remote',         desc: 'Routes matching requests to a different upstream origin. Path and query string are preserved. Configure in Rules → Map Remote.',  config: '/rules' },
  ThrottlingMiddleware:      { icon: 'activity',  label: 'Throttle',           desc: 'Injects latency and clamps bandwidth on proxied responses. Configure in Rules → Throttling.',               config: '/rules' },
  UnifiedRewriteMiddleware:  { icon: 'edit',      label: 'Rewrite Rules',      desc: 'Applies Location-matched rule sets: set/remove headers, redirect, block, rewrite host/path/query, and more.', config: '/rules' },
  BreakpointMiddleware:      { icon: 'pause',     label: 'Breakpoints',        desc: 'Pauses requests or responses matching a pattern, allowing manual inspection and editing before forwarding.', config: '/breakpoints' },
  JwtInspectorMiddleware:    { icon: 'key',       label: 'JWT Inspector',      desc: 'Decodes JWT tokens in Authorization and cookie headers. Decoded claims appear in the session detail panel.', config: null },
  GraphQLInspectorMiddleware:{ icon: 'filter',    label: 'GraphQL Inspector',  desc: 'Parses GraphQL operations from request bodies. Operation name and type shown in the session detail panel.',  config: null },
  GrpcInspectorMiddleware:   { icon: 'layers',    label: 'gRPC Inspector',     desc: 'Decodes gRPC frames (application/grpc). Frame type and message shown in the session detail panel.',          config: null },
  InspectionMiddleware:      { icon: 'inspector', label: 'Traffic Inspector',  desc: 'Records request/response pairs to the session log and broadcasts change events via SSE.',                   config: null },
  MapLocalMiddleware:        { icon: 'folder',    label: 'Map Local',          desc: 'Serves local files as responses for matching requests, bypassing the upstream. Configure in Rules → Map Local.', config: '/rules' },
  MockMiddleware:            { icon: 'shield',    label: 'Mock Server',        desc: 'Returns synthetic responses for matching path patterns, short-circuiting the real upstream.',                config: '/mock' },
  LuaEngineMiddleware:       { icon: 'bolt',      label: 'Lua Engine',         desc: 'Runs sandboxed Lua 5.4 scripts per-request. Scripts managed in the Lua Scripts surface.',                   config: '/lua' },
};

function InspectorsSurface() {
  const [plugins, setPlugins] = React.useState([]);
  React.useEffect(() => {
    fetchJson('/admin/plugins', { plugins: [] }).then(data => setPlugins(data.plugins || []));
  }, []);

  return (
    <SurfaceShell title="Inspectors" sub={`${plugins.length} middleware plugins active in proxy chain`}>
      {plugins.length === 0 && <div className="empty">No inspector plugins registered.</div>}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(340px, 1fr))', gap: 12, padding: 16 }}>
        {plugins.map(name => {
          const meta = PLUGIN_META[name] || { icon: 'inspector', label: name, desc: 'Active in the proxy middleware chain.', config: null };
          return (
            <div key={name} className="insp-card" style={{ margin: 0 }}>
              <div className="head">
                <Icon name={meta.icon} size={15} stroke={1.6} />
                <h3>{meta.label}</h3>
                <div className="right">
                  <span className="tag-badge" style={{ background: 'rgba(16,185,129,0.12)', color: 'var(--c-2xx)', border: '1px solid rgba(16,185,129,0.25)', fontFamily: 'var(--font-mono)', fontSize: 10 }}>active</span>
                </div>
              </div>
              <div className="body" style={{ paddingTop: 6 }}>
                <p style={{ margin: 0, color: 'var(--text-mid)', fontSize: 12, lineHeight: 1.6 }}>{meta.desc}</p>
                <p style={{ margin: '8px 0 0', color: 'var(--text-faint)', fontSize: 11 }}>managed by runtime configuration</p>
              </div>
            </div>
          );
        })}
      </div>
    </SurfaceShell>
  );
}

async function computeCertFingerprint(pemText) {
  try {
    const b64 = pemText.replace(/-----[^-]+-----/g, '').replace(/\s+/g, '');
    const der = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
    const hash = await crypto.subtle.digest('SHA-256', der);
    return Array.from(new Uint8Array(hash)).map(b => b.toString(16).padStart(2, '0').toUpperCase()).join(':');
  } catch { return null; }
}

// ─── Root CA surface ───────────────────────────────────────────────────
function CertSurface() {
  const [certInfo, setCertInfo] = React.useState({ loaded: false, bytes: 0, fingerprint: null });
  React.useEffect(() => {
    fetch('/admin/ca')
      .then(r => r.ok ? r.text() : '')
      .then(async text => {
        const fingerprint = text ? await computeCertFingerprint(text) : null;
        setCertInfo({ loaded: !!text, bytes: text.length, fingerprint });
      })
      .catch(() => setCertInfo({ loaded: false, bytes: 0, fingerprint: null }));
  }, []);

  return (
    <SurfaceShell
      title="Root CA"
      sub="HTTPS interception relies on a CA your client trusts"
      actions={<a className="btn ghost" href="/setup" target="_blank" rel="noopener">Client setup guide</a>}>
      <div className="ca-grid">
        <div>
          <div className="ca-card">
            <h3>oproxy Root CA</h3>
            <div className="desc">Self-signed certificate authority used to mint per-domain leaf certs during MITM interception. Generated on first run.</div>
            <div className="kv" style={{ gridTemplateColumns: '140px 1fr', fontSize: 12 }}>
              <div className="k">Endpoint</div><div className="v"><code>/admin/ca</code></div>
              <div className="k">Certificate</div><div className="v">{certInfo.loaded ? `${certInfo.bytes.toLocaleString()} bytes loaded` : 'Unavailable'}</div>
              <div className="k">Leaf certs</div><div className="v">issued per-domain during MITM interception</div>
            </div>
            {certInfo.fingerprint && (
              <div style={{ marginTop: 14 }}>
                <div className="mute" style={{ fontSize: 10.5, textTransform: 'uppercase', letterSpacing: '0.08em', marginBottom: 6 }}>SHA-256 fingerprint</div>
                <div className="ca-fingerprint">{certInfo.fingerprint}</div>
              </div>
            )}
            <div className="row" style={{ marginTop: 14, gap: 6 }}>
              <a className="btn" href="/admin/ca" download="oproxy-root.crt"><Icon name="download" size={11} stroke={1.8} /> Download certificate</a>
              <a className="btn ghost" href="/setup/mobile" target="_blank" rel="noopener">Open install guide</a>
              <div className="spacer" />
            </div>
          </div>
        </div>
      </div>
    </SurfaceShell>
  );
}

window.RulesSurface = RulesSurface;
window.BreakpointsSurface = BreakpointsSurface;
window.InspectorsSurface = InspectorsSurface;
window.CertSurface = CertSurface;
window.Toggle = Toggle;
