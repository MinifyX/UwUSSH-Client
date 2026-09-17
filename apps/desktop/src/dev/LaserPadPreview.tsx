/**
 * Dev-only preview of Nyu's laser pad: /laser-pad.html on the Vite dev server.
 *
 * Progress is fake: it counts the bytes the pad hands out. Query parameters set
 * the starting state for screenshots, e.g. ?theme=light&motion=reduced&target=6000.
 */

import '@fontsource-variable/manrope';
import React, { useCallback, useEffect, useRef, useState } from 'react';
import ReactDOM from 'react-dom/client';
import { NyuLaserPad } from '../components/keygen/NyuLaserPad';
import '../components/nyu/nyu.css';
import '../styles/app.css';
import '../styles/tokens.css';

type Theme = 'light' | 'dark';
const TARGETS = [600, 6000] as const;

const params = new URLSearchParams(window.location.search);

function Segmented<T extends string | number>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <div className="field">
      <span>{label}</span>
      <div className="segmented" role="radiogroup" aria-label={label}>
        {options.map((option) => (
          <button
            key={String(option.value)}
            role="radio"
            aria-checked={option.value === value}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function LaserPadPreview() {
  const [theme, setTheme] = useState<Theme>(params.get('theme') === 'light' ? 'light' : 'dark');
  const [reduced, setReduced] = useState(params.get('motion') === 'reduced');
  const [target, setTarget] = useState<number>(Number(params.get('target')) || TARGETS[0]);
  const [paused, setPaused] = useState(params.get('paused') === '1');
  const [bytes, setBytes] = useState(0);
  const collected = useRef(0);
  const flush = useRef(0);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    if (reduced) document.documentElement.dataset.motion = 'reduced';
    else delete document.documentElement.dataset.motion;
  }, [reduced]);

  // Samples arrive by the hundred per second; React hears about them ~8 times a second.
  const onEntropy = useCallback((sample: Uint8Array) => {
    collected.current += sample.length;
    if (flush.current) return;
    flush.current = window.setTimeout(() => {
      flush.current = 0;
      setBytes(collected.current);
    }, 120);
  }, []);

  const reset = () => {
    collected.current = 0;
    setBytes(0);
  };

  const progress = Math.min(1, bytes / target);
  const active = !paused && progress < 1;

  return (
    <main
      style={{
        minHeight: '100%',
        display: 'grid',
        placeContent: 'center',
        justifyItems: 'center',
        gap: 20,
        padding: 24,
        overflow: 'auto',
      }}
    >
      <section
        className="modal"
        aria-labelledby="modal-title"
        style={{ width: 'min(520px, calc(100vw - 48px))' }}
      >
        <h2 id="modal-title" className="modal-title">
          Neuer SSH-Schlüssel
        </h2>
        <div className="modal-body">
          <p className="dialog-lead">
            Für einen guten Schlüssel braucht Nyu ein bisschen Zufall. Lass sie den Laserpunkt
            jagen!
          </p>
          <NyuLaserPad
            progress={progress}
            active={active}
            onEntropy={onEntropy}
            hint={
              progress >= 1
                ? 'Genug Zufall gesammelt ✧'
                : paused
                  ? 'Einen Moment …'
                  : 'Bewege die Maus über das Feld'
            }
          />
        </div>
        <div className="modal-footer">
          <span style={{ color: 'var(--uwu-muted)', fontSize: 13 }} data-testid="bytes">
            {bytes} / {target} Bytes · {Math.round(progress * 100)} %
          </span>
          <span className="spacer" />
          <button className="quiet" onClick={reset}>
            Nochmal
          </button>
          <button className="primary" disabled={progress < 1}>
            Schlüssel erzeugen
          </button>
        </div>
      </section>

      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 16, justifyContent: 'center' }}>
        <Segmented
          label="Theme"
          value={theme}
          options={[
            { value: 'dark', label: 'Dunkel' },
            { value: 'light', label: 'Hell' },
          ]}
          onChange={setTheme}
        />
        <Segmented
          label="Animationen"
          value={reduced ? 'reduced' : 'full'}
          options={[
            { value: 'full', label: 'An' },
            { value: 'reduced', label: 'Aus' },
          ]}
          onChange={(value) => setReduced(value === 'reduced')}
        />
        <Segmented
          label="Ziel"
          value={target}
          options={TARGETS.map((value) => ({ value, label: `${value} Bytes` }))}
          onChange={setTarget}
        />
        <Segmented
          label="Pad"
          value={paused ? 'paused' : 'live'}
          options={[
            { value: 'live', label: 'Sammelt' },
            { value: 'paused', label: 'Pausiert' },
          ]}
          onChange={(value) => setPaused(value === 'paused')}
        />
      </div>
    </main>
  );
}

const root = document.getElementById('root');
if (!root) throw new Error('#root missing from laser-pad.html');

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <LaserPadPreview />
  </React.StrictMode>,
);
