import { Button, Icon, IconButton, ICONS, Toggle, Wordmark } from '@uwusuite/design';
import clsx from 'clsx';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { api, type Info, type Options } from './api';
import {
  DoneScene,
  ErrorScene,
  GoodbyeScene,
  PuzzledScene,
  WelcomeScene,
  WorkingScene,
} from './scenes';
import { pling } from './sound';
import { fill, texts as t } from './texts';

type Screen =
  'loading' | 'welcome' | 'running' | 'working' | 'done' | 'error' | 'uninstall' | 'goodbye';
type Job = 'install' | 'update' | 'uninstall';

/** Long enough to see Nyu at work, even when copying takes a split second. */
const MIN_WORKING_MS = 1800;

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The window controls' 10 px glyphs, the same as in the package's title bar. */
function Glyph({ path }: { path: string }) {
  return (
    <svg viewBox="0 0 10 10" className="glyph" aria-hidden>
      <path d={path} />
    </svg>
  );
}

/** The package's pill button at the installer's size; the quiet one is a plum ghost on the gradient. */
function SetupButton({
  children,
  onClick,
  variant = 'primary',
  disabled,
  autoFocus,
}: {
  children: ReactNode;
  onClick: () => void;
  variant?: 'primary' | 'quiet';
  disabled?: boolean;
  autoFocus?: boolean;
}) {
  return (
    <Button
      size="lg"
      variant={variant === 'primary' ? 'primary' : 'ghost'}
      autoFocus={autoFocus}
      disabled={disabled}
      onClick={onClick}
      className={clsx(
        'text-reading px-6 font-extrabold',
        variant === 'quiet' && 'text-plum-soft hover:text-plum hover:bg-white/70',
      )}
    >
      {children}
    </Button>
  );
}

/** One setting on the card: the package's Toggle, with the hint in the installer's plum. */
function SetupToggle({
  checked,
  onChange,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  hint?: string;
}) {
  return (
    <div className="[&_.text-muted]:text-plum-soft rounded-2xl px-2 py-2">
      <Toggle checked={checked} onChange={onChange} label={label} description={hint} />
    </div>
  );
}

/*
 * The installer keeps its own header instead of the package's TitleBar: its window is frameless
 * on every platform, macOS included (where TitleBar draws nothing), it can't be maximized, and
 * closing has to go through `finish` (cleanup after an uninstall from a temp copy) and is locked
 * while Nyu works. The controls borrow the TitleBar's glyphs; the sound button is the package's
 * IconButton.
 */
function Window({
  children,
  busy,
  muted,
  onToggleSound,
}: {
  children: ReactNode;
  busy: boolean;
  muted: boolean;
  onToggleSound: () => void;
}) {
  return (
    <div className="setup-sparkles flex h-full flex-col">
      <header data-tauri-drag-region className="flex h-11 shrink-0 items-center gap-1 px-2">
        <span data-tauri-drag-region className="mr-auto pl-3">
          <Wordmark product="SSH" shell="terminal" className="text-body" />
        </span>
        <IconButton
          size="sm"
          icon={muted ? ICONS.soundOff : ICONS.sound}
          label={muted ? t.soundOn : t.soundOff}
          onClick={onToggleSound}
          className="text-plum-soft! hover:text-plum! hover:bg-white/80!"
        />
        <button
          type="button"
          className="setup-control"
          onClick={() => void api.minimize()}
          title={t.minimize}
          aria-label={t.minimize}
        >
          <Glyph path="M0 5.5h10" />
        </button>
        <button
          type="button"
          className="setup-control"
          data-kind="close"
          disabled={busy}
          onClick={() => void api.finish()}
          title={t.close}
          aria-label={t.close}
        >
          <Glyph path="M.5.5l9 9 M9.5.5l-9 9" />
        </button>
      </header>
      <main className="flex min-h-0 flex-1 flex-col items-center px-7 pb-5">{children}</main>
    </div>
  );
}

