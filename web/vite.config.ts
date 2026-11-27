// L'interface du Core. `npm run dev` : avec un serveur strata-core lancé à côté (STRATA_SERVER, sinon
// http://localhost:8080). `npm run build` : dist/, embarqué dans le binaire strata-core.
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const server = process.env.STRATA_SERVER ?? 'http://localhost:8080';

export default defineConfig({
  plugins: [react()],
  // @strata/ui est lié depuis packages/ui : un seul React, un seul zustand.
  resolve: { dedupe: ['react', 'react-dom', 'zustand', 'lucide-react'] },
  build: { assetsDir: 'static' },
  server: { proxy: { '/api': server } },
});
