/**
 * The English catalogue: German string → English string, one file per area of
 * the app. See `lib/i18n.ts`.
 */

import app from './app.json';
import assist from './assist.json';
import files from './files.json';
import hosts from './hosts.json';
import keygen from './keygen.json';
import onboarding from './onboarding.json';
import settings from './settings.json';
import tunnels from './tunnels.json';

export const EN: Readonly<Record<string, string>> = {
  ...app,
  ...hosts,
  ...files,
  ...settings,
  ...keygen,
  ...tunnels,
  ...assist,
  ...onboarding,
};
