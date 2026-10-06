import { Button, Hint, ICONS, Select } from '@uwusuite/design';
import { useState } from 'react';
import { t, useLanguage } from '../lib/i18n';
import {
  asSyncFailure,
  lockLeaveUwusync,
  lockMove,
  lockSendEmailCode,
  lockSignIn,
  lockSignOut,
  syncNow,
  type AcceptSpace,
  type Difference,
  type LockOutcome,
  type SyncStatus,
  type TwoFactorMethod,
} from '../lib/sync';
import { NyuScene } from './nyu/scenes';
import {
  ago,
  APP_SYNC_OFF,
  failureText,
  PassDetails,
  PasswordConfirm,
  syncDot,
  SyncRow,
  useAppSyncOff,
} from './SyncParts';
import { VaultDialog } from './VaultDialog';

/** A two-step method, as the person knows it. */
function methodName(method: TwoFactorMethod): string {
  switch (method.kind) {
    case 'authenticator':
      return t('Authenticator-App');
    case 'email':
      return method.hint
        ? t('Code per E-Mail an {address}', { address: method.hint })
        : t('Code per E-Mail');
    case 'yubikey':
      return t('YubiKey (OTP)');
    default:
      return method.kind;
  }
}

/** The records the move's check found missing or out of date, briefly. */
function differencesText(differences: Difference[]): string {
  const missing = differences.filter((d) => d.problem === 'missing').length;
  const older = differences.length - missing;
  return t('{missing} fehlen, {older} sind veraltet.', { missing, older });
}

/**
 * Signing in to UwULock: server, email and master password, then the code of
 * the second step if the account has one. `move` does the same and then the
 * move from UwUSync, which may take a moment longer.
 */
