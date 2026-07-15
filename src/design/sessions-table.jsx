import React from 'react';
/* Sessions list — table with column sorting, drag-to-resize columns, sticky header */

const fmtBytes = (n) => {
  if (n == null) return '—';
  if (n === 0) return '0';
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
  return (n / 1024 / 1024).toFixed(2) + ' MB';
};
const fmtMs = (n) => {
  if (!n && n !== 0) return '—';
  if (n === 0) return '0 ms';
  if (n < 1000) return n + ' ms';
  return (n / 1000).toFixed(2) + ' s';
};
const fmtTime = (ts) => {
  const d = new Date(ts);
  return d.toTimeString().slice(0, 8) + '.' + String(d.getMilliseconds()).padStart(3, '0');
};
const statusBucket = (s) => {
  if (s === 0) return '-';
  return String(s)[0];
};
// Compact label for the negotiated wire protocol, e.g. "HTTP/2" → "H2".
const protoShort = (p) => {
  if (!p) return '—';
  if (p === 'HTTP/1.1' || p === 'HTTP/1.0') return '1.1';
  if (p === 'HTTP/2') return 'H2';
  if (p === 'HTTP/3') return 'H3';
  if (p === 'SOCKS5') return 'SOCKS';
  return p.replace(/^HTTP\//, '');
};
// Stable bucket used for the badge colour/data attribute.
const protoBucket = (p) => {
  if (p === 'HTTP/2') return 'h2';
  if (p === 'HTTP/3') return 'h3';
  if (p === 'HTTP/1.1' || p === 'HTTP/1.0') return 'h1';
  if (p === 'SOCKS5') return 'socks';
  return 'other';
};

const appShort = (p) => {
  if (p === 'WebSocket') return 'WS';
  if (p === 'gRPC') return 'gRPC';
  if (p === 'Tunnel') return 'TUNNEL';
  return p || 'HTTP';
};
const appBucket = (p) => {
  if (p === 'WebSocket') return 'ws';
  if (p === 'gRPC') return 'grpc';
  if (p === 'Tunnel') return 'socks';
  return 'other';
};
const contentLabel = (value) => {
  const v = String(value || 'http').toLowerCase();
  if (v === 'ws') return 'FRAMES';
  if (v === 'grpc') return 'PROTO';
  if (v === 'pending') return '—';
  if (v === 'tunnel') return 'BYTES';
  return v.toUpperCase();
};

window.fmtBytes = fmtBytes;
window.fmtMs = fmtMs;
window.fmtTime = fmtTime;
window.statusBucket = statusBucket;
window.protoShort = protoShort;

function MiniWaterfall({ timing, max }) {
  const t = timing;
  const total = t.dns + t.tcp + t.tls + t.ttfb + t.body;
  const w = Math.max(2, (total / max) * 100);
  return (
    <div className="waterfall" style={{ width: w + '%' }}>
      {t.dns > 0 && <span className="seg dns" style={{ left: 0, width: pct(t.dns, total) }} />}
      {t.tcp > 0 && <span className="seg tcp" style={{ left: pct(t.dns, total), width: pct(t.tcp, total) }} />}
      {t.tls > 0 && <span className="seg tls" style={{ left: pct(t.dns + t.tcp, total), width: pct(t.tls, total) }} />}
      {t.ttfb > 0 && <span className="seg ttfb" style={{ left: pct(t.dns + t.tcp + t.tls, total), width: pct(t.ttfb, total) }} />}
      {t.body > 0 && <span className="seg body" style={{ left: pct(t.dns + t.tcp + t.tls + t.ttfb, total), width: pct(t.body, total) }} />}
    </div>
  );
}
function pct(n, total) { return ((n / total) * 100) + '%'; }

// ── Column definitions ─────────────────────────────────────────────────────
// key        → localStorage key + React key
// label      → header text
// sortKey    → sort.key value (matches SESSION_SORT_KEYS in app.jsx), null = not sortable
// defaultWidth → px (null = flex: takes remaining space)
// align      → text-align for header and cells
// tooltip    → extra hint shown in th title attribute
const COLUMN_DEFS = [
  { key: 'method',    label: 'METHOD',    sortKey: 'method',  defaultWidth: 64,  align: 'left'   },
  { key: 'status',    label: 'STATUS',    sortKey: 'status',  defaultWidth: 62,  align: 'left'   },
  { key: 'host',      label: 'HOST',      sortKey: 'host',    defaultWidth: 180, align: 'left'   },
  { key: 'path',      label: 'PATH',      sortKey: 'path',    defaultWidth: null, align: 'left'  }, // flex — gets remaining space
  { key: 'app',       label: 'APP',       sortKey: null,      defaultWidth: 72,  align: 'center', tooltip: 'Application family' },
  { key: 'proto',     label: 'WIRE',      sortKey: 'protocol', defaultWidth: 68, align: 'center', tooltip: 'Downstream wire protocol' },
  { key: 'type',      label: 'CONTENT',   sortKey: 'type',    defaultWidth: 72,  align: 'left', tooltip: 'Payload/content shape' },
  { key: 'source',    label: 'SOURCE',    sortKey: null,      defaultWidth: 76,  align: 'left', tooltip: 'Capture source' },
  { key: 'tls',       label: 'TLS',       sortKey: null,      defaultWidth: 40,  align: 'center', tooltip: 'Transport security' },
  { key: 'size',      label: 'SIZE',      sortKey: 'reqSize', defaultWidth: 68,  align: 'right'  },
  { key: 'time',      label: 'TIME',      sortKey: 'total',   defaultWidth: 72,  align: 'right'  },
  { key: 'waterfall', label: 'WATERFALL', sortKey: null,      defaultWidth: 170, align: 'left'   },
  { key: 'when',      label: 'WHEN',      sortKey: 'ts',      defaultWidth: 90,  align: 'right'  },
];

const COL_STORAGE_KEY = 'oproxy_col_widths_v1';
const COL_MIN_WIDTH = 36;

function loadColWidths() {
  try {
    const raw = localStorage.getItem(COL_STORAGE_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch { return {}; }
}
function saveColWidths(widths) {
  try { localStorage.setItem(COL_STORAGE_KEY, JSON.stringify(widths)); } catch {}
}

// ── Context menu for creating rules/mocks/breakpoints from sessions ───────────

function SessionContextMenu({ x, y, session, onClose }) {
  const ref = React.useRef(null);

  React.useEffect(() => {
    const dismiss = (e) => { if (ref.current && !ref.current.contains(e.target)) onClose(); };
    document.addEventListener('mousedown', dismiss);
    document.addEventListener('scroll', onClose, true);
    return () => { document.removeEventListener('mousedown', dismiss); document.removeEventListener('scroll', onClose, true); };
  }, [onClose]);

  const dispatch = (action) => {
    const loc = {
      host: session.host || '',
      path: session.path || '.*',
      methods: session.method && session.method !== '*' && !['CONNECT'].includes(session.method)
        ? [session.method] : undefined,
      mode: 'glob',
    };
    const prefill = { location: loc, name: `${action === 'mock' ? 'Mock' : action === 'rule' ? 'Rewrite' : 'BP'} ${session.method || ''} ${session.host || ''}${session.path || '/'}`.trim() };
    window.dispatchEvent(new CustomEvent('oproxy:create-from-session', { detail: { action, prefill } }));
    onClose();
  };

  return (
    <div
      ref={ref}
      style={{
        position: 'fixed', left: x, top: y, zIndex: 9999,
        background: 'var(--surface-2, #2a2a2a)', border: '1px solid var(--border)',
        borderRadius: 6, boxShadow: '0 4px 16px rgba(0,0,0,0.4)', padding: '4px 0',
        minWidth: 190, fontSize: 12,
      }}
      onClick={e => e.stopPropagation()}>
      <div style={{ padding: '3px 10px 5px', fontSize: 10, color: 'var(--text-faint)', textTransform: 'uppercase', letterSpacing: '0.08em', borderBottom: '1px solid var(--border-soft)', marginBottom: 3 }}>
        {session.method} {session.host}{session.path}
      </div>
      {[
        ['mock',       '🔲 Create mock rule'],
        ['rule',       '✏️ Add rewrite rule'],
        ['breakpoint', '⏸ Add breakpoint'],
      ].map(([action, label]) => (
        <button key={action}
          style={{ display: 'block', width: '100%', textAlign: 'left', padding: '6px 14px', background: 'none', border: 'none', color: 'var(--text)', cursor: 'pointer', fontSize: 12 }}
          onMouseEnter={e => e.currentTarget.style.background = 'var(--surface-hi, rgba(255,255,255,0.07))'}
          onMouseLeave={e => e.currentTarget.style.background = 'none'}
          onClick={() => dispatch(action)}>
          {label}
        </button>
      ))}
    </div>
  );
}

function SessionsTable({ sessions, selectedId, onSelect, sort, onSort, bulkSel, onBulkToggle, onBulkToggleAll, emptyState }) {
  const maxTotal = Math.max(...sessions.map(s => s.total), 1);
  const hasBulk = !!onBulkToggle;
  const [contextMenu, setContextMenu] = React.useState(null); // {x, y, session}
  const allChecked = hasBulk && sessions.length > 0 && sessions.every(s => bulkSel?.has(s.id));

  // ── Column widths (localStorage-backed) ───────────────────────────────────
  const [colWidths, setColWidths] = React.useState(() => {
    const saved = loadColWidths();
    const result = {};
    COLUMN_DEFS.forEach(c => { result[c.key] = saved[c.key] ?? c.defaultWidth; });
    return result;
  });

  const resizeRef = React.useRef(null); // { key, startX, startWidth }

  const onResizeStart = React.useCallback((e, key) => {
    e.preventDefault();
    e.stopPropagation();
    const currentWidth = colWidths[key] ?? COLUMN_DEFS.find(c => c.key === key)?.defaultWidth ?? 100;
    resizeRef.current = { key, startX: e.clientX, startWidth: currentWidth };

    const onMove = (me) => {
      if (!resizeRef.current) return;
      const { key: k, startX, startWidth } = resizeRef.current;
      const newWidth = Math.max(COL_MIN_WIDTH, startWidth + (me.clientX - startX));
      setColWidths(prev => ({ ...prev, [k]: newWidth }));
    };

    const onUp = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      document.body.classList.remove('col-resizing');
      setColWidths(prev => { saveColWidths(prev); return prev; });
      resizeRef.current = null;
    };

    document.body.classList.add('col-resizing');
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }, [colWidths]);

  // ── Column header renderer ─────────────────────────────────────────────────
  const colHead = (colDef) => {
    const { key, label, sortKey, align, tooltip } = colDef;
    const dir = (sortKey && sort.key === sortKey) ? sort.dir : null;
    const sortHint = sortKey
      ? (dir === 'asc'  ? ' · click to sort descending'
       : dir === 'desc' ? ' · click to clear sort'
       :                  ' · click to sort ascending')
      : '';
    const title = [tooltip, label + sortHint].filter(Boolean).join(' — ');
    const width = colWidths[key];

    return (
      <th
        key={key}
        onClick={sortKey ? () => onSort(sortKey) : undefined}
        title={title}
        style={{
          textAlign: align || 'left',
          width: width != null ? width + 'px' : undefined,
          cursor: sortKey ? 'pointer' : 'default',
          position: 'relative',
          overflow: 'visible',
        }}
      >
        {label}
        {dir && <span className="sort">{dir === 'asc' ? '↑' : '↓'}</span>}
        <span
          className="col-resize-handle"
          onMouseDown={(e) => onResizeStart(e, key)}
          onClick={(e) => e.stopPropagation()}
          title="Drag to resize column"
        />
      </th>
    );
  };

  return (
    <div className="table-wrap" role="grid" onClick={() => contextMenu && setContextMenu(null)}>
      <table className="t">
        <colgroup>
          {hasBulk && <col style={{ width: '28px' }} />}
          {COLUMN_DEFS.map(c => (
            <col key={c.key} style={colWidths[c.key] != null ? { width: colWidths[c.key] + 'px' } : {}} />
          ))}
        </colgroup>
        <thead>
          <tr>
            {hasBulk && (
              <th className="cell-check" style={{ position: 'relative', overflow: 'visible', width: '28px' }}>
                <input type="checkbox"
                       aria-label="Select all visible sessions"
                       checked={allChecked}
                       onChange={(e) => onBulkToggleAll(e.target.checked)}
                       onClick={(e) => e.stopPropagation()} />
              </th>
            )}
            {COLUMN_DEFS.map(c => colHead(c))}
          </tr>
        </thead>
        <tbody>
          {sessions.map(s => {
            const bucket = statusBucket(s.status);
            const tls = (s.scheme === 'https' || s.scheme === 'wss')
              ? (s.method === 'CONNECT' ? 'tunnel' : 'ok')
              : 'plain';
            return (
              <tr key={s.id}
                  className={[
                    selectedId === s.id ? 'selected' : '',
                    s.paused  ? 'paused'  : '',
                    s.pending ? 'pending' : '',
                  ].filter(Boolean).join(' ')}
                  onClick={() => onSelect(s.id)}
                  onContextMenu={e => { e.preventDefault(); setContextMenu({ x: e.clientX, y: e.clientY, session: s }); }}>
                {hasBulk && (
                  <td className="cell-check" onClick={(e) => e.stopPropagation()}>
                    <input type="checkbox"
                           aria-label={`Select session ${s.displayMethod || s.method} ${s.status || 'pending'} ${s.host}${s.path}`}
                           checked={bulkSel?.has(s.id) || false}
                           onChange={() => onBulkToggle(s.id)} />
                  </td>
                )}
                <td><span className="cell-method" data-m={s.displayMethod || s.method}>{s.displayMethod || s.method}</span></td>
                <td>
                  <span className="cell-status" data-c={s.paused ? 'bp' : bucket}>
                    {s.paused ? '⏸' : s.pending ? '···' : (s.status || '—')}
                  </span>
                </td>
                <td className="cell-host" title={s.host}>{s.host}</td>
                <td className="cell-path" title={s.path + s.query}>
                  {s.path}{s.query && <span className="dim">{s.query}</span>}
                  {s.tags.includes('replay')   && <span className="tag-badge replay">REPLAY</span>}
                  {s.tags.includes('mock')     && <span className="tag-badge mock">MOCK</span>}
                  {s.tags.includes('rewrite')  && <span className="tag-badge rewrite">REWRITE</span>}
                  {s.tags.includes('bp')       && <span className="tag-badge bp">BP</span>}
                  {s.tags.includes('mitm')     && <span className="tag-badge mitm">MITM</span>}
                  {s.tags.includes('ws')       && <span className="tag-badge ws">WS</span>}
                  {s.tags.includes('sse')      && <span className="tag-badge sse">SSE</span>}
                  {s.tags.includes('streamed') && <span className="tag-badge streamed" title="Body streamed past the proxy; not fully captured">STREAMED</span>}
                </td>
                <td>
                  {(s.paused || s.pending) ? <span className="dim">—</span>
                    : <span className="proto-badge" data-proto={appBucket(s.appProtocol)} title={s.appProtocol}>{appShort(s.appProtocol)}</span>}
                </td>
                <td>
                  {(s.paused || s.pending) ? <span className="dim">—</span>
                    : <span className="proto-badge" data-proto={protoBucket(s.wireProtocol)} title={s.wireProtocol}>{protoShort(s.wireProtocol)}</span>}
                </td>
                <td className="cell-type">{contentLabel(s.type)}</td>
                <td className="cell-type" title={s.sourceLabel}>{s.sourceLabel}</td>
                <td><span className={'tls-cell ' + tls}>{tls === 'ok' ? '🔒' : tls === 'tunnel' ? '⇿' : '○'}</span></td>
                <td className="cell-num">{fmtBytes(s.resSize || s.reqSize)}</td>
                <td className="cell-num">{(s.paused || s.pending) ? '—' : fmtMs(s.total)}</td>
                <td>
                  {!s.paused && !s.pending && <MiniWaterfall timing={s.timing} max={maxTotal} />}
                </td>
                <td className="cell-num" style={{ fontSize: '10.5px' }}>{fmtTime(s.ts)}</td>
              </tr>
            );
          })}
          {sessions.length === 0 && (
            <tr><td colSpan={hasBulk ? COLUMN_DEFS.length + 1 : COLUMN_DEFS.length}>
              <div className="empty">
                {emptyState?.title || 'No sessions match the current filters.'}
                <br />
                <span className="mute">{emptyState?.hint || 'Try clearing search or method filters.'}</span>
              </div>
            </td></tr>
          )}
        </tbody>
      </table>
      {contextMenu && (
        <SessionContextMenu
          x={contextMenu.x} y={contextMenu.y} session={contextMenu.session}
          onClose={() => setContextMenu(null)} />
      )}
    </div>
  );
}

/* Structure view — host/path tree */
function StructureView({ sessions, selectedId, onSelect, emptyState }) {
  const tree = React.useMemo(() => {
    const t = {};
    sessions.forEach(s => {
      const segs = s.path.split('/').filter(Boolean);
      const seg = segs[0] || '/';
      t[s.host] = t[s.host] || {};
      t[s.host][seg] = t[s.host][seg] || [];
      t[s.host][seg].push(s);
    });
    return t;
  }, [sessions]);
  const [openHosts, setOpenHosts] = React.useState(() => new Set(Object.keys(tree)));
  const [openSegs, setOpenSegs] = React.useState(() => new Set());
  const toggleHost = h => setOpenHosts(p => { const n = new Set(p); n.has(h) ? n.delete(h) : n.add(h); return n; });
  const toggleSeg = key => setOpenSegs(p => { const n = new Set(p); n.has(key) ? n.delete(key) : n.add(key); return n; });

  return (
    <div className="table-wrap">
      <div className="tree">
        {Object.keys(tree).length === 0 && (
          <div className="empty">
            {emptyState?.title || 'No sessions match the current filters.'}
            <br />
            <span className="mute">{emptyState?.hint || 'Try clearing search or method filters.'}</span>
          </div>
        )}
        {Object.entries(tree).map(([host, segs]) => {
          const hostOpen = openHosts.has(host);
          const count = Object.values(segs).reduce((a, arr) => a + arr.length, 0);
          return (
            <div key={host}>
              <div className="tree-node" onClick={() => toggleHost(host)}>
                <span className="twig">{hostOpen ? '▾' : '▸'}</span>
                <span className="name">{host}</span>
                <span className="count">{count}</span>
              </div>
              {hostOpen && Object.entries(segs).map(([seg, leaves]) => {
                const key = host + '/' + seg;
                const segOpen = openSegs.has(key);
                return (
                  <div key={key}>
                    <div className="tree-node" style={{ paddingLeft: 34 }} onClick={() => toggleSeg(key)}>
                      <span className="twig">{segOpen ? '▾' : '▸'}</span>
                      <span className="name dim">/{seg}</span>
                      <span className="count">{leaves.length}</span>
                    </div>
                    {segOpen && leaves.map(s => (
                      <div key={s.id}
                           className={'tree-node tree-leaf' + (selectedId === s.id ? ' selected' : '')}
                           style={{ paddingLeft: 56 }}
                           onClick={() => onSelect(s.id)}>
                        <span className="cell-method" data-m={s.method}>{s.method}</span>
                        <span className="path">{s.path}{s.query && <span className="dim">{s.query}</span>}</span>
                        <span className="status cell-status" data-c={statusBucket(s.status)}>{s.status || '⏸'}</span>
                      </div>
                    ))}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}

window.StructureView = StructureView;
window.SessionsTable = SessionsTable;
window.MiniWaterfall = MiniWaterfall;
