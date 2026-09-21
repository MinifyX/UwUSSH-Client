// Builds UwUSSH's own installer for the system it runs on: the app and
// UwUKeygen, packed into the setup with Nyu in it.
//
//   pnpm build:setup                       for this machine
//   pnpm build:setup --target <triple>     another architecture (macOS:
//                                          x86_64-apple-darwin on Apple silicon)
//
// Everything lands in target/installers/:
//
//   Windows  UwUSSH-Setup-<v>.exe                    what people run, and what updates run
//   macOS    UwUSSH-Setup-<v>-macos-<arch>.dmg       what people open
//            UwUSSH-Setup-<v>-macos-<arch>-update    the setup program inside it, for updates
//   Linux    UwUSSH-Setup-<v>-linux-<arch>.AppImage  what people run, and what updates run
//            UwUSSH-<v>-linux-<arch>.deb             the app alone, for apt; no setup, no updater
//
// With TAURI_SIGNING_PRIVATE_KEY (and _PASSWORD) set, the files the updater
// runs are signed too, with a .sig next to each. The release signs on the
// machine that holds the key; CI builds without it.

import { execFileSync, execSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const run = (command, env = {}) =>
  execSync(command, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });

// Keep the update-signing key out of the app build. Only `tauri signer sign`
// needs it; the builds below run hundreds of third-party build scripts (Cargo
// build.rs, npm) that would otherwise see it in their environment. Captured
// here and removed from the environment, then handed only to the signing command.
const signingKey = process.env.TAURI_SIGNING_PRIVATE_KEY;
const signingPassword = process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '';
delete process.env.TAURI_SIGNING_PRIVATE_KEY;
delete process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;

const fail = (message) => {
  console.error(`\n✗ ${message}`);
  process.exit(1);
};

const targetIndex = process.argv.indexOf('--target');
const target = targetIndex > 0 ? process.argv[targetIndex + 1] : undefined;
const targetArg = target ? ` --target ${target}` : '';

