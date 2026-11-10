// Kept free of imports so `node --test` can load it.

/** Longest OSC 52 payload taken (base64): a few hundred KB of text is plenty for a copy. */
const OSC52_MAX = 1_000_000;

/**
 * The text of an OSC 52 clipboard write (`<targets>;<base64>`), or `null`
 * for anything else. A query (`?`) is a server asking to read the
 * clipboard and is never answered; an empty payload would clear it.
 */
export function osc52Text(data: string): string | null {
  const split = data.indexOf(';');
  if (split < 0) return null;
  const payload = data.slice(split + 1);
  if (!payload || payload === '?' || payload.length > OSC52_MAX) return null;
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(payload)) return null;
  try {
    const bytes = Uint8Array.from(atob(payload), (c) => c.charCodeAt(0));
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

export type WrappedLink = {
  url: string;
  /** Index into the lines passed in. */
  firstLine: number;
  lastLine: number;
  /** 0-based column of the first character, on `firstLine`. */
  start: number;
  /** 0-based column after the last character, on `lastLine`. */
  end: number;
};

/** A full line ends this close to the right edge (programs like Ink keep a margin). */
const EDGE = 4;

/**
 * Web addresses a program broke over several lines with real line breaks
 * (Ink, many CLIs), so xterm.js sees each piece as its own line: a piece
 * reaching the right edge continues with the next line's first word.
 * shortcut: guesses from the layout, so an address that happens to end at the
 * edge picks up the next line's first word; ask before opening if that bites.
 */
export function wrappedLinks(lines: readonly string[], cols: number): WrappedLink[] {
  const found: WrappedLink[] = [];
  const reachesEdge = (line: string) => line.trimEnd().length >= cols - EDGE;
  for (let i = 0; i < lines.length - 1; i++) {
    const line = lines[i]!.trimEnd();
    if (!reachesEdge(line)) continue;
    const match = /https?:\/\/\S+$/.exec(line);
    if (!match) continue;
    let url = match[0];
    let last = i;
    let end = line.length;
    while (last + 1 < lines.length && reachesEdge(lines[last]!)) {
      const next = lines[last + 1]!;
      const piece = /^(\s*)(\S+)/.exec(next);
      if (!piece || /^https?:\/\//i.test(piece[2]!)) break;
      url += piece[2];
      last += 1;
      end = piece[1]!.length + piece[2]!.length;
      // The piece is not the whole line: the address ended within it.
      if (end < next.trimEnd().length) break;
    }
    if (last === i) continue;
    found.push({ url, firstLine: i, lastLine: last, start: match.index, end });
    i = last;
  }
  return found;
}
