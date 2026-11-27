// Après le build d'une release : écrit dans <dist> la fiche de la release (release.json) et les
// empreintes (SHA256SUMS) de chacun de ses fichiers. Le Core les lit pour installer et mettre à jour
// (voir docs/RELEASES.md) : il refuse un fichier dont l'empreinte ne correspond pas.
// Usage : node scripts/release-manifest.mjs <dist> <version> [nom] [strata-app.toml]
// Pour une appli : son identifiant comme nom, et sa fiche, d'où vient la plage de versions du Core
// acceptée (`requires.core`). Son archive s'appelle <id>-<version>-linux-x86_64.tar.gz.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const [dist, version, name = 'strata-core', appToml] = process.argv.slice(2);
if (!dist || !version) {
  console.error('usage: node scripts/release-manifest.mjs <dist> <version> [nom] [strata-app.toml]');
  process.exit(2);
}

/** Ce qu'est un fichier, d'après son nom. */
function kind(file) {
  if (/^strata-ui-.+\.tgz$/.test(file)) return 'ui-package';
  if (/^strata-core-.+-linux-x86_64\.tar\.gz$/.test(file)) return 'core-linux-x86_64';
  if (/^strata-install-.+-linux-x86_64$/.test(file)) return 'installer-linux-x86_64';
  if (file === 'install.sh') return 'installer-bootstrap';
  if (/-linux-x86_64\.tar\.gz$/.test(file)) return 'app-linux-x86_64';
  return 'other';
}

const skip = new Set(['release.json', 'SHA256SUMS']);
const files = fs.readdirSync(dist).filter((f) => !skip.has(f) && fs.statSync(path.join(dist, f)).isFile()).sort();
if (!files.length) {
  console.error(`aucun fichier dans ${dist}`);
  process.exit(1);
}
const assets = files.map((f) => {
  const buf = fs.readFileSync(path.join(dist, f));
  return { name: f, kind: kind(f), size: buf.length, sha256: crypto.createHash('sha256').update(buf).digest('hex') };
});

/** La plage de versions du Core acceptée, lue dans la section [requires] de la fiche de l'appli. */
function requires() {
  if (!appToml) return {};
  const text = fs.readFileSync(appToml, 'utf8');
  const section = text.split(/^\[requires\]\s*$/m)[1]?.split(/^\[/m)[0] ?? '';
  const core = /^\s*core\s*=\s*"([^"]+)"/m.exec(section)?.[1];
  if (!core) {
    console.error(`${appToml} : pas de core = "…" dans [requires]`);
    process.exit(1);
  }
  const id = /^\s*id\s*=\s*"([^"]+)"/m.exec(text)?.[1];
  const v = /^\s*version\s*=\s*"([^"]+)"/m.exec(text)?.[1];
  if (id !== name || v !== version) {
    console.error(`${appToml} dit ${id} ${v}, la release ${name} ${version}`);
    process.exit(1);
  }
  return { core };
}

const manifest = {
  name,
  version,
  tag: `v${version}`,
  commit: process.env.GITHUB_SHA ?? null,
  date: new Date().toISOString(),
  // Pour une appli : la plage de versions du Core acceptée, reprise de sa strata-app.toml.
  requires: requires(),
  assets,
};
fs.writeFileSync(path.join(dist, 'release.json'), JSON.stringify(manifest, null, 2) + '\n');
fs.writeFileSync(path.join(dist, 'SHA256SUMS'), assets.map((a) => `${a.sha256}  ${a.name}`).join('\n') + '\n');
for (const a of assets) console.log(`${a.kind.padEnd(24)} ${a.name} (${a.size} octets)`);
console.log(`release.json et SHA256SUMS écrits dans ${dist}`);
