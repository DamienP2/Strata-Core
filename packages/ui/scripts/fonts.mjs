// Après `vite build` : la police à part, en fichiers (le mode bibliothèque de Vite l'aurait mise en base64
// dans le CSS, toutes graisses chargées d'office). Copie les WOFF2 et leur licence dans dist/fonts et met
// leurs @font-face en tête de dist/strata-ui.css ; l'appli qui importe le CSS les sert comme ses propres fichiers.
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const src = path.join(root, 'src', 'styles');
const dist = path.join(root, 'dist');
fs.mkdirSync(path.join(dist, 'fonts'), { recursive: true });
for (const f of fs.readdirSync(path.join(src, 'fonts'))) fs.copyFileSync(path.join(src, 'fonts', f), path.join(dist, 'fonts', f));
const css = path.join(dist, 'strata-ui.css');
fs.writeFileSync(css, fs.readFileSync(path.join(src, 'fonts.css'), 'utf8') + '\n' + fs.readFileSync(css, 'utf8'));
console.log('fonts: dist/fonts + @font-face in dist/strata-ui.css');
