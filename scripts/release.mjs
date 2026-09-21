// Publishes the UwUSSH version in tauri.conf.json, for Windows, macOS and
// Linux, from this PC.
//
//   pnpm release                build and sign the Windows setup, fetch and sign
//                               the macOS and Linux setups CI built for the tag,
//                               check everything, publish it
//   pnpm release --no-build     use the Windows setup already in target/installers
//   pnpm release --windows-only leave macOS and Linux out, when CI can't help
//
// Needs a clean tree whose HEAD carries the pushed tag v<version>,
// release-notes/<version>.json, the GitHub CLI signed in with write access and
// the update signing key: TAURI_SIGNING_PRIVATE_KEY + TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
// or a folder with uwussh-update.key and PASSWORT.txt in UWUSSH_UPDATE_KEY_DIR
// (default: Documents\UwUSSH-Update-Schluessel).
//
// The key never leaves this machine: CI (.github/workflows/installers.yml)
// builds the macOS and Linux setups unsigned when the tag is pushed, and this
// script downloads them and signs the files the updater runs here.
//
// Creates the GitHub release with every setup and updates the feeds on the
// `updates` branch, creating that branch the first time.

import { execFileSync } from 'node:child_process';
import { createHash, createPublicKey, verify } from 'node:crypto';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { FEED_BRANCH, REPOSITORY, releaseFeeds } from './release-feeds.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const git = (args, cwd = root) => execFileSync('git', args, { cwd, encoding: 'utf8' }).trim();
const fail = (message) => {
  console.error(`\n✗ ${message}`);
  process.exit(1);
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let token; // GitHub token for the API, looked up once
const build = !process.argv.includes('--no-build');
const windowsOnly = process.argv.includes('--windows-only');
const conf = JSON.parse(readFileSync(join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
const version = conf.version;
const tag = `v${version}`;
const installers = join(root, 'target', 'installers');
const windowsName = `UwUSSH-Setup-${version}.exe`;
const windowsSetup = join(installers, windowsName);

/**
 * What the release carries: each file, and for the ones an installed app
 * updates itself with, the feed's platform key.
 */
const PLATFORMS = [
  { artifact: null, file: windowsName, feed: 'windows-x86_64' },
  {
    artifact: 'installers-macos-arm64',
    file: `UwUSSH-Setup-${version}-macos-arm64-update`,
    feed: 'darwin-aarch64',
  },
  { artifact: 'installers-macos-arm64', file: `UwUSSH-Setup-${version}-macos-arm64.dmg` },
  {
    artifact: 'installers-macos-x64',
    file: `UwUSSH-Setup-${version}-macos-x64-update`,
    feed: 'darwin-x86_64',
  },
  { artifact: 'installers-macos-x64', file: `UwUSSH-Setup-${version}-macos-x64.dmg` },
  {
    artifact: 'installers-linux-x64',
    file: `UwUSSH-Setup-${version}-linux-x64.AppImage`,
    feed: 'linux-x86_64',
  },
  { artifact: 'installers-linux-x64', file: `UwUSSH-${version}-linux-x64.deb` },
].filter((entry) => !windowsOnly || entry.artifact === null);

console.log(`\n▸ Checking UwUSSH ${version}`);
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(version)) fail(`Unexpected version ${version}`);
if (git(['status', '--porcelain', '--untracked-files=no'])) {
  fail('The working tree has uncommitted changes.');
}
const head = git(['rev-parse', 'HEAD']);
let tagged = '';
try {
  tagged = git(['rev-parse', `${tag}^{commit}`]);
} catch {
  fail(`Tag ${tag} is missing. Tag the release commit and push the tag first.`);
}
if (tagged !== head) fail(`HEAD isn't ${tag}. Check out the tag first.`);
const pushed = git(['ls-remote', '--tags', 'origin', `refs/tags/${tag}`]).split(/\s/)[0];
if (pushed !== git(['rev-parse', `refs/tags/${tag}`])) fail(`Push ${tag} first.`);
if (!ghToken()) fail('Sign in to the GitHub CLI first (gh auth login).');
if (await github(`releases/tags/${tag}`)) fail(`${tag} is already released.`);
const notesFile = join(root, 'release-notes', `${version}.json`);
if (!existsSync(notesFile)) fail(`release-notes/${version}.json is missing.`);
const notes = JSON.parse(readFileSync(notesFile, 'utf8'));
if (typeof notes.de !== 'string' || typeof notes.en !== 'string') {
  fail('The release notes need de and en.');
}
const key = signingKey();

if (build) {
  console.log('\n▸ Building and signing the Windows setup');
  execFileSync(process.execPath, [join(root, 'scripts', 'build-setup.mjs')], {
    cwd: root,
    stdio: 'inherit',
    env: { ...process.env, ...key },
  });
} else if (!existsSync(windowsSetup)) {
  fail(`${windowsSetup} is missing. Run without --no-build.`);
} else if (statSync(windowsSetup).mtimeMs < Number(git(['log', '-1', '--format=%ct'])) * 1000) {
  fail('The setup in target/installers is older than the release commit. Run without --no-build.');
}

const work = mkdtempSync(join(tmpdir(), 'uwussh-release-'));
try {
  const files = new Map([[windowsName, windowsSetup]]);
  if (!windowsOnly) {
    console.log('\n▸ Fetching the macOS and Linux setups CI built for this tag');
    const ci = join(work, 'ci');
    const run = await ciRun(head);
    execFileSync('gh', ['run', 'download', String(run), '--repo', REPOSITORY, '--dir', ci], {
      stdio: 'inherit',
    });
    for (const entry of PLATFORMS.filter((p) => p.artifact)) {
      const from = join(ci, entry.artifact, entry.file);
      if (!existsSync(from)) fail(`CI left no ${entry.file} in ${entry.artifact}.`);
      const to = join(installers, entry.file);
      mkdirSync(installers, { recursive: true });
      copyFileSync(from, to);
      files.set(entry.file, to);
    }
    console.log('\n▸ Signing what the updater runs on macOS and Linux');
    for (const entry of PLATFORMS.filter((p) => p.artifact && p.feed)) {
      const file = files.get(entry.file);
      rmSync(`${file}.sig`, { force: true });
      execFileSync(
        'pnpm',
        ['--filter', '@uwussh/desktop', 'exec', 'tauri', 'signer', 'sign', file],
        {
          cwd: root,
          stdio: 'inherit',
          shell: process.platform === 'win32',
          env: { ...process.env, ...key },
        },
      );
    }
  }

  console.log('\n▸ Checking the signatures against the updater key');
  const signed = {};
  for (const entry of PLATFORMS.filter((p) => p.feed)) {
    const file = files.get(entry.file);
    if (!existsSync(`${file}.sig`)) fail(`${file}.sig is missing: it wasn't signed.`);
    const signature = readFileSync(`${file}.sig`, 'utf8').trim();
    checkSignature(readFileSync(file), signature, conf.plugins.updater.pubkey, entry.file);
    signed[entry.feed] = { name: entry.file, signature };
    files.set(`${entry.file}.sig`, `${file}.sig`);
    console.log(`  ✓ ${entry.file}`);
  }

  // A checksum for each download, LF-only so `sha256sum -c` reads it.
  const sums = join(work, 'SHA256SUMS.txt');
  const lines = [...files.entries()].map(
    ([name, path]) => `${createHash('sha256').update(readFileSync(path)).digest('hex')}  ${name}`,
  );
  writeFileSync(sums, `${lines.join('\n')}\n`);
  files.set('SHA256SUMS.txt', sums);

  console.log(`\n▸ Creating the release on ${REPOSITORY}`);
  const notesPath = join(work, 'notes.md');
  writeFileSync(notesPath, releaseBody());
  const channel = version.includes('-') ? '--prerelease' : '--latest';
  execFileSync(
    'gh',
    [
      'release',
      'create',
      tag,
      ...files.values(),
      '--repo',
      REPOSITORY,
      '--verify-tag',
      '--title',
      `UwUSSH ${version}`,
      '--notes-file',
      notesPath,
      channel,
    ],
    { stdio: 'inherit' },
  );

  const feeds = releaseFeeds({ version, notes, setups: signed });
  console.log(`\n▸ Updating ${Object.keys(feeds).join(', ')} on ${FEED_BRANCH}`);
  const dir = join(work, 'feeds');
  const remote = `https://github.com/${REPOSITORY}.git`;
  const exists = git(['ls-remote', '--heads', remote, FEED_BRANCH], work) !== '';
  if (exists) {
    git(['clone', '-q', '--depth', '1', '--branch', FEED_BRANCH, remote, dir], work);
  } else {
    // First release: the branch holds nothing but the feeds.
    git(['init', '-q', '-b', FEED_BRANCH, dir], work);
    git(['remote', 'add', 'origin', remote], dir);
    writeFileSync(
      join(dir, 'README.md'),
      '# Update feeds\n\nWritten by `pnpm release` on the main branch. Installed UwUSSH apps read ' +
        '`stable.json` or `beta.json` from here, depending on their update channel.\n',
    );
  }
  for (const [name, feed] of Object.entries(feeds)) {
    writeFileSync(join(dir, name), `${JSON.stringify(feed, null, 2)}\n`);
  }
  git(['add', '--all'], dir);
  git(
    [
      '-c',
      'user.name=UwUSSH Release',
      '-c',
      'user.email=release@uwussh.invalid',
      'commit',
      '-qm',
      `UwUSSH ${version}`,
    ],
    dir,
  );
  execFileSync('git', ['push', '-q', 'origin', `HEAD:${FEED_BRANCH}`], {
    cwd: dir,
    stdio: 'inherit',
  });

  console.log("\n▸ Checking what's online");
  const release = await github(`releases/tags/${tag}`);
  for (const [name, path] of files) {
    const asset = release?.assets?.find((a) => a.name === name);
    if (!asset || asset.state !== 'uploaded') fail(`The release has no ${name}.`);
    if (asset.size !== statSync(path).size) fail(`The published ${name} has a different size.`);
  }
  const feedName = version.includes('-') ? 'beta.json' : 'stable.json';
  const file = await github(`contents/${feedName}?ref=${FEED_BRANCH}`);
  const update = file && JSON.parse(Buffer.from(file.content, 'base64').toString('utf8'));
  if (update?.version !== version) fail(`${feedName} wasn't updated.`);
  for (const [platform, setup] of Object.entries(signed)) {
    const entry = update.platforms?.[platform];
    const asset = release.assets.find((a) => a.name === setup.name);
    if (entry?.signature !== setup.signature) fail(`${feedName} has no ${platform}.`);
    if (entry.url !== asset.browser_download_url)
      fail(`${feedName} points elsewhere for ${platform}.`);
  }
  console.log(`\n✧ UwUSSH ${version} is out: ${release.html_url}`);
  console.log(
    `  ${feedName} updated for ${Object.keys(signed).join(', ')}${version.includes('-') ? ' (Beta channel)' : ''}`,
  );
} finally {
  rmSync(work, { recursive: true, force: true });
}

/** The release page: what changed, then how to install on each system. */
function releaseBody() {
  const guide = `https://github.com/${REPOSITORY}/blob/main/docs/install.md`;
  const has = (name) => PLATFORMS.some((p) => p.file === name);
  const de = [
    `### Windows 10/11 (64 Bit)\n\`${windowsName}\` herunterladen und starten. Warnt Windows („Der Computer wurde durch Windows geschützt“): **Weitere Informationen → Trotzdem ausführen**.`,
  ];
  const en = [
    `### Windows 10/11 (64-bit)\nDownload \`${windowsName}\` and run it. If Windows warns that it "protected your PC": **More info → Run anyway**.`,
  ];
  if (has(`UwUSSH-Setup-${version}-macos-arm64.dmg`)) {
    de.push(
      `### macOS 11 oder neuer\nApple-Chip (M1 und neuer): \`UwUSSH-Setup-${version}-macos-arm64.dmg\`, Intel: \`UwUSSH-Setup-${version}-macos-x64.dmg\`. Öffnen und **UwUSSH Setup** starten. UwUSSH ist nicht bei Apple notarisiert: sagt macOS, es könne das Programm nicht prüfen, unter **Systemeinstellungen → Datenschutz & Sicherheit → Trotzdem öffnen** freigeben.`,
    );
    en.push(
      `### macOS 11 or newer\nApple silicon (M1 and later): \`UwUSSH-Setup-${version}-macos-arm64.dmg\`, Intel: \`UwUSSH-Setup-${version}-macos-x64.dmg\`. Open it and start **UwUSSH Setup**. UwUSSH isn't notarized by Apple: if macOS says it can't check the app, allow it under **System Settings → Privacy & Security → Open Anyway**.`,
    );
  }
  if (has(`UwUSSH-Setup-${version}-linux-x64.AppImage`)) {
    de.push(
      `### Linux (x86_64)\n\`UwUSSH-Setup-${version}-linux-x64.AppImage\` herunterladen, ausführbar machen (\`chmod +x\`) und starten – installiert nach \`~/.local/share/uwussh\`, mit Eintrag im Anwendungsmenü und automatischen Updates. Ohne FUSE: \`./UwUSSH-Setup-…AppImage --appimage-extract-and-run\`. Lieber ein Paket? \`UwUSSH-${version}-linux-x64.deb\` (ohne Setup und ohne automatische Updates).`,
    );
    en.push(
      `### Linux (x86_64)\nDownload \`UwUSSH-Setup-${version}-linux-x64.AppImage\`, make it executable (\`chmod +x\`) and run it – it installs into \`~/.local/share/uwussh\`, with a menu entry and automatic updates. Without FUSE: \`./UwUSSH-Setup-…AppImage --appimage-extract-and-run\`. Rather have a package? \`UwUSSH-${version}-linux-x64.deb\` (no setup, no automatic updates).`,
    );
  }
  return [
    `## Deutsch\n\n${notes.de}\n`,
    `## English\n\n${notes.en}\n`,
    `## Installieren\n\n${de.join('\n\n')}\n\n[Anleitung](${guide})\n`,
    `## Install\n\n${en.join('\n\n')}\n\n[Install guide](${guide})\n`,
    `Prüfsummen · checksums: \`SHA256SUMS.txt\`. Die \`.sig\`-Dateien sind die Signaturen des Updaters · the \`.sig\` files are the updater's signatures.\n`,
  ].join('\n');
}

/** The Installers run for this commit, once it has finished green. */
async function ciRun(sha) {
  const started = Date.now();
  let announced = false;
  for (;;) {
    const runs = JSON.parse(
      execFileSync(
        'gh',
        [
          'run',
          'list',
          '--repo',
          REPOSITORY,
          '--workflow',
          'installers.yml',
          '--commit',
          sha,
          '--json',
          'databaseId,status,conclusion,event',
          '--limit',
          '10',
        ],
        { encoding: 'utf8' },
      ),
    );
    const done = runs.find((run) => run.status === 'completed' && run.conclusion === 'success');
    if (done) return done.databaseId;
    const running = runs.find((run) => run.status !== 'completed');
    if (!running && runs.length > 0 && Date.now() - started > 60_000) {
      fail(
        `The Installers run for ${sha.slice(0, 7)} failed. Fix it, or release with --windows-only.`,
      );
    }
    if (Date.now() - started > 90 * 60_000) fail('Gave up waiting for CI after 90 minutes.');
    if (!announced) {
      console.log('  waiting for CI to finish the macOS and Linux builds…');
      announced = true;
    }
    await sleep(30_000);
  }
}

/** The update signing key from the environment or the key folder. */
function signingKey() {
  if (process.env.TAURI_SIGNING_PRIVATE_KEY) {
    return {
      TAURI_SIGNING_PRIVATE_KEY: process.env.TAURI_SIGNING_PRIVATE_KEY,
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '',
    };
  }
  const folder =
    process.env.UWUSSH_UPDATE_KEY_DIR || join(homedir(), 'Documents', 'UwUSSH-Update-Schluessel');
  const file = join(folder, 'uwussh-update.key');
  if (!existsSync(file)) {
    fail('No signing key: set TAURI_SIGNING_PRIVATE_KEY(_PASSWORD) or UWUSSH_UPDATE_KEY_DIR.');
  }
  const password = join(folder, 'PASSWORT.txt');
  return {
    TAURI_SIGNING_PRIVATE_KEY: readFileSync(file, 'utf8').trim(),
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: existsSync(password)
      ? readFileSync(password, 'utf8').trim()
      : '',
  };
}

/** Verifies a Tauri updater signature (base64 minisign) the way installed apps do. */
function checkSignature(file, signatureBase64, pubkeyBase64, expectedName) {
  const lines = (text) => Buffer.from(text, 'base64').toString('utf8').split(/\r?\n/);
  const pub = Buffer.from(lines(pubkeyBase64)[1], 'base64');
  const [, signatureLine, trustedLine, globalLine] = lines(signatureBase64);
  const sig = Buffer.from(signatureLine, 'base64');
  if (pub.length !== 42 || sig.length !== 74) fail('Malformed key or signature.');
  if (!sig.subarray(2, 10).equals(pub.subarray(2, 10))) {
    fail(`${expectedName} was signed with a different key.`);
  }
  const publicKey = createPublicKey({
    key: { kty: 'OKP', crv: 'Ed25519', x: pub.subarray(10).toString('base64url') },
    format: 'jwk',
  });
  const algorithm = sig.subarray(0, 2).toString('latin1');
  const signed = algorithm === 'ED' ? createHash('blake2b512').update(file).digest() : file;
  if (!verify(null, signed, publicKey, sig.subarray(10))) {
    fail(`The signature doesn't match ${expectedName}.`);
  }
  const trusted = Buffer.from(trustedLine.replace(/^trusted comment: /, ''), 'utf8');
  if (
    !verify(
      null,
      Buffer.concat([sig.subarray(10), trusted]),
      publicKey,
      Buffer.from(globalLine, 'base64'),
    )
  ) {
    fail(`The trusted comment of ${expectedName}'s signature doesn't verify.`);
  }
  // Installed apps refuse a setup whose signature names another file: that is
  // what ties the feed's version to the signed setup.
  const names = trusted
    .toString('utf8')
    .split('\t')
    .map((part) => part.trim().replace(/^file:/, ''));
  if (!names.includes(expectedName)) fail(`The signature doesn't name ${expectedName}.`);
}

async function github(path) {
  const headers = { accept: 'application/vnd.github+json', 'user-agent': 'uwussh-release' };
  token ??= process.env.GH_TOKEN || process.env.GITHUB_TOKEN || ghToken();
  if (token) headers.authorization = `Bearer ${token}`;
  const response = await fetch(`https://api.github.com/repos/${REPOSITORY}/${path}`, { headers });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`GitHub answered ${response.status} for ${path}`);
  return response.json();
}

function ghToken() {
  try {
    return execFileSync('gh', ['auth', 'token'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return '';
  }
}
