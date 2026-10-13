/**
 * Settings → Tastatur: the connected keyboard as a picture, with the app's
 * shortcuts on it.
 *
 * The modifiers above the picture say which level is shown (Strg+Umschalt,
 * ⌘, …); a key that does something on that level is coloured. Clicking a key
 * picks the combination, and the panel below gives it an action, takes it
 * away or puts the default back. "Tastenkombination drücken" picks one by
 * pressing it instead. Every change is saved at once, into the vault, and
 * travels with the sync.
 */

import { Button, IconButton, ICONS, Select, Switch, TextArea, TextInput } from '@uwusuite/design';
import { useEffect, useMemo, useState } from 'react';
import { localShellAvailable } from '../lib/flavor';
import { t } from '../lib/i18n';
import { useKeyboard } from '../lib/keyboard';
import { LAYOUTS, mainRows, NAVIGATION_ROWS, type Key } from '../lib/keyboardLayouts';
import {
  ACTION_GROUPS,
  actionOf,
  bind,
  chordOf,
  chordsFor,
  chordWarning,
  defaultChords,
  holderOf,
  isModifierCode,
  keyName,
  pruneOverrides,
  resetAction,
  TAB_ACTIONS,
  unbind,
  type BoundAction,
  type Chord,
  type ChordWarning,
  type Overrides,
} from '../lib/keymap';
import {
  keptTexts,
  keymapFamily,
  saveKeymap,
  useKeymap,
  type KeymapText,
  type TextDraft,
} from '../lib/keymapStore';
import { listHosts, type HostRecord } from '../lib/session';
import { updateSettings, useSettings } from '../lib/settings';
import { actionName, isMac, keyNames, keysOf } from '../lib/shortcuts';

type Modifier = 'ctrl' | 'shift' | 'alt' | 'meta';
type Modifiers = Record<Modifier, boolean>;

const MODIFIER_OF: Record<string, Modifier> = {
  ControlLeft: 'ctrl',
  ControlRight: 'ctrl',
  ShiftLeft: 'shift',
  ShiftRight: 'shift',
  AltLeft: 'alt',
  AltRight: 'alt',
  MetaLeft: 'meta',
  MetaRight: 'meta',
};

const TEXT_LIMIT = 4000;
const LABEL_LIMIT = 100;
const TEXTS_LIMIT = 100;

function modifierName(modifier: Modifier, mac: boolean): string {
  const names = keyNames();
  if (mac) return { ctrl: '⌃', shift: '⇧', alt: '⌥', meta: '⌘' }[modifier];
  return { ctrl: names.ctrl, shift: names.shift, alt: names.alt, meta: 'Win' }[modifier];
}

/** What a key says in the picture. */
function capLabel(code: string, mac: boolean): string {
  const modifier = MODIFIER_OF[code];
  if (modifier) return modifierName(modifier, mac);
  if (code === 'CapsLock') return '⇪';
  if (code === 'IntlYen') return '¥';
  if (code === 'Insert') return mac ? 'Ins' : t('Einfg');
  if (code === 'Escape') return 'Esc';
  if (code === 'IntlRo') return mac ? '_' : '/';
  const names = keyNames();
  // Short names in the picture: the words would not fit on a key.
  return keyName(code, true, { ...names, more: undefined });
}

function warningText(warning: ChordWarning): string {
  switch (warning) {
    case 'typing':
      return t('Diese Taste schreibt sonst ein Zeichen – im Terminal fehlt es dann.');
    case 'terminal':
      return t(
        'Programme im Terminal benutzen diese Kombination (z. B. Strg+C, Strg+R). Sie kommt dort dann nicht mehr an.',
      );
    case 'altgr':
      return t(
        'Strg+Alt ist auf vielen Tastaturen AltGr: Zeichen wie @, { oder \\ könnten danach fehlen.',
      );
    case 'option':
      return t('⌥ schreibt auf dem Mac Sonderzeichen – dieses fehlt dann im Terminal.');
    case 'system':
      return t('Das System behält diese Kombination meist für sich; sie kommt womöglich nie an.');
  }
}

