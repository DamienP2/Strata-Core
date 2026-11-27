// Les erreurs du serveur (texte court en anglais) dans la langue de l'interface.
import { t } from '@strata/ui';

/** Les erreurs qui portent un nom ou un nombre : leur forme, et le texte à traduire. */
const SHAPED: [RegExp, string, string[]][] = [
  [/^(.+) needs the Core (.+) \(this one is (.+)\)$/, '{app} needs the Core {range} (this one is {core})', ['app', 'range', 'core']],
  [/^(.+) uses port (\d+) too: deactivate it first$/, '{app} uses port {port} too: deactivate it first', ['app', 'port']],
  [/^port (\d+) is the Core's own$/, 'Port {port} is the Core’s own', ['port']],
  [/^(.+) is started apart from the Core$/, '{app} is started apart from the Core', ['app']],
  // Installer une appli
  [/^no release of (.+) accepts the Core (.+) \(the latest, (.+), needs (.+)\)$/, 'No release of {repo} accepts the Core {core} (the latest, {version}, needs {range})', ['repo', 'core', 'version', 'range']],
  [/^(.+) has no Strata release$/, '{repo} has no Strata release', ['repo']],
  [/^(.+) was not found on GitHub \(a private repository needs a key\)$/, '{repo} was not found on GitHub (a private repository needs a key)', ['repo']],
  [/^GitHub refused the key of (.+)$/, 'GitHub refused the key of {repo}', ['repo']],
  [/^cannot reach GitHub: (.+)$/, 'Cannot reach GitHub: {why}', ['why']],
  [/^GitHub answered HTTP (\d+)$/, 'GitHub answered HTTP {code}', ['code']],
  [/^(.+) already uses port (\d+)$/, '{app} already uses port {port}', ['app', 'port']],
  [/^(.+) is already installed from (.+)$/, '{app} is already installed from {repo}', ['app', 'repo']],
  [/^(.+) is already being installed$/, '{repo} is already being installed', ['repo']],
  [/^(.+) is active: deactivate it first$/, '{app} is active: deactivate it first', ['app']],
  [/^not a GitHub repository: (.*)$/, 'Not a GitHub repository: {repo}', ['repo']],
  [/^the release is (.+) but its archive holds (.+)$/, 'The release is {release} but its archive holds {archive}', ['release', 'archive']],
  [/^(.+) now publishes (.+) instead of (.+)$/, '{repo} now publishes {other} instead of {app}', ['repo', 'other', 'app']],
  [/^(.+) was not installed from GitHub$/, '{app} was not installed from GitHub', ['app']],
  [/^cannot unpack the archive: (.+)$/, 'Cannot unpack the archive: {why}', ['why']],
  [/^the download was cut: (.+)$/, 'The download was cut: {why}', ['why']],
  // Ce serveur, HTTPS
  [/^cannot listen on port (\d+): (.+)$/, 'Cannot listen on port {port}: {why}', ['port', 'why']],
  [/^not a DNS name: (.*)$/, 'Not a DNS name: {name} (words of letters, digits and dashes, separated by dots)', ['name']],
  [/^not a time zone: (.*)$/, 'Not a time zone: {zone}', ['zone']],
  [/^port (\d+) is already the HTTP one$/, 'Port {port} is already the HTTP one', ['port']],
  [/^the certificate does not cover (.+)$/, 'The certificate does not cover {names}', ['names']],
  [/^the certificate expired on (.+)$/, 'The certificate expired on {date}', ['date']],
  // Matériel
  [/^SDI (\d+) of (.+) cannot be an input$/, 'SDI {n} of {model} cannot be an input', ['n', 'model']],
  [/^SDI (\d+) of (.+) cannot be an output$/, 'SDI {n} of {model} cannot be an output', ['n', 'model']],
  [/invalid socket address/, 'An address is written ip:port (239.10.2.1:5004)', []],
  [/^(.+) has (\d+) connectors$/, '{model} has {n} connectors', ['model', 'n']],
  [/^(.+): the genlock offset is out of range$/, '{card}: the genlock offset is out of range', ['card']],
  [/^(.+): the (red|blue) address must be multicast \(224\.0\.0\.0 to 239\.255\.255\.255\) with a port$/, '{flow}: the {leg} address must be multicast (224.0.0.0 to 239.255.255.255) with a port', ['flow', 'leg']],
  [/^(.+) is used by two flows$/, '{address} is used by two flows', ['address']],
  [/^(.+): in SMPTE 2022-7, a flow has an address on each network \(red and blue\)$/, '{flow}: in SMPTE 2022-7, a flow has an address on each network (red and blue)', ['flow']],
  [/^(.+): the media port is 1 or 2$/, '{flow}: the media port is 1 or 2', ['flow']],
  // Mises à jour
  [/^(.+) has no release of the Core$/, '{repo} has no release of the Core', ['repo']],
  [/^this Core is already the latest \((.+)\)$/, 'This Core is already the latest ({version})', ['version']],
  [/^the archive holds the Core (.+), not (.+)$/, 'The archive holds the Core {got}, not {want}', ['got', 'want']],
  [/^this archive holds the Core (.+); this one is already (.+)$/, 'This archive holds the Core {got}; this one is already {core}', ['got', 'core']],
  [/^the new Core does not start: (.+)$/, 'The new Core does not start: {why}', ['why']],
  [/^cannot replace (.+): (.+)$/, 'Cannot replace {path}: {why}', ['path', 'why']],
  [/^cannot keep the current Core: (.+)$/, 'Cannot keep the current Core: {why}', ['why']],
  [/^release (.+) has no Core archive$/, 'Release {tag} has no Core archive', ['tag']],
  // Entrées partagées
  [/^cannot open the input: (.+)$/, 'cannot open the input: {why}', ['why']],
  [/^cannot share the input \((.+)\): (.+)$/, 'cannot share the input ({path}): {why}', ['path', 'why']],
  [/^the certificate is valid only from (.+)$/, 'The certificate is valid only from {date}', ['date']],
];

export function friendly(e: unknown): string {
  const raw = ((e as Error)?.message ?? String(e)).trim();
  for (const [re, text, names] of SHAPED) {
    const m = re.exec(raw);
    if (m) return t(text, Object.fromEntries(names.map((n, i) => [n, m[i + 1]])));
  }
  return t(raw);
}
