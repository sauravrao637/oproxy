import React from 'react';
import { createRoot } from 'react-dom/client';
import './styles.css';

await import('./tweaks-panel.jsx');
await import('./redaction.jsx');
await import('./icons.jsx');
await import('./sessions-table.jsx');
await import('./detail-panel.jsx');
await import('./surfaces.jsx');
await import('./surfaces-extra.jsx');
await import('./compose.jsx');
await import('./app.jsx');

createRoot(document.getElementById('root')).render(<window.App />);
