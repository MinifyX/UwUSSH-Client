import { Button, Hint } from '@uwusuite/design';
import { useCallback, useState, type ReactNode } from 'react';
import { N_, t, useLanguage } from '../lib/i18n';
import type { SpaceRef } from '../lib/sync';
import { useSpaces } from '../lib/teamCommands';
import {
  asTeamFailure,
  type AttentionNotice,
  failureCase,
  fingerprintRows,
  inviteRemaining,
  teamsOnServer,
  type TeamFailure,
  type TeamRole,
} from '../lib/teams';
import { Modal } from './Modal';
import { VaultDialog } from './VaultDialog';

// What the Teams page and its dialogs share.

/** The roles as the pages call them. */
export const ROLE_NAMES: Record<TeamRole, string> = {
  owner: N_('Inhaber'),
  admin: N_('Admin'),
  editor: N_('Bearbeiter'),
  reader: N_('Nur lesen'),
};

/** A failure in words the person can act on. */
export function teamFailureText(failure: TeamFailure): string {
  switch (failureCase(failure)) {
    case 'identity-changed':
      return t('Der Schlüssel dieses Mitglieds hat sich geändert. Bitte neu vergleichen.');
    case 'rate-limited':
      return t('Zu viele Versuche. Bitte später erneut.');
    case 'vault-locked':
      return t('Der Tresor ist gesperrt.');
    case 'offline':
      return t('Der Server ist gerade nicht erreichbar. Versuch es gleich noch einmal.');
    case 'not-allowed':
      return t('Dafür reicht deine Rolle in diesem Team nicht.');
    case 'limit':
      return t(
        'Der Server lässt nicht mehr zu: Die Grenze für Teams oder Einladungen ist erreicht.',
      );
    case 'invalid-code':
      return t('Der Code stimmt nicht oder ist abgelaufen.');
    case 'last-owner':
      return t(
        'Ein Team braucht einen Inhaber. Mach zuerst jemand anderen zum Inhaber oder lösche das Team.',
      );
    case 'unsupported':
      return t('Dieser Server kann keine Teams.');
    case 'error':
      return failure.message
        ? t('Das hat nicht geklappt: {reason}', { reason: failure.message })
        : t('Das hat nicht geklappt.');
  }
}

/** What stands in the way of a team, in one line. */
export function attentionText(notice: AttentionNotice): string {
  const name = notice.member ?? t('Ein Mitglied');
  switch (notice.kind) {
    case 'unavailable':
      return t(
        'Der Server bietet dieses Team gerade nicht an. Was auf diesem Gerät liegt, bleibt erhalten.',
      );
    case 'key-refused':
      return t(
        'Der Server bietet einen Schlüssel an, der nicht an den bekannten anschließt. Das Team wird nicht synchronisiert. Wende dich an den Inhaber – oder verlass das Team.',
      );
    case 'member-key-changed':
      return t(
        'Der Schlüssel von {name} hat sich geändert. Der neue Team-Schlüssel wartet, bis {name} neu bestätigt ist.',
        { name },
      );
    case 'member-returned':
      return t(
        '{name} wurde entfernt und steht wieder im Team. Der neue Team-Schlüssel wartet, bis {name} neu bestätigt ist.',
        { name },
      );
    case 'member-unconfirmed':
      return t(
        '{name} wurde von jemand anderem aufgenommen. Der neue Team-Schlüssel wartet, bis du {name} bestätigt hast.',
        { name },
      );
  }
}

/**
 * In the question before a server is disconnected or signed out of: its
 * teams go from this device with it. Nothing without teams.
 */
export function TeamsGoNote({ server }: { server: SpaceRef }) {
  useLanguage();
  const teams = teamsOnServer(useSpaces(), server);
  if (teams.length === 0) return null;
  const names = teams.map((team) => team.label.trim()).join(', ');
  return (
    <Hint tone="warning">
      {teams.length === 1
        ? t(
            'Auch das Team {team} verschwindet von diesem Gerät. Auf dem Server bleibt es bestehen.',
            { team: names },
          )
        : t(
            'Auch die Teams dieses Servers verschwinden von diesem Gerät: {teams}. Auf dem Server bleiben sie bestehen.',
            { teams: names },
          )}
    </Hint>
  );
}

/** "noch 6 Tage", or that it has run out. */
export function remainingText(expiresMs: number, nowMs: number = Date.now()): string {
  const left = inviteRemaining(expiresMs, nowMs);
  if (!left) return t('Abgelaufen');
  const one = left.n === 1;
  const time =
    left.unit === 'days'
      ? one
        ? t('1 Tag')
        : t('{n} Tage', { n: left.n })
      : left.unit === 'hours'
        ? one
          ? t('1 Stunde')
          : t('{n} Stunden', { n: left.n })
        : one
          ? t('1 Minute')
          : t('{n} Minuten', { n: left.n });
  return t('Gültig noch {time}', { time });
}

