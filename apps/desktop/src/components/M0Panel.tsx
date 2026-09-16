import type { Renderer } from '../lib/driver';
import type { Outcome, Progress, ScenarioResult } from '../lib/m0';

const OUTCOME_LABEL: Record<Outcome, string> = {
  complete: 'vollständig',
  'lost-data': 'Daten verloren',
  stalled: 'hängt',
  timeout: 'Timeout',
};

/**
 * The M0 readout: a live status line while a scenario runs, and the results
 * table once scenarios finish.
 *
 * The columns answer "does the UI survive a flood?" rather than showing off a
 * big MiB/s number: how far the screen fell behind and how long the window
 * froze matter more than raw speed.
 */

type StatusProps = {
  renderer: Renderer | null;
  progress: Progress | null;
  reportPath: string | null;
  error: string | null;
};

export function M0Status({ renderer, progress, reportPath, error }: StatusProps) {
  return (
    <div className="m0-status">
      {renderer && (
        <span
          className="stat"
          data-alarm={renderer === 'canvas'}
          title="Ohne WebGL misst man den Renderer statt der IPC-Grenze"
        >
          {renderer}
        </span>
      )}
      {error ? (
        <span className="stat" data-alarm>
          {error}
        </span>
      ) : progress ? (
        <span className="stat">
          {progress.index + 1}/{progress.total} · {progress.scenario.label} ·{' '}
          <b>{progress.writtenMib.toFixed(0)}</b> von {progress.sentMib.toFixed(0)} MiB verarbeitet
          · {progress.seconds.toFixed(1)} s
        </span>
      ) : reportPath ? (
        <span className="stat" title={reportPath}>
          Bericht: {reportPath}
        </span>
      ) : (
        <span className="stat">lokale Shell · noch kein SSH</span>
      )}
    </div>
  );
}

export function M0Results({ results }: { results: ScenarioResult[] }) {
  if (results.length === 0) return null;

  return (
    <div className="m0-table-wrap">
      <table className="m0-table">
        <thead>
          <tr>
            <th>Szenario</th>
            <th title="Bis xterm.js das letzte Byte verarbeitet hat">Zeit</th>
            <th>MiB/s</th>
            <th title="Gesendet, aber noch nicht verarbeitet — so weit hing der Schirm hinterher">
              Rückstand max
            </th>
            <th title="Längste Lücke zwischen zwei Animation-Frames">UI-Frame max</th>
            <th title="Frames über 50 ms, also sichtbares Ruckeln">Ruckler</th>
            <th>ø Frame</th>
            <th title="Wie oft die Engine auf die WebView gewartet hat">Pausen</th>
          </tr>
        </thead>
        <tbody>
          {results.map((r) => (
            <tr key={r.id}>
              <td>
                {r.label}
                {r.outcome !== 'complete' && (
                  <span className="flag"> {OUTCOME_LABEL[r.outcome]}</span>
                )}
              </td>
              <td>{r.seconds.toFixed(1)} s</td>
              <td data-alarm={r.mibPerSec === null}>
                {r.mibPerSec === null ? (
                  `${r.discardedMib.toFixed(0)} MiB verloren`
                ) : (
                  <b>{r.mibPerSec.toFixed(1)}</b>
                )}
              </td>
              <td data-alarm={r.peakLagMib > 8}>{r.peakLagMib.toFixed(1)} MiB</td>
              <td data-alarm={r.ui.maxGapMs > 250}>{r.ui.maxGapMs} ms</td>
              <td data-alarm={r.ui.over50ms > 10}>{r.ui.over50ms}</td>
              <td>{r.meanFrameKib.toFixed(0)} KiB</td>
              <td>{r.flowPauses}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
