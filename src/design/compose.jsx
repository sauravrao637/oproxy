import React from 'react';
const { Icon, SurfaceShell, notifyError, Toggle } = window;
/* Compose workspace — Postman-like, multi-tab, collections + variables. */

const COMPOSE_INITIAL = {
  collections: [],
  variables: [],
};
const COMPOSE_STORAGE_KEY = 'oproxy.compose.workspace.v1';

function loadComposeState() {
  try {
    const raw = localStorage.getItem(COMPOSE_STORAGE_KEY);
    if (!raw) return COMPOSE_INITIAL;
    const parsed = JSON.parse(raw);
    return {
      collections: Array.isArray(parsed?.collections) ? parsed.collections : [],
      variables: Array.isArray(parsed?.variables) ? parsed.variables : [],
    };
  } catch {
    return COMPOSE_INITIAL;
  }
}

function saveComposeState(state) {
  try {
    localStorage.setItem(COMPOSE_STORAGE_KEY, JSON.stringify({
      collections: state.collections || [],
      variables: state.variables || [],
    }));
  } catch {
    // Browser storage can be disabled or full; Compose still works in-memory.
  }
}

const DEFAULT_HEADERS = [];

const DEFAULT_BODY = '';

function makeTab(overrides = {}) {
  return {
    id: 't_' + Math.random().toString(36).slice(2, 8),
    name: 'Untitled',
    method: 'GET',
    url: '',
    headers: [...DEFAULT_HEADERS],
    params: [],
    body: DEFAULT_BODY,
    bodyMode: 'raw',
    contentType: 'application/json',
    authType: 'none',
    authToken: '',
    authUser: '',
    authPass: '',
    response: null,
    dirty: false,
    ...overrides,
  };
}

function enabledPairs(items) {
  return (items || []).filter(x => x.on !== false && x.key);
}

function buildComposeUrl(tab, resolveVars) {
  const base = resolveVars(tab.url || '');
  if (!base) return '';
  const params = enabledPairs(tab.params);
  if (params.length === 0) return base;
  try {
    const url = new URL(base);
    params.forEach(p => url.searchParams.set(p.key, resolveVars(p.value || '')));
    return url.toString();
  } catch {
    const qs = params.map(p => `${encodeURIComponent(p.key)}=${encodeURIComponent(resolveVars(p.value || ''))}`).join('&');
    return base + (base.includes('?') ? '&' : '?') + qs;
  }
}

function validateComposeTarget(url) {
  if (!url) return 'Enter an absolute http:// or https:// URL.';
  if (url.includes('{{')) return 'Resolve all variables before sending.';
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return 'Only http:// and https:// URLs can be sent.';
    }
    return '';
  } catch {
    return 'Enter a valid absolute URL.';
  }
}

function buildComposeCurl(tab, resolveVars) {
  const url = buildComposeUrl(tab, resolveVars);
  const parts = ['curl'];
  if (tab.method && tab.method !== 'GET') parts.push('-X', shellQuote(tab.method));
  enabledPairs(tab.headers).forEach(h => parts.push('-H', shellQuote(`${h.key}: ${resolveVars(h.value || '')}`)));
  const authHeader = authHeaderValue(tab, resolveVars);
  if (authHeader) parts.push('-H', shellQuote(`Authorization: ${authHeader}`));
  if (tab.body) parts.push('--data-raw', shellQuote(resolveVars(tab.body)));
  parts.push(shellQuote(url));
  return parts.join(' ');
}

