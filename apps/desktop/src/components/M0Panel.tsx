import type { Renderer } from '../lib/driver';
import { N_, t, useLanguage } from '../lib/i18n';
import type { Outcome, Progress, ScenarioResult } from '../lib/m0';

const OUTCOME_LABEL: Record<Outcome, string> = {
  complete: N_('vollständig'),
  'lost-data': N_('Daten verloren'),
  stalled: N_('hängt'),
  timeout: N_('Timeout'),
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
  useLanguage();
  const [processedBefore, processedAfter] = progress
    ? t('{written} von {sent} MiB verarbeitet', { sent: progress.sentMib.toFixed(0) }).split(
        '{written}',
      )
    : [];
  return (
    <div className="m0-status">
      {renderer && (
        <span
          className="stat"
          data-alarm={renderer === 'canvas'}
          title={t('Ohne WebGL misst man den Renderer statt der IPC-Grenze')}
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
          {progress.index + 1}/{progress.total} · {t(progress.scenario.label)} · {processedBefore}
          <b>{progress.writtenMib.toFixed(0)}</b>
          {processedAfter} · {progress.seconds.toFixed(1)} s
        </span>
      ) : reportPath ? (
        <span className="stat" title={reportPath}>
          {t('Bericht: {path}', { path: reportPath })}
        </span>
      ) : (
        <span className="stat">{t('lokale Shell · noch kein SSH')}</span>
      )}
    </div>
  );
}

export function M0Results({ results }: { results: ScenarioResult[] }) {
  useLanguage();
  if (results.length === 0) return null;

  return (
    <div className="m0-table-wrap">
      <table className="m0-table">
        <thead>
          <tr>
            <th>{t('Szenario')}</th>
            <th title={t('Bis xterm.js das letzte Byte verarbeitet hat')}>{t('Zeit')}</th>
            <th>MiB/s</th>
            <th
              title={t('Gesendet, aber noch nicht verarbeitet — so weit hing der Schirm hinterher')}
            >
              {t('Rückstand max')}
            </th>
            <th title={t('Längste Lücke zwischen zwei Animation-Frames')}>{t('UI-Frame max')}</th>
            <th title={t('Frames über 50 ms, also sichtbares Ruckeln')}>{t('Ruckler')}</th>
            <th>{t('ø Frame')}</th>
            <th title={t('Wie oft die Engine auf die WebView gewartet hat')}>{t('Pausen')}</th>
          </tr>
        </thead>
        <tbody>
          {results.map((r) => (
            <tr key={r.id}>
              <td>
                {t(r.label)}
                {r.outcome !== 'complete' && (
                  <span className="flag"> {t(OUTCOME_LABEL[r.outcome])}</span>
                )}
              </td>
              <td>{r.seconds.toFixed(1)} s</td>
              <td data-alarm={r.mibPerSec === null}>
                {r.mibPerSec === null ? (
                  t('{size} MiB verloren', { size: r.discardedMib.toFixed(0) })
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
