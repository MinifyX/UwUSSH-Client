import { useEffect, useMemo, useState, type FormEvent } from 'react';
import { N_, t, useLanguage } from '../lib/i18n';
import type { HostRecord } from '../lib/session';
import {
  deleteTunnel,
  describeRoute,
  describeTunnelError,
  listTunnels,
  saveTunnel,
  stopTunnel,
  useTunnelStatuses,
  type TunnelDraft,
  type TunnelKind,
  type TunnelRecord,
  type TunnelStatus,
} from '../lib/tunnels';
import { Icon } from './Icon';
import { Modal } from './Modal';

type Props = {
  hosts: HostRecord[];
  /** Show this host's tunnels, and add new ones to it. */
  host: HostRecord | null;
  /**
   * Start a tunnel on a connection of its own, asking whatever logging in
   * needs. Resolves with what went wrong, or `null`.
   */
  onStart: (tunnel: TunnelRecord) => Promise<string | null>;
  onClose: () => void;
};

const ERRORS: Record<string, Record<string, string>> = {
  bindAddress: {
    whitespace: N_('Die Adresse darf keine Leerzeichen enthalten'),
    'too-long': N_('Die Adresse ist zu lang'),
  },
  bindPort: { 'out-of-range': N_('Port zwischen 1 und 65535') },
  targetHost: {
    required: N_('Ziel fehlt'),
    whitespace: N_('Das Ziel darf keine Leerzeichen enthalten'),
    'too-long': N_('Das Ziel ist zu lang'),
  },
  targetPort: { 'out-of-range': N_('Port zwischen 1 und 65535') },
  name: {
    'too-long': N_('Der Name ist zu lang'),
    control: N_('Ungültiger Name'),
  },
};

type Form = {
  id: string | null;
  hostId: string;
  name: string;
  kind: TunnelKind;
  bindAddress: string;
  bindPort: string;
  targetHost: string;
  targetPort: string;
  autostart: boolean;
};

function formOf(tunnel: TunnelRecord): Form {
  return {
    id: tunnel.id,
    hostId: tunnel.hostId,
    name: tunnel.name,
    kind: tunnel.kind === 'remote' ? 'remote' : 'local',
    bindAddress: tunnel.bindAddress,
    bindPort: String(tunnel.bindPort),
    targetHost: tunnel.targetHost,
    targetPort: String(tunnel.targetPort),
    autostart: tunnel.autostart,
  };
}

function port(text: string): number {
  const value = Number(text.trim());
  return Number.isInteger(value) && value > 0 && value <= 65535 ? value : 0;
}

function statusText(tunnel: TunnelRecord, status: TunnelStatus | undefined): string {
  if (!status) return t('Aus');
  switch (status.state) {
    case 'starting':
      return t('Startet…');
    case 'running': {
      const parts = [t('Läuft auf Port {port}', { port: status.boundPort })];
      if (status.connections > 0) {
        parts.push(
          status.connections === 1
            ? t('1 Verbindung')
            : t('{count} Verbindungen', { count: status.connections }),
        );
      }
      if (status.session) parts.push(t('mit dem Terminal'));
      return parts.join(' · ');
    }
    case 'failed':
      return describeTunnelError(status.error);
    case 'stopped':
      return t('Aus');
  }
  return tunnel.name;
}

/**
 * The tunnels: of one host or of all, each with its state and a button to
 * start or stop it, and a form to add or change one.
 */
