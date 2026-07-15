import React from 'react';

const { Icon } = window;

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max);
}

function normalizeItems(items = []) {
  return items.filter(item => item && !item.hidden);
}

function ContextMenu({ menu, onClose }) {
  const ref = React.useRef(null);
  const items = normalizeItems(menu?.items);
  const [pos, setPos] = React.useState(() => ({ left: menu?.x || 0, top: menu?.y || 0 }));

  React.useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const rect = node.getBoundingClientRect();
    setPos({
      left: clamp(menu.x, 8, Math.max(8, window.innerWidth - rect.width - 8)),
      top: clamp(menu.y, 8, Math.max(8, window.innerHeight - rect.height - 8)),
    });
  }, [menu?.x, menu?.y, items.length]);

  React.useEffect(() => {
    const dismiss = (event) => {
      if (ref.current && !ref.current.contains(event.target)) onClose?.();
    };
    const onKey = (event) => {
      if (event.key === 'Escape') onClose?.();
    };
    document.addEventListener('mousedown', dismiss);
    document.addEventListener('contextmenu', dismiss);
    document.addEventListener('scroll', onClose, true);
    document.addEventListener('keydown', onKey);
    window.addEventListener('resize', onClose);
    return () => {
      document.removeEventListener('mousedown', dismiss);
      document.removeEventListener('contextmenu', dismiss);
      document.removeEventListener('scroll', onClose, true);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', onClose);
    };
  }, [onClose]);

  if (!menu || items.length === 0) return null;

  const select = (item) => {
    if (item.disabled) return;
    item.onSelect?.();
    onClose?.();
  };

  return (
    <div
      ref={ref}
      className="context-menu"
      role="menu"
      style={{ left: pos.left, top: pos.top }}
      onClick={event => event.stopPropagation()}
      onContextMenu={event => event.preventDefault()}
    >
      {(menu.title || menu.subtitle) && (
        <div className="context-menu-head">
          {menu.title && <div className="context-menu-title">{menu.title}</div>}
          {menu.subtitle && <div className="context-menu-subtitle">{menu.subtitle}</div>}
        </div>
      )}
      {items.map(item => item.type === 'separator' ? (
        <div key={item.id || Math.random()} className="context-menu-sep" role="separator" />
      ) : (
        <button
          key={item.id}
          type="button"
          role="menuitem"
          className={[
            'context-menu-item',
            item.danger ? 'danger' : '',
            item.disabled ? 'disabled' : '',
            item.separatorBefore ? 'sep-before' : '',
          ].filter(Boolean).join(' ')}
          disabled={!!item.disabled}
          title={item.disabledReason || item.title || item.label}
          onClick={() => select(item)}
        >
          {item.icon && <Icon name={item.icon} size={13} stroke={1.8} />}
          <span className="context-menu-label">{item.label}</span>
          {item.shortcut && <span className="context-menu-shortcut">{item.shortcut}</span>}
        </button>
      ))}
    </div>
  );
}

window.ContextMenu = ContextMenu;