export function LockSignInForm({
  mode,
  status,
  onBack,
  onDone,
}: {
  mode: 'sign-in' | 'move';
  status: SyncStatus;
  onBack: () => void;
  onDone: (outcome: LockOutcome) => void;
}) {
  useLanguage();
  const [server, setServer] = useState(status.lock.serverUrl ?? '');
  const [email, setEmail] = useState(status.lock.email ?? '');
  const [password, setPassword] = useState('');
  const [methods, setMethods] = useState<TwoFactorMethod[] | null>(null);
  const [provider, setProvider] = useState<number | null>(null);
  const [code, setCode] = useState('');
  const [remember, setRemember] = useState(true);
  const [mailed, setMailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [differences, setDifferences] = useState<Difference[] | null>(null);
  const [unlocking, setUnlocking] = useState(false);
  // The account's space is another than the one this device used: asked
  // about, and once agreed to, taken on the next try.
  const [spaceChange, setSpaceChange] = useState<Extract<
    LockOutcome,
    { kind: 'space-changed' }
  > | null>(null);
  const [acceptSpace, setAcceptSpace] = useState<AcceptSpace | null>(null);

  // Said before anyone types a password: the server would refuse anyway.
  const appSyncOff = useAppSyncOff(server);
  const usable = methods?.filter((m) => m.supported) ?? [];
  const chosen = usable.find((m) => m.provider === provider) ?? usable[0] ?? null;
  const ready =
    !busy &&
    !appSyncOff &&
    server.trim().length > 0 &&
    email.trim().length > 0 &&
    password.length > 0 &&
    (methods === null || (chosen !== null && code.trim().length > 0));

  const submit = async (accept: AcceptSpace | null = acceptSpace) => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    setDifferences(null);
    const twoFactor =
      methods && chosen ? { provider: chosen.provider, code: code.trim(), remember } : null;
    try {
      const run = mode === 'move' ? lockMove : lockSignIn;
      const outcome = await run(server.trim(), email.trim(), password, twoFactor, accept);
      if (outcome.kind === 'space-changed') {
        setSpaceChange(outcome);
        setMethods(null);
        setCode('');
        return;
      }
      if (outcome.kind === 'two-factor') {
        setMethods(outcome.methods);
        setCode('');
        if (outcome.message) setError(outcome.message);
        else if (!outcome.methods.some((m) => m.supported)) {
          setError(
            t(
              'Dein Konto nutzt nur Zwei-Faktor-Wege, die UwUSSH noch nicht kann. Richte im UwULock-Web-Tresor eine Authenticator-App oder Codes per E-Mail ein.',
            ),
          );
        }
        return;
      }
      setPassword('');
      setCode('');
      onDone(outcome);
    } catch (e) {
      const failure = asSyncFailure(e);
      if (failure.kind === 'vault-locked') {
        setUnlocking(true);
        return;
      }
      if (failure.kind === 'move-check') setDifferences(failure.differences);
      setError(failureText(failure));
      // A code is used up by trying it; the password stays for the next try.
      setCode('');
    } finally {
      setBusy(false);
    }
  };

  const sendMail = async () => {
    setError(null);
    try {
      await lockSendEmailCode(server.trim(), email.trim(), password);
      setMailed(true);
    } catch (e) {
      setError(failureText(asSyncFailure(e)));
    }
  };

  if (spaceChange) {
    const agree = () => {
      const accept = { id: spaceChange.now };
      setAcceptSpace(accept);
      setSpaceChange(null);
      void submit(accept);
    };
    return (
      <div className="form sync-form">
        <p className="setting-label">
          {spaceChange.now
            ? t('Neuer Schlüssel für deine UwUSSH-Daten in UwULock?')
            : t('Deine UwUSSH-Daten in UwULock sind weg')}
        </p>
        <p className="dialog-lead">
          {spaceChange.now
            ? t(
                'Dein UwULock-Konto hält die UwUSSH-Daten jetzt unter einem anderen Schlüssel als dem, mit dem dieses Gerät bisher synchronisiert hat. Das ist in Ordnung, wenn du auf einem anderen Gerät einen neuen Schlüssel erzeugt hast – etwa um ein verlorenes Gerät auszusperren. Wenn nicht, verhält sich womöglich der Server falsch: Dann brich ab und frag bei der Person nach, die ihn betreibt. Übernimmst du den neuen Schlüssel, lädt UwUSSH alles von diesem Gerät dorthin hoch.',
              )
            : t(
                'Dein UwULock-Konto hat keine UwUSSH-Daten mehr, obwohl dieses Gerät bisher damit synchronisiert hat. Das ist in Ordnung, wenn du sie im UwULock-Web-Tresor gelöscht hast. Wenn nicht, verhält sich womöglich der Server falsch: Dann brich ab und frag bei der Person nach, die ihn betreibt. Machst du weiter, legt UwUSSH sie neu an und lädt alles von diesem Gerät hinein.',
              )}
        </p>
        <div className="sync-actions">
          <Button
            data-secondary
            onClick={() => {
              setSpaceChange(null);
              setAcceptSpace(null);
            }}
          >
            {t('Abbrechen')}
          </Button>
          <span className="spacer" />
          <Button variant="primary" onClick={agree}>
            {spaceChange.now ? t('Neuen Schlüssel übernehmen') : t('Neu anlegen')}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <form
      className="form sync-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <p className="setting-label">
        {mode === 'move' ? t('Zu UwULock umziehen') : t('Mit UwULock anmelden')}
      </p>
      <p className="dialog-lead">
        {mode === 'move'
          ? t(
              'UwUSSH meldet sich bei UwULock an, kopiert alles von UwUSync in dein UwULock-Konto, prüft die Kopie und wechselt erst dann. Auf UwUSync bleibt alles, wie es ist. Danach öffnet das Master-Passwort deines UwULock-Kontos den Tresor hier.',
            )
          : t(
              'Hosts, Keys und Passwörter auf diesem Gerät kommen mit in dein Konto. Danach öffnet das Master-Passwort deines UwULock-Kontos den Tresor hier.',
            )}
      </p>
      {methods === null ? (
        <>
          <label className="field">
            <span>{t('Server')}</span>
            <input
              value={server}
              autoFocus={server.length === 0}
              spellCheck={false}
              placeholder="https://lock.example.com"
              onChange={(e) => {
                setServer(e.target.value);
                setAcceptSpace(null);
              }}
            />
          </label>
          <label className="field">
            <span>{t('E-Mail')}</span>
            <input
              type="email"
              value={email}
              autoComplete="username"
              spellCheck={false}
              onChange={(e) => {
                setEmail(e.target.value);
                setAcceptSpace(null);
              }}
            />
          </label>
          <label className="field">
            <span>{t('Master-Passwort')}</span>
            <input
              type="password"
              value={password}
              autoFocus={server.length > 0}
              autoComplete="current-password"
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
        </>
      ) : (
        <>
          <p className="field-hint">
            {t('Dein Konto hat eine Zwei-Faktor-Anmeldung. Gib den Code ein.')}
          </p>
          {usable.length > 1 && (
            <label className="field">
              <span>{t('Weg')}</span>
              <Select
                value={chosen?.provider ?? ''}
                onChange={(e) => {
                  setProvider(Number(e.target.value));
                  setMailed(false);
                }}
              >
                {usable.map((method) => (
                  <option key={method.provider} value={method.provider}>
                    {methodName(method)}
                  </option>
                ))}
              </Select>
            </label>
          )}
          {usable.length === 1 && chosen && <p className="field-hint">{methodName(chosen)}</p>}
          {chosen && (
            <label className="field">
              <span>{t('Code')}</span>
              <input
                value={code}
                autoFocus
                autoComplete="one-time-code"
                inputMode={chosen.kind === 'yubikey' ? 'text' : 'numeric'}
                spellCheck={false}
                onChange={(e) => setCode(e.target.value)}
              />
            </label>
          )}
          {chosen?.kind === 'email' && (
            <Button
              size="sm"
              className="justify-self-start"
              icon={mailed ? ICONS.done : ICONS.send}
              onClick={() => void sendMail()}
              disabled={busy}
            >
              {mailed ? t('Code ist unterwegs') : t('Code per E-Mail schicken')}
            </Button>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => setRemember(e.target.checked)}
            />
            <span>
              <b>{t('Dieses Gerät merken')}</b>
              <small>{t('Beim nächsten Anmelden fragt UwULock hier nicht nach dem Code.')}</small>
            </span>
          </label>
        </>
      )}
      {error && (
        <p className="field-error" role="alert">
          {error}
          {differences && differences.length > 0 && <> {differencesText(differences)}</>}
        </p>
      )}
      {appSyncOff && !error && (
        <p className="field-error" role="alert">
          {APP_SYNC_OFF()}
        </p>
      )}
      {busy && mode === 'move' && (
        <p className="field-hint" role="status">
          {t('Zieht um… das dauert bei vielen Einträgen einen Moment.')}
        </p>
      )}
      <div className="sync-actions">
        <Button
          data-secondary
          onClick={() => {
            if (methods) {
              setMethods(null);
              setError(null);
            } else onBack();
          }}
          disabled={busy}
        >
          {t('Zurück')}
        </Button>
        <span className="spacer" />
        <Button
          type="submit"
          variant="primary"
          busy={busy}
          disabled={!ready}
          title={appSyncOff ? APP_SYNC_OFF() : undefined}
        >
          {busy
            ? mode === 'move'
              ? t('Zieht um…')
              : t('Melde an…')
            : mode === 'move'
              ? t('Umziehen')
              : t('Anmelden')}
        </Button>
      </div>
      {unlocking && (
        <VaultDialog
          reason={t(
            'Zuerst den Tresor dieses Geräts öffnen: seine Passwörter und Keys ziehen mit in dein UwULock-Konto.',
          )}
          onDone={() => {
            setUnlocking(false);
            void submit();
          }}
          onCancel={() => setUnlocking(false)}
        />
      )}
    </form>
  );
}

/**
 * Right after the move: what was copied, and whether this device should be
 * removed from UwUSync.
 */
export function MoveDone({
  outcome,
  leftBehind,
  onDone,
}: {
  outcome: Extract<LockOutcome, { kind: 'moved' }>;
  leftBehind: boolean;
  onDone: () => void;
}) {
  useLanguage();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { report } = outcome;

  const leave = async (revoke: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await lockLeaveUwusync(revoke);
      onDone();
    } catch (e) {
      setError(failureText(asSyncFailure(e)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="sync-kit">
      <NyuScene name="done" className="sync-scene" />
      <p className="setting-label">{t('Umgezogen ✧ UwUSSH synchronisiert jetzt über UwULock')}</p>
      <p className="dialog-lead">
        {t(
          '{read} Einträge von UwUSync sind in deinem UwULock-Konto, geprüft: {copied} jetzt kopiert, {there} waren schon da.',
          {
            read: report.read,
            copied: report.copied,
            there: report.alreadyThere + report.newerThere,
          },
        )}{' '}
        {t('Der Tresor hier öffnet ab jetzt mit dem Master-Passwort deines UwULock-Kontos.')}
      </p>
      {report.unreadable > 0 && (
        <Hint tone="warning">
          {t(
            '{n} Einträge auf UwUSync ließen sich mit dem Schlüssel dieses Geräts nicht öffnen und sind nicht umgezogen.',
            { n: report.unreadable },
          )}
        </Hint>
      )}
      {leftBehind && outcome.lastDevice ? (
        // UwUSync keeps its last device (it refuses to remove it): nothing
        // to leave, only an account its admin can delete now.
        <>
          <p className="setting-description">
            {t(
              'Das war das letzte Gerät auf UwUSync: Wer den Server betreibt, kann das Konto dort jetzt löschen.',
            )}
          </p>
          <div className="sync-actions">
            <span className="spacer" />
            <Button variant="primary" disabled={busy} onClick={() => void leave(false)}>
              {t('Fertig')}
            </Button>
          </div>
        </>
      ) : leftBehind ? (
        <>
          <p className="setting-description">
            {t(
              'Auf UwUSync bleiben deine Daten unberührt. Soll dieses Gerät dort ausgetragen werden? Die anderen Geräte synchronisieren dort weiter, bis auch sie umgezogen sind.',
            )}
          </p>
          {error && (
            <p className="field-error" role="alert">
              {error}
            </p>
          )}
          <div className="sync-actions">
            <Button data-secondary disabled={busy} onClick={() => void leave(false)}>
              {t('Eingetragen lassen')}
            </Button>
            <span className="spacer" />
            <Button variant="primary" busy={busy} disabled={busy} onClick={() => void leave(true)}>
              {t('Bei UwUSync austragen')}
            </Button>
          </div>
        </>
      ) : (
        <div className="sync-actions">
          <span className="spacer" />
          <Button variant="primary" onClick={onDone}>
            {t('Fertig')}
          </Button>
        </div>
      )}
    </div>
  );
}

/** Settings → Sync while this device syncs through UwULock. */
export function LockOverview({ status, onChanged }: { status: SyncStatus; onChanged: () => void }) {
  useLanguage();
  const [signingIn, setSigningIn] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [unlocking, setUnlocking] = useState(false);
  const locked = status.vault !== 'unlocked';
  const { lock } = status;

  if (signingIn) {
    return (
      <LockSignInForm
        mode="sign-in"
        status={status}
        onBack={() => setSigningIn(false)}
        onDone={() => {
          setSigningIn(false);
          onChanged();
        }}
      />
    );
  }

  return (
    <>
      <SyncRow
        label={t('Verbunden mit UwULock {server}', { server: lock.serverUrl ?? '' })}
        dot={syncDot(status)}
        control={
          lock.needsSignIn ? (
            <Button
              size="sm"
              variant="primary"
              icon={ICONS.signIn}
              onClick={() => setSigningIn(true)}
            >
              {t('Neu anmelden…')}
            </Button>
          ) : locked ? (
            <Button size="sm" icon={ICONS.unlocked} onClick={() => setUnlocking(true)}>
              {t('Entsperren')}
            </Button>
          ) : (
            <Button
              size="sm"
              icon={ICONS.refresh}
              onClick={() => {
                void syncNow().then(() => window.setTimeout(onChanged, 400));
              }}
              disabled={status.running}
            >
              {t('Jetzt synchronisieren')}
            </Button>
          )
        }
      >
        {lock.email && <p className="setting-description">{lock.email}</p>}
        {lock.needsSignIn ? (
          <Hint tone="warning" role="alert" className="mt-1">
            {t(
              'Die Anmeldung ist abgelaufen oder wurde beendet. Bis du dich neu anmeldest, bleiben Änderungen auf diesem Gerät.',
            )}
          </Hint>
        ) : lock.switchedOff ? (
          <p className="setting-description" role="status">
            {t(
              'Dieser UwULock-Server hat den App-Sync abgeschaltet. Änderungen bleiben auf diesem Gerät; ist der Sync wieder an, gleicht UwUSSH alles ab – ohne neue Anmeldung.',
            )}
          </p>
        ) : (
          <>
            <PassDetails status={status} />
            {!locked && (
              <p className="setting-description">
                {lock.live
                  ? t('Live: Änderungen anderer Geräte kommen sofort an.')
                  : lock.liveRefused
                    ? t(
                        'Der Server bietet keine Live-Verbindung an – UwUSSH fragt regelmäßig nach.',
                      )
                    : t('Ohne Live-Verbindung – UwUSSH fragt jede Minute nach.')}
              </p>
            )}
          </>
        )}
      </SyncRow>

      <SyncRow label={t('Geräte')}>
        <p className="setting-description">
          {t(
            'Ein weiteres Gerät meldet sich einfach mit demselben UwULock-Konto an. Deine Geräte siehst und entfernst du im UwULock-Web-Tresor.',
          )}
        </p>
        {lock.signedInMs && (
          <p className="setting-description">
            {t('Angemeldet {when}.', { when: ago(lock.signedInMs) })}
          </p>
        )}
      </SyncRow>

      <SyncRow
        label={t('Abmelden')}
        control={
          <Button size="sm" variant="danger" icon={ICONS.signOut} onClick={() => setLeaving(true)}>
            {t('Abmelden…')}
          </Button>
        }
      >
        <p className="setting-description">
          {t(
            'Dieses Gerät synchronisiert nicht mehr. Hosts und Tresor bleiben hier, das Master-Passwort deines UwULock-Kontos öffnet ihn weiter.',
          )}
        </p>
      </SyncRow>

      {leaving && (
        <PasswordConfirm
          title={t('Bei UwULock abmelden?')}
          lead={t(
            'Die anderen Geräte synchronisieren weiter. Wieder anmelden geht jederzeit unter Einstellungen → Sync.',
          )}
          action={t('Abmelden')}
          run={(password) => lockSignOut(password)}
          onCancel={() => setLeaving(false)}
          onDone={() => {
            setLeaving(false);
            onChanged();
          }}
        />
      )}
      {unlocking && (
        <VaultDialog
          reason={t('Der Sync braucht den offenen Tresor, um zu ver- und entschlüsseln.')}
          onDone={() => {
            setUnlocking(false);
            onChanged();
            void syncNow();
          }}
          onCancel={() => setUnlocking(false)}
        />
      )}
    </>
  );
}
