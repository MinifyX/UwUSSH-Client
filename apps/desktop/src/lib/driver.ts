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
  cursorLineText,
  Highlighter,
  passwordPrompt,
  waitsForAnswer,
  type PasswordPrompt,
  type Rule,
} from './highlight';
import {
  ACK_CHUNK,
  ackSession,
  closeSession,
  openTerminalLink,
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

export type TerminalOptions = {
  fontSize: number;
  cursorStyle: 'block' | 'bar' | 'underline';
  cursorBlink: boolean;
  scrollback: number;
};

/** How long output has to pause before a partial ack chunk is sent anyway. */
const ACK_IDLE_MS = 20;

/** How long output has to rest before the cursor's line is checked for a prompt. */
const PROMPT_IDLE_MS = 120;

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
  private readonly host: HTMLElement;
  private readonly resizeObserver: ResizeObserver;
  private sessionId: SessionId | null = null;
  /** Bumped on every attach/detach, so stragglers from an old session are ignored. */
  private generation = 0;
  private pendingAck = 0;
  private ackTimer: number | undefined;
  private readonly highlighter: Highlighter;
  private promptTimer: number | undefined;
  /** The password prompt under the cursor, as last seen. */
  private prompted: PasswordPrompt | null = null;
  private readonly promptListeners = new Set<(prompt: PasswordPrompt | null) => void>();

  constructor(host: HTMLElement, options: TerminalOptions) {
    this.host = host;
    this.term = new Terminal({
      fontFamily: "'JetBrains Mono', ui-monospace, Consolas, monospace",
      lineHeight: 1.25,
      theme: NYU_THEME,
      allowProposedApi: true,
      // Links a program prints (OSC 8) open only on Ctrl+click, only for http
      // and https, and through Rust, which checks again. xterm.js' default
      // would ask with confirm() and then navigate to whatever the server sent.
      linkHandler: {
        allowNonHttpProtocols: false,
        activate: (event, uri) => {
          if (event.ctrlKey && /^https?:\/\//i.test(uri)) {
            void openTerminalLink(uri).catch(() => undefined);
          }
        },
      },
      ...options,
    });

    this.term.loadAddon(this.fit);
    this.term.open(host);
    this.loadWebgl();
    this.refit();

    this.term.onData((data) => {
      if (this.sessionId) void writeSession(this.sessionId, data);
      // Typing answers a prompt, or moves past it.
      this.setPrompt(null);
    });

    this.highlighter = new Highlighter(this.term);
    this.term.onWriteParsed(() => {
      window.clearTimeout(this.promptTimer);
      this.promptTimer = window.setTimeout(() => this.setPrompt(this.prompt()), PROMPT_IDLE_MS);
    });

    // A tab in the background has no size (display: none); refit() skips
    // it, and the observer fires again once the tab shows.
    this.resizeObserver = new ResizeObserver(() => this.refit());
    this.resizeObserver.observe(host);
  }

  /** Font size, cursor and scrollback from the settings, applied to a running terminal. */
  applyOptions(options: TerminalOptions) {
    const { term } = this;
    if (term.options.fontSize !== options.fontSize) term.options.fontSize = options.fontSize;
    if (term.options.cursorStyle !== options.cursorStyle)
      term.options.cursorStyle = options.cursorStyle;
    if (term.options.cursorBlink !== options.cursorBlink)
      term.options.cursorBlink = options.cursorBlink;
    if (term.options.scrollback !== options.scrollback)
      term.options.scrollback = options.scrollback;
    this.refit();
  }

  setHighlightRules(rules: Rule[]) {
    this.highlighter.setRules(rules);
  }

  /** A clean screen for a new session, highlights and prompt state included. */
  resetScreen() {
    this.term.reset();
    this.highlighter.reset();
    this.setPrompt(null);
  }

  /** Called whenever a password prompt appears under the cursor or goes away. */
  onPasswordPrompt(listener: (prompt: PasswordPrompt | null) => void): () => void {
    this.promptListeners.add(listener);
    return () => this.promptListeners.delete(listener);
  }

  /** The sudo or doas prompt under the cursor right now, or `null`. */
  prompt(): PasswordPrompt | null {
    if (this.term.buffer.active.type !== 'normal') return null;
    return passwordPrompt(cursorLineText(this.term));
  }

  /** Whether the cursor sits after a question, like `Password:`. */
  waitsForAnswer(): boolean {
    return waitsForAnswer(cursorLineText(this.term));
  }

  private setPrompt(prompt: PasswordPrompt | null) {
    if (prompt?.user === this.prompted?.user && !prompt === !this.prompted) return;
    this.prompted = prompt;
    for (const listener of this.promptListeners) listener(prompt);
  }

  private refit() {
    // A hidden tab (display: none) has no layout box, but the fit addon reads
    // the computed style, which then is the declared `100%` — 100 px. That
    // fitted a background session to about ten columns and told the server so,
    // which wrapped everything it printed meanwhile word by word.
    if (this.host.getClientRects().length === 0) return;
    const proposed = this.fit.proposeDimensions();
    if (!proposed || !Number.isFinite(proposed.cols) || proposed.cols < 2) return;
    const { cols, rows } = this.term;
    this.fit.fit();
    if (this.sessionId && (cols !== this.term.cols || rows !== this.term.rows)) {
      void resizeSession(this.sessionId, this.term.cols, this.term.rows);
    }
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
    this.setPrompt(null);
    this.pendingAck = 0;
    window.clearTimeout(this.ackTimer);
    this.ackTimer = undefined;
    if (id) await closeSession(id).catch(() => undefined);
  }

  dispose() {
    void this.detach();
    window.clearTimeout(this.promptTimer);
    this.promptListeners.clear();
    this.highlighter.dispose();
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