function authHeaderValue(tab, resolveVars) {
  if (tab.authType === 'bearer' && tab.authToken) return `Bearer ${resolveVars(tab.authToken)}`;
  if (tab.authType === 'basic' && tab.authUser) {
    const raw = `${resolveVars(tab.authUser)}:${resolveVars(tab.authPass || '')}`;
    return `Basic ${btoa(raw)}`;
  }
  return '';
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
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

function downloadText(text, filename, type = 'application/json') {
  const blob = new Blob([text], { type });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function beautifyBody(tab, updateActive) {
  if (!tab?.body) return;
  if ((tab.contentType || '').includes('json')) {
    try {
      updateActive({ body: JSON.stringify(JSON.parse(tab.body), null, 2) });
    } catch {
      notifyError('Body is not valid JSON.');
    }
  }
}

function ComposeSurface({ incomingRequest }) {
  const [state, setState] = React.useState(loadComposeState);
  const [tabs, setTabs] = React.useState([]);
  const [activeTabId, setActiveTabId] = React.useState(null);
  const active = tabs.find(t => t.id === activeTabId);

  // Inline rename state for collections and tabs
  const [editingCollId, setEditingCollId] = React.useState(null);
  const [editingCollName, setEditingCollName] = React.useState('');
  const [editingTabId, setEditingTabId] = React.useState(null);
  const [editingTabName, setEditingTabName] = React.useState('');

  // Save-as bar state
  const [savingMode, setSavingMode] = React.useState(false);
  const [saveName, setSaveName] = React.useState('');
  const [saveCollId, setSaveCollId] = React.useState('');

  const commitCollRename = () => {
    if (editingCollName.trim()) {
      setState(prev => ({...prev, collections: prev.collections.map(c => c.id === editingCollId ? {...c, name: editingCollName.trim()} : c)}));
    }
    setEditingCollId(null);
  };

  const commitTabRename = () => {
    if (editingTabName.trim()) {
      setTabs(prev => prev.map(t => t.id === editingTabId ? {...t, name: editingTabName.trim()} : t));
    }
    setEditingTabId(null);
  };

  const openSaveBar = () => {
    saveActive();
  };

  const doSave = () => {
    if (!active) return;
    const name = saveName.trim() || `${active.method} ${active.url || '/'}`;
    const savedId = active.savedId || 'r_' + Date.now();
    setState(prev => {
      let collections = prev.collections.length > 0
        ? prev.collections
        : [{ id: 'c_' + Date.now(), name: 'Collection 1', open: true, requests: [] }];
      const targetId = saveCollId || collections[0].id;
      const req = { id: savedId, name, method: active.method, url: active.url, headers: active.headers, params: active.params, body: active.body, bodyMode: active.bodyMode, contentType: active.contentType };
      return {
        ...prev,
        collections: collections.map(c => c.id === targetId
          ? { ...c, open: true, requests: [...c.requests.filter(r => r.id !== req.id), req] }
          : c),
      };
    });
    updateActive({ savedId, name, dirty: false });
    setSavingMode(false);
  };

  React.useEffect(() => {
    saveComposeState(state);
  }, [state]);

  React.useEffect(() => {
    if (!incomingRequest) return;
    const t = makeTab(incomingRequest);
    setTabs(prev => [...prev, t]);
    setActiveTabId(t.id);
  }, [incomingRequest?.importId]);

  const updateActive = (patch) => {
    setTabs(prev => prev.map(t => t.id === activeTabId ? { ...t, ...patch, dirty: patch.dirty ?? true } : t));
  };

  const toggleCollection = (cid) => setState(prev => ({
    ...prev,
    collections: prev.collections.map(c => c.id === cid ? { ...c, open: !c.open } : c),
  }));

  const openRequestInTab = (req) => {
    const exists = tabs.find(t => t.name === req.name);
    if (exists) { setActiveTabId(exists.id); return; }
    const t = makeTab(req);
    setTabs(prev => [...prev, t]);
    setActiveTabId(t.id);
  };
  const newTab = () => {
    const t = makeTab();
    setTabs(prev => [...prev, t]);
    setActiveTabId(t.id);
  };
  const closeTab = (id, e) => {
    e?.stopPropagation();
    setTabs(prev => {
      const next = prev.filter(t => t.id !== id);
      if (id === activeTabId) setActiveTabId(next[next.length - 1]?.id || null);
      return next;
    });
  };

  const addCollection = () => {
    const id = 'c_' + Date.now();
    setState(prev => ({
      ...prev,
      collections: [...prev.collections, { id, name: `Collection ${prev.collections.length + 1}`, open: true, requests: [] }],
    }));
  };

  const addVariable = () => {
    const id = 'v_' + Date.now();
    setState(prev => ({
      ...prev,
      variables: [...prev.variables, { id, enabled: true, key: `var_${prev.variables.length + 1}`, value: '' }],
    }));
  };

  const saveActive = () => {
    if (!active) return;
    const savedId = active.savedId || 'r_' + Date.now();
    setState(prev => {
      const collections = prev.collections.length > 0
        ? prev.collections
        : [{ id: 'c_' + Date.now(), name: 'Collection 1', open: true, requests: [] }];
      const target = collections[0];
      const req = {
        id: savedId,
        name: active.name || `${active.method} ${active.url || '/'}`,
        method: active.method,
        url: active.url,
        headers: active.headers,
        params: active.params,
        body: active.body,
        bodyMode: active.bodyMode,
        contentType: active.contentType,
      };
      return {
        ...prev,
        collections: collections.map(c => c.id === target.id
          ? { ...c, open: true, requests: [...c.requests.filter(r => r.id !== req.id), req] }
          : c),
      };
    });
    updateActive({ savedId, dirty: false });
  };

  const send = async () => {
    if (!active) return;
    const started = performance.now();
    const url = buildComposeUrl(active, resolveVars);
    const validationError = validateComposeTarget(url);
    if (validationError) {
      updateActive({
        response: {
          status: 0,
          statusText: 'Not sent',
          timeMs: 0,
          size: validationError.length,
          body: validationError,
          headers: {},
          when: Date.now(),
        },
      });
      return;
    }
    const headers = {};
    enabledPairs(active.headers).forEach(h => { headers[h.key] = resolveVars(h.value || ''); });
    const authHeader = authHeaderValue(active, resolveVars);
    if (authHeader && !Object.keys(headers).some(k => k.toLowerCase() === 'authorization')) {
      headers.Authorization = authHeader;
    }
    if (active.body && active.bodyMode === 'raw' && active.contentType && !Object.keys(headers).some(k => k.toLowerCase() === 'content-type')) {
      headers['content-type'] = active.contentType;
    }
    try {
      const res = await fetch('/admin/forward', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          method: active.method,
          url,
          headers,
          body: active.body ? resolveVars(active.body) : null,
        }),
      });
      const contentType = res.headers.get('content-type') || '';
      const text = await res.text();
      let data = {};
      if (contentType.includes('application/json') && text) {
        try { data = JSON.parse(text); } catch { data = { body: text }; }
      } else {
        data = { body: text };
      }
      updateActive({
        response: {
          status: data.status || res.status,
          statusText: data.statusText || res.statusText || '',
          timeMs: Math.round(performance.now() - started),
          size: data.body ? String(data.body).length : 0,
          body: data.error || data.body || '',
          headers: data.headers || {},
          when: Date.now(),
        },
        dirty: false,
      });
    } catch (err) {
      updateActive({
        response: {
          status: 0,
          statusText: 'Failed',
          timeMs: Math.round(performance.now() - started),
          size: 0,
          body: String(err),
          headers: {},
          when: Date.now(),
        },
      });
    }
  };

  const resolveVars = (str) => {
    if (!str) return '';
    const env = {};
    state.variables.filter(v => v.enabled).forEach(v => { env[v.key] = v.value; });
    return String(str).replace(/\{\{(\w+)\}\}/g, (_, k) => env[k] || `{{${k}}}`);
  };

  const actions = (
    <>
      <button className="btn" onClick={addCollection}><span style={{fontSize:14, lineHeight:0}}>＋</span> Collection</button>
      <button className="btn" onClick={() => downloadText(JSON.stringify(state, null, 2), 'oproxy-collections.json')}><Icon name="download" size={11} stroke={1.8} /> Export</button>
    </>
  );

  return (
    <SurfaceShell title="Compose" sub="craft, save, and replay requests with variables" actions={actions}>
      <div className="cmp">
        <div className="cmp-side">
          <div className="cmp-side-section">
            <div className="cmp-side-head">
              <span>Collections</span>
              <button className="copy-btn" title="New collection" aria-label="New collection" onClick={addCollection}>＋</button>
            </div>
            <div className="cmp-tree">
              {state.collections.map(c => (
                <div key={c.id}>
                  <div className="cmp-coll" onClick={() => editingCollId !== c.id && toggleCollection(c.id)}>
                    <Icon name={c.open ? 'chevronDown' : 'chevronRight'} size={11} stroke={2} />
                    {editingCollId === c.id ? (
                      <input
                        className="cmp-coll-name"
                        value={editingCollName}
                        onChange={e => setEditingCollName(e.target.value)}
                        onBlur={commitCollRename}
                        onKeyDown={e => { if (e.key === 'Enter') commitCollRename(); if (e.key === 'Escape') setEditingCollId(null); }}
                        onClick={e => e.stopPropagation()}
                        autoFocus
                      />
                    ) : (
                      <span className="cmp-coll-name" onDoubleClick={e => { e.stopPropagation(); setEditingCollId(c.id); setEditingCollName(c.name); }} title="Double-click to rename">{c.name}</span>
                    )}
                    <span className="cmp-coll-count">{c.requests.length}</span>
                  </div>
                  {c.open && c.requests.map(r => (
                    <div key={r.id}
                         className={'cmp-req' + (active && active.name === r.name ? ' on' : '')}
                         onClick={() => openRequestInTab(r)}>
                      <span className="cell-method" data-m={r.method}>{r.method}</span>
                      <span className="cmp-req-name">{r.name}</span>
                    </div>
                  ))}
                </div>
              ))}
            </div>
          </div>

          <div className="cmp-side-section vars">
            <div className="cmp-side-head">
              <span>Variables · env</span>
              <button className="copy-btn" title="New variable" aria-label="New variable" onClick={addVariable}>＋</button>
            </div>
            <div className="cmp-vars">
              {state.variables.map(v => (
                <div key={v.id} className={'cmp-var' + (v.enabled ? '' : ' off')}>
                  <Toggle label={`Toggle variable ${v.key}`} on={v.enabled} onChange={(on) => setState(p => ({...p, variables: p.variables.map(x => x.id === v.id ? {...x, enabled: on} : x)}))} />
                  <span style={{ fontSize: 11, color: 'var(--text-mid)', minWidth: 0 }}>{v.key}</span>
                  <div className="cmp-var-k"><input aria-label="Variable name" value={v.key} onChange={e => setState(p => ({...p, variables: p.variables.map(x => x.id === v.id ? {...x, key: e.target.value} : x)}))} /></div>
                  <div className="cmp-var-v"><input aria-label="Variable value" value={v.value} title={v.value} onChange={e => setState(p => ({...p, variables: p.variables.map(x => x.id === v.id ? {...x, value: e.target.value} : x)}))} /></div>
                </div>
              ))}
            </div>
          </div>
        </div>

        <div className="cmp-editor">
          <div className="cmp-tabs">
            {tabs.map(t => (
              <div key={t.id}
                   className={'cmp-tab' + (t.id === activeTabId ? ' on' : '')}
                   onClick={() => { if (editingTabId !== t.id) setActiveTabId(t.id); }}>
                <span className="cell-method" data-m={t.method} style={{ fontSize: 10 }}>{t.method}</span>
                {editingTabId === t.id ? (
                  <input
                    className="cmp-tab-name"
                    value={editingTabName}
                    onChange={e => setEditingTabName(e.target.value)}
                    onBlur={commitTabRename}
                    onKeyDown={e => { if (e.key === 'Enter') commitTabRename(); if (e.key === 'Escape') setEditingTabId(null); }}
                    onClick={e => e.stopPropagation()}
                    autoFocus
                  />
                ) : (
                  <span className="cmp-tab-name" onDoubleClick={e => { e.stopPropagation(); setEditingTabId(t.id); setEditingTabName(t.name); }} title="Double-click to rename">{t.name}{t.dirty && <span className="cmp-dirty">●</span>}</span>
                )}
                <button className="cmp-tab-x" onClick={(e) => closeTab(t.id, e)} aria-label={`Close request tab ${t.name}`}>×</button>
              </div>
            ))}
            <button className="cmp-tab-new" onClick={newTab} title="New tab" aria-label="New request tab">＋</button>
          </div>

          {savingMode && (
            <div className="cmp-save-bar">
              <span style={{ fontSize: 11, color: 'var(--text-mid)', whiteSpace: 'nowrap' }}>Save as</span>
              <input className="kvedit-i" value={saveName} onChange={e => setSaveName(e.target.value)} placeholder="Request name" style={{ flex: 1, minWidth: 0 }} aria-label="Save request name" />
              <select className="kvedit-i" value={saveCollId} onChange={e => setSaveCollId(e.target.value)} style={{ width: 160 }} aria-label="Save to collection">
                {state.collections.map(c => <option key={c.id} value={c.id}>{c.name}</option>)}
                {state.collections.length === 0 && <option value="">New collection</option>}
              </select>
              <button className="btn primary sm" onClick={doSave}>Save</button>
              <button className="btn sm ghost" onClick={() => setSavingMode(false)}>Cancel</button>
            </div>
          )}

          {!active && (
            <div className="empty" style={{ flex: 1 }}>
              No request open. <button className="btn primary sm" onClick={newTab} style={{ marginLeft: 8 }}>+ New request</button>
            </div>
          )}

          {active && <ComposeEditor tab={active} updateActive={updateActive} send={send} openSaveBar={openSaveBar} resolveVars={resolveVars} />}
        </div>
      </div>
    </SurfaceShell>
  );
}

