import { fileURLToPath, URL } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// UwUKeygen is UwUSSH's key generator as a window of its own. The panel, Nyu
// and the styles all come from the app, so both always look and work alike.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@desktop': fileURLToPath(new URL('../desktop/src', import.meta.url)),
    },
    dedupe: ['react', 'react-dom', '@tauri-apps/api'],
  },
  clearScreen: false,
  server: {
    port: 1440,
    strictPort: true,
    fs: { allow: ['..'] },
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: 'chrome110',
    sourcemap: false,
  },
});
