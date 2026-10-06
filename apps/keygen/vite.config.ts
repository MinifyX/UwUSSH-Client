import { fileURLToPath, URL } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import { bootScript } from '@uwusuite/design';
import react from '@vitejs/plugin-react';
import { defineConfig, type Plugin } from 'vite';

/**
 * The theme, contrast and motion go onto <html> before the first paint, as in
 * UwUSSH (apps/desktop/vite.config.ts): the same settings key, so both windows
 * look alike, and dark until the person picks something. The CSP allows
 * scripts from 'self' only, so it is a file, /boot.js, not inline.
 */
function boot(): Plugin {
  const source = bootScript('uwussh.settings');
  const dark = source.replace('s.theme||"system"', 's.theme||"dark"');
  if (dark === source) throw new Error('bootScript changed: the dark default no longer applies');
  return {
    name: 'uwukeygen-boot',
    configureServer(server) {
      server.middlewares.use('/boot.js', (_request, response) => {
        response.setHeader('content-type', 'text/javascript');
        response.end(dark);
      });
    },
    generateBundle() {
      this.emitFile({ type: 'asset', fileName: 'boot.js', source: dark });
    },
  };
}

// UwUKeygen is UwUSSH's key generator as a window of its own. The panel, Nyu
// and the styles all come from the app, so both always look and work alike.
export default defineConfig({
  plugins: [react(), tailwindcss(), boot()],
  resolve: {
    alias: {
      '@desktop': fileURLToPath(new URL('../desktop/src', import.meta.url)),
    },
    dedupe: ['react', 'react-dom', '@tauri-apps/api', '@uwusuite/design'],
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
