import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { prepareDocument } from './lib/appearance';
import './styles/index.css';

// Dark by default, as the concept says (/boot.js put the theme on <html>
// already); Settings → Darstellung switches theme, contrast, motion and font.
prepareDocument();

const root = document.getElementById('root');
if (!root) throw new Error('#root missing from index.html');

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
