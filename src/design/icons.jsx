import React from 'react';
/* Inline SVG icons — stroke-based, 16px viewBox 24x24. Kept simple. */
const Icon = ({ name, size = 16, stroke = 1.6 }) => {
  const paths = ICONS[name];
  if (!paths) return null;
  return (
    <svg width={size} height={size} viewBox="0 0 24 24"
         fill="none" stroke="currentColor" strokeWidth={stroke}
         strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {paths}
    </svg>
  );
};

const ICONS = {
  // Brand glyph (also used as left-rail logo)
  logo: <>
    <path d="M4 7l8-4 8 4-8 4-8-4z" />
    <path d="M4 12l8 4 8-4" opacity="0.6" />
    <path d="M4 17l8 4 8-4" opacity="0.3" />
  </>,
  search: <>
    <circle cx="11" cy="11" r="6.5" />
    <path d="M16.5 16.5L21 21" />
  </>,
  record: <circle cx="12" cy="12" r="5" fill="currentColor" stroke="none" />,
  pause: <>
    <rect x="7" y="5" width="3.5" height="14" rx="1" fill="currentColor" stroke="none" />
    <rect x="13.5" y="5" width="3.5" height="14" rx="1" fill="currentColor" stroke="none" />
  </>,
  play: <path d="M7 5l12 7-12 7V5z" fill="currentColor" stroke="none" />,
  trash: <>
    <path d="M4 7h16" />
    <path d="M9 7V4h6v3" />
    <path d="M6 7l1 13h10l1-13" />
    <path d="M10 11v6M14 11v6" />
  </>,
  replay: <>
    <path d="M3 12a9 9 0 1 0 3-6.7" />
    <path d="M3 4v5h5" />
  </>,
  save: <>
    <path d="M5 4h11l3 3v13H5z" />
    <path d="M7 4v6h9V4" />
    <rect x="8" y="13" width="8" height="5" />
  </>,
  filter: <>
    <path d="M4 5h16l-6 8v6l-4-2v-4z" />
  </>,
  sun: <>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
  </>,
  moon: <path d="M21 13.5A9 9 0 1 1 10.5 3a7 7 0 0 0 10.5 10.5z" />,
  layout: <>
    <rect x="3" y="4" width="18" height="16" rx="1.5" />
    <path d="M12 4v16" />
  </>,
  layoutH: <>
    <rect x="3" y="4" width="18" height="16" rx="1.5" />
    <path d="M3 13h18" />
  </>,
  sliders: <>
    <path d="M4 6h16M4 12h16M4 18h16" />
    <circle cx="8" cy="6" r="2" fill="var(--bg)" />
    <circle cx="14" cy="12" r="2" fill="var(--bg)" />
    <circle cx="17" cy="18" r="2" fill="var(--bg)" />
  </>,
  cog: <>
    <circle cx="12" cy="12" r="3" />
    <path d="M19 12a7 7 0 0 0-.13-1.3l2-1.6-2-3.4-2.3.9a7 7 0 0 0-2.2-1.3l-.4-2.4h-4l-.4 2.4a7 7 0 0 0-2.2 1.3l-2.3-.9-2 3.4 2 1.6A7 7 0 0 0 5 12c0 .44.05.87.13 1.3l-2 1.6 2 3.4 2.3-.9a7 7 0 0 0 2.2 1.3l.4 2.4h4l.4-2.4a7 7 0 0 0 2.2-1.3l2.3.9 2-3.4-2-1.6c.08-.43.13-.86.13-1.3z" />
  </>,
  list: <>
    <path d="M3 6h18M3 12h18M3 18h18" />
  </>,
  rules: <>
    <path d="M4 6h12M4 12h16M4 18h10" />
    <circle cx="19" cy="6" r="2" />
    <circle cx="14" cy="18" r="2" />
  </>,
  pauseRail: <>
    <rect x="6" y="4" width="4" height="16" rx="1" />
    <rect x="14" y="4" width="4" height="16" rx="1" />
  </>,
  inspector: <>
    <circle cx="11" cy="11" r="6" />
    <path d="M15.5 15.5L21 21" />
    <path d="M11 8v6M8 11h6" />
  </>,
  cert: <>
    <circle cx="12" cy="10" r="4" />
    <path d="M9 13l-1 8 4-2 4 2-1-8" />
  </>,
  copy: <>
    <rect x="8" y="8" width="12" height="12" rx="1.5" />
    <path d="M4 16V5a1 1 0 0 1 1-1h11" />
  </>,
  download: <>
    <path d="M12 4v12M7 11l5 5 5-5" />
    <path d="M5 20h14" />
  </>,
  upload: <>
    <path d="M12 20V8M7 13l5-5 5 5" />
    <path d="M5 4h14" />
  </>,
  open: <>
    <path d="M5 5h6M5 5v6M5 5l8 8" />
    <path d="M19 19h-6M19 19v-6M19 19l-8-8" />
  </>,
  chevronDown: <path d="M6 9l6 6 6-6" />,
  chevronRight: <path d="M9 6l6 6-6 6" />,
  x: <path d="M6 6l12 12M18 6L6 18" />,
  shield: <>
    <path d="M12 3l8 3v6c0 5-3.5 8.5-8 9-4.5-.5-8-4-8-9V6z" />
    <path d="M9 12l2 2 4-4" />
  </>,
  bolt: <path d="M13 2L4 14h7l-1 8 9-12h-7l1-8z" />,
  wifi: <>
    <path d="M5 12a10 10 0 0 1 14 0" />
    <path d="M8.5 15a5 5 0 0 1 7 0" />
    <circle cx="12" cy="18" r="1" fill="currentColor" />
  </>,
  resume: <path d="M7 5l12 7-12 7V5z" fill="currentColor" stroke="none" />,
  send: <>
    <path d="M22 2L11 13" />
    <path d="M22 2l-7 20-4-9-9-4 20-7z" />
  </>,
  composer: <>
    <path d="M4 4h12l4 4v12H4z" />
    <path d="M16 4v4h4" />
    <path d="M8 13h8M8 17h5" />
  </>,
  star: <path d="M12 3l2.9 6 6.6.6-5 4.4 1.5 6.5L12 17l-6 3.5L7.5 14l-5-4.4 6.6-.6L12 3z" />,
  clock: <>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5l3 2" />
  </>,
  plus: <>
    <path d="M12 5v14M5 12h14" />
  </>,
};

window.Icon = Icon;
