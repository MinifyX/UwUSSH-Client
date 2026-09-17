// Writes the update feeds installed apps look for on the `updates` branch:
//
//   node scripts/release-feeds.mjs <version> <out-dir> --setup <exe>
//
// The setup needs its updater signature (.sig) next to it. Versions with a
// suffix (-beta.1) only go into the Beta feed, plain versions into Stable and
// Beta.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPOSITORY = 'MinifyX/UwUSSH-Client';
export const FEED_BRANCH = 'updates';

export const downloadUrl = (version, name) =>
  `https://github.com/${REPOSITORY}/releases/download/v${version}/${name}`;

/** Feed file name → content, for a setup ({ name, signature }). */
export function releaseFeeds({ version, notes, setup, date = new Date() }) {
  const channels = version.includes('-') ? ['beta'] : ['stable', 'beta'];
  const feeds = {};
  for (const channel of channels) {
    // Tauri's updater format; the notes stay a JSON string the app reads per language.
    feeds[`${channel}.json`] = {
      version,
      notes: JSON.stringify(notes),
      pub_date: date.toISOString().replace(/\.\d+Z$/, 'Z'),
      platforms: {
        'windows-x86_64': { signature: setup.signature, url: downloadUrl(version, setup.name) },
      },
    };
  }
  return feeds;
}

if (import.meta.main) {
  const [version, out] = process.argv.slice(2);
  const index = process.argv.indexOf('--setup');
  const setupPath = index > 0 ? process.argv[index + 1] : undefined;
  if (!version || !out || !setupPath) {
    console.error('Usage: node scripts/release-feeds.mjs <version> <out-dir> --setup <exe>');
    process.exit(1);
  }
  const root = join(dirname(fileURLToPath(import.meta.url)), '..');
  const notes = JSON.parse(readFileSync(join(root, 'release-notes', `${version}.json`), 'utf8'));
  const feeds = releaseFeeds({
    version,
    notes,
    setup: {
      name: basename(setupPath),
      signature: readFileSync(`${setupPath}.sig`, 'utf8').trim(),
    },
  });
  mkdirSync(out, { recursive: true });
  for (const [name, feed] of Object.entries(feeds)) {
    writeFileSync(join(out, name), `${JSON.stringify(feed, null, 2)}\n`);
    console.log(`${name}: ${feed.version}`);
  }
}
