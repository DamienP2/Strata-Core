// Les erreurs du serveur (texte court en anglais) dans la langue de l'interface.
import { t, toast } from '@strata/ui';
import { ApiError } from './api';

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
  // Stockage
  [/^cannot make the folder there: (.+)$/, 'cannot make the folder there: {why}', ['why']],
  [/^cannot write there: (.+)$/, 'Cannot write there: {why}', ['why']],
  [/^cannot read this folder: (.+)$/, 'Cannot read this folder: {why}', ['why']],
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
  // Flux réseau
  [/^ffmpeg is missing \((.+)\): run the installer again and choose Repair, or sudo apt install ffmpeg$/, 'ffmpeg is missing ({why}): run the installer again and choose Repair, or sudo apt install ffmpeg', ['why']],
  [/^MXL cannot open its folder \((.+)\): run the installer again and choose Repair, then restart the Core$/, 'MXL cannot open its folder ({why}): run the installer again and choose Repair, then restart the Core', ['why']],
  [/^MXL does not load \((.+)\): run the installer again and choose Update$/, 'MXL does not load ({why}): run the installer again and choose Update', ['why']],
  // Mires
  [/^(.+): another generator already sends to this destination$/, '{name}: another generator already sends to this destination', ['name']],
  [/^(.+): the colour is #RRGGBB$/, '{name}: the colour is #RRGGBB', ['name']],
  // Destinations NDI
  [/^(.+): bad NDI destination id$/, '{name}: bad NDI destination id', ['name']],
  [/^two NDI destinations have the id (.+)$/, 'Two NDI destinations have the id {id}', ['id']],
  [/^(.+): an NDI name has no brackets and at most 100 characters$/, '{name}: an NDI name has no brackets and at most 100 characters', ['name']],
  [/^two NDI destinations are called (.+)$/, 'Two NDI destinations are called {name}', ['name']],
  [/^(.+): NDI groups are names separated by commas$/, '{name}: NDI groups are names separated by commas', ['name']],
  [/^(.+): the SRT mode is caller, listener or rendezvous$/, '{name}: the SRT mode is caller, listener or rendezvous', ['name']],
  [/^(.+): the SRT passphrase has 10 to 79 characters$/, '{name}: the SRT passphrase has 10 to 79 characters', ['name']],
  [/^(.+): the SRT key length is 16, 24 or 32$/, '{name}: the SRT key length is 16, 24 or 32', ['name']],
  [/^(.+): the RTSP transport is tcp, udp or http$/, '{name}: the RTSP transport is tcp, udp or http', ['name']],
  [/^(.+): the local address is an IPv4 address of this server$/, '{name}: the local address is an IPv4 address of this server', ['name']],
  [/^(.+): the sources are IPv4 addresses separated by commas$/, '{name}: the sources are IPv4 addresses separated by commas', ['name']],
  [/^(.+): the RIST profile is simple, main or advanced$/, '{name}: the RIST profile is simple, main or advanced', ['name']],
  [/^(.+): the RIST encryption is 128 or 256$/, '{name}: the RIST encryption is 128 or 256', ['name']],
  [/^(.+): no control characters$/, '{name}: no control characters', ['name']],
  // Réseau
  [/^(.+): a fixed address is needed \(10\.0\.0\.2\/24\)$/, '{name}: a fixed address is needed (10.0.0.2/24)', ['name']],
  [/^(.+): not an address with its prefix: (.+) \(10\.0\.0\.2\/24\)$/, '{name}: not an address with its prefix: {a} (10.0.0.2/24)', ['name', 'a']],
  [/^(.+): not a gateway address: (.+)$/, '{name}: not a gateway address: {a}', ['name', 'a']],
  [/^(.+): not a DNS server address: (.+)$/, '{name}: not a DNS server address: {a}', ['name', 'a']],
  [/^(.+): the MTU is between 576 and 9216$/, '{name}: the MTU is between 576 and 9216', ['name']],
  // Mises à jour
  [/^(.+) has no release of the Core$/, '{repo} has no release of the Core', ['repo']],
  [/^this Core is already the latest \((.+)\)$/, 'This Core is already the latest ({version})', ['version']],
  [/^the archive holds the Core (.+), not (.+)$/, 'The archive holds the Core {got}, not {want}', ['got', 'want']],
  [/^this archive holds the Core (.+); this one is already (.+)$/, 'This archive holds the Core {got}; this one is already {core}', ['got', 'core']],
  [/^the new Core does not start: (.+)$/, 'The new Core does not start: {why}', ['why']],
  [/^cannot replace (.+): (.+)$/, 'Cannot replace {path}: {why}', ['path', 'why']],
  [/^cannot keep the current Core: (.+)$/, 'Cannot keep the current Core: {why}', ['why']],
  [/^release (.+) has no Core archive$/, 'Release {tag} has no Core archive', ['tag']],
  // Serveurs
  [/^cannot reach the master: (.+)$/, 'cannot reach the master: {why}', ['why']],
  [/^the master answered HTTP (\d+)$/, 'the master answered HTTP {code}', ['code']],
  // SNMP
  [/^not an address or a network: (.+)$/, 'not an address or a network: {a}', ['a']],
  [/^not an address for the traps: (.+)$/, 'not an address for the traps: {a}', ['a']],
  [/^cannot listen on UDP port (\d+): (.+)$/, 'cannot listen on UDP port {port}: {why}', ['port', 'why']],
  // Alertes
  [/^the webhook answered HTTP (\d+)$/, 'the webhook answered HTTP {code}', ['code']],
  // Horloge
  [/^not an NTP server: (.+)$/, 'not an NTP server: {name}', ['name']],
  [/^cannot write (.+): (.+)$/, 'cannot write {path}: {why}', ['path', 'why']],
  // Entrées partagées
  [/^cannot open the input: (.+)$/, 'cannot open the input: {why}', ['why']],
  [/^cannot share the input \((.+)\): (.+)$/, 'cannot share the input ({path}): {why}', ['path', 'why']],
  [/^the certificate is valid only from (.+)$/, 'The certificate is valid only from {date}', ['date']],
  // Fonctions
  [/^(.+): HTML5 output address: letters, digits, - or _ only$/, '{name}: HTML5 output address: letters, digits, - or _ only', ['name']],
  [/^(.+): HTML5 output address already used: `(.+)`$/, '{name}: HTML5 output address already used: `{path}`', ['name', 'path']],
  [/^(.+): the background is a CSS colour \(#000000, transparent…\)$/, '{name}: the background is a CSS colour (#000000, transparent…)', ['name']],
  [/^MTL is not loaded: (.+)$/, 'MTL is not loaded: {why}', ['why']],
];

export function friendly(e: unknown): string {
  const raw = ((e as Error)?.message ?? String(e)).trim();
  for (const [re, text, names] of SHAPED) {
    const m = re.exec(raw);
    if (m) return t(text, Object.fromEntries(names.map((n, i) => [n, m[i + 1]])));
  }
  return t(raw);
}

/** L'erreur d'un sondage régulier : dite, sauf quand le serveur ne répond pas (il redémarre, ou
 *  se met à jour : l'état de la connexion le montre déjà, sans un message toutes les secondes). */
export function pollError(e: unknown) {
  if (e instanceof ApiError && e.status === 0) return;
  toast(friendly(e), 'error');
}