function ComposeEditor({ tab, updateActive, send, openSaveBar, resolveVars }) {
  const [bodyTab, setBodyTab] = React.useState('headers');
  const [resTab, setResTab] = React.useState('body');
  const resolved = resolveVars(tab.url);
  const showResolved = resolved !== tab.url && tab.url.includes('{{');
  const validationError = validateComposeTarget(resolved);

  return (
    <>
      <div className="cmp-req-line">
        <select className="cmp-method"
                aria-label="Request method"
                value={tab.method}
                onChange={e => updateActive({ method: e.target.value })}
                data-m={tab.method}>
          {['GET','POST','PUT','PATCH','DELETE','HEAD','OPTIONS'].map(m => <option key={m}>{m}</option>)}
        </select>
        <input className="cmp-url"
               aria-label="Request URL"
               placeholder="https://{{base}}/api/resource"
               value={tab.url}
               onChange={e => updateActive({ url: e.target.value })} />
        <button className="btn primary" onClick={send} disabled={!!validationError}>
          <Icon name="resume" size={10} /> Send
        </button>
        <button className="btn" onClick={() => copyText(buildComposeCurl(tab, resolveVars))}>cURL</button>
        <button className="btn" onClick={openSaveBar}>Save</button>
      </div>
      {showResolved && (
        <div className="cmp-resolved">
          <span className="mute">→</span>
          <span>{resolved}</span>
        </div>
      )}
      {validationError && (
        <div className="cmp-error">
          {validationError}
        </div>
      )}

      <div className="cmp-body-tabs">
        {['headers','params','auth','body'].map(k => (
          <button key={k}
                  className={'tab' + (bodyTab === k ? ' on' : '')}
                  onClick={() => setBodyTab(k)}>
            {k.charAt(0).toUpperCase() + k.slice(1)}
            {k === 'headers' && <span className="count">{tab.headers.filter(h => h.on).length}</span>}
            {k === 'params' && <span className="count">{tab.params.filter(p => p.on).length}</span>}
            {k === 'auth' && tab.authType !== 'none' && <span className="count">1</span>}
            {k === 'body' && tab.body && <span className="count">1</span>}
          </button>
        ))}
      </div>

      <div className="cmp-pane">
        {bodyTab === 'headers' && (
          <KvEditor items={tab.headers} onChange={(headers) => updateActive({ headers })}
                    addLabel="+ Header" placeholderK="Header" placeholderV="Value" />
        )}
        {bodyTab === 'params' && (
          <KvEditor items={tab.params} onChange={(params) => updateActive({ params })}
                    addLabel="+ Param" placeholderK="Key" placeholderV="Value" />
        )}
        {bodyTab === 'auth' && (
          <div className="cmp-body" style={{ padding: 12, gap: 10 }}>
            <div className="kvedit-row" style={{ gridTemplateColumns: '120px 1fr' }}>
              <label className="mute" style={{ fontSize: 11 }}>Type</label>
              <select className="kvedit-i" aria-label="Authentication type" value={tab.authType || 'none'} onChange={e => updateActive({ authType: e.target.value })}>
                <option value="none">No auth</option>
                <option value="bearer">Bearer token</option>
                <option value="basic">Basic auth</option>
              </select>
            </div>
            {tab.authType === 'bearer' && (
              <div className="kvedit-row" style={{ gridTemplateColumns: '120px 1fr' }}>
                <label className="mute" style={{ fontSize: 11 }}>Token</label>
                <input className="kvedit-i" aria-label="Bearer token" value={tab.authToken || ''} onChange={e => updateActive({ authToken: e.target.value })} placeholder="{{token}} or token value" />
              </div>
            )}
            {tab.authType === 'basic' && (
              <>
                <div className="kvedit-row" style={{ gridTemplateColumns: '120px 1fr' }}>
                  <label className="mute" style={{ fontSize: 11 }}>Username</label>
                  <input className="kvedit-i" aria-label="Basic auth username" value={tab.authUser || ''} onChange={e => updateActive({ authUser: e.target.value })} placeholder="user" />
                </div>
                <div className="kvedit-row" style={{ gridTemplateColumns: '120px 1fr' }}>
                  <label className="mute" style={{ fontSize: 11 }}>Password</label>
                  <input className="kvedit-i" type="password" aria-label="Basic auth password" value={tab.authPass || ''} onChange={e => updateActive({ authPass: e.target.value })} placeholder="password" />
                </div>
              </>
            )}
          </div>
        )}
        {bodyTab === 'body' && (
          <div className="cmp-body">
            <div className="cmp-body-bar">
              <div className="segctl">
                <button className={tab.bodyMode === 'none' ? 'on' : ''} onClick={() => updateActive({ bodyMode: 'none' })}>none</button>
                <button className={tab.bodyMode !== 'none' ? 'on' : ''} onClick={() => updateActive({ bodyMode: 'raw' })}>raw</button>
              </div>
              {tab.bodyMode !== 'none' && (
                <select className="cmp-ct" value={tab.contentType}
                        aria-label="Request body content type"
                        onChange={e => updateActive({ contentType: e.target.value })}>
                  <option>application/json</option>
                  <option>text/plain</option>
                  <option>text/html</option>
                  <option>application/xml</option>
                </select>
              )}
              <div className="spacer" />
              {tab.bodyMode !== 'none' && <button className="copy-btn" onClick={() => beautifyBody(tab, updateActive)}>Beautify</button>}
            </div>
            {tab.bodyMode !== 'none' && (
              <textarea className="cmp-body-ta"
                        aria-label="Request body"
                        spellCheck={false}
                        value={tab.body || ''}
                        onChange={e => updateActive({ body: e.target.value })} />
            )}
          </div>
        )}
      </div>

      {tab.response && (
        <div className="cmp-response">
          <div className="cmp-res-bar">
            <span className="cell-status" data-c={String(tab.response.status)[0]} style={{ fontFamily: 'var(--font-mono)' }}>
              {tab.response.status} {tab.response.statusText}
            </span>
            <span className="mute" style={{ fontFamily: 'var(--font-mono)', fontSize: 11 }}>·</span>
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text)' }}>{tab.response.timeMs} ms</span>
            <span className="mute" style={{ fontFamily: 'var(--font-mono)', fontSize: 11 }}>·</span>
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text)' }}>{fmtBytes(tab.response.size)}</span>
            <div className="spacer" />
            <div className="cmp-body-tabs" style={{ margin: 0, border: 0 }}>
              {['body','headers','timing'].map(k => (
                <button key={k} className={'tab' + (resTab === k ? ' on' : '')} onClick={() => setResTab(k)} style={{ padding: '4px 10px' }}>
                  {k}
                </button>
              ))}
            </div>
            <button className="icon-btn" onClick={() => updateActive({ response: null })} title="Close response" aria-label="Close response"><Icon name="x" size={12} /></button>
          </div>
          <div className="cmp-res-body">
            {resTab === 'body' && (
              <pre className="cmp-json">{typeof tab.response.body === 'string' ? tab.response.body : JSON.stringify(tab.response.body, null, 2)}</pre>
            )}
            {resTab === 'headers' && <HeaderList obj={tab.response.headers} />}
            {resTab === 'timing' && (
              <div style={{ padding: 12, fontFamily: 'var(--font-mono)', fontSize: 11.5 }}>
                <div className="kv">
                  <div className="k">Request</div><div className="v">{tab.response.timeMs} ms</div>
                  <div className="k">Total</div><div className="v hi">{tab.response.timeMs} ms</div>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </>
  );
}

function HeaderList({ obj }) {
  const entries = Object.entries(obj || {});
  if (entries.length === 0) {
    return <div className="empty">No headers</div>;
  }
  return (
    <div className="kv">
      {entries.map(([k, v], idx) => (
        <React.Fragment key={`${k}-${idx}`}>
          <div className="k">{k}</div>
          <div className="v" title={String(v)}>{String(v)}</div>
        </React.Fragment>
      ))}
    </div>
  );
}

function KvEditor({ items, onChange, addLabel, placeholderK, placeholderV }) {
  const update = (id, patch) => onChange(items.map(it => it.id === id ? { ...it, ...patch } : it));
  const remove = (id) => onChange(items.filter(it => it.id !== id));
  const add = () => onChange([...items, { id: 'n' + Date.now(), on: true, key: '', value: '' }]);
  return (
    <div className="kvedit">
      <div className="kvedit-head">
        <div></div>
        <div>{placeholderK}</div>
        <div>{placeholderV}</div>
        <div></div>
      </div>
      {items.map(it => (
        <div key={it.id} className={'kvedit-row' + (it.on ? '' : ' off')}>
          <Toggle label={`Toggle ${placeholderK.toLowerCase()} row`} on={it.on} onChange={(on) => update(it.id, { on })} />
          <input className="kvedit-i" aria-label={placeholderK} placeholder={placeholderK} value={it.key} onChange={e => update(it.id, { key: e.target.value })} />
          <input className="kvedit-i" aria-label={placeholderV} placeholder={placeholderV} value={it.value} onChange={e => update(it.id, { value: e.target.value })} />
          <button className="kvedit-x" onClick={() => remove(it.id)} title="Remove" aria-label={`Remove ${placeholderK.toLowerCase()} row`}>×</button>
        </div>
      ))}
      <button className="btn sm ghost" onClick={add} style={{ margin: '8px 12px' }}>{addLabel}</button>
    </div>
  );
}

window.ComposeSurface = ComposeSurface;
