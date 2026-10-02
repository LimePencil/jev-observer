import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';

export default defineConfig({
  plugins: [react(), {
    name: 'observer-license',
    generateBundle() {
      this.emitFile({
        type: 'asset',
        fileName: 'licenses/jev-observer-MIT.txt',
        source: readFileSync(new URL('../LICENSE', import.meta.url), 'utf8'),
      });
    },
  }],
  server: { proxy: { '/api': {
    target: 'http://127.0.0.1:8765',
    changeOrigin: true,
    configure(proxy) {
      proxy.on('proxyReq', (proxyRequest, request) => {
        // Only translate the local dev server's own Origin; foreign origins
        // must reach the backend unchanged so its mutation guard rejects them.
        if (request.headers.origin === `http://${request.headers.host}`) proxyRequest.setHeader('Origin', 'http://127.0.0.1:8765');
      });
    },
  } } },
  build: { target: 'es2022', sourcemap: false },
});
