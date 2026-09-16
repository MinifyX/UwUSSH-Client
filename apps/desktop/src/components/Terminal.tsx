import '@xterm/xterm/css/xterm.css';
import { useEffect, useRef } from 'react';
import { TerminalDriver } from '../lib/driver';

type TerminalViewProps = {
  onReady: (driver: TerminalDriver) => void;
};

/**
 * Mounts a {@link TerminalDriver} and hands it up. Deliberately nothing more:
 * the driver owns the terminal, React only owns the box it sits in.
 *
 * Under React 18 StrictMode the effect runs mount → cleanup → mount in dev, so
 * `onReady` can fire twice with two different drivers. The first one is
 * disposed immediately, which closes any session it opened; callers keep the
 * latest driver and treat a "superseded" error from the first as expected.
 */
export function TerminalView({ onReady }: TerminalViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!hostRef.current) return;
    const driver = new TerminalDriver(hostRef.current);
    onReady(driver);
    return () => driver.dispose();
    // Mount-only on purpose: the terminal must not be torn down because a
    // parent re-rendered.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div className="terminal-host" ref={hostRef} />;
}
