import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import './styles/app.css';
import './styles/tokens.css';

// Dark by default, as the concept says; the light theme is equally maintained
// and gets a real toggle once settings exist.
document.documentElement.dataset.theme = 'dark';

// Animations follow the system until Settings → Appearance can override it.
if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
  document.documentElement.dataset.motion = 'reduced';
}

const root = document.getElementById('root');
if (!root) throw new Error('#root missing from index.html');

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
