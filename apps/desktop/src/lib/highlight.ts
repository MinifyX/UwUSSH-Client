/**
 * Keyword highlighting, like Termius: words such as "error" or "active" and
 * things like IP addresses get a colour in the terminal, whatever the program
 * printed them in.
 *
 * The bytes on their way to the terminal are never touched — rewriting escape
 * sequences mid-stream breaks programs, and the flow control counts exactly
 * what was sent. Instead, once xterm.js has parsed new output, the lines that
 * changed are read back from its buffer and matches get xterm.js decorations,
 * which recolour cells without changing them. Full-screen programs (the
 * alternate buffer: vim, htop, less) are left alone; colouring their screens
 * would fight them.
 *
 * Work is bounded: a pass runs at most every 80 ms and looks at no more than
 * the last 400 lines, so a flood of output costs a little colour at worst,
 * never throughput.
 */

import type { IBufferLine, IDecoration, IMarker, Terminal } from '@xterm/xterm';
import type { HighlightColor, HighlightSettings } from './settings';

/** The Nyu terminal palette's colours, so highlights look like the rest. */
export const HIGHLIGHT_HEX: Record<HighlightColor, string> = {
  red: '#ff6b8b',
  yellow: '#e8c07a',
  green: '#5cc7ac',
  blue: '#8fb8f0',
  magenta: '#ff7fac',
  cyan: '#7fd6d0',
};

const ERRORS = [
  'error',
  'errors',
  'err',
  'fail',
  'failed',
  'failing',
  'failure',
  'fatal',
  'critical',
  'crit',
  'panic',
  'denied',
  'refused',
  'invalid',
  'exception',
  'traceback',
  'unreachable',
  'segfault',
  'killed',
  'emerg',
  'aborted',
  'not found',
  'no such file or directory',
  'permission denied',
  'timed out',
];
const WARNINGS = ['warn', 'warning', 'warnings', 'deprecated', 'caution', 'retrying', 'degraded'];
const SUCCESS = [
  'ok',
  'success',
  'successful',
  'successfully',
  'succeeded',
  'done',
  'complete',
  'completed',
  'passed',
  'active',
  'running',
  'enabled',
  'started',
  'healthy',
  'listening',
];

export type Rule = { regex: RegExp; color: string };

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function words(list: string[], color: HighlightColor): Rule {
  return {
    regex: new RegExp(`\\b(?:${list.map(escape).join('|')})\\b`, 'gi'),
    color: HIGHLIGHT_HEX[color],
  };
}

