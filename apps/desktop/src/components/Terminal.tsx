import '@xterm/xterm/css/xterm.css';
import { useEffect, useRef } from 'react';
import { TerminalDriver } from '../lib/driver';
import { compileRules } from '../lib/highlight';
import {
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  getSettings,
  subscribeSettings,
  updateSettings,
} from '../lib/settings';

type TerminalViewProps = {
  onReady: (driver: TerminalDriver) => void;
  /** The driver is gone: the tab closed, or StrictMode's first mount ended. */
  onDispose: (driver: TerminalDriver) => void;
};

/** How much wheel movement is one font size step: a mouse notch, or that much touchpad. */
const ZOOM_STEP = 100;

/**
 * Mounts a {@link TerminalDriver} and hands it up. Deliberately nothing more:
 * the driver owns the terminal, React only owns the box it sits in.
 *
 * Under React 18 StrictMode the effect runs mount → cleanup → mount in dev, so
 * `onReady` can fire twice with two different drivers. The first one is
 * disposed immediately, which closes any session it opened; callers keep the
 * latest driver and treat a "superseded" error from the first as expected.
 */
export function TerminalView({ onReady, onDispose }: TerminalViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const options = () => {
      const { fontSize, cursorStyle, cursorBlink, scrollback } = getSettings();
      return { fontSize, cursorStyle, cursorBlink, scrollback };
    };
    const driver = new TerminalDriver(host, options());
    let highlight = getSettings().highlight;
    driver.setHighlightRules(compileRules(highlight));
    const unsubscribe = subscribeSettings(() => {
      driver.applyOptions(options());
      const next = getSettings().highlight;
      if (next !== highlight) {
        highlight = next;
        driver.setHighlightRules(compileRules(next));
      }
    });

    // Ctrl + mouse wheel makes the text bigger or smaller, in every terminal.
    // Captured before xterm.js, which would scroll instead.
    let pending = 0;
    const zoom = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      event.stopPropagation();
      let steps: number;
      if (
        event.deltaMode !== WheelEvent.DOM_DELTA_PIXEL ||
        Math.abs(event.deltaY) >= ZOOM_STEP / 2
      ) {
        // A mouse notch is one step, however display scaling sizes its delta.
        pending = 0;
        steps = Math.sign(event.deltaY);
      } else {
        // A touchpad sends small movements; they add up.
        pending += event.deltaY;
        steps = Math.trunc(pending / ZOOM_STEP);
        pending -= steps * ZOOM_STEP;
      }
      if (steps === 0) return;
      const size = getSettings().fontSize - steps;
      updateSettings({ fontSize: Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, size)) });
    };
    host.addEventListener('wheel', zoom, { capture: true, passive: false });

    onReady(driver);
    return () => {
      host.removeEventListener('wheel', zoom, { capture: true });
      unsubscribe();
      driver.dispose();
      onDispose(driver);
    };
    // Mount-only on purpose: the terminal must not be torn down because a
    // parent re-rendered.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The padding lives on the outer box. xterm.js' fit addon measures the
  // element the terminal opens in with its padding included, so a padded one
  // gets a row and a few columns more than fit, and the last line is cut off.
  return (
    <div className="terminal-host">
      <div className="terminal-screen" ref={hostRef} />
    </div>
  );
}
