// Construit docs/maquettes/cockpit.html (un seul fichier, sans rien à charger) depuis src/cockpit.html :
// le vrai CSS de LiveCut (styles.css + styles-light.css), la police Inter, la signature F et les icônes lucide utilisées.
// Usage : node docs/maquettes/build.mjs <Strata-Cut/web/src> <lucide-static/icons>
import fs from 'node:fs';
import path from 'node:path';

const here = path.dirname(new URL(import.meta.url).pathname);
const [cutSrc, iconsDir] = process.argv.slice(2);
if (!cutSrc || !iconsDir) {
  console.error('usage: node build.mjs <Strata-Cut/web/src> <lucide-static/icons>');
  process.exit(2);
}
let html = fs.readFileSync(path.join(here, 'src', 'cockpit.html'), 'utf8');

// CSS de LiveCut, polices en data: (le fichier s'ouvre seul).
let css = fs.readFileSync(path.join(cutSrc, 'styles.css'), 'utf8') + '\n' + fs.readFileSync(path.join(cutSrc, 'styles-light.css'), 'utf8');
css = css.replace(/url\('\.\/fonts\/(inter-\d+\.woff2)'\)/g, (_, f) => `url(data:font/woff2;base64,${fs.readFileSync(path.join(cutSrc, 'fonts', f)).toString('base64')})`);
html = html.replace('/*CSS*/', () => css);

// Le F de la signature, tel que dans ui/Brand.tsx.
const F = fs.readFileSync(path.join(cutSrc, 'ui', 'Brand.tsx'), 'utf8').match(/const F = '([^']+)'/)[1];
html = html.replace('/*F*/', F);

// Icônes lucide nommées dans la maquette : ic('nom'…), [.., 'nom', ..] des listes.
const names = new Set([...html.matchAll(/'([a-z0-9]+(?:-[a-z0-9]+)*)'/g)].map((m) => m[1]).filter((n) => fs.existsSync(path.join(iconsDir, `${n}.svg`))));
const map = {};
for (const n of names) {
  const src = fs.readFileSync(path.join(iconsDir, `${n}.svg`), 'utf8');
  map[n] = src.slice(src.indexOf('>', src.indexOf('<svg')) + 1, src.lastIndexOf('</svg>')).replace(/\s*\n\s*/g, '');
}
html = html.replace('/*ICONS*/{}', JSON.stringify(map));

fs.writeFileSync(path.join(here, 'cockpit.html'), html);
console.log(`cockpit.html : ${names.size} icônes, ${(html.length / 1024).toFixed(0)} Ko`);