const { version } = JSON.parse(
  readFileSync(join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'),
);
for (const part of ['setup', 'keygen']) {
  const conf = JSON.parse(
    readFileSync(join(root, `apps/${part}/src-tauri/tauri.conf.json`), 'utf8'),
  );
  if (conf.version !== version) fail(`apps/${part} says ${conf.version}, the app says ${version}.`);
}

// Safety net against a future refactor: the builds must never run with the signing key in reach.
if (process.env.TAURI_SIGNING_PRIVATE_KEY || process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
  throw new Error('The update-signing key must be removed from the environment before building.');
}

const release = join(root, 'target', ...(target ? [target] : []), 'release');
const bundles = join(release, 'bundle');
const out = join(root, 'target', 'installers');
mkdirSync(out, { recursive: true });

/** The architecture as release file names say it. */
function arch() {
  const name = target ?? `${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}-host`;
  return name.startsWith('aarch64') ? 'arm64' : 'x64';
}

/** A Tauri config override, as a file: quoting JSON on a command line is a trap on every shell. */
function configFile(name, config) {
  const path = join(mkdtempSync(join(tmpdir(), 'uwussh-build-')), `${name}.json`);
  writeFileSync(path, JSON.stringify(config));
  return path;
}

function sign(file) {
  if (!signingKey) return;
  console.log(`\n▸ Signing ${file} for the updater`);
  run(`pnpm --filter @uwussh/desktop exec tauri signer sign "${file}"`, {
    TAURI_SIGNING_PRIVATE_KEY: signingKey,
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: signingPassword,
  });
  if (!existsSync(`${file}.sig`)) fail('The signer reported success but wrote no .sig.');
}

function only(dir, test, what) {
  const found = existsSync(dir) ? readdirSync(dir).filter(test) : [];
  if (found.length !== 1) fail(`Expected one ${what} in ${dir}, found ${found.length}.`);
  return join(dir, found[0]);
}

const produced = [];

if (process.platform === 'win32') {
  console.log(`\n▸ Building UwUSSH ${version}`);
  run(`pnpm --filter @uwussh/desktop tauri build --no-bundle${targetArg}`);
  const app = join(release, 'uwussh-desktop.exe');
  if (!existsSync(app)) fail(`Missing ${app}`);

  console.log(`\n▸ Building UwUKeygen ${version}`);
  run(`pnpm --filter @uwussh/keygen tauri build --no-bundle${targetArg}`);
  const keygen = join(release, 'uwukeygen.exe');
  if (!existsSync(keygen)) fail(`Missing ${keygen}`);

  console.log('\n▸ Packing both into the setup');
  run(`pnpm --filter @uwussh/setup tauri build --no-bundle${targetArg}`, {
    UWUSSH_SETUP_PAYLOAD: app,
    UWUSSH_SETUP_KEYGEN_PAYLOAD: keygen,
  });
  const setup = join(out, `UwUSSH-Setup-${version}.exe`);
  copyFileSync(join(release, 'uwussh-setup.exe'), setup);
  // Where the Windows release has always looked for it.
  copyFileSync(setup, join(release, `UwUSSH-Setup-${version}.exe`));
  sign(setup);
  if (signingKey) copyFileSync(`${setup}.sig`, join(release, `UwUSSH-Setup-${version}.exe.sig`));
  produced.push(setup);
} else if (process.platform === 'darwin') {
  // Ad-hoc signed: no Apple developer ID, but a sealed bundle, which Apple
  // silicon insists on and which keeps the app intact through the setup.
  const macOS = { signingIdentity: '-', minimumSystemVersion: '11.0' };
  const apps = mkdtempSync(join(tmpdir(), 'uwussh-apps-'));

  console.log(`\n▸ Building UwUSSH ${version}`);
  run(
    `pnpm --filter @uwussh/desktop tauri build --bundles app${targetArg} --config "${configFile('app', { bundle: { macOS } })}"`,
  );
  execFileSync('ditto', [join(bundles, 'macos', 'UwUSSH.app'), join(apps, 'UwUSSH.app')]);

  console.log(`\n▸ Building UwUKeygen ${version}`);
  run(
    `pnpm --filter @uwussh/keygen tauri build --bundles app${targetArg} --config "${configFile('keygen', { bundle: { macOS } })}"`,
  );
  execFileSync('ditto', [join(bundles, 'macos', 'UwUKeygen.app'), join(apps, 'UwUKeygen.app')]);

  console.log('\n▸ Packing both into the setup');
  const setupConfig = configFile('setup', {
    bundle: { active: true, targets: ['dmg'], macOS },
  });
  run(
    `pnpm --filter @uwussh/setup tauri build --bundles dmg${targetArg} --config "${setupConfig}"`,
    {
      UWUSSH_SETUP_PAYLOAD: apps,
    },
  );
  const dmg = join(out, `UwUSSH-Setup-${version}-macos-${arch()}.dmg`);
  copyFileSync(
    only(join(bundles, 'dmg'), (name) => name.endsWith('.dmg'), 'disk image'),
    dmg,
  );
  const update = join(out, `UwUSSH-Setup-${version}-macos-${arch()}-update`);
  copyFileSync(join(release, 'uwussh-setup'), update);
  sign(update);
  produced.push(dmg, update);
  rmSync(apps, { recursive: true, force: true });
} else {
  // AppImages, unpacked: the installed app runs from its AppDir and needs no
  // FUSE; the setup stays one AppImage that brings its own WebKit.
  const apps = mkdtempSync(join(tmpdir(), 'uwussh-apps-'));
  const unpackAppImage = (image, name) => {
    const work = mkdtempSync(join(tmpdir(), 'uwussh-appimage-'));
    execFileSync('chmod', ['+x', image]);
    execFileSync(image, ['--appimage-extract'], { cwd: work, stdio: 'ignore' });
    renameSync(join(work, 'squashfs-root'), join(apps, name));
    rmSync(work, { recursive: true, force: true });
  };

  console.log(`\n▸ Building UwUSSH ${version}`);
  run(`pnpm --filter @uwussh/desktop tauri build --bundles appimage,deb${targetArg}`);
  unpackAppImage(
    only(join(bundles, 'appimage'), (name) => name.endsWith('.AppImage'), 'AppImage of UwUSSH'),
    'UwUSSH',
  );
  const deb = join(out, `UwUSSH-${version}-linux-${arch()}.deb`);
  copyFileSync(
    only(join(bundles, 'deb'), (name) => name.endsWith('.deb'), 'deb'),
    deb,
  );
  // The next build bundles into the same folders.
  rmSync(join(bundles, 'appimage'), { recursive: true, force: true });

  console.log(`\n▸ Building UwUKeygen ${version}`);
  run(`pnpm --filter @uwussh/keygen tauri build --bundles appimage${targetArg}`);
  unpackAppImage(
    only(join(bundles, 'appimage'), (name) => name.endsWith('.AppImage'), 'AppImage of UwUKeygen'),
    'UwUKeygen',
  );
  rmSync(join(bundles, 'appimage'), { recursive: true, force: true });

  console.log('\n▸ Packing both into the setup');
  const setupConfig = configFile('setup', { bundle: { active: true, targets: ['appimage'] } });
  run(
    `pnpm --filter @uwussh/setup tauri build --bundles appimage${targetArg} --config "${setupConfig}"`,
    { UWUSSH_SETUP_PAYLOAD: apps },
  );
  const image = join(out, `UwUSSH-Setup-${version}-linux-${arch()}.AppImage`);
  copyFileSync(
    only(join(bundles, 'appimage'), (name) => name.endsWith('.AppImage'), 'AppImage of the setup'),
    image,
  );
  execFileSync('chmod', ['+x', image]);
  sign(image);
  produced.push(image, deb);
  rmSync(apps, { recursive: true, force: true });
}

if (!signingKey) {
  console.log('\n▸ No TAURI_SIGNING_PRIVATE_KEY, so nothing is signed for the updater:');
  console.log('  fine for trying out and for CI; `pnpm release` signs.');
}
for (const file of produced) console.log(`✧ ${file}`);