export function TunnelsDialog({ hosts, host, onStart, onClose }: Props) {
  useLanguage();
  const statuses = useTunnelStatuses();
  const [tunnels, setTunnels] = useState<TunnelRecord[]>([]);
  const [only, setOnly] = useState<string | null>(host?.id ?? null);
  const [form, setForm] = useState<Form | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [failures, setFailures] = useState<Record<string, string>>({});
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = () =>
    void listTunnels()
      .then((loaded) => {
        setTunnels(loaded);
        setLoadError(null);
      })
      .catch((e) => setLoadError(String(e)));

  useEffect(load, []);

  const hostById = useMemo(() => new Map(hosts.map((h) => [h.id, h])), [hosts]);
  const shown = useMemo(() => {
    const visible = tunnels.filter((tunnel) => !only || tunnel.hostId === only);
    const byHost = new Map<string, TunnelRecord[]>();
    for (const tunnel of visible) {
      byHost.set(tunnel.hostId, [...(byHost.get(tunnel.hostId) ?? []), tunnel]);
    }
    return [...byHost.entries()]
      .map(([hostId, list]) => ({ host: hostById.get(hostId), tunnels: list }))
      .filter((group): group is { host: HostRecord; tunnels: TunnelRecord[] } =>
        Boolean(group.host),
      )
      .sort((a, b) => a.host.name.localeCompare(b.host.name, 'de'));
  }, [tunnels, only, hostById]);

  const mark = (id: string, on: boolean) =>
    setBusy((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const start = async (tunnel: TunnelRecord) => {
    mark(tunnel.id, true);
    setFailures(({ [tunnel.id]: _, ...rest }) => rest);
    try {
      const failure = await onStart(tunnel);
      if (failure) setFailures((current) => ({ ...current, [tunnel.id]: failure }));
    } finally {
      mark(tunnel.id, false);
    }
  };

  const stop = async (tunnel: TunnelRecord) => {
    mark(tunnel.id, true);
    try {
      await stopTunnel(tunnel.id);
      setFailures(({ [tunnel.id]: _, ...rest }) => rest);
    } catch (e) {
      setFailures((current) => ({ ...current, [tunnel.id]: String(e) }));
    } finally {
      mark(tunnel.id, false);
    }
  };

  const remove = async (tunnel: TunnelRecord) => {
    if (confirmDelete !== tunnel.id) {
      setConfirmDelete(tunnel.id);
      return;
    }
    setConfirmDelete(null);
    try {
      await deleteTunnel(tunnel.id);
      load();
    } catch (e) {
      setFailures((current) => ({ ...current, [tunnel.id]: String(e) }));
    }
  };

  const newForm = () => {
    const hostId = only ?? host?.id ?? hosts[0]?.id;
    if (!hostId) return;
    setErrors({});
    setForm({
      id: null,
      hostId,
      name: '',
      kind: 'local',
      bindAddress: '127.0.0.1',
      bindPort: '',
      targetHost: 'localhost',
      targetPort: '',
      autostart: false,
    });
  };

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    if (!form) return;
    const draft: TunnelDraft = {
      id: form.id,
      hostId: form.hostId,
      name: form.name,
      kind: form.kind,
      bindAddress: form.bindAddress.trim() || null,
      bindPort: port(form.bindPort),
      targetHost: form.targetHost,
      targetPort: port(form.targetPort),
      autostart: form.autostart,
    };
    try {
      await saveTunnel(draft);
      setForm(null);
      load();
    } catch (error) {
      const failure = error as {
        kind?: string;
        field?: string;
        problem?: string;
        message?: string;
      };
      if (failure?.kind === 'invalid' && failure.field) {
        const text = ERRORS[failure.field]?.[failure.problem ?? ''] ?? N_('Bitte prüfen');
        setErrors({ [failure.field]: text });
      } else {
        setErrors({ form: failure?.message ?? String(error) });
      }
    }
  };

  const edit =
    <K extends keyof Form>(key: K) =>
    (value: Form[K]) => {
      setForm((current) => (current ? { ...current, [key]: value } : current));
      setErrors(({ [key]: _, form: __, ...rest }) => rest);
    };

  // ── The form ──────────────────────────────────────────────────────────────

  if (form) {
    const local = form.kind === 'local';
    const formHost = hostById.get(form.hostId);
    return (
      <Modal
        key="form"
        title={form.id ? t('Tunnel bearbeiten') : t('Neuer Tunnel')}
        onCancel={() => setForm(null)}
        footer={
          <>
            <span className="spacer" />
            <button data-secondary onClick={() => setForm(null)}>
              {t('Abbrechen')}
            </button>
            <button className="primary" onClick={() => void submit()}>
              {t('Speichern')}
            </button>
          </>
        }
      >
        <form className="form" onSubmit={submit}>
          {form.id ? (
            <p className="dialog-lead">{t('Tunnel über {name}', { name: formHost?.name ?? '' })}</p>
          ) : (
            <label className="field">
              <span>{t('Über den Host')}</span>
              <select value={form.hostId} onChange={(e) => edit('hostId')(e.target.value)}>
                {hosts.map((h) => (
                  <option key={h.id} value={h.id}>
                    {h.name}
                  </option>
                ))}
              </select>
            </label>
          )}

          <fieldset className="field">
            <span>{t('Richtung')}</span>
            <div className="segmented" role="radiogroup" aria-label={t('Richtung')}>
              <button
                type="button"
                role="radio"
                aria-checked={local}
                onClick={() => edit('kind')('local')}
              >
                {t('Lokal (-L)')}
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={!local}
                onClick={() => edit('kind')('remote')}
              >
                {t('Remote (-R)')}
              </button>
            </div>
            <em className="field-hint">
              {local
                ? t(
                    'Ein Port auf diesem Computer führt zu einem Ziel, das der Server erreicht – etwa eine Datenbank, die nur dort lauscht.',
                  )
                : t(
                    'Ein Port auf dem Server führt zu einem Ziel, das dieser Computer erreicht – etwa ein Dienst, den du gerade lokal entwickelst.',
                  )}
            </em>
          </fieldset>

          <div className="form-row">
            <label className="field grow">
              <span>{local ? t('Lauscht auf diesem Computer') : t('Lauscht auf dem Server')}</span>
              <input
                value={form.bindAddress}
                onChange={(e) => edit('bindAddress')(e.target.value)}
                placeholder="127.0.0.1"
                aria-invalid={Boolean(errors.bindAddress)}
                autoComplete="off"
                spellCheck={false}
              />
              {errors.bindAddress ? (
                <em className="field-error">{t(errors.bindAddress)}</em>
              ) : (
                <em className="field-hint">
                  {t('127.0.0.1 nur von hier aus, 0.0.0.0 für alle im Netz.')}
                </em>
              )}
            </label>
            <label className="field port">
              <span>{t('Port')}</span>
              <input
                value={form.bindPort}
                onChange={(e) => edit('bindPort')(e.target.value)}
                inputMode="numeric"
                placeholder="8080"
                aria-invalid={Boolean(errors.bindPort)}
              />
              {errors.bindPort && <em className="field-error">{t(errors.bindPort)}</em>}
            </label>
          </div>

          <div className="form-row">
            <label className="field grow">
              <span>
                {local
                  ? t('Ziel, vom Server aus gesehen')
                  : t('Ziel, von diesem Computer aus gesehen')}
              </span>
              <input
                value={form.targetHost}
                onChange={(e) => edit('targetHost')(e.target.value)}
                placeholder="localhost"
                aria-invalid={Boolean(errors.targetHost)}
                autoComplete="off"
                spellCheck={false}
              />
              {errors.targetHost && <em className="field-error">{t(errors.targetHost)}</em>}
            </label>
            <label className="field port">
              <span>{t('Port')}</span>
              <input
                value={form.targetPort}
                onChange={(e) => edit('targetPort')(e.target.value)}
                inputMode="numeric"
                placeholder="80"
                aria-invalid={Boolean(errors.targetPort)}
              />
              {errors.targetPort && <em className="field-error">{t(errors.targetPort)}</em>}
            </label>
          </div>

          <label className="field">
            <span>{t('Name')}</span>
            <input
              value={form.name}
              onChange={(e) => edit('name')(e.target.value)}
              placeholder={t('optional, z. B. Datenbank')}
              aria-invalid={Boolean(errors.name)}
              autoComplete="off"
            />
            {errors.name && <em className="field-error">{t(errors.name)}</em>}
          </label>

          <label className="check">
            <input
              type="checkbox"
              checked={form.autostart}
              onChange={(e) => edit('autostart')(e.target.checked)}
            />
            <span>
              <b>{t('Mit dem Terminal starten')}</b>
              <small>
                {t(
                  'Startet, sobald ein Terminal zu diesem Host verbunden ist, auf dessen Verbindung – und endet mit ihm.',
                )}
              </small>
            </span>
          </label>

          {errors.form && <p className="form-error">{errors.form}</p>}
          <button type="submit" hidden />
        </form>
      </Modal>
    );
  }

  // ── The list ──────────────────────────────────────────────────────────────

  const onlyHost = only ? hostById.get(only) : undefined;
  return (
    <Modal
      key="list"
      title={onlyHost ? t('Tunnel von {name}', { name: onlyHost.name }) : t('Tunnel')}
      size="wide"
      onCancel={onClose}
      footer={
        <>
          {only && (
            <button data-secondary onClick={() => setOnly(null)}>
              {t('Alle Hosts')}
            </button>
          )}
          <span className="spacer" />
          <button data-secondary onClick={onClose}>
            {t('Schließen')}
          </button>
          <button className="primary" onClick={newForm} disabled={hosts.length === 0}>
            <Icon name="plus" size={15} />
            {t('Tunnel hinzufügen')}
          </button>
        </>
      }
    >
      <div className="tunnels-body">
        <p className="dialog-lead">
          {t(
            'Ein Tunnel leitet einen Port durch die SSH-Verbindung: lokal (-L) holt etwas vom Server auf diesen Computer, remote (-R) bringt etwas von hier auf den Server. Er läuft auch ohne offenes Terminal.',
          )}
        </p>
        {loadError && <p className="form-error">{loadError}</p>}

        {shown.length === 0 ? (
          <p className="sidebar-note">
            {onlyHost
              ? t('{name} hat noch keine Tunnel.', { name: onlyHost.name })
              : t('Noch keine Tunnel.')}
          </p>
        ) : (
          shown.map(({ host: owner, tunnels: list }) => (
            <section key={owner.id} className="tunnel-group">
              {!only && <h3>{owner.name}</h3>}
              <ul className="tunnel-list">
                {list.map((tunnel) => {
                  const status = statuses.get(tunnel.id);
                  const known = tunnel.kind === 'local' || tunnel.kind === 'remote';
                  const running = status?.state === 'running' || status?.state === 'starting';
                  const working = busy.has(tunnel.id);
                  const failure = failures[tunnel.id];
                  const state = working
                    ? 'connecting'
                    : (status?.state ?? (failure ? 'failed' : 'idle'));
                  return (
                    <li key={tunnel.id} className="tunnel-row" data-state={state}>
                      <i className="dot" data-state={state} aria-hidden />
                      <div className="tunnel-text">
                        <span className="tunnel-name">
                          <b>{tunnel.name}</b>
                          <span className="tunnel-tag">
                            {tunnel.kind === 'local'
                              ? t('Lokal')
                              : tunnel.kind === 'remote'
                                ? t('Remote')
                                : tunnel.kind}
                          </span>
                          {tunnel.autostart && (
                            <span className="tunnel-tag">{t('mit Terminal')}</span>
                          )}
                        </span>
                        <span className="meta">{describeRoute(tunnel)}</span>
                        <span className="tunnel-status" role="status">
                          {!known
                            ? t('Braucht ein neueres UwUSSH.')
                            : working && !running
                              ? t('Verbindet…')
                              : status
                                ? statusText(tunnel, status)
                                : (failure ?? t('Aus'))}
                        </span>
                      </div>
                      <span className="tunnel-actions">
                        {known &&
                          (running ? (
                            <button
                              className="quiet"
                              onClick={() => void stop(tunnel)}
                              disabled={working}
                            >
                              <Icon name="stop" size={14} />
                              {t('Stoppen')}
                            </button>
                          ) : (
                            <button
                              className="quiet"
                              onClick={() => void start(tunnel)}
                              disabled={working}
                            >
                              <Icon name="play" size={14} />
                              {t('Starten')}
                            </button>
                          ))}
                        {known && (
                          <button
                            className="icon-button"
                            onClick={() => {
                              setErrors({});
                              setForm(formOf(tunnel));
                            }}
                            title={t('Bearbeiten')}
                            aria-label={t('{name} bearbeiten', { name: tunnel.name })}
                          >
                            <Icon name="pencil" size={15} />
                          </button>
                        )}
                        <button
                          className={confirmDelete === tunnel.id ? 'danger' : 'icon-button'}
                          onClick={() => void remove(tunnel)}
                          onBlur={() => setConfirmDelete(null)}
                          title={t('Löschen')}
                          aria-label={t('{name} löschen', { name: tunnel.name })}
                        >
                          {confirmDelete === tunnel.id ? (
                            t('Wirklich löschen')
                          ) : (
                            <Icon name="trash" size={15} />
                          )}
                        </button>
                      </span>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))
        )}
      </div>
    </Modal>
  );
}
