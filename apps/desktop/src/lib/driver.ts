/**
 * One xterm.js instance and whichever session is currently attached to it.
 *
 * Kept outside React on purpose: this is the hot path, and nothing in it
 * should wait for a render. React mounts it and reads its counters; it never
 * sits between a frame and the terminal.
 */

import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { Terminal } from '@xterm/xterm';
import {
  ACK_CHUNK,
  ackSession,
  closeSession,
  resizeSession,
  writeSession,
  type Spawner,
  type SessionId,
} from './session';

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

export type Renderer = 'webgl' | 'canvas';

/** How long output has to pause before a partial ack chunk is sent anyway. */
const ACK_IDLE_MS = 20;

export class TerminalDriver {
  readonly term: Terminal;
  renderer: Renderer = 'canvas';

  /** Bytes that arrived over IPC. */
  received = 0;
  /** Bytes xterm.js has finished parsing — the number that is actually on screen. */
  written = 0;
  /**
   * Bytes xterm.js refused. Once more than ~50 MB is pending it throws on
   * `write()` ("write data discarded, use flow control to avoid losing data"),
   * so without flow control a fast flood silently loses output.
   */
  discarded = 0;
  lastWrittenAt = performance.now();
  lastDataAt = performance.now();

  private readonly fit = new FitAddon();
  private readonly resizeObserver: ResizeObserver;
  private sessionId: SessionId | null = null;
  /** Bumped on every attach/detach, so stragglers from an old session are ignored. */
  private generation = 0;
  private pendingAck = 0;
  private ackTimer: number | undefined;

  constructor(host: HTMLElement) {
    this.term = new Terminal({
      fontFamily: "'JetBrains Mono', ui-monospace, Consolas, monospace",
      fontSize: 13,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 10_000,
      theme: NYU_THEME,
      allowProposedApi: true,
    });

    this.term.loadAddon(this.fit);
    this.term.open(host);
    this.loadWebgl();
    this.fit.fit();

    this.term.onData((data) => {
      if (this.sessionId) void writeSession(this.sessionId, data);
    });

    this.resizeObserver = new ResizeObserver(() => {
      this.fit.fit();
      if (this.sessionId) void resizeSession(this.sessionId, this.term.cols, this.term.rows);
    });
    this.resizeObserver.observe(host);
  }

  get session(): SessionId | null {
    return this.sessionId;
  }

  resetCounters() {
    this.received = 0;
    this.written = 0;
    this.discarded = 0;
    this.lastWrittenAt = performance.now();
    this.lastDataAt = performance.now();
  }

  /**
   * Close whatever is attached, then attach the session `spawn` creates.
   * `onEnd` fires once if that session's stream ends on its own — the remote
   * shell exited, the connection dropped — but not when it is detached.
   */
  async attach(spawn: Spawner, onEnd?: () => void): Promise<SessionId> {
    await this.detach();
    const generation = this.generation;
    const id = await spawn(
      (bytes) => this.onData(generation, bytes),
      () => {
        if (generation === this.generation) onEnd?.();
      },
    );

    if (generation !== this.generation) {
      void closeSession(id).catch(() => undefined);
      throw new Error('superseded by a newer session');
    }
    this.sessionId = id;
    // Output may have arrived before the id did; its acks were held back.
    this.flushAck();
    return id;
  }

  async detach() {
    const id = this.sessionId;
    this.generation += 1;
    this.sessionId = null;
    this.pendingAck = 0;
    window.clearTimeout(this.ackTimer);
    this.ackTimer = undefined;
    if (id) await closeSession(id).catch(() => undefined);
  }

  dispose() {
    void this.detach();
    this.resizeObserver.disconnect();
    this.term.dispose();
  }

  private onData(generation: number, bytes: Uint8Array) {
    if (generation !== this.generation) return;
    const size = bytes.length;
    this.received += size;
    this.lastDataAt = performance.now();

    // The callback fires once xterm.js has parsed the chunk. Acknowledging
    // there, not on arrival, is what makes backpressure reach the webview:
    // arrival only proves the IPC hop, parsing proves the terminal kept up.
    try {
      this.term.write(bytes, () => {
        if (generation !== this.generation) return;
        this.written += size;
        this.lastWrittenAt = performance.now();
        this.pendingAck += size;
        if (this.pendingAck >= ACK_CHUNK) this.flushAck();
        else this.scheduleAckFlush();
      });
    } catch {
      this.discarded += size;
    }
  }

  private flushAck() {
    window.clearTimeout(this.ackTimer);
    this.ackTimer = undefined;
    if (!this.sessionId || this.pendingAck === 0) return;
    const bytes = this.pendingAck;
    this.pendingAck = 0;
    void ackSession(this.sessionId, bytes).catch(() => undefined);
  }

  /** Acknowledge a partial chunk once output goes quiet, so nothing stays outstanding at rest. */
  private scheduleAckFlush() {
    if (this.ackTimer !== undefined) return;
    this.ackTimer = window.setTimeout(() => this.flushAck(), ACK_IDLE_MS);
  }

  private loadWebgl() {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl.dispose();
        this.renderer = 'canvas';
      });
      this.term.loadAddon(webgl);
      this.renderer = 'webgl';
    } catch {
      this.renderer = 'canvas';
    }
  }
}
