import { AppLabels } from '@desktop/components/AppLabels';
import { prepareDocument } from '@desktop/lib/appearance';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './styles.css';

// /boot.js put theme, contrast and motion on <html> already (dark by default,
// as in UwUSSH); the font and the language follow the shared settings.
prepareDocument();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppLabels>
      <App />
    </AppLabels>
  </StrictMode>,
);
