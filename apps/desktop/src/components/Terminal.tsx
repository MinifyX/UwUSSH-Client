import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { Terminal as XTerm } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { useEffect, useRef } from 'react';
import { resizeSession, spawnLocalSession, writeSession, type SessionId } from '../lib/session';

/**
 * "Nyu" — the default terminal theme: the 16 ANSI colours translated into the
 * UwU palette. Classic schemes ship alongside it untouched; a terminal that
 * fights the colours of the program running in it is a broken terminal.
 */
const NYU_THEME = {
  background: '#141016',
  foreground: '#f0e7ee',
  cursor: '#ff7fac',
  cursorAccent: '#141016',
  selectionBackground: '#4d2338',
  black: '#2c2430',
  red: '#ff6b8b',
  green: '#5cc7ac',
  yellow: '#e8c07a',
  blue: '#8fb8f0',
  magenta: '#ff7fac',
  cyan: '#7fd6d0',
  white: '#e4dbe2',
  brightBlack: '#6d6474',
  brightRed: '#ff8fa8',
  brightGreen: '#7fdcc4',
  brightYellow: '#f5d79b',
  brightBlue: '#adcbf5',
  brightMagenta: '#ffa3c4',
  brightCyan: '#a3e5e0',
  brightWhite: '#faf5f9',
};

type TerminalViewProps = {
  onSession: (id: SessionId) => void;
  /** Which renderer won. Relevant to M0: the DOM fallback is far slower. */
  onRenderer: (renderer: 'webgl' | 'canvas') => void;
  onError: (message: string) => void;
};

export function TerminalView({ onSession, onRenderer, onError }: TerminalViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  // React 18 StrictMode mounts effects twice in dev. Without this guard that
  // would silently spawn two shells and halve the measured throughput.
  const startedRef = useRef(false);

  useEffect(() => {
    if (startedRef.current || !hostRef.current) return;
    startedRef.current = true;

    const term = new XTerm({
      fontFamily: "'JetBrains Mono', ui-monospace, Consolas, monospace",
      fontSize: 13,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 10_000,
      theme: NYU_THEME,
      allowProposedApi: true,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);

    try {
      const webgl = new WebglAddon();
      // A lost context must not leave a dead terminal behind.
      webgl.onContextLoss(() => {
        webgl.dispose();
        onRenderer('canvas');
      });
      term.loadAddon(webgl);
      onRenderer('webgl');
    } catch {
      onRenderer('canvas');
    }

    fit.fit();

    let sessionId: SessionId | null = null;
    let disposed = false;

    spawnLocalSession(term.cols, term.rows, (bytes) => term.write(bytes))
      .then((id) => {
        if (disposed) return;
        sessionId = id;
        onSession(id);
      })
      .catch((err: unknown) => onError(String(err)));

    const dataSub = term.onData((data) => {
      if (sessionId) void writeSession(sessionId, data);
    });

    const resizeObserver = new ResizeObserver(() => {
      fit.fit();
      if (sessionId) void resizeSession(sessionId, term.cols, term.rows);
    });
    resizeObserver.observe(hostRef.current);

    return () => {
      disposed = true;
      resizeObserver.disconnect();
      dataSub.dispose();
      term.dispose();
    };
    // Mount-only on purpose: the terminal owns its own lifecycle and must not
    // be torn down because a parent re-rendered.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div className="terminal-host" ref={hostRef} />;
}
