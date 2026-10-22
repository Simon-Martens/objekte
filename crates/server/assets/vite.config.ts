import { fileURLToPath } from 'node:url';
import { relative, isAbsolute } from 'node:path';
import { defineConfig } from 'vite';

const templates = fileURLToPath(new URL('../templates', import.meta.url));

export default defineConfig({
  base: '/assets/',
  server: {
    port: 5173,
    strictPort: true,
    origin: process.env.EDITOR_VITE_URL || 'http://127.0.0.1:5173',
    cors: { origin: /^http:\/\/(localhost|127\.0\.0\.1):\d+$/ },
  },
  plugins: [{
    name: 'editor-template-reload',
    apply: 'serve',
    configureServer(server) {
      server.watcher.add(templates);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const reload = (file: string) => {
        const path = relative(templates, file);
        if (path.startsWith('..') || isAbsolute(path) || !path.endsWith('.html')) return;
        clearTimeout(timer);
        timer = setTimeout(() => server.ws.send({ type: 'full-reload', path: '*' }), 100);
      };
      for (const event of ['add', 'change', 'unlink'] as const) server.watcher.on(event, reload);
      server.httpServer?.once('close', () => {
        clearTimeout(timer);
        for (const event of ['add', 'change', 'unlink'] as const) server.watcher.off(event, reload);
      });
    },
  }],
  build: { manifest: true, rolldownOptions: { input: 'src/main.ts' } },
});
