import { useMemo, useState } from 'react';
import type { HostRecord } from '../lib/session';
import { Nyu } from './nyu/Nyu';

type Props = {
  hosts: HostRecord[];
  /** `'shell'` for the local shell, a host id, or nothing: what the active tab shows. */
  activeId: string | null;
  /** Hosts with at least one live session in some tab. */
  onlineIds: ReadonlySet<string>;
  /** Hosts a tab is connecting to right now. */
  connectingIds: ReadonlySet<string>;
  onConnect: (host: HostRecord) => void;
  onLocalShell: () => void;
  onAdd: () => void;
  onEdit: (host: HostRecord) => void;
  onImport: () => void;
};

function groupHosts(hosts: HostRecord[]): [string | null, HostRecord[]][] {
  const groups = new Map<string | null, HostRecord[]>();
  for (const host of hosts) {
    const key = host.groupPath ?? null;
    groups.set(key, [...(groups.get(key) ?? []), host]);
  }
  // Ungrouped hosts first, then groups alphabetically — the store already
  // sorts hosts within a group.
  return [...groups.entries()].sort(([a], [b]) =>
    a === null ? -1 : b === null ? 1 : a.localeCompare(b, 'de'),
  );
}

function matches(host: HostRecord, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [host.name, host.address, host.username, host.groupPath ?? ''].some((field) =>
    field.toLowerCase().includes(q),
  );
}

export function HostList({
  hosts,
  activeId,
  onlineIds,
  connectingIds,
  onConnect,
  onLocalShell,
  onAdd,
  onEdit,
  onImport,
}: Props) {
  const [query, setQuery] = useState('');
  const visible = useMemo(() => hosts.filter((h) => matches(h, query)), [hosts, query]);
  const groups = useMemo(() => groupHosts(visible), [visible]);

  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <h2>Hosts</h2>
        <span className="spacer" />
        <button
          className="icon-button"
          onClick={onImport}
          title="Hosts importieren"
          aria-label="Hosts importieren"
        >
          ↓
        </button>
        <button
          className="icon-button"
          onClick={onAdd}
          title="Host hinzufügen"
          aria-label="Host hinzufügen"
        >
          +
        </button>
      </div>

      {hosts.length > 0 && (
        <input
          className="search"
          type="search"
          placeholder="Suchen"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label="Hosts durchsuchen"
        />
      )}

      <ul className="host-list">
        <li>
          <button className="host" aria-current={activeId === 'shell'} onClick={onLocalShell}>
            <span className="host-glyph" aria-hidden>
              ›_
            </span>
            <span className="host-name">Lokale Shell</span>
          </button>
        </li>
      </ul>

      {groups.map(([group, members]) => (
        <section key={group ?? ''} className="host-group">
          {group && <h3>{group}</h3>}
          <ul className="host-list">
            {members.map((host) => (
              <li key={host.id} className="host-row">
                <button
                  className="host"
                  aria-current={activeId === host.id}
                  aria-busy={connectingIds.has(host.id)}
                  onClick={() => onConnect(host)}
                  title={`${host.username}@${host.address}:${host.port} · öffnet einen neuen Tab`}
                >
                  <i className="dot" data-state={onlineIds.has(host.id) ? 'online' : 'idle'} />
                  <span className="host-name">{host.name}</span>
                  <span className="meta">
                    {connectingIds.has(host.id)
                      ? 'verbindet…'
                      : host.port === 22
                        ? host.address
                        : `${host.address}:${host.port}`}
                  </span>
                </button>
                <button
                  className="icon-button host-edit"
                  onClick={() => onEdit(host)}
                  title={`${host.name} bearbeiten`}
                  aria-label={`${host.name} bearbeiten`}
                >
                  ✎
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}

      {hosts.length === 0 && (
        <div className="sidebar-empty">
          <Nyu size={72} />
          <p>Noch keine Hosts.</p>
          <button className="primary" onClick={onAdd}>
            Host hinzufügen
          </button>
          <button className="quiet" onClick={onImport}>
            Aus Termius, PuTTY, KiTTY oder ssh_config importieren
          </button>
        </div>
      )}

      {hosts.length > 0 && visible.length === 0 && (
        <p className="sidebar-note">Kein Host passt zu „{query}".</p>
      )}
    </aside>
  );
}
