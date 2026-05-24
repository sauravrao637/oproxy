import React from 'react';
const { Icon } = window;
/* Surfaces — Rules / Breakpoints / Inspectors / Root CA
   Activated via the left rail; each renders inside <main> instead of the
   sessions list/detail split. */

// ─── small primitives ──────────────────────────────────────────────────
function Toggle({ on, onChange, label = 'Toggle' }) {
  return <button className={'toggle' + (on ? ' on' : '')} onClick={() => onChange && onChange(!on)} aria-pressed={on} aria-label={label} />;
}

async function fetchJson(url, fallback) {
  try {
    const res = await fetch(url);
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
    const close = (result) => {
      overlay.remove();
      resolve(result);
    };
    overlay.querySelector('[data-cancel]').addEventListener('click', () => close(null));
    overlay.addEventListener('click', e => { if (e.target === overlay) close(null); });
    overlay.querySelector('form').addEventListener('submit', e => {
      e.preventDefault();
      close(input.value.trim());
    });
    input.focus();
    input.select();
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
        <h3>${title}</h3>
        ${fieldHtml}
        <div class="ui-dialog-actions">
          <button type="button" class="btn ghost" data-cancel>Cancel</button>
          <button type="submit" class="btn primary">Save</button>
        </div>
      </form>`;
    document.body.appendChild(overlay);
    const close = (result) => {
      overlay.remove();
      resolve(result);
    };
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
    first?.focus();
    first?.select?.();
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
    const close = (result) => {
      overlay.remove();
      resolve(result);
    };
    overlay.querySelector('[data-cancel]').addEventListener('click', () => close(false));
    overlay.addEventListener('click', e => { if (e.target === overlay) close(false); });
    overlay.querySelector('form').addEventListener('submit', e => {
      e.preventDefault();
      close(true);
    });
  });
}

Object.assign(window, { Toggle, SurfaceShell, fetchJson, sendJson, notifyError, ask, formDialog, confirmAction, nonEmpty });

function nonEmpty(v) {
  return v != null && String(v).trim() !== '';
}

function criteriaKind(criteria) {
  if (!criteria) return 'ALL';
  if (criteria.Host) return 'HOST';
  if (criteria.Path) return 'PATH';
  if (criteria.Body) return 'BODY';
  if (criteria.Header) return 'HDR';
  return 'MATCH';
}
function criteriaValue(criteria) {
  if (!criteria) return null;
  if (criteria.Host) return `host contains ${criteria.Host}`;
  if (criteria.Path) return `path regex ${criteria.Path}`;
  if (criteria.Body) return `body regex ${criteria.Body}`;
  if (criteria.Header) return `${criteria.Header.name}: ${criteria.Header.value}`;
  return JSON.stringify(criteria);
}
function actionLabel(action) {
  if (!action) return '';
  if (action.AddHeader) return `add ${action.AddHeader.name}: ${action.AddHeader.value}`;
  if (action.RemoveHeader) return action.RemoveHeader.name;
  if (action.ReplaceHeader) return `${action.ReplaceHeader.name} → ${action.ReplaceHeader.replacement}`;
  if (action.ReplaceBody) return `pattern: ${action.ReplaceBody.pattern}`;
  if (action.Redirect) return action.Redirect.location || `${action.Redirect.status}`;
  if (action.Block) return `${action.Block.status}`;
  return JSON.stringify(action);
}
function actionKind(action) {
  if (!action) return '';
  if (action.AddHeader) return 'ADD HDR';
  if (action.RemoveHeader) return 'DEL HDR';
  if (action.ReplaceHeader) return 'SET HDR';
  if (action.ReplaceBody) return 'BODY';
  if (action.Redirect) return `${action.Redirect.status}`;
  if (action.Block) return `BLOCK ${action.Block.status}`;
  return '';
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
            <button key={t.key}
                    className={'tab' + (activeTab === t.key ? ' on' : '')}
                    onClick={() => onTab(t.key)}>
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

// ─── Rules surface ─────────────────────────────────────────────────────
const RULES_INITIAL = {
  routes: [],
  rewrites: [],
  headers: [],
  mods: [],
  mapLocal: [],
  throttle: {
    enabled: false,
    preset: 'off',
    latency: 0,
    downKbps: 0,
    upKbps: 0,
    jitter: 0,
  },
};

function RulesSurface() {
  const [tab, setTab] = React.useState('routes');
  const [rules, setRules] = React.useState(RULES_INITIAL);

  const load = React.useCallback(async () => {
    const [routes, rewrites, headers, mods, mapLocal, throttle] = await Promise.all([
      fetchJson('/admin/routes', {}),
      fetchJson('/admin/rewrites', []),
      fetchJson('/admin/header-maps', []),
      fetchJson('/admin/modifications', []),
      fetchJson('/admin/map-local', {}),
      fetchJson('/admin/throttling', RULES_INITIAL.throttle),
    ]);
    setRules({
      routes: Object.entries(routes || {}).map(([src, dst], index) => ({
        name: `Route ${index + 1}`,
        match: src, matchKind: 'HOST',
        action: dst, actionKind: 'ROUTE',
        on: true, toggle: false, raw: { src, dst },
      })),
      rewrites: (rewrites || []).map((r, index) => ({
        name: r.name || `Rewrite ${index + 1}`,
        match: criteriaValue(r.criteria), matchKind: criteriaKind(r.criteria),
        action: actionLabel(r.action), actionKind: actionKind(r.action),
        on: !!r.enabled, raw: r, index,
      })),
      headers: (headers || []).map(r => ({
        name: r.name,
        match: r.match || null, matchKind: r.scope === 'all' ? 'ALL' : 'HOST',
        action: `${r.action || 'Set'} ${r.name}${r.value ? `: ${r.value}` : ''}`, actionKind: (r.action || 'Set').toUpperCase(),
        on: r.enabled !== false, raw: r,
      })),
      mods: (mods || []).map((r, index) => {
        const hdrCount = Object.keys(r.header_replacements || {}).length;
        return {
          name: `Modification ${index + 1}`,
          match: r.request_uri_pattern || null, matchKind: r.request_uri_pattern ? 'URI' : 'ALL',
          action: [hdrCount > 0 && `${hdrCount} header${hdrCount > 1 ? 's' : ''}`, r.body_replacement && 'body'].filter(Boolean).join(' + ') || 'no-op',
          actionKind: 'MOD',
          on: true, toggle: false, raw: r, index,
        };
      }),
      mapLocal: Object.entries(mapLocal || {}).map(([host, filePath]) => ({
        name: host,
        match: host, matchKind: 'HOST',
        action: filePath, actionKind: 'FILE',
        on: true, toggle: false, raw: { host, file_path: filePath },
      })),
      throttle: {
        enabled: !!throttle?.enabled,
        preset: throttle?.enabled ? 'custom' : 'off',
        latency: throttle?.latency_ms || 0,
        downKbps: throttle?.bandwidth_limit_kbps || 0,
        upKbps: 0,
        jitter: 0,
      },
    });
  }, []);

  React.useEffect(() => { load(); }, [load]);

  const addRule = async () => {
    try {
      if (tab === 'routes') {
        const form = await formDialog('Add route', [
          { name: 'src', label: 'Source host  (request Host header)', placeholder: 'api.local or api.local:8080' },
          { name: 'dst', label: 'Destination base URL', value: 'http://127.0.0.1:3000' },
        ]);
        if (!form || !nonEmpty(form.src) || !nonEmpty(form.dst)) return;
        const current = await fetchJson('/admin/routes', {});
        await sendJson('/admin/routes', 'POST', { ...current, [form.src]: form.dst });

      } else if (tab === 'rewrites') {
        const form = await formDialog('Add rewrite rule', [
          { name: 'ruleName',      label: 'Rule name', placeholder: 'e.g. Add CORS header' },
          { name: 'criteriaType',  label: 'Match on', type: 'select', value: 'Path', options: [
            { value: 'Path',   label: 'Path regex' },
            { value: 'Host',   label: 'Host contains' },
            { value: 'Body',   label: 'Body regex' },
          ]},
          { name: 'pattern', label: 'Pattern', value: '/api/.*' },
          { name: 'actionType',    label: 'Action', type: 'select', value: 'AddHeader', options: [
            { value: 'AddHeader',    label: 'Add / set header' },
            { value: 'RemoveHeader', label: 'Remove header' },
            { value: 'Redirect',     label: 'Redirect (302)' },
            { value: 'Block',        label: 'Block request' },
          ]},
          { name: 'param1', label: 'Header name  ·  Redirect URL  ·  Block status code', placeholder: 'x-custom  or  https://…  or  403' },
          { name: 'param2', label: 'Header value  (Add / set only)', placeholder: '1' },
        ]);
        if (!form || !nonEmpty(form.pattern)) return;
        const criteria = { [form.criteriaType]: form.pattern };
        let action;
        if (form.actionType === 'AddHeader')         action = { AddHeader:    { name: form.param1 || 'x-header', value: form.param2 || '' } };
        else if (form.actionType === 'RemoveHeader') action = { RemoveHeader: { name: form.param1 || 'x-header' } };
        else if (form.actionType === 'Redirect')     action = { Redirect: { status: 302, location: form.param1 || '/' } };
        else if (form.actionType === 'Block')        action = { Block: { status: Number(form.param1) || 403 } };
        else                                          action = { AddHeader: { name: 'x-header', value: '' } };
        await sendJson('/admin/rewrites', 'POST', {
          name: form.ruleName || `${form.actionType} on ${form.pattern}`,
          criteria, action, enabled: true,
        });

      } else if (tab === 'headers') {
        const form = await formDialog('Add header map', [
          { name: 'name',   label: 'Header name', placeholder: 'x-forwarded-for' },
          { name: 'value',  label: 'Header value', placeholder: '1' },
          { name: 'action', label: 'Action', type: 'select', value: 'Set', options: [
            { value: 'Set',    label: 'Set  (overwrite)' },
            { value: 'Add',    label: 'Add  (append)' },
            { value: 'Remove', label: 'Remove header' },
          ]},
          { name: 'match', label: 'Host pattern  (blank = all requests)', placeholder: 'api.example.com' },
        ]);
        if (!form || !nonEmpty(form.name)) return;
        await sendJson('/admin/header-maps', 'POST', {
          id: '',
          scope: form.match ? 'host' : 'all',
          match: form.match || '',
          action: form.action || 'Set',
          name: form.name,
          value: form.value || '',
          enabled: true,
        });

      } else if (tab === 'mods') {
        const form = await formDialog('Add response modification', [
          { name: 'pattern', label: 'Request URI regex  (blank = all)', placeholder: '/api/.*' },
          { name: 'header',  label: 'Response header to set', placeholder: 'x-modified' },
          { name: 'value',   label: 'Header value', placeholder: '1' },
        ]);
        if (!form) return;
        await sendJson('/admin/modifications', 'POST', {
          request_uri_pattern: form.pattern || '',
          header_replacements: form.header ? { [form.header]: form.value || '' } : {},
          body_replacement: null,
        });

      } else if (tab === 'maplocal') {
        const form = await formDialog('Map host to local file', [
          { name: 'host',     label: 'Host  (exact match)', placeholder: 'api.local:8080' },
          { name: 'filePath', label: 'File path  (relative to storage dir)', placeholder: 'mapped/response.json' },
        ]);
        if (!form || !nonEmpty(form.host) || !nonEmpty(form.filePath)) return;
        await sendJson('/admin/map-local', 'POST', { host: form.host, file_path: form.filePath });
      }
      await load();
    } catch (e) {
      notifyError(`Failed to save rule: ${e.message || e}`);
    }
  };

  const saveThrottle = async (cfg = rules.throttle) => {
    try {
      await sendJson('/admin/throttling', 'POST', {
        enabled: !!cfg.enabled,
        latency_ms: Number(cfg.latency) || 0,
        bandwidth_limit_kbps: Number(cfg.downKbps) || 0,
      });
      await load();
    } catch (e) {
      notifyError(`Failed to save throttling: ${e.message || e}`);
    }
  };

  const setOn = (group) => async (i, v) => {
    const row = rules[group][i];
    if (!row || row.toggle === false) return;
    try {
      if (group === 'rewrites') {
        await sendJson(`/admin/rewrites/${row.index}`, 'PUT', { ...row.raw, enabled: v });
      } else if (group === 'headers') {
        await sendJson(`/admin/header-maps/${encodeURIComponent(row.raw.id)}`, 'PUT', { ...row.raw, enabled: v });
      }
      await load();
    } catch (e) {
      notifyError(`Failed to update rule: ${e.message || e}`);
    }
  };

  const editRow = async (group, i, row) => {
    try {
      if (group === 'routes') {
        const form = await formDialog(`Edit route — ${row.raw.src}`, [
          { name: 'dst', label: 'Destination base URL', value: row.raw.dst },
        ]);
        if (!form || !nonEmpty(form.dst)) return;
        const current = await fetchJson('/admin/routes', {});
        await sendJson('/admin/routes', 'POST', { ...current, [row.raw.src]: form.dst });

      } else if (group === 'rewrites') {
        const raw = row.raw;
        const existingActionType = raw.action
          ? (raw.action.AddHeader ? 'AddHeader' : raw.action.RemoveHeader ? 'RemoveHeader'
            : raw.action.Redirect ? 'Redirect' : raw.action.Block ? 'Block' : 'AddHeader')
          : 'AddHeader';
        const existingCriteriaType = raw.criteria
          ? (raw.criteria.Host ? 'Host' : raw.criteria.Path ? 'Path' : raw.criteria.Body ? 'Body' : 'Path')
          : 'Path';
        const existingCriteriaValue = criteriaValue(raw.criteria) || '';
        const existingP1 = raw.action?.AddHeader?.name || raw.action?.RemoveHeader?.name || raw.action?.Redirect?.location || String(raw.action?.Block?.status || '') || '';
        const existingP2 = raw.action?.AddHeader?.value || raw.action?.ReplaceHeader?.replacement || '';
        const form = await formDialog(`Edit rewrite — ${raw.name || ''}`, [
          { name: 'ruleName',      label: 'Rule name', value: raw.name || '' },
          { name: 'criteriaType',  label: 'Match on', type: 'select', value: existingCriteriaType, options: [
            { value: 'Path', label: 'Path regex' },
            { value: 'Host', label: 'Host contains' },
            { value: 'Body', label: 'Body regex' },
          ]},
          { name: 'pattern', label: 'Pattern', value: existingCriteriaValue },
          { name: 'actionType',    label: 'Action', type: 'select', value: existingActionType, options: [
            { value: 'AddHeader',    label: 'Add / set header' },
            { value: 'RemoveHeader', label: 'Remove header' },
            { value: 'Redirect',     label: 'Redirect (302)' },
            { value: 'Block',        label: 'Block request' },
          ]},
          { name: 'param1', label: 'Header name  ·  Redirect URL  ·  Block status', value: existingP1 },
          { name: 'param2', label: 'Header value  (Add / set only)', value: existingP2 },
        ]);
        if (!form || !nonEmpty(form.pattern)) return;
        const criteria = { [form.criteriaType]: form.pattern };
        let action;
        if (form.actionType === 'AddHeader')         action = { AddHeader:    { name: form.param1 || 'x-header', value: form.param2 || '' } };
        else if (form.actionType === 'RemoveHeader') action = { RemoveHeader: { name: form.param1 || 'x-header' } };
        else if (form.actionType === 'Redirect')     action = { Redirect: { status: 302, location: form.param1 || '/' } };
        else if (form.actionType === 'Block')        action = { Block: { status: Number(form.param1) || 403 } };
        else                                          action = raw.action;
        await sendJson(`/admin/rewrites/${row.index}`, 'PUT', {
          ...raw, name: form.ruleName || raw.name, criteria, action,
        });

      } else if (group === 'headers') {
        const form = await formDialog(`Edit header map — ${row.raw.name}`, [
          { name: 'value',  label: 'Header value', value: row.raw.value || '' },
          { name: 'action', label: 'Action', type: 'select', value: row.raw.action || 'Set', options: [
            { value: 'Set',    label: 'Set  (overwrite)' },
            { value: 'Add',    label: 'Add  (append)' },
            { value: 'Remove', label: 'Remove header' },
          ]},
          { name: 'match', label: 'Host pattern  (blank = all)', value: row.raw.match || '' },
        ]);
        if (!form) return;
        await sendJson(`/admin/header-maps/${encodeURIComponent(row.raw.id)}`, 'PUT', {
          ...row.raw,
          value: form.value,
          action: form.action || row.raw.action,
          scope: form.match ? 'host' : 'all',
          match: form.match || '',
        });

      } else if (group === 'mods') {
        const hdrs = row.raw.header_replacements || {};
        const existingHeader = Object.keys(hdrs)[0] || '';
        const existingValue = hdrs[existingHeader] || '';
        const form = await formDialog('Edit response modification', [
          { name: 'pattern', label: 'Request URI regex  (blank = all)', value: row.raw.request_uri_pattern || '' },
          { name: 'header',  label: 'Response header to set', value: existingHeader },
          { name: 'value',   label: 'Header value', value: existingValue },
        ]);
        if (!form) return;
        await sendJson(`/admin/modifications/${row.index}`, 'DELETE');
        await sendJson('/admin/modifications', 'POST', {
          ...row.raw,
          request_uri_pattern: form.pattern || '',
          header_replacements: form.header ? { [form.header]: form.value || '' } : {},
        });

      } else if (group === 'mapLocal') {
        const form = await formDialog(`Edit map-local — ${row.raw.host}`, [
          { name: 'filePath', label: 'File path  (relative to storage dir)', value: row.raw.file_path },
        ]);
        if (!form || !nonEmpty(form.filePath)) return;
        await sendJson('/admin/map-local', 'POST', { host: row.raw.host, file_path: form.filePath });
      }
      await load();
    } catch (e) {
      notifyError(`Failed to edit rule: ${e.message || e}`);
    }
  };

  const deleteRow = async (group, i, row) => {
    if (!await confirmAction('Delete this rule?', 'Delete', 'danger')) return;
    try {
      if (group === 'routes') {
        const current = await fetchJson('/admin/routes', {});
        delete current[row.raw.src];
        await sendJson('/admin/routes', 'POST', current);
      } else if (group === 'rewrites') {
        await fetch(`/admin/rewrites/${row.index}`, { method: 'DELETE' });
      } else if (group === 'headers') {
        await fetch(`/admin/header-maps/${encodeURIComponent(row.raw.id)}`, { method: 'DELETE' });
      } else if (group === 'mods') {
        await fetch(`/admin/modifications/${row.index}`, { method: 'DELETE' });
      } else if (group === 'mapLocal') {
        await fetch(`/admin/map-local/${encodeURIComponent(row.raw.host)}`, { method: 'DELETE' });
      }
      await load();
    } catch (e) {
      notifyError(`Failed to delete rule: ${e.message || e}`);
    }
  };

  const tabs = [
    { key: 'routes',   label: 'Routes',        count: rules.routes.length },
    { key: 'rewrites', label: 'Rewrites',      count: rules.rewrites.length },
    { key: 'headers',  label: 'Header maps',   count: rules.headers.length },
    { key: 'mods',     label: 'Modifications', count: rules.mods.length },
    { key: 'maplocal', label: 'Map local',     count: rules.mapLocal.length },
    { key: 'throttle', label: 'Throttling',    count: rules.throttle.enabled ? '●' : null },
  ];

  const actions = (
    <>
      {tab !== 'throttle' && <button className="btn primary" onClick={addRule}><span style={{fontSize:14, lineHeight:0}}>＋</span> Add rule</button>}
    </>
  );

  return (
    <SurfaceShell
      title="Rules"
      sub="match → transform · evaluated in chain order on every proxied request"
      tabs={tabs} activeTab={tab} onTab={setTab}
      actions={actions}>
      {tab === 'routes' && (
        <>
          <div style={{ padding: '12px 16px 0', fontSize: 12, color: 'var(--text-mid)' }}>Destination</div>
          <RuleTable
            rows={rules.routes}
            onToggle={setOn('routes')} onEdit={(i, r) => editRow('routes', i, r)} onDelete={(i, r) => deleteRow('routes', i, r)}
            emptyTitle="No routes configured"
            emptyDesc="Routes redirect a source host to a different upstream. Useful for mapping api.local → http://127.0.0.1:3000 so local apps can use meaningful hostnames."
          />
        </>
      )}
      {tab === 'rewrites' && (
        <>
          <div style={{ padding: '12px 16px 0', fontSize: 12, color: 'var(--text-mid)' }}>Target (regex)</div>
          <RuleTable
            rows={rules.rewrites}
            onToggle={setOn('rewrites')} onEdit={(i, r) => editRow('rewrites', i, r)} onDelete={(i, r) => deleteRow('rewrites', i, r)}
            emptyTitle="No rewrite rules"
            emptyDesc="Rewrite rules match a path, host, or body pattern and apply an action: add or remove a header, redirect, or block the request."
          />
        </>
      )}
      {tab === 'headers' && (
        <>
          <div style={{ padding: '12px 16px 0', fontSize: 12, color: 'var(--text-mid)' }}>Target</div>
          <RuleTable
            rows={rules.headers}
            onToggle={setOn('headers')} onEdit={(i, r) => editRow('headers', i, r)} onDelete={(i, r) => deleteRow('headers', i, r)}
            emptyTitle="No header maps"
            emptyDesc="Header maps add, set, or remove a specific header on every request (or only those matching a host pattern). Applied before the request is forwarded."
          />
        </>
      )}
      {tab === 'mods' && (
        <>
          <div style={{ padding: '12px 16px 0', fontSize: 12, color: 'var(--text-mid)' }}>URI contains</div>
          <RuleTable
            rows={rules.mods}
            onToggle={setOn('mods')} onEdit={(i, r) => editRow('mods', i, r)} onDelete={(i, r) => deleteRow('mods', i, r)}
            emptyTitle="No response modifications"
            emptyDesc="Modifications rewrite response headers or bodies for requests whose URI matches a pattern. Applied after the upstream responds."
          />
        </>
      )}
      {tab === 'maplocal' && (
        <>
          <div style={{ padding: '12px 16px 0', fontSize: 12, color: 'var(--text-mid)' }}>Local file</div>
          <RuleTable
            rows={rules.mapLocal}
            onToggle={setOn('mapLocal')} onEdit={(i, r) => editRow('mapLocal', i, r)} onDelete={(i, r) => deleteRow('mapLocal', i, r)}
            emptyTitle="No map-local rules"
            emptyDesc="Map-local serves a file from the storage directory as the response for a specific host, bypassing the real upstream entirely."
          />
        </>
      )}
      {tab === 'throttle' && <ThrottleControls cfg={rules.throttle} onChange={(t) => setRules(p => ({...p, throttle: t}))} onSave={saveThrottle} />}
    </SurfaceShell>
  );
}

function ThrottleControls({ cfg, onChange, onSave }) {
  const PRESETS = [
    { id: 'wifi',    name: 'Wifi',          latency: 2,    down: 30000 },
    { id: '3g-fast', name: '3G fast',       latency: 80,   down: 1600 },
    { id: '3g-slow', name: '3G slow',       latency: 200,  down: 400 },
    { id: 'edge',    name: 'Edge / 2G',     latency: 800,  down: 240 },
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
            <button key={p.id}
                    className={'preset' + (cfg.preset === p.id ? ' on' : '')}
                    onClick={() => applyPreset(p)}>{p.name}</button>
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

function BreakpointsSurface({ sessions, onResume, onAbort }) {
  const [bps, setBps] = React.useState(BP_INITIAL);
  const [pending, setPending] = React.useState([]);
  const load = React.useCallback(async () => {
    const [rules, held] = await Promise.all([
      fetchJson('/admin/breakpoints', []),
      fetchJson('/admin/breakpoints/pending', []),
    ]);
    setBps((rules || []).map(r => ({ name: r.pattern, match: r.pattern, action: r.bp_type, meta: 'regex', on: !!r.enabled, raw: r })));
    setPending(held || []);
  }, []);
  React.useEffect(() => { load(); const id = setInterval(load, 2000); return () => clearInterval(id); }, [load]);
  // Breakpoints queue must reflect only live backend-held requests.
  // Falling back to historical paused sessions causes aborted items to reappear.
  const paused = pending;

  const addBreakpoint = async () => {
    const form = await formDialog('Add breakpoint', [
      { name: 'pattern', label: 'URI/body regex (* or blank = all)', value: '.*' },
      { name: 'bpType', label: 'Pause on', type: 'select', value: 'Request', options: [
        { value: 'Request', label: 'Request' },
        { value: 'Response', label: 'Response' },
      ] },
    ]);
    if (!form) return;
    const pattern = nonEmpty(form.pattern) ? form.pattern : '.*';
    await sendJson('/admin/breakpoints', 'POST', { id: '', pattern, bp_type: form.bpType, enabled: true }).catch(e => notifyError(e.message || e));
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
    for (const held of pending) {
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
      <button className="btn primary" onClick={addBreakpoint}><span style={{fontSize:14, lineHeight:0}}>＋</span> Add breakpoint</button>
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
        );})}
      </div>

      <div style={{ padding: '20px 16px 8px' }}>
        <div style={{ fontSize: 10.5, color: 'var(--text-faint)', textTransform: 'uppercase', letterSpacing: '0.08em', marginBottom: 8 }}>
          Breakpoint rules
        </div>
      </div>
      <RuleTable
        headers={['Match', 'Action', 'Filter', '']}
        rows={bps}
        onToggle={toggleBreakpoint}
        onDelete={deleteBreakpoint}
      />
    </SurfaceShell>
  );
}

// ─── Inspectors surface ────────────────────────────────────────────────
const PLUGIN_META = {
  capture_filter:      { icon: 'filter',    label: 'Capture Filter',     desc: 'Controls which hosts are recorded into the session log. Configure in the Capture Filter surface.',       config: '/capture-filter' },
  routing:             { icon: 'route',     label: 'Routing',            desc: 'Redirects matching host headers to a different upstream base URL. Configure in Rules → Routes.',           config: '/rules' },
  dns_override:        { icon: 'globe',     label: 'DNS Override',       desc: 'Resolves specific hostnames to fixed IPs before forwarding. Configure in the DNS Override surface.',        config: '/dns' },
  throttle:            { icon: 'activity',  label: 'Throttle',           desc: 'Injects latency and clamps bandwidth on proxied responses. Configure in Rules → Throttling.',              config: '/rules' },
  rewrite:             { icon: 'edit',      label: 'Rewrite',            desc: 'Applies header, body, redirect, and block rules matched by path, host, or body regex. Configure in Rules → Rewrites.', config: '/rules' },
  header_map:          { icon: 'layers',    label: 'Header Map',         desc: 'Adds, sets, or removes headers on matched requests. Configure in Rules → Header maps.',                    config: '/rules' },
  breakpoint:          { icon: 'pause',     label: 'Breakpoints',        desc: 'Pauses requests or responses matching a URI/body pattern, allowing manual inspection and editing.',          config: '/breakpoints' },
  jwt_inspector:       { icon: 'key',       label: 'JWT Inspector',      desc: 'Decodes JWT tokens found in Authorization and cookie headers. Decoded claims appear in the session detail panel.',   config: null },
  graphql_inspector:   { icon: 'filter',    label: 'GraphQL Inspector',  desc: 'Parses GraphQL operations from request bodies. Operation name and type shown in the session detail panel.', config: null },
  grpc_inspector:      { icon: 'layers',    label: 'gRPC Inspector',     desc: 'Decodes gRPC frames (application/grpc). Frame type and message shown in the session detail panel.',         config: null },
  inspection:          { icon: 'inspector', label: 'Traffic Inspector',  desc: 'Records request/response pairs to the session log and broadcasts change events via SSE.',                  config: null },
  modification:        { icon: 'edit',      label: 'Modification',       desc: 'Replaces response headers or body for requests whose URI matches a pattern. Configure in Rules → Modifications.', config: '/rules' },
  mock:                { icon: 'shield',    label: 'Mock Server',        desc: 'Returns synthetic responses for matching path patterns, short-circuiting the real upstream.',               config: '/mock' },
  map_local:           { icon: 'folder',    label: 'Map Local',          desc: 'Serves a local file as the response for a specific host, bypassing the real upstream entirely.',            config: '/rules' },
  lua:                 { icon: 'bolt',      label: 'Lua Engine',         desc: 'Runs sandboxed Lua 5.4 scripts per-request after rewrite middleware. Scripts managed in the Lua Scripts surface.', config: '/lua' },
};

function InspectorsSurface() {
  const [plugins, setPlugins] = React.useState([]);
  React.useEffect(() => {
    fetchJson('/admin/plugins', { plugins: [] }).then(data => {
      setPlugins(data.plugins || []);
    });
  }, []);

  return (
    <SurfaceShell title="Inspectors" sub={`${plugins.length} middleware plugins active in proxy chain`}>
      {plugins.length === 0 && (
        <div className="empty">No inspector plugins registered.</div>
      )}
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
    return Array.from(new Uint8Array(hash))
      .map(b => b.toString(16).padStart(2, '0').toUpperCase())
      .join(':');
  } catch {
    return null;
  }
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
      actions={
        <>
          <a className="btn ghost" href="/setup" target="_blank" rel="noopener">Client setup guide</a>
        </>
      }>
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