function chordWith(modifiers: Modifiers, code: string): Chord {
  return {
    ...(modifiers.ctrl ? { ctrl: true } : {}),
    ...(modifiers.shift ? { shift: true } : {}),
    ...(modifiers.alt ? { alt: true } : {}),
    ...(modifiers.meta ? { meta: true } : {}),
    code,
  };
}

function newId(): string {
  return crypto.randomUUID();
}

type TextEdit = { id: string; label: string; enter: boolean; value: string; isNew: boolean };

function TextEditor({
  edit,
  busy,
  onChange,
  onSave,
  onCancel,
}: {
  edit: TextEdit;
  busy: boolean;
  onChange: (edit: TextEdit) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const ready = edit.label.trim().length > 0 && (edit.value.length > 0 || !edit.isNew);
  return (
    <div className="keymap-text-edit">
      <TextInput
        aria-label={t('Name')}
        placeholder={t('Name, z. B. „Logs ansehen“')}
        maxLength={LABEL_LIMIT}
        value={edit.label}
        autoFocus
        onChange={(event) => onChange({ ...edit, label: event.target.value })}
      />
      <TextArea
        aria-label={t('Text')}
        className="keymap-text-value"
        rows={3}
        spellCheck={false}
        maxLength={TEXT_LIMIT}
        placeholder={
          edit.isNew
            ? t('Was ins Terminal getippt wird')
            : t('Leer lassen, um den gespeicherten Text zu behalten')
        }
        value={edit.value}
        onChange={(event) => onChange({ ...edit, value: event.target.value })}
      />
      <div className="keymap-text-actions">
        <Switch
          label={t('Mit Eingabe abschicken')}
          checked={edit.enter}
          onChange={(enter) => onChange({ ...edit, enter })}
        />
        <span className="keymap-spacer" />
        <Button variant="ghost" onClick={onCancel}>
          {t('Abbrechen')}
        </Button>
        <Button variant="primary" disabled={!ready || busy} onClick={onSave}>
          {t('Speichern')}
        </Button>
      </div>
    </div>
  );
}

export function KeyboardSettings() {
  const settings = useSettings();
  const keyboard = useKeyboard();
  const { bindings, texts } = useKeymap();
  const family = keymapFamily();
  const mac = isMac();
  const [hosts, setHosts] = useState<HostRecord[]>([]);
  const [modifiers, setModifiers] = useState<Modifiers>({
    ctrl: !mac,
    shift: !mac,
    alt: false,
    meta: mac,
  });
  const [code, setCode] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [edit, setEdit] = useState<TextEdit | null>(null);

  useEffect(() => {
    listHosts()
      .then(setHosts)
      .catch(() => setHosts([]));
  }, []);

  // "Tastenkombination drücken": the next key with its modifiers is the pick.
  useEffect(() => {
    if (!capturing) return;
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (isModifierCode(event.code)) return;
      const chord = chordOf(event);
      setCapturing(false);
      if (event.code === 'Escape' && !chord.ctrl && !chord.shift && !chord.alt && !chord.meta) {
        return;
      }
      setModifiers({
        ctrl: Boolean(chord.ctrl),
        shift: Boolean(chord.shift),
        alt: Boolean(chord.alt),
        meta: Boolean(chord.meta),
      });
      setCode(chord.code);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [capturing]);

  const hostNames = useMemo(() => new Map(hosts.map((host) => [host.id, host.name])), [hosts]);
  const textNames = useMemo(() => new Map(texts.map((text) => [text.id, text.label])), [texts]);

  const nameOf = (id: string): string => {
    const action = actionOf(id);
    if (!action) return id;
    if (action.kind === 'select-tab') return t('Zu Tab {number}', { number: action.index + 1 });
    if (action.kind === 'host') {
      return t('Verbinden: {name}', { name: hostNames.get(action.id) ?? t('gelöschter Host') });
    }
    if (action.kind === 'text') {
      return t('Text: {name}', { name: textNames.get(action.id) ?? t('gelöschter Text') });
    }
    return actionName(action.kind as BoundAction);
  };

  const save = async (next: Overrides, drafts: readonly TextDraft[] = keptTexts(texts)) => {
    setError(null);
    setBusy(true);
    try {
      await saveKeymap(next, drafts);
      return true;
    } catch (failure) {
      const locked =
        typeof failure === 'object' &&
        failure !== null &&
        'kind' in failure &&
        failure.kind === 'vault-locked';
      setError(
        locked
          ? t('Der Tresor ist gesperrt. Entsperre ihn, um Texte zu speichern.')
          : t('Die Tastenkürzel ließen sich nicht speichern.'),
      );
      return false;
    } finally {
      setBusy(false);
    }
  };

  const rows = useMemo(
    () => mainRows(keyboard.layout.geometry, mac),
    [keyboard.layout.geometry, mac],
  );
  const selected = code ? chordWith(modifiers, code) : null;
  const holder = selected ? holderOf(selected, family, bindings) : null;
  const warning = selected ? chordWarning(selected, family) : null;
  const changed = Object.keys(bindings).length > 0;

  const groups: { label: string; ids: string[] }[] = [
    {
      label: t('Tabs'),
      ids: [
        ...ACTION_GROUPS.tabs.filter((id) => id !== 'new-shell' || localShellAvailable()),
        ...TAB_ACTIONS,
      ],
    },
    { label: t('Terminal'), ids: [...ACTION_GROUPS.terminal] },
    { label: t('App'), ids: [...ACTION_GROUPS.app] },
    {
      label: t('Hosts verbinden'),
      ids: [...hosts].sort((a, b) => a.name.localeCompare(b.name)).map((host) => `host:${host.id}`),
    },
    { label: t('Texte senden'), ids: texts.map((text) => `text:${text.id}`) },
  ].filter((group) => group.ids.length > 0);

  const cap = (key: Key | null, index: number) => {
    if (!key) return <span key={index} className="kb-gap" style={{ flexGrow: 1 }} />;
    const modifier = MODIFIER_OF[key.code];
    const inert = key.code === 'CapsLock';
    const chord = chordWith(modifiers, key.code);
    const held = modifier || inert ? null : holderOf(chord, family, bindings);
    const active = modifier ? modifiers[modifier] : code === key.code;
    const label = capLabel(key.code, mac);
    return (
      <button
        key={key.code}
        type="button"
        className="kb-key"
        style={{ flexGrow: key.width ?? 1 }}
        data-bound={held ? '' : undefined}
        data-modifier={modifier ? '' : undefined}
        aria-pressed={active}
        disabled={inert}
        title={held ? `${keysOf(chord)} – ${nameOf(held)}` : modifier ? undefined : keysOf(chord)}
        onClick={() => {
          if (modifier) setModifiers((now) => ({ ...now, [modifier]: !now[modifier] }));
          else setCode(key.code);
        }}
      >
        <span className="kb-cap">{label}</span>
        {held && <span className="kb-action">{nameOf(held)}</span>}
      </button>
    );
  };

  const jumpTo = (id: string) => {
    const chord = chordsFor(id, family, bindings)[0];
    if (!chord) return;
    setModifiers({
      ctrl: Boolean(chord.ctrl),
      shift: Boolean(chord.shift),
      alt: Boolean(chord.alt),
      meta: Boolean(chord.meta),
    });
    setCode(chord.code);
  };

  const saveText = async () => {
    if (!edit) return;
    const label = edit.label.trim();
    const draft: TextDraft = {
      id: edit.id,
      label,
      enter: edit.enter,
      text: edit.value.length > 0 ? { kind: 'set', value: edit.value } : { kind: 'keep' },
    };
    const drafts = edit.isNew
      ? [...keptTexts(texts), draft]
      : keptTexts(texts).map((kept) => (kept.id === edit.id ? draft : kept));
    if (await save(bindings, drafts)) setEdit(null);
  };

  const removeText = (text: KeymapText) => {
    const drafts = keptTexts(texts).filter((kept) => kept.id !== text.id);
    void save(
      pruneOverrides(bindings, (id) => id !== `text:${text.id}`),
      drafts,
    );
  };

  return (
    <div className="keymap">
      <div className="keymap-bar">
        <Select
          className="w-56"
          aria-label={t('Tastaturlayout')}
          value={settings.keyboardLayout}
          onChange={(event) => updateSettings({ keyboardLayout: event.target.value })}
        >
          <option value="auto">
            {settings.keyboardLayout === 'auto'
              ? t('Automatisch ({name})', { name: keyboard.layout.name })
              : t('Automatisch')}
          </option>
          {LAYOUTS.map((layout) => (
            <option key={layout.id} value={layout.id}>
              {layout.name}
            </option>
          ))}
        </Select>
        <span className="keymap-spacer" />
        <Button
          variant={capturing ? 'primary' : 'secondary'}
          onClick={() => setCapturing((on) => !on)}
        >
          {capturing ? t('Jetzt drücken … (Esc bricht ab)') : t('Tastenkombination drücken')}
        </Button>
        <Button
          variant="ghost"
          disabled={!changed || busy}
          onClick={() => void save(pruneOverrides(bindings, (id) => /^(?:host|text):/.test(id)))}
        >
          {t('Alle zurücksetzen')}
        </Button>
      </div>

      <div className="keymap-modifiers" role="group" aria-label={t('Zusatztasten')}>
        {(mac
          ? (['ctrl', 'alt', 'shift', 'meta'] as const)
          : (['ctrl', 'shift', 'alt', 'meta'] as const)
        ).map((modifier) => (
          <button
            key={modifier}
            type="button"
            className="keymap-modifier"
            aria-pressed={modifiers[modifier]}
            onClick={() => setModifiers((now) => ({ ...now, [modifier]: !now[modifier] }))}
          >
            {modifierName(modifier, mac)}
          </button>
        ))}
        <span className="keymap-hint">
          {t('Zusatztasten wählen, dann eine Taste anklicken. Farbige Tasten sind belegt.')}
        </span>
      </div>

      <div className="kb" data-capturing={capturing ? '' : undefined}>
        <div className="kb-main">
          {rows.map((row, index) => (
            <div key={index} className="kb-row">
              {row.map(cap)}
            </div>
          ))}
        </div>
        <div className="kb-nav">
          <div className="kb-row" />
          {NAVIGATION_ROWS.map((row, index) => (
            <div key={index} className="kb-row">
              {row.map(cap)}
            </div>
          ))}
        </div>
      </div>

      {error && (
        <p className="keymap-error" role="alert">
          {error}
        </p>
      )}

      <div className="keymap-panel">
        {selected ? (
          <>
            <div className="keymap-panel-head">
              <kbd className="keymap-chord">{keysOf(selected)}</kbd>
              <Select
                className="keymap-action"
                aria-label={t('Aktion')}
                value={holder ?? ''}
                disabled={busy}
                onChange={(event) => {
                  const id = event.target.value;
                  if (id) void save(bind(bindings, family, id, selected));
                  else if (holder) void save(unbind(bindings, family, holder, selected));
                }}
              >
                <option value="">{t('Keine Aktion')}</option>
                {groups.map((group) => (
                  <optgroup key={group.label} label={group.label}>
                    {group.ids.map((id) => (
                      <option key={id} value={id}>
                        {nameOf(id)}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </Select>
              {holder && (
                <IconButton
                  icon={ICONS.delete}
                  label={t('Belegung entfernen')}
                  disabled={busy}
                  onClick={() => void save(unbind(bindings, family, holder, selected))}
                />
              )}
            </div>
            {holder && (
              <p className="keymap-note">
                {t('Liegt auf „{action}“. Eine andere Aktion übernimmt die Kombination.', {
                  action: nameOf(holder),
                })}
                {chordsFor(holder, family, bindings).length > 1 &&
                  ` ${t('Alle Kürzel dieser Aktion: {keys}', {
                    keys: chordsFor(holder, family, bindings).map(keysOf).join(' · '),
                  })}`}
              </p>
            )}
            {holder && bindings[holder] && (
              <p className="keymap-note">
                {defaultChords(holder, family).length > 0
                  ? t('Standard für „{action}“: {keys}', {
                      action: nameOf(holder),
                      keys: defaultChords(holder, family).map(keysOf).join(' · '),
                    })
                  : t('„{action}“ hat kein Standardkürzel.', { action: nameOf(holder) })}{' '}
                <button
                  type="button"
                  className="keymap-link"
                  disabled={busy}
                  onClick={() => void save(resetAction(bindings, family, holder))}
                >
                  {t('Zurücksetzen')}
                </button>
              </p>
            )}
            {warning && <p className="keymap-warning">{warningText(warning)}</p>}
          </>
        ) : (
          <p className="keymap-note">
            {t(
              'Klicke eine Taste an, um ihr eine Aktion zu geben – oder suche eine Aktion, um ihre Taste zu sehen.',
            )}
          </p>
        )}
        <div className="keymap-find">
          <Select
            className="keymap-action"
            aria-label={t('Aktion suchen')}
            value=""
            onChange={(event) => jumpTo(event.target.value)}
          >
            <option value="">{t('Wo liegt …?')}</option>
            {groups.map((group) => (
              <optgroup key={group.label} label={group.label}>
                {group.ids.map((id) => {
                  const chords = chordsFor(id, family, bindings);
                  return (
                    <option key={id} value={id} disabled={chords.length === 0}>
                      {chords.length > 0
                        ? `${nameOf(id)} – ${chords.map(keysOf).join(' · ')}`
                        : t('{action} – ohne Kürzel', { action: nameOf(id) })}
                    </option>
                  );
                })}
              </optgroup>
            ))}
          </Select>
        </div>
      </div>

      <section className="keymap-texts">
        <div className="keymap-texts-head">
          <div>
            <p className="text-body font-semibold">{t('Texte')}</p>
            <p className="keymap-note">
              {t(
                'Befehle oder Textbausteine, die eine Taste ins Terminal tippt. Sie liegen verschlüsselt im Tresor und werden nach dem Speichern nicht mehr angezeigt.',
              )}
            </p>
          </div>
          <Button
            variant="secondary"
            disabled={edit !== null || texts.length >= TEXTS_LIMIT}
            onClick={() => setEdit({ id: newId(), label: '', enter: true, value: '', isNew: true })}
          >
            {t('Text hinzufügen')}
          </Button>
        </div>
        {texts.map((text) =>
          edit?.id === text.id ? (
            <TextEditor
              key={text.id}
              edit={edit}
              busy={busy}
              onChange={setEdit}
              onSave={() => void saveText()}
              onCancel={() => setEdit(null)}
            />
          ) : (
            <div key={text.id} className="keymap-text">
              <span className="keymap-text-name">{text.label}</span>
              <span className="keymap-text-keys">
                {chordsFor(`text:${text.id}`, family, bindings).map(keysOf).join(' · ') ||
                  t('ohne Kürzel')}
                {text.enter ? ` · ${t('mit Eingabe')}` : ''}
              </span>
              <IconButton
                size="sm"
                icon={ICONS.edit}
                label={t('Bearbeiten')}
                disabled={edit !== null}
                onClick={() =>
                  setEdit({
                    id: text.id,
                    label: text.label,
                    enter: text.enter,
                    value: '',
                    isNew: !text.hasText,
                  })
                }
              />
              <IconButton
                icon={ICONS.delete}
                label={t('Löschen')}
                disabled={busy}
                onClick={() => removeText(text)}
              />
            </div>
          ),
        )}
        {edit?.isNew && !texts.some((text) => text.id === edit.id) && (
          <TextEditor
            edit={edit}
            busy={busy}
            onChange={setEdit}
            onSave={() => void saveText()}
            onCancel={() => setEdit(null)}
          />
        )}
      </section>

      <p className="keymap-note keymap-fixed">
        {mac
          ? t(
              'Fest belegt: ⌘V fügt ein, ⌘← · ⌘→ · ⌘⌫ springen zum Zeilenanfang, zum Zeilenende und löschen bis zum Cursor.',
            )
          : t(
              'Fest belegt: Strg+Umschalt+V fügt ein. Ob Strg+C kopiert, wenn etwas markiert ist, steht unter „Terminal“.',
            )}{' '}
        {t(
          'Tastenkürzel und Texte werden mit deinem Tresor synchronisiert – Mac und Windows/Linux getrennt.',
        )}
      </p>
    </div>
  );
}
