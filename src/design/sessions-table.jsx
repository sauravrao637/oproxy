import React from 'react';
/* Sessions list — table with column sorting, sticky header, status badges, mini waterfall */

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

window.fmtBytes = fmtBytes;
window.fmtMs = fmtMs;
window.fmtTime = fmtTime;
window.statusBucket = statusBucket;

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

function SessionsTable({ sessions, selectedId, onSelect, sort, onSort, bulkSel, onBulkToggle, onBulkToggleAll, emptyState }) {
  const maxTotal = Math.max(...sessions.map(s => s.total), 1);
  const hasBulk = !!onBulkToggle;
  const allChecked = hasBulk && sessions.length > 0 && sessions.every(s => bulkSel?.has(s.id));

  const colHead = (key, label, align) => {
    const dir = sort.key === key ? sort.dir : null;
    const next = dir === 'asc' ? '↓ next' : dir === 'desc' ? 'clear next' : '↑ next';
    return (
      <th onClick={() => onSort(key)}
          style={{ textAlign: align || 'left' }}
          title={'Sort by ' + label + ' · click again to reverse · third click clears (' + next + ')'}>
        {label}
        {dir && <span className="sort">{dir === 'asc' ? '↑' : '↓'}</span>}
      </th>
    );
  };

  return (
    <div className="table-wrap" role="grid">
      <table className="t">
        <colgroup>
          {hasBulk && <col style={{ width: '28px' }} />}
          <col style={{ width: '38px' }} />
          <col style={{ width: '58px' }} />
          <col style={{ width: '58px' }} />
          <col style={{ width: '170px' }} />
          <col />
          <col style={{ width: '54px' }} />
          <col style={{ width: '38px' }} />
          <col style={{ width: '64px' }} />
          <col style={{ width: '70px' }} />
          <col style={{ width: '170px' }} />
          <col style={{ width: '88px' }} />
        </colgroup>
        <thead>
          <tr>
            {hasBulk && (
              <th className="cell-check">
                <input type="checkbox"
                       aria-label="Select all visible sessions"
                       checked={allChecked}
                       onChange={(e) => onBulkToggleAll(e.target.checked)}
                       onClick={(e) => e.stopPropagation()} />
              </th>
            )}
            {colHead('idx', '#')}
            {colHead('method', 'METHOD')}
            {colHead('status', 'STATUS')}
            {colHead('host', 'HOST')}
            {colHead('path', 'PATH')}
            {colHead('type', 'TYPE')}
            <th title="Transport security">TLS</th>
            {colHead('reqSize', 'SIZE', 'right')}
            {colHead('total', 'TIME', 'right')}
            <th>WATERFALL</th>
            {colHead('ts', 'WHEN', 'right')}
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
                    s.paused ? 'paused' : ''
                  ].join(' ')}
                  onClick={() => onSelect(s.id)}>
                {hasBulk && (
                  <td className="cell-check" onClick={(e) => e.stopPropagation()}>
                    <input type="checkbox"
                           aria-label={`Select session ${s.method} ${s.status || 'pending'} ${s.host}${s.path}`}
                           checked={bulkSel?.has(s.id) || false}
                           onChange={() => onBulkToggle(s.id)} />
                  </td>
                )}
                <td className="dim" style={{ textAlign: 'right' }}>{s.idx}</td>
                <td><span className="cell-method" data-m={s.method}>{s.method}</span></td>
                <td>
                  <span className="cell-status" data-c={bucket}>
                    {s.paused ? '⏸' : (s.status || '—')}
                  </span>
                </td>
                <td className="cell-host" title={s.host}>{s.host}</td>
                <td className="cell-path" title={s.path + s.query}>
                  {s.path}{s.query && <span className="dim">{s.query}</span>}
                  {s.tags.includes('replay')  && <span className="tag-badge replay">REPLAY</span>}
                  {s.tags.includes('mock')    && <span className="tag-badge mock">MOCK</span>}
                  {s.tags.includes('rewrite') && <span className="tag-badge rewrite">REWRITE</span>}
                  {s.tags.includes('bp')      && <span className="tag-badge bp">BP</span>}
                  {s.tags.includes('mitm')    && <span className="tag-badge mitm">MITM</span>}
                  {s.tags.includes('ws')      && <span className="tag-badge ws">WS</span>}
                  {s.tags.includes('sse')     && <span className="tag-badge sse">SSE</span>}
                </td>
                <td className="cell-type">{s.type}</td>
                <td><span className={'tls-cell ' + tls}>{tls === 'ok' ? '🔒' : tls === 'tunnel' ? '⇿' : '○'}</span></td>
                <td className="cell-num">{fmtBytes(s.resSize || s.reqSize)}</td>
                <td className="cell-num">{s.paused ? '—' : fmtMs(s.total)}</td>
                <td>
                  {!s.paused && <MiniWaterfall timing={s.timing} max={maxTotal} />}
                </td>
                <td className="cell-num" style={{ fontSize: '10.5px' }}>{fmtTime(s.ts)}</td>
              </tr>
            );
          })}
          {sessions.length === 0 && (
            <tr><td colSpan={hasBulk ? 12 : 11}>
              <div className="empty">
                {emptyState?.title || 'No sessions match the current filters.'}
                <br />
                <span className="mute">{emptyState?.hint || 'Try clearing search or method filters.'}</span>
              </div>
            </td></tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

/* Structure view — host/path tree */
function StructureView({ sessions, selectedId, onSelect, emptyState }) {
  // build host -> first-segment -> leaves
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
