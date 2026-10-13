import { Button, Card, Hint, Icon, ICONS, Select, Tag } from '@uwusuite/design';
import { useCallback, useEffect, useState } from 'react';
import { t, useLanguage } from '../lib/i18n';
import {
  teamConfirm,
  teamCreate,
  teamDelete,
  teamInvite,
  teamInvites,
  teamJoin,
  teamLeave,
  teamMembers,
  teamRemove,
  teamRename,
  teamRevokeInvite,
  teamSetRole,
} from '../lib/teamCommands';
import {
  asTeamFailure,
  failureCase,
  assignableRoles,
  INVITE_ROLES,
  may,
  mayConfirm,
  mayRemove,
  sortMembers,
  teamServers,
  type TeamInfo,
  type TeamInvite,
  type TeamMember,
  type TeamRemoval,
  type TeamRole,
  type TeamServer,
} from '../lib/teams';
import { ago, copy, SyncRow } from './SyncParts';
import {
  AskDialog,
  Fingerprint,
  FormDialog,
  NoticeDialog,
  remainingText,
  ROLE_NAMES,
  teamFailureText,
  useTeamRun,
} from './TeamParts';
import { Modal } from './Modal';

/**
 * Settings → Teams. Per server that knows teams: this account's fingerprint,
 * its teams, making one and joining one with a code. A team opens into its
 * members and – for owner and admin – invites, roles and the rest.
 *
 * The page exists only with such a server (see `teamsVisible`); the servers
 * come from Settings, which decides that.
 */
export function TeamSettings({
  servers,
  refresh,
}: {
  servers: TeamServer[] | null;
  refresh: () => void;
}) {
  useLanguage();
  const [open, setOpen] = useState<{ server: string; team: string } | null>(null);
  const shown = teamServers(servers);

  const openServer = shown.find((server) => server.space === open?.server);
  const openTeam = openServer?.teams.find((team) => team.id === open?.team);
  // A team that went (deleted, left, access lost) closes by itself.
  if (openServer && openTeam && openTeam.state === 'active') {
    return (
      <TeamDetail
        key={openTeam.id}
        server={openServer}
        team={openTeam}
        stamp={servers}
        refresh={refresh}
        onBack={() => setOpen(null)}
      />
    );
  }

  return (
    <div className="sync-spaces">
      <p className="setting-description">
        {t(
          'Ein Team teilt Hosts mit anderen – samt Anmeldung, Keys und Passwörtern. Der Server sieht davon nur Verschlüsseltes.',
        )}
      </p>
      {shown.map((server) => (
        <ServerSection
          key={server.space}
          server={server}
          titled={shown.length > 1}
          refresh={refresh}
          onOpen={(team) => setOpen({ server: server.space, team })}
        />
      ))}
    </div>
  );
}

function memberCount(n: number): string {
  return n === 1 ? t('1 Mitglied') : t('{n} Mitglieder', { n });
}