function Stage({
  scene,
  title,
  body,
  compact,
}: {
  scene: ReactNode;
  title: string;
  body?: string;
  compact?: boolean;
}) {
  return (
    <div className="animate-slide-up flex w-full flex-col items-center">
      <div
        className={clsx(
          'pt-1 transition-[width] duration-300',
          compact ? 'w-[168px]' : 'w-[272px]',
        )}
      >
        {scene}
      </div>
      <h1 className="pt-3 text-center text-title leading-tight font-extrabold tracking-tight">
        {title}
      </h1>
      {body && !compact && (
        <p className="text-plum-soft max-w-[340px] pt-1.5 text-body text-center leading-relaxed">
          {body}
        </p>
      )}
    </div>
  );
}

export function App() {
  const [info, setInfo] = useState<Info | null>(null);
  const [screen, setScreen] = useState<Screen>('loading');
  const [options, setOptions] = useState<Options | null>(null);
  const [showOptions, setShowOptions] = useState(false);
  const [job, setJob] = useState<Job>('install');
  const [target, setTarget] = useState(0);
  const [shown, setShown] = useState(0);
  const [quote, setQuote] = useState(0);
  const [error, setError] = useState('');
  const [muted, setMuted] = useState(false);
  const [keepData, setKeepData] = useState(true);
  const mutedRef = useRef(muted);
  const started = useRef(false);

  useEffect(() => {
    mutedRef.current = muted;
  }, [muted]);

  const run = async (kind: Job, chosen: Options, loaded: Info, keep = true) => {
    setJob(kind);
    setTarget(0);
    setShown(0);
    setError('');
    setScreen('working');
    const begin = Date.now();
    try {
      if (kind === 'uninstall') await api.uninstall(keep);
      else await api.install(chosen);
      await wait(Math.max(0, MIN_WORKING_MS - (Date.now() - begin)));
      setTarget(1);
      await wait(350);
      setScreen(kind === 'uninstall' ? 'goodbye' : 'done');
      if (!mutedRef.current) pling();
      if (kind === 'update') {
        await wait(1800);
        if (loaded.relaunch) await api.launchApp();
        await api.finish();
      }
    } catch (reason) {
      setError(typeof reason === 'string' ? reason : String(reason));
      setScreen('error');
    }
  };
  const runRef = useRef(run);
  useEffect(() => {
    runRef.current = run;
  });

  useEffect(() => {
    let cancelled = false;
    void api.info().then((loaded) => {
      if (cancelled) return;
      setInfo(loaded);
      setOptions(loaded.options);
      if (loaded.mode === 'uninstall') {
        setScreen('uninstall');
      } else if (loaded.mode === 'update') {
        // Strict mode runs effects twice in development; start only once.
        if (!started.current) {
          started.current = true;
          void runRef.current('update', loaded.options, loaded);
        }
      } else {
        setScreen('welcome');
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(
    () => api.onProgress((progress) => setTarget((current) => Math.max(current, progress.overall))),
    [],
  );

  // Glide towards the reported progress instead of jumping.
  useEffect(() => {
    let frame = 0;
    const tick = () => {
      setShown((current) => {
        const next = current + (target - current) * 0.14;
        return Math.abs(target - next) < 0.003 ? target : next;
      });
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [target]);

  useEffect(() => {
    if (screen !== 'working') return;
    const timer = setInterval(() => setQuote((current) => (current + 1) % t.quotes.length), 2000);
    return () => clearInterval(timer);
  }, [screen]);

  const busy = screen === 'working';
  const shell = (content: ReactNode) => (
    <Window busy={busy} muted={muted} onToggleSound={() => setMuted(!muted)}>
      {content}
    </Window>
  );

  if (!info || !options || screen === 'loading') return shell(null);

  const installed = info.installed && !info.installed.legacy ? info.installed : null;
  const computer =
    info.platform === 'macos'
      ? t.computerMac
      : info.platform === 'linux'
        ? t.computerLinux
        : t.computerWindows;
  const actionLabel = !installed
    ? t.install
    : installed.version === info.version
      ? t.reinstall
      : t.update;
  const startInstall = () => {
    if (info.appRunning) setScreen('running');
    else void run('install', options, info);
  };

  if (screen === 'welcome') {
    return shell(
      <>
        <Stage
          scene={<WelcomeScene />}
          compact={showOptions}
          title={installed ? t.againTitle : t.welcomeTitle}
          body={
            installed
              ? fill(t.againBody, { installed: installed.version ?? '', version: info.version })
              : fill(t.welcomeBody, { computer })
          }
        />
        <div className="flex flex-col items-center gap-2 pt-5">
          <SetupButton onClick={startInstall} autoFocus disabled={!info.hasPayload}>
            {actionLabel} ♡
          </SetupButton>
          <button
            type="button"
            onClick={() => setShowOptions(!showOptions)}
            aria-expanded={showOptions}
            className="text-plum-soft hover:text-plum flex items-center gap-1 rounded-full text-meta px-3 py-1 font-bold"
          >
            {showOptions ? t.fewerOptions : t.options}
            <Icon
              icon={ICONS.expand}
              size="xs"
              className={clsx('transition-transform', showOptions && 'rotate-180')}
            />
          </button>
        </div>
        {showOptions && (
          <div className="setup-card animate-slide-up rounded-dialog mt-1 w-full p-3">
            <div className="flex items-center gap-2 px-2 pb-1">
              <Icon icon={ICONS.folder} size="sm" className="text-pink-solid shrink-0" />
              <span className="min-w-0 flex-1">
                <span className="text-plum-soft text-caption block font-bold">{t.folder}</span>
                <span className="text-meta block truncate font-semibold" title={options.dir}>
                  {options.dir}
                </span>
              </span>
              <button
                type="button"
                className="hover:bg-pink-tint text-pink-solid text-caption shrink-0 rounded-full px-3 py-1 font-bold"
                onClick={async () => {
                  const dir = await api.pickFolder(options.dir);
                  if (dir) setOptions({ ...options, dir });
                }}
              >
                {t.change}
              </button>
            </div>
            {info.platform !== 'macos' && (
              <SetupToggle
                checked={options.desktopShortcut}
                onChange={(desktopShortcut) => setOptions({ ...options, desktopShortcut })}
                label={t.desktopShortcut}
              />
            )}
            {info.hasKeygen && (
              <SetupToggle
                checked={options.keygen}
                onChange={(keygen) => setOptions({ ...options, keygen })}
                label={t.keygen}
              />
            )}
          </div>
        )}
        {installed && info.platform !== 'windows' && !showOptions && (
          <button
            type="button"
            onClick={() =>
              void api
                .beginUninstall()
                .then(() => setScreen('uninstall'))
                .catch((reason) => {
                  setError(String(reason));
                  setScreen('error');
                })
            }
            className="text-plum-soft hover:text-plum text-caption mt-2 rounded-full px-3 py-1 font-bold"
          >
            {t.removeInstead}
          </button>
        )}
        <p className="text-plum-soft mt-auto pt-3 text-badge text-center">
          {!info.hasPayload
            ? t.devBuild
            : info.sandbox
              ? t.sandbox
              : fill(t.footer, { version: info.version })}
        </p>
      </>,
    );
  }

  if (screen === 'running') {
    return shell(
      <div className="my-auto flex w-full flex-col items-center pb-10">
        <Stage scene={<PuzzledScene />} title={t.runningTitle} body={t.runningBody} />
        <div className="flex flex-col items-center gap-1 pt-6">
          <SetupButton
            autoFocus
            onClick={async () => {
              try {
                await api.closeApp();
                void run('install', options, info);
              } catch (reason) {
                setError(String(reason));
                setScreen('error');
              }
            }}
          >
            {t.closeAndContinue}
          </SetupButton>
          <SetupButton variant="quiet" onClick={() => setScreen('welcome')}>
            {t.back}
          </SetupButton>
        </div>
      </div>,
    );
  }

  if (screen === 'working') {
    const title =
      job === 'uninstall'
        ? t.progressUninstall
        : job === 'update'
          ? t.progressUpdate
          : t.progressInstall;
    const percent = Math.round(shown * 100);
    return shell(
      <div className="my-auto flex w-full flex-col items-center pb-10">
        <Stage scene={<WorkingScene />} title={title} />
        <div className="w-full max-w-[340px] pt-7">
          <div
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
            className="h-4 overflow-hidden rounded-full bg-white/80 p-[3px] shadow-[inset_0_1px_3px_rgb(75_29_63/0.12)]"
          >
            <div
              className="setup-bar h-full rounded-full"
              style={{ width: `${Math.max(6, percent)}%` }}
            />
          </div>
          <div className="text-plum-soft flex items-center justify-between text-meta pt-2.5 font-semibold">
            <span key={quote} className="animate-slide-up">
              {t.quotes[quote]}
            </span>
            <span className="tabular-nums">{percent} %</span>
          </div>
        </div>
      </div>,
    );
  }

  if (screen === 'done') {
    if (job === 'update') {
      return shell(
        <div className="my-auto w-full pb-10">
          <Stage
            scene={<DoneScene />}
            title={t.updateDoneTitle}
            body={fill(t.updateDoneBody, { version: info.version })}
          />
        </div>,
      );
    }
    return shell(
      <>
        <Stage scene={<DoneScene />} title={t.doneTitle} body={t.doneBody} />
        <div className="setup-card animate-slide-up rounded-dialog mt-4 w-full px-4 py-3">
          <p className="text-pink-solid text-caption pb-1 font-extrabold tracking-wide uppercase">
            {t.tipsTitle}
          </p>
          <ul className="flex flex-col gap-1.5">
            {t.tips.map((tip) => (
              <li key={tip} className="text-meta flex gap-2 leading-snug">
                <Icon icon={ICONS.done} size="xs" className="text-pink-solid mt-0.5 shrink-0" />
                {tip}
              </li>
            ))}
          </ul>
        </div>
        <div className="mt-auto flex items-center gap-2 pt-3">
          <SetupButton variant="quiet" onClick={() => void api.finish()}>
            {t.close}
          </SetupButton>
          <SetupButton
            autoFocus
            onClick={async () => {
              await api.launchApp();
              await api.finish();
            }}
          >
            {t.start}
          </SetupButton>
        </div>
      </>,
    );
  }

  if (screen === 'error') {
    return shell(
      <>
        <Stage scene={<ErrorScene />} title={t.errorTitle} />
        <p className="text-plum-soft mt-3 w-full rounded-2xl bg-white/80 px-4 py-3 text-caption text-center break-words select-text">
          {error}
        </p>
        <div className="flex items-center gap-2 pt-5">
          <SetupButton
            variant="quiet"
            onClick={() =>
              void (async () => {
                // UwUSSH closed itself for the update; bring the installed version back.
                if (job === 'update' && info.relaunch) await api.launchApp().catch(() => undefined);
                await api.finish();
              })()
            }
          >
            {t.close}
          </SetupButton>
          <SetupButton autoFocus onClick={() => void run(job, options, info, keepData)}>
            {t.retry}
          </SetupButton>
        </div>
      </>,
    );
  }

  if (screen === 'uninstall') {
    return shell(
      <>
        <Stage
          scene={<GoodbyeScene />}
          title={t.uninstallTitle}
          body={fill(t.uninstallBody, { computer })}
        />
        <div className="setup-card rounded-dialog mt-5 w-full p-2">
          <SetupToggle
            checked={keepData}
            onChange={setKeepData}
            label={t.keepData}
            hint={t.keepDataHint}
          />
        </div>
        <div className="mt-auto flex items-center gap-2 pt-3">
          <SetupButton variant="quiet" onClick={() => void api.finish()}>
            {t.keep}
          </SetupButton>
          <SetupButton autoFocus onClick={() => void run('uninstall', options, info, keepData)}>
            {t.uninstall}
          </SetupButton>
        </div>
      </>,
    );
  }

  return shell(
    <div className="my-auto flex w-full flex-col items-center pb-10">
      <Stage scene={<GoodbyeScene />} title={t.goodbyeTitle} body={t.goodbyeBody} />
      <div className="pt-6">
        <SetupButton autoFocus onClick={() => void api.finish()}>
          {t.close}
        </SetupButton>
      </div>
    </div>,
  );
}
