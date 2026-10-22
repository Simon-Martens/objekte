import { defineConfig } from 'vite';
export default defineConfig({
  base: '/assets/',
  server: { port: 5173, strictPort: true, cors: { origin: /^http:\/\/(localhost|127\.0\.0\.1):\d+$/ } },
  build: { manifest: true, rolldownOptions: { input: 'src/main.ts' } },
});