/** One server's teams, and the ways into a new one. */
function ServerSection({
  server,
  titled,
  refresh,
  onOpen,
}: {
  server: TeamServer;
  /** With two servers each section says which one it is. */
  titled: boolean;
  refresh: () => void;
  onOpen: (team: string) => void;
}) {
  useLanguage();
  const [creating, setCreating] = useState(false);
  const [joining, setJoining] = useState(false);
  const [joined, setJoined] = useState<TeamInfo | null>(null);
  const [leaving, setLeaving] = useState<TeamInfo | null>(null);
  const [deleting, setDeleting] = useState<TeamInfo | null>(null);
  const name = server.label.trim() || t('Nicht verbunden');

  return (
    <section className="sync-space" aria-label={titled ? name : t('Teams')}>
      {titled && (
        <header className="sync-space-head">
          <Icon icon={ICONS.server} className="text-muted" />
          <b>{name}</b>
        </header>
      )}

      <SyncRow label={t('Dein Fingerabdruck')}>
        {server.fingerprint ? (
          <>
            <Fingerprint value={server.fingerprint} />
            <p className="setting-description">
              {t('Daran erkennt ein Admin beim Beitritt, dass wirklich du es bist.')}
            </p>
          </>
        ) : (
          <p className="setting-description">
            {t('Entsteht, sobald du ein Team anlegst oder einem beitrittst.')}
          </p>
        )}
      </SyncRow>

      <div className="sync-devices">
        <p className="setting-label">{t('Teams')}</p>
        {server.teams.length === 0 ? (
          <p className="setting-description">{t('Noch kein Team auf diesem Server.')}</p>
        ) : (
          <ul className="sync-device-list">
            {server.teams.map((team) => (
              <li key={team.id} data-team-state={team.state}>
                <Icon icon={ICONS.people} className="text-muted" />
                <span className="sync-device-text">
                  <b>
                    <span className="team-name">{team.name}</span>
                    <Tag tone={team.role === 'owner' ? 'pink' : 'neutral'}>
                      {t(ROLE_NAMES[team.role])}
                    </Tag>
                    {team.state === 'pending' && (
                      <Tag tone="warning">{t('Wartet auf Bestätigung')}</Tag>
                    )}
                  </b>
                  <small>
                    {team.state === 'pending'
                      ? t(
                          'Ein Admin des Teams muss dich bestätigen. Er fragt dich nach deinem Fingerabdruck – lies ihn vor.',
                        )
                      : memberCount(team.members)}
                  </small>
                </span>
                <span className="spacer" />
                {team.state === 'active' ? (
                  <Button size="sm" onClick={() => onOpen(team.id)}>
                    {may(team, 'manage') ? t('Verwalten') : t('Ansehen')}
                  </Button>
                ) : (
                  <>
                    {may(team, 'delete') && (
                      <Button size="sm" variant="ghost" onClick={() => setDeleting(team)}>
                        {t('Löschen…')}
                      </Button>
                    )}
                    <Button size="sm" variant="ghost" onClick={() => setLeaving(team)}>
                      {t('Zurückziehen…')}
                    </Button>
                  </>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {server.mayCreate && (
        <SyncRow
          label={t('Neues Team')}
          control={
            <Button size="sm" icon={ICONS.add} onClick={() => setCreating(true)}>
              {t('Team anlegen…')}
            </Button>
          }
        >
          <p className="setting-description">
            {t('Du wirst Inhaber und lädst die anderen mit einem Code ein.')}
          </p>
        </SyncRow>
      )}

      <SyncRow
        label={t('Einem Team beitreten')}
        control={
          <Button size="sm" icon={ICONS.oneTimeCode} onClick={() => setJoining(true)}>
            {t('Code eingeben…')}
          </Button>
        }
      >
        <p className="setting-description">
          {t('Mit dem Einladungscode, den dir ein Admin des Teams gegeben hat.')}
        </p>
      </SyncRow>

      {creating && (
        <CreateDialog
          server={server.space}
          onCancel={() => setCreating(false)}
          onDone={(team) => {
            setCreating(false);
            refresh();
            onOpen(team.id);
          }}
        />
      )}
      {joining && (
        <JoinDialog
          server={server.space}
          onCancel={() => setJoining(false)}
          onDone={(team) => {
            setJoining(false);
            setJoined(team);
            refresh();
          }}
        />
      )}
      {joined && (
        <NoticeDialog title={t('Jetzt bestätigt dich ein Admin')} onClose={() => setJoined(null)}>
          <p className="dialog-lead">
            {t(
              'Du wartest auf die Bestätigung für {team}. Der Admin sieht deinen Fingerabdruck und vergleicht ihn mit dir – am Telefon oder persönlich. Lies ihn vor:',
              { team: joined.name },
            )}
          </p>
          {server.fingerprint ? (
            <Card className="team-code-card p-4">
              <Fingerprint value={server.fingerprint} large />
            </Card>
          ) : (
            <p className="setting-description">
              {t('Dein Fingerabdruck steht gleich oben auf dieser Seite.')}
            </p>
          )}
        </NoticeDialog>
      )}
      {leaving && (
        <AskDialog
          title={t('Beitritt zu {team} zurückziehen?', { team: leaving.name })}
          lead={t('Deine Anfrage wird gelöscht. Zurück kommst du nur mit einer neuen Einladung.')}
          action={t('Zurückziehen')}
          run={() => teamLeave(server.space, leaving.id)}
          onCancel={() => setLeaving(null)}
          onDone={() => {
            setLeaving(null);
            refresh();
          }}
        />
      )}
      {deleting && (
        <DeleteDialog
          server={server.space}
          team={deleting}
          onCancel={() => setDeleting(null)}
          onDone={() => {
            setDeleting(null);
            refresh();
          }}
        />
      )}
    </section>
  );
}

function CreateDialog({
  server,
  onCancel,
  onDone,
}: {
  server: string;
  onCancel: () => void;
  onDone: (team: TeamInfo) => void;
}) {
  const [name, setName] = useState('');
  const [displayName, setDisplayName] = useState('');
  return (
    <FormDialog
      title={t('Team anlegen')}
      action={t('Anlegen')}
      ready={Boolean(name.trim() && displayName.trim())}
      run={() => teamCreate(server, name.trim(), displayName.trim())}
      onCancel={onCancel}
      onDone={(team) => onDone(team as TeamInfo)}
    >
      <label className="field">
        <span>{t('Name des Teams')}</span>
        <input
          value={name}
          maxLength={80}
          onChange={(event) => setName(event.target.value)}
          placeholder={t('z. B. Infrastruktur')}
          autoComplete="off"
        />
      </label>
      <DisplayNameField value={displayName} onChange={setDisplayName} />
    </FormDialog>
  );
}

function DisplayNameField({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="field">
      <span>{t('Dein Name im Team')}</span>
      <input
        value={value}
        maxLength={60}
        onChange={(event) => onChange(event.target.value)}
        autoComplete="off"
      />
      <em className="field-hint">{t('So sehen dich die anderen Mitglieder.')}</em>
    </label>
  );
}

function JoinDialog({
  server,
  onCancel,
  onDone,
}: {
  server: string;
  onCancel: () => void;
  onDone: (team: TeamInfo) => void;
}) {
  const [code, setCode] = useState('');
  const [displayName, setDisplayName] = useState('');
  return (
    <FormDialog
      title={t('Team beitreten')}
      action={t('Beitreten')}
      ready={Boolean(code.trim() && displayName.trim())}
      run={() => teamJoin(server, code.trim(), displayName.trim())}
      onCancel={onCancel}
      onDone={(team) => onDone(team as TeamInfo)}
    >
      <label className="field">
        <span>{t('Einladungscode')}</span>
        <input
          className="team-code-input"
          value={code}
          maxLength={64}
          onChange={(event) => setCode(event.target.value)}
          placeholder="XXXXX-XXXXX-XXXXX"
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
        />
      </label>
      <DisplayNameField value={displayName} onChange={setDisplayName} />
      <p className="field-hint">
        {t('Danach bestätigt dich ein Admin des Teams. Erst dann siehst du dessen Hosts.')}
      </p>
    </FormDialog>
  );
}

function DeleteDialog({
  server,
  team,
  onCancel,
  onDone,
}: {
  server: string;
  team: TeamInfo;
  onCancel: () => void;
  onDone: () => void;
}) {
  return (
    <AskDialog
      title={t('{team} löschen?', { team: team.name })}
      lead={t(
        'Das Team und alle seine Hosts verschwinden für alle Mitglieder. Das lässt sich nicht rückgängig machen.',
      )}
      action={t('Team löschen')}
      run={() => teamDelete(server, team.id)}
      onCancel={onCancel}
      onDone={onDone}
    />
  );
}

// ── One team ────────────────────────────────────────────────────────────────

/** A team's members and, for those who run it, invites, roles and the rest. */
function TeamDetail({
  server,
  team,
  stamp,
  refresh,
  onBack,
}: {
  server: TeamServer;
  team: TeamInfo;
  /** Changes whenever the teams did: the lists load again. */
  stamp: unknown;
  refresh: () => void;
  onBack: () => void;
}) {
  useLanguage();
  const manage = may(team, 'manage');
  const [members, setMembers] = useState<TeamMember[] | null>(null);
  const [invites, setInvites] = useState<TeamInvite[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<TeamMember | null>(null);
  const [removing, setRemoving] = useState<TeamMember | null>(null);
  const [removed, setRemoved] = useState<{
    member: TeamMember;
    hosts: TeamRemoval['hosts'];
  } | null>(null);
  const [inviteRole, setInviteRole] = useState<TeamRole>('editor');
  const [invite, setInvite] = useState<TeamInvite | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const task = useTeamRun();

  const load = useCallback(() => {
    void teamMembers(server.space, team.id)
      .then((found) => {
        setMembers(found);
        setLoadError(null);
      })
      .catch((e) => setLoadError(teamFailureText(asTeamFailure(e))));
    if (manage) {
      void teamInvites(server.space, team.id)
        .then(setInvites)
        .catch(() => undefined);
    }
  }, [server.space, team.id, manage]);

  useEffect(load, [load, stamp]);

  const changed = () => {
    load();
    refresh();
  };

  return (
    <div className="sync-spaces">
      <section className="sync-space" aria-label={team.name}>
        <header className="sync-space-head">
          <Button size="sm" variant="ghost" icon={ICONS.back} onClick={onBack}>
            {t('Teams')}
          </Button>
          <b>{team.name}</b>
          <Tag tone={team.role === 'owner' ? 'pink' : 'neutral'}>{t(ROLE_NAMES[team.role])}</Tag>
        </header>

        {team.role === 'reader' && (
          <p className="setting-description team-note">
            {t('Du kannst die Hosts dieses Teams nutzen, aber nicht ändern.')}
          </p>
        )}
        {team.space === null && (
          <p className="setting-description team-note">
            {t('Die Hosts des Teams kommen mit dem nächsten Sync auf dieses Gerät.')}
          </p>
        )}
        {manage && team.needsRotation && (
          <p className="setting-description team-note">
            {t('Der Schlüssel des Teams wird gerade erneuert.')}
          </p>
        )}

        <div className="sync-devices">
          <p className="setting-label">{t('Mitglieder')}</p>
          {loadError && <p className="setting-description text-warning-ink">{loadError}</p>}
          {!members && !loadError && <p className="setting-description">{t('Wird geladen…')}</p>}
          {members && (
            <ul className="sync-device-list">
              {sortMembers(members).map((member) => {
                const roles = assignableRoles(team.role, member);
                const label = t('Rolle von {name}', { name: member.displayName });
                return (
                  <li key={member.accountId} data-member-state={member.state}>
                    <Icon icon={ICONS.account} className="text-muted" />
                    <span className="sync-device-text">
                      <b>
                        <span className="team-name">{member.displayName}</span>
                        {member.me && <Tag tone="pink">{t('du')}</Tag>}
                        {member.state === 'pending' && <Tag tone="warning">{t('wartet')}</Tag>}
                      </b>
                      <small>
                        {roles.length === 0 && `${t(ROLE_NAMES[member.role])} · `}
                        {member.state === 'pending'
                          ? t('Code eingegeben {when}', { when: ago(member.joinedMs) })
                          : t('Beigetreten {when}', { when: ago(member.joinedMs) })}
                      </small>
                    </span>
                    <span className="spacer" />
                    {roles.length > 0 && (
                      <Select
                        className="w-36 [&>select]:h-9"
                        aria-label={label}
                        title={label}
                        value={member.role}
                        disabled={task.busy}
                        onChange={(event) => {
                          const role = event.target.value as TeamRole;
                          void task.run(
                            () => teamSetRole(server.space, team.id, member.accountId, role),
                            changed,
                          );
                        }}
                      >
                        {roles.map((role) => (
                          <option key={role} value={role}>
                            {t(ROLE_NAMES[role])}
                          </option>
                        ))}
                      </Select>
                    )}
                    {mayConfirm(team.role, member) && (
                      <Button size="sm" variant="primary" onClick={() => setConfirming(member)}>
                        {t('Bestätigen…')}
                      </Button>
                    )}
                    {mayRemove(team.role, member) && (
                      <Button size="sm" variant="ghost" onClick={() => setRemoving(member)}>
                        {t('Entfernen…')}
                      </Button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {task.error && (
            <p className="field-error" role="alert">
              {task.error}
            </p>
          )}
        </div>

        {manage && (
          <div className="sync-devices">
            <p className="setting-label">{t('Einladungen')}</p>
            <p className="setting-description">
              {t('Ein Code gilt für eine Person und läuft nach 7 Tagen ab.')}
            </p>
            {invites && invites.length > 0 && (
              <ul className="sync-device-list">
                {invites.map((open) => (
                  <li key={open.id}>
                    <Icon icon={ICONS.invitation} className="text-muted" />
                    <span className="sync-device-text">
                      <b>{t('Als {role}', { role: t(ROLE_NAMES[open.role]) })}</b>
                      <small>{remainingText(open.expiresMs)}</small>
                    </span>
                    <span className="spacer" />
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={task.busy}
                      onClick={() =>
                        void task.run(
                          () => teamRevokeInvite(server.space, team.id, open.id),
                          changed,
                        )
                      }
                    >
                      {t('Widerrufen')}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            <div className="team-invite-new">
              <Select
                className="w-36 [&>select]:h-9"
                aria-label={t('Rolle der Einladung')}
                value={inviteRole}
                onChange={(event) => setInviteRole(event.target.value as TeamRole)}
              >
                {INVITE_ROLES.map((role) => (
                  <option key={role} value={role}>
                    {t(ROLE_NAMES[role])}
                  </option>
                ))}
              </Select>
              <Button
                size="sm"
                icon={ICONS.add}
                disabled={task.busy}
                onClick={() =>
                  void task.run(
                    () => teamInvite(server.space, team.id, inviteRole),
                    (made) => {
                      setInvite(made);
                      changed();
                    },
                  )
                }
              >
                {t('Einladung erstellen')}
              </Button>
            </div>
          </div>
        )}

        {may(team, 'rename') && (
          <SyncRow
            label={t('Name')}
            control={
              <Button size="sm" icon={ICONS.edit} onClick={() => setRenaming(true)}>
                {t('Umbenennen…')}
              </Button>
            }
          >
            <p className="setting-description">{team.name}</p>
          </SyncRow>
        )}
        <SyncRow
          label={t('Team verlassen')}
          control={
            <Button size="sm" icon={ICONS.signOut} onClick={() => setLeaving(true)}>
              {t('Verlassen…')}
            </Button>
          }
        >
          <p className="setting-description">
            {t('Die Hosts des Teams verschwinden von deinen Geräten.')}
          </p>
        </SyncRow>
        {may(team, 'delete') && (
          <SyncRow
            label={t('Team löschen')}
            control={
              <Button
                size="sm"
                variant="danger"
                icon={ICONS.delete}
                onClick={() => setDeleting(true)}
              >
                {t('Löschen…')}
              </Button>
            }
          >
            <p className="setting-description">
              {t('Löscht das Team mit allen Hosts, für alle Mitglieder.')}
            </p>
          </SyncRow>
        )}
      </section>

      {task.dialog}
      {confirming && (
        <ConfirmMember
          server={server.space}
          team={team}
          // After a reload the key the server names now, not the one from before.
          member={
            members?.find(
              (member) => member.accountId === confirming.accountId && member.state === 'pending',
            ) ?? confirming
          }
          onStale={load}
          onCancel={() => setConfirming(null)}
          onDone={() => {
            setConfirming(null);
            changed();
          }}
        />
      )}
      {removing && (
        <AskDialog
          title={t('{name} entfernen?', { name: removing.displayName })}
          lead={t(
            '{name} verliert den Zugriff auf {team}. Der Schlüssel des Teams wird danach von selbst erneuert.',
            { name: removing.displayName, team: team.name },
          )}
          action={t('Entfernen')}
          run={() => teamRemove(server.space, team.id, removing.accountId)}
          onCancel={() => setRemoving(null)}
          onDone={(result) => {
            setRemoved({ member: removing, hosts: (result as TeamRemoval | null)?.hosts ?? [] });
            setRemoving(null);
            changed();
          }}
        />
      )}
      {removed && (
        <NoticeDialog
          title={removed.hosts.length > 0 ? t('Zugangsdaten ändern') : t('Entfernt')}
          tone={removed.hosts.length > 0 ? 'warning' : 'default'}
          onClose={() => setRemoved(null)}
        >
          {removed.hosts.length > 0 ? (
            <>
              <p className="dialog-lead">
                {t(
                  '{name} ist entfernt, kennt aber weiter, was im Team lag. Ändere Passwort und Key dieser Hosts – auf den Servern selbst:',
                  { name: removed.member.displayName },
                )}
              </p>
              <ul className="team-hosts">
                {removed.hosts.map((host) => (
                  <li key={host.id}>
                    <Icon icon={ICONS.server} size="sm" className="text-muted" />
                    {host.name}
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p className="dialog-lead">
              {t(
                '{name} ist entfernt. Kein Host des Teams hat ein gespeichertes Passwort oder einen Key – es gibt nichts zu ändern.',
                { name: removed.member.displayName },
              )}
            </p>
          )}
        </NoticeDialog>
      )}
      {invite && <InviteMade team={team} invite={invite} onClose={() => setInvite(null)} />}
      {renaming && (
        <RenameDialog
          server={server.space}
          team={team}
          onCancel={() => setRenaming(false)}
          onDone={() => {
            setRenaming(false);
            refresh();
          }}
        />
      )}
      {leaving && (
        <AskDialog
          title={t('{team} verlassen?', { team: team.name })}
          lead={t(
            'Die Hosts des Teams verschwinden von deinen Geräten. Zurück kommst du nur mit einer neuen Einladung.',
          )}
          action={t('Verlassen')}
          run={() => teamLeave(server.space, team.id)}
          onCancel={() => setLeaving(false)}
          onDone={() => {
            setLeaving(false);
            refresh();
            onBack();
          }}
        />
      )}
      {deleting && (
        <DeleteDialog
          server={server.space}
          team={team}
          onCancel={() => setDeleting(false)}
          onDone={() => {
            setDeleting(false);
            refresh();
            onBack();
          }}
        />
      )}
    </div>
  );
}

/**
 * Letting a waiting member in. This is where the team key is handed over, so
 * the dialog shows the key the server names and asks for it to be compared
 * with the person, away from this server, before anything is sent.
 */
function ConfirmMember({
  server,
  team,
  member,
  onStale,
  onCancel,
  onDone,
}: {
  server: string;
  team: TeamInfo;
  member: TeamMember;
  /** The member's key is another one by now: load the members again. */
  onStale: () => void;
  onCancel: () => void;
  onDone: () => void;
}) {
  useLanguage();
  // The key that was compared: a new one on screen has to be compared again.
  const [comparedKey, setComparedKey] = useState<string | null>(null);
  const compared = comparedKey === member.fingerprint;
  const task = useTeamRun();
  const name = member.displayName;
  return (
    <>
      <Modal
        title={t('{name} bestätigen?', { name })}
        tone="warning"
        onCancel={() => !task.busy && onCancel()}
        footer={
          <>
            <Button data-autofocus onClick={onCancel} disabled={task.busy}>
              {t('Abbrechen')}
            </Button>
            <Button
              variant="primary"
              data-secondary
              busy={task.busy}
              disabled={!compared || task.busy}
              onClick={() =>
                void task.run(
                  // Exactly the key that was on screen, not whatever the server says now.
                  () => teamConfirm(server, team.id, member.accountId, member.fingerprint),
                  onDone,
                  (failure) => {
                    if (failureCase(failure) !== 'identity-changed') return;
                    setComparedKey(null);
                    onStale();
                  },
                )
              }
            >
              {task.busy ? t('Einen Moment…') : t('Bestätigen')}
            </Button>
          </>
        }
      >
        <p className="dialog-lead">
          {t('{name} bekommt den Schlüssel von {team} – als {role}.', {
            name,
            team: team.name,
            role: t(ROLE_NAMES[member.role]),
          })}
        </p>
        <Card className="team-code-card p-4">
          <span className="setting-description">{t('Fingerabdruck von {name}', { name })}</span>
          <Fingerprint value={member.fingerprint} large />
        </Card>
        <Hint tone="warning">
          {t(
            'Vergleiche ihn mit {name} auf einem anderen Weg – am Telefon oder persönlich. {name} findet ihn unter Einstellungen → Teams. Weicht er ab, brich ab.',
            { name },
          )}
        </Hint>
        <label className="check">
          <input
            type="checkbox"
            checked={compared}
            onChange={(event) => setComparedKey(event.target.checked ? member.fingerprint : null)}
          />
          <span>
            <b>{t('Mit {name} verglichen – er stimmt überein', { name })}</b>
          </span>
        </label>
        {task.error && (
          <p className="field-error" role="alert">
            {task.error}
          </p>
        )}
      </Modal>
      {task.dialog}
    </>
  );
}

/** The code of a new invite: shown this once. */
function InviteMade({
  team,
  invite,
  onClose,
}: {
  team: TeamInfo;
  invite: TeamInvite;
  onClose: () => void;
}) {
  useLanguage();
  const [copied, setCopied] = useState(false);
  return (
    <Modal
      title={t('Einladung für {team}', { team: team.name })}
      onCancel={onClose}
      footer={
        <Button variant="primary" onClick={onClose}>
          {t('Fertig')}
        </Button>
      }
    >
      <p className="dialog-lead">
        {t(
          'Gib diesen Code einer Person. Sie trägt ihn unter Einstellungen → Teams ein, danach bestätigst du sie hier.',
        )}
      </p>
      <Card className="team-code-card p-4">
        <span className="setting-description">
          {t('Als {role}', { role: t(ROLE_NAMES[invite.role]) })} ·{' '}
          {remainingText(invite.expiresMs)}
        </span>
        <code className="sync-kit-code">{invite.code ?? ''}</code>
      </Card>
      <Button
        className="justify-self-start"
        icon={copied ? ICONS.done : ICONS.copy}
        onClick={() => {
          void copy(invite.code ?? '');
          setCopied(true);
        }}
      >
        {copied ? t('Kopiert') : t('Code kopieren')}
      </Button>
      <p className="field-hint">{t('Der Code ist nur jetzt zu sehen.')}</p>
    </Modal>
  );
}

function RenameDialog({
  server,
  team,
  onCancel,
  onDone,
}: {
  server: string;
  team: TeamInfo;
  onCancel: () => void;
  onDone: () => void;
}) {
  const [name, setName] = useState(team.name);
  return (
    <FormDialog
      title={t('Team umbenennen')}
      action={t('Umbenennen')}
      ready={Boolean(name.trim()) && name.trim() !== team.name}
      run={() => teamRename(server, team.id, name.trim())}
      onCancel={onCancel}
      onDone={onDone}
    >
      <label className="field">
        <span>{t('Name des Teams')}</span>
        <input
          value={name}
          maxLength={80}
          onChange={(event) => setName(event.target.value)}
          autoComplete="off"
        />
      </label>
    </FormDialog>
  );
}
