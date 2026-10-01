/**
 * Which system the app runs on, for the few words and keys that differ.
 *
 * The webview's user agent says it plainly on all three: WebView2 on Windows,
 * WKWebView on macOS, WebKitGTK on Linux.
 */

export type Platform = 'windows' | 'macos' | 'linux';

export function platform(): Platform {
  return platformOf(`${navigator.userAgent} ${navigator.platform ?? ''}`);
}

/**
 * macOS first: "Darwin" contains "win", and WKWebView's agent says
 * "Macintosh; Intel Mac OS X" on Apple silicon too.
 */
export function platformOf(agent: string): Platform {
  const lower = agent.toLowerCase();
  if (lower.includes('mac')) return 'macos';
  if (lower.includes('win')) return 'windows';
  return 'linux';
}

/** "Windows", "macOS" or "Linux", as the system calls itself. */
export function systemName(): string {
  switch (platform()) {
    case 'windows':
      return 'Windows';
    case 'macos':
      return 'macOS';
    default:
      return 'Linux';
  }
}