/** A member key's fingerprint, in rows to read out. */
export function Fingerprint({ value, large }: { value: string; large?: boolean }) {
  return (
    <code className="team-fingerprint" data-large={large || undefined}>
      {fingerprintRows(value).map((row) => (
        <span key={row}>{row}</span>
      ))}
    </code>
  );
}

/**
 * Runs a team command: busy while it runs, the failure in words afterwards.
 * With the vault shut it asks for the master password and runs it again.
 */
export function useTeamRun() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState<(() => void) | null>(null);

  const run = useCallback(
    async <T,>(
      action: () => Promise<T>,
      done?: (value: T) => void,
      /** Told what went wrong, next to the sentence shown for it. */
      failed?: (failure: TeamFailure) => void,
    ) => {
      setBusy(true);
      setError(null);
      try {
        const value = await action();
        done?.(value);
      } catch (raw) {
        const failure = asTeamFailure(raw);
        if (failure.kind === 'vault-locked') {
          setRetry(() => () => void run(action, done, failed));
        } else {
          setError(teamFailureText(failure));
          failed?.(failure);
        }
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  const dialog = retry && (
    <VaultDialog
      reason={t('Teams brauchen den offenen Tresor: Dort liegt dein Schlüssel für sie.')}
      onDone={() => {
        const again = retry;
        setRetry(null);
        again();
      }}
      onCancel={() => setRetry(null)}
    />
  );

  return { busy, error, run, dialog, clear: () => setError(null) };
}

/** A question with one answer that does something: leave, delete, remove. */
export function AskDialog({
  title,
  lead,
  action,
  danger = true,
  children,
  run,
  onCancel,
  onDone,
}: {
  title: string;
  lead: string;
  action: string;
  danger?: boolean;
  children?: ReactNode;
  run: () => Promise<unknown>;
  onCancel: () => void;
  onDone: (value: unknown) => void;
}) {
  useLanguage();
  const task = useTeamRun();
  return (
    <>
      <Modal
        title={title}
        tone={danger ? 'warning' : 'default'}
        // Not while it works: the × would leave it running unseen.
        onCancel={() => !task.busy && onCancel()}
        footer={
          <>
            <Button data-autofocus onClick={onCancel} disabled={task.busy}>
              {t('Abbrechen')}
            </Button>
            <Button
              variant={danger ? 'danger' : 'primary'}
              data-secondary
              busy={task.busy}
              disabled={task.busy}
              onClick={() => void task.run(run, onDone)}
            >
              {task.busy ? t('Einen Moment…') : action}
            </Button>
          </>
        }
      >
        <p className="dialog-lead">{lead}</p>
        {children}
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

/** A short form in a dialog: a few fields and one button that sends them. */
export function FormDialog({
  title,
  action,
  ready,
  children,
  run,
  onCancel,
  onDone,
}: {
  title: string;
  action: string;
  /** Everything needed is filled in. */
  ready: boolean;
  children: ReactNode;
  run: () => Promise<unknown>;
  onCancel: () => void;
  onDone: (value: unknown) => void;
}) {
  useLanguage();
  const task = useTeamRun();
  const submit = () => {
    if (ready && !task.busy) void task.run(run, onDone);
  };
  return (
    <>
      <Modal
        title={title}
        onCancel={() => !task.busy && onCancel()}
        footer={
          <>
            <Button data-secondary onClick={onCancel} disabled={task.busy}>
              {t('Abbrechen')}
            </Button>
            <Button
              variant="primary"
              busy={task.busy}
              disabled={!ready || task.busy}
              onClick={submit}
            >
              {task.busy ? t('Einen Moment…') : action}
            </Button>
          </>
        }
      >
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          {children}
          {task.error && (
            <p className="field-error" role="alert">
              {task.error}
            </p>
          )}
          <button type="submit" hidden />
        </form>
      </Modal>
      {task.dialog}
    </>
  );
}

/** Something to read and acknowledge; nothing happens on closing it. */
export function NoticeDialog({
  title,
  tone = 'default',
  children,
  onClose,
}: {
  title: string;
  tone?: 'default' | 'warning';
  children: ReactNode;
  onClose: () => void;
}) {
  useLanguage();
  return (
    <Modal
      title={title}
      tone={tone}
      onCancel={onClose}
      footer={
        <Button variant="primary" onClick={onClose}>
          {t('Verstanden')}
        </Button>
      }
    >
      {children}
    </Modal>
  );
}
