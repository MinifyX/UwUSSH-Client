/**
 * Dev-only contact sheet for the OS icons: every icon at list and header sizes
 * on a light and a dark ground. Served by Vite at /os-icons.html; the
 * production build only bundles index.html.
 */

import '@fontsource-variable/manrope';
import React from 'react';
import ReactDOM from 'react-dom/client';
import { OS_IDS, OS_LABELS, OsIcon, type OsId } from '../components/OsIcon';
import '../components/nyu/nyu.css';

// `?sizes=64,96` swaps in other sizes for a closer look at the drawings.
const SIZES = new URLSearchParams(location.search)
  .get('sizes')
  ?.split(',')
  .map(Number)
  .filter((n) => n > 0) ?? [18, 20, 28, 40];
const CELL = Math.max(42, ...SIZES.map((size) => size + 2));

const GROUNDS = [
  { name: 'Light', background: '#FFF7FA', ink: '#1C1420', muted: '#716672' },
  { name: 'Dark', background: '#1A1320', ink: '#F8F2F6', muted: '#B3A8B3' },
] as const;

const ENTRIES: (OsId | null)[] = [...OS_IDS, null];

function Sheet({ ground }: { ground: (typeof GROUNDS)[number] }) {
  return (
    <section
      style={{
        background: ground.background,
        color: ground.ink,
        padding: 20,
        borderRadius: 16,
        flex: '1 1 0',
      }}
    >
      <h2 style={{ margin: '0 0 12px', fontSize: 16, fontWeight: 600 }}>{ground.name}</h2>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(2, minmax(0, 1fr))',
          gap: '10px 20px',
        }}
      >
        {ENTRIES.map((os) => (
          <div key={os ?? 'unknown'} style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            {SIZES.map((size) => (
              <span
                key={size}
                style={{ display: 'grid', placeItems: 'center', width: CELL, height: CELL }}
              >
                <OsIcon os={os} size={size} />
              </span>
            ))}
            <span style={{ fontSize: 13 }}>
              {os ? OS_LABELS[os] : <span style={{ color: ground.muted }}>unknown / null</span>}
            </span>
          </div>
        ))}
      </div>
      <h3 style={{ margin: '20px 0 8px', fontSize: 13, fontWeight: 600, color: ground.muted }}>
        In a host row
      </h3>
      {(['proxmox', 'ubuntu', 'raspberry', null] as const).map((os, i) => (
        <div
          key={os ?? 'unknown'}
          style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '5px 0', fontSize: 14 }}
        >
          <OsIcon os={os} size={20} title="" />
          <span style={{ fontWeight: 600 }}>{['prox-1', 'web-02', 'pi-hole', 'router'][i]}</span>
          <span style={{ color: ground.muted, fontSize: 13 }}>root@10.0.0.{i + 2}</span>
        </div>
      ))}
    </section>
  );
}

function Preview() {
  return (
    <main
      style={{
        display: 'flex',
        gap: 16,
        padding: 16,
        fontFamily: "'Manrope Variable', 'Segoe UI', sans-serif",
      }}
    >
      {GROUNDS.map((ground) => (
        <Sheet key={ground.name} ground={ground} />
      ))}
    </main>
  );
}

const root = document.getElementById('root');
if (!root) throw new Error('#root missing from os-icons.html');

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <Preview />
  </React.StrictMode>,
);