/** The rules the settings ask for. Custom rules win over built-in ones. */
export function compileRules(settings: HighlightSettings): Rule[] {
  if (!settings.enabled) return [];
  const rules: Rule[] = [];
  if (settings.errors) rules.push(words(ERRORS, 'red'));
  if (settings.warnings) rules.push(words(WARNINGS, 'yellow'));
  if (settings.success) rules.push(words(SUCCESS, 'green'));
  if (settings.network) {
    rules.push({
      regex:
        /\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}(?:\/\d{1,2}|:\d{1,5})?\b/g,
      color: HIGHLIGHT_HEX.blue,
    });
    rules.push({
      regex: /\b(?:[0-9a-f]{1,4}:){3,7}[0-9a-f]{1,4}\b/gi,
      color: HIGHLIGHT_HEX.blue,
    });
    rules.push({ regex: /\bhttps?:\/\/[^\s'"<>`]+/gi, color: HIGHLIGHT_HEX.cyan });
  }
  for (const rule of settings.custom) {
    const regex = customRegex(rule.pattern, rule.regex, rule.caseSensitive);
    if (regex) rules.push({ regex, color: HIGHLIGHT_HEX[rule.color] });
  }
  return rules;
}

/**
 * A repetition inside a repeated group, like `(a+)+` or `(\w+\s?)*`. Such a
 * pattern can take exponential time on one line of server output and freeze
 * the window, so it is refused, as are backreferences.
 */
const RISKY_PATTERN =
  /\((?:[^()\\]|\\.)*(?:[+*]|\{\d+,?\d*\})(?:[^()\\]|\\.)*\)(?:[+*]|\{\d+,?\d*\})|\\[1-9]/;

/** A user's pattern as a regular expression, or `null` when it doesn't compile or is risky. */
export function customRegex(pattern: string, regex: boolean, caseSensitive: boolean) {
  if (!pattern) return null;
  if (regex && RISKY_PATTERN.test(pattern)) return null;
  try {
    const source = regex ? pattern : `\\b${escape(pattern)}\\b`;
    const compiled = new RegExp(source, caseSensitive ? 'g' : 'gi');
    // A pattern that matches the empty string would loop forever.
    if (compiled.test('')) return null;
    compiled.lastIndex = 0;
    return compiled;
  } catch {
    return null;
  }
}

/** Where each character of a line's text sits, in cells — wide characters take two. */
function textWithColumns(line: IBufferLine, cols: number): { text: string; columns: number[] } {
  let text = '';
  const columns: number[] = [];
  for (let x = 0; x < cols; x += 1) {
    const cell = line.getCell(x);
    if (!cell) break;
    const chars = cell.getChars();
    if (!chars) {
      if (cell.getWidth() === 0) continue;
      text += ' ';
      columns.push(x);
      continue;
    }
    for (let i = 0; i < chars.length; i += 1) columns.push(x);
    text += chars;
  }
  return { text: text.replace(/\s+$/, ''), columns };
}

const PASS_MS = 80;
const MAX_LINES = 400;
const MAX_MATCHES_PER_LINE = 40;

export class Highlighter {
  private rules: Rule[] = [];
  /** Decorated lines. Markers move with their line, so rows are read from them. */
  private readonly entries = new Set<{ marker: IMarker; decorations: IDecoration[] }>();
  /** The text each looked-at row had, so a row a program rewrote is redone. */
  private texts = new Map<number, string>();
  /** Rows before this one are final and done. */
  private scannedUpTo = 0;
  /**
   * A marker on the row the last pass ended at. When the scrollback is full,
   * xterm.js drops lines at the top and every row number shifts; how far this
   * marker moved says by how much.
   */
  private sentinel: { marker: IMarker; row: number } | null = null;
  private timer: number | undefined;
  private readonly subscriptions: { dispose(): void }[] = [];

  constructor(private readonly term: Terminal) {
    this.subscriptions.push(
      term.onWriteParsed(() => this.schedule()),
      term.buffer.onBufferChange(() => this.schedule()),
    );
  }

  setRules(rules: Rule[]) {
    this.rules = rules;
    this.clear();
    // Colour what is on screen, and what comes, with the new rules.
    this.scannedUpTo = Math.max(0, this.term.buffer.active.viewportY);
    this.schedule();
  }

  /** The terminal was reset: every old row means something else now. */
  reset() {
    this.clear();
    this.scannedUpTo = 0;
  }

  dispose() {
    window.clearTimeout(this.timer);
    this.clear();
    for (const subscription of this.subscriptions) subscription.dispose();
  }

  private clear() {
    for (const { marker } of [...this.entries]) marker.dispose();
    this.entries.clear();
    this.texts.clear();
    this.sentinel?.marker.dispose();
    this.sentinel = null;
  }

  private schedule() {
    if (this.timer !== undefined) return;
    this.timer = window.setTimeout(() => {
      this.timer = undefined;
      this.scan();
    }, PASS_MS);
  }

  /** Follow rows that shifted up because the scrollback dropped lines. */
  private followTrim(cursorRow: number) {
    const sentinel = this.sentinel;
    if (!sentinel) return;
    this.sentinel = null;
    const shift = sentinel.marker.isDisposed ? -1 : sentinel.row - sentinel.marker.line;
    sentinel.marker.dispose();
    if (shift === 0) return;
    if (shift < 0) {
      // Too much changed to follow: look at the screen afresh.
      this.texts.clear();
      this.scannedUpTo = Math.max(0, cursorRow - this.term.rows);
      return;
    }
    const moved = new Map<number, string>();
    for (const [row, text] of this.texts) if (row - shift >= 0) moved.set(row - shift, text);
    this.texts = moved;
    this.scannedUpTo = Math.max(0, this.scannedUpTo - shift);
  }

  private scan() {
    const buffer = this.term.buffer.active;
    if (buffer.type === 'alternate' || this.rules.length === 0) return;
    const cursorRow = buffer.baseY + buffer.cursorY;
    this.followTrim(cursorRow);
    if (this.scannedUpTo > cursorRow + 1)
      this.scannedUpTo = Math.max(0, cursorRow - this.term.rows);

    const byRow = new Map<number, { marker: IMarker; decorations: IDecoration[] }>();
    for (const entry of this.entries) byRow.set(entry.marker.line, entry);

    const rows = new Set<number>();
    const start = Math.max(this.scannedUpTo, cursorRow - MAX_LINES);
    for (let row = start; row <= cursorRow; row += 1) rows.add(row);
    // The visible screen too: `clear` and progress bars rewrite rows that
    // were already done, and their old colours must not stay on new text.
    const bottom = Math.min(buffer.length - 1, buffer.baseY + this.term.rows - 1);
    for (let row = buffer.baseY; row <= bottom; row += 1) rows.add(row);
    for (const row of rows) this.decorate(row, cursorRow, byRow.get(row));

    // The cursor's row may still grow; it is looked at again next time.
    this.scannedUpTo = cursorRow;
    const marker = this.term.registerMarker(0);
    this.sentinel = marker ? { marker, row: cursorRow } : null;
    if (this.texts.size > MAX_LINES * 4) {
      for (const row of this.texts.keys()) {
        if (row < buffer.baseY - MAX_LINES) this.texts.delete(row);
      }
    }
  }

  private decorate(
    row: number,
    cursorRow: number,
    previous: { marker: IMarker; decorations: IDecoration[] } | undefined,
  ) {
    const line = this.term.buffer.active.getLine(row);
    if (!line) return;
    const { text, columns } = textWithColumns(line, this.term.cols);
    if (this.texts.get(row) === text) return;
    this.texts.set(row, text);
    if (previous) {
      previous.marker.dispose();
      this.entries.delete(previous);
    }
    if (!text.trim()) return;

    const ranges: { start: number; end: number; color: string }[] = [];
    for (const rule of this.rules) {
      rule.regex.lastIndex = 0;
      let match: RegExpExecArray | null;
      let found = 0;
      while ((match = rule.regex.exec(text)) && found < MAX_MATCHES_PER_LINE) {
        if (match[0].length === 0) {
          rule.regex.lastIndex += 1;
          continue;
        }
        found += 1;
        const range = { start: match.index, end: match.index + match[0].length, color: rule.color };
        // Later rules (custom ones) replace what they overlap.
        for (let i = ranges.length - 1; i >= 0; i -= 1) {
          const other = ranges[i]!;
          if (other.start < range.end && range.start < other.end) ranges.splice(i, 1);
        }
        ranges.push(range);
      }
    }
    if (ranges.length === 0) return;

    const marker = this.term.registerMarker(row - cursorRow);
    if (!marker) return;
    const entry = { marker, decorations: [] as IDecoration[] };
    for (const range of ranges) {
      const x = columns[range.start];
      const lastColumn = columns[range.end - 1];
      if (x === undefined || lastColumn === undefined) continue;
      const decoration = this.term.registerDecoration({
        marker,
        x,
        width: lastColumn - x + 1,
        foregroundColor: range.color,
        layer: 'top',
      });
      if (decoration) entry.decorations.push(decoration);
    }
    this.entries.add(entry);
    marker.onDispose(() => this.entries.delete(entry));
  }
}

/**
 * Prompts that ask for the login's own password on the same machine: sudo
 * (English and German) and doas, each naming the user it asks for. Nothing
 * else — not `Password:` from su, docker or ftp, not `user@host's password`
 * from another ssh, not git's `Password for 'https://…'` — since there the
 * password would go somewhere else.
 */
const PASSWORD_PROMPTS = [
  /^\[sudo\] (?:password|passwort) (?:for|für) ([^\s:]{1,64}):\s*$/i,
  /^doas \(([^\s@)]{1,64})@[^\s)]{1,128}\) password:\s*$/i,
];

/**
 * The user a password prompt on this line asks for, or `null` when the line
 * is no such prompt. Matched against the line the cursor is on.
 */
export function passwordPromptUser(line: string): string | null {
  const trimmed = line.replace(/\s+$/, ' ').trimStart();
  if (trimmed.length > 200) return null;
  for (const prompt of PASSWORD_PROMPTS) {
    const match = prompt.exec(trimmed);
    if (match) return match[1]!;
  }
  return null;
}

/** The text on the cursor's line, up to the cursor. */
export function cursorLineText(term: Terminal): string {
  const buffer = term.buffer.active;
  const line = buffer.getLine(buffer.baseY + buffer.cursorY);
  return line ? line.translateToString(true, 0, buffer.cursorX) : '';
}
