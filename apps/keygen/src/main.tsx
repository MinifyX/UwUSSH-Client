import '@fontsource-variable/manrope';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@desktop/components/nyu/nyu.css';
import { applyAppearance } from '@desktop/lib/settings';
import '@desktop/styles/app.css';
import '@desktop/styles/features.css';
import '@desktop/styles/tokens.css';
import { App } from './App';
import './keygen-app.css';

applyAppearance();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
