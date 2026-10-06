import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { AppLabels } from './components/AppLabels';
import { prepareDocument } from './lib/appearance';
import { loadFlavor } from './lib/flavor';
import './styles/index.css';

// Dark by default, as the concept says (/boot.js put the theme on <html>
// already); Settings → Darstellung switches theme, contrast, motion and font.
prepareDocument();

const root = document.getElementById('root');
if (!root) throw new Error('#root missing from index.html');

// Which build this is decides what the first render offers (lib/flavor.ts).
void loadFlavor().then(() =>
  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <AppLabels>
        <App />
      </AppLabels>
    </React.StrictMode>,
  ),
);
