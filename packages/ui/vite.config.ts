// `npm run dev` : le banc d'essai (index.html → playground/).
// `npm run build` : la bibliothèque dans dist/ (strata-ui.js, strata-ui.css, polices).
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  build: {
    lib: { entry: 'src/index.ts', formats: ['es'], fileName: () => 'strata-ui.js', cssFileName: 'strata-ui' },
    assetsInlineLimit: 0, // les polices restent des fichiers à part
    rolldownOptions: {
      external: ['react', 'react-dom', 'react/jsx-runtime', 'zustand', 'lucide-react'],
    },
  },
});
