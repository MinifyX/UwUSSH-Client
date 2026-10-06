import { UwuLabels } from '@uwusuite/design';
import type { ReactNode } from 'react';
import { useLanguage } from '../lib/i18n';

/**
 * The words @uwusuite/design's components say themselves (a dialog's ×, the
 * window controls on Windows and Linux) in the app's language. The package
 * speaks German unless told otherwise.
 */
export function AppLabels({ children }: { children: ReactNode }) {
  return <UwuLabels labels={useLanguage()}>{children}</UwuLabels>;
}
