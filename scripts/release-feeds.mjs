// The update feeds installed apps look for on the `updates` branch, written
// by `pnpm release`.
//
// Each platform gets the setup it runs to update itself: the Windows setup,
// the bare setup program from the macOS disk image, the Linux AppImage. Every
// one needs its updater signature (.sig). Versions with a suffix (-beta.1) only
// go into the Beta feed, plain versions into Stable and Beta.

export const REPOSITORY = 'MinifyX/UwUSSH-Client';
export const FEED_BRANCH = 'updates';

export const downloadUrl = (version, name) =>
  `https://github.com/${REPOSITORY}/releases/download/v${version}/${name}`;

/**
 * Feed file name → content.
 *
 * `setups` maps a Tauri platform key (`windows-x86_64`, `darwin-aarch64`,
 * `darwin-x86_64`, `linux-x86_64`) to `{ name, signature }`.
 */
export function releaseFeeds({ version, notes, setups, date = new Date() }) {
  const channels = version.includes('-') ? ['beta'] : ['stable', 'beta'];
  const platforms = Object.fromEntries(
    Object.entries(setups).map(([platform, setup]) => [
      platform,
      { signature: setup.signature, url: downloadUrl(version, setup.name) },
    ]),
  );
  const feeds = {};
  for (const channel of channels) {
    // Tauri's updater format; the notes stay a JSON string the app reads per language.
    feeds[`${channel}.json`] = {
      version,
      notes: JSON.stringify(notes),
      pub_date: date.toISOString().replace(/\.\d+Z$/, 'Z'),
      platforms,
    };
  }
  return feeds;
}
