// Les erreurs du serveur (texte court en anglais) dans la langue de l'interface.
import { t } from '@strata/ui';

export function friendly(e: unknown): string {
  const raw = (e as Error)?.message ?? String(e);
  const m = /^(.+) needs the Core (.+) \(this one is (.+)\)$/.exec(raw);
  if (m) return t('{app} needs the Core {range} (this one is {core})', { app: m[1], range: m[2], core: m[3] });
  const port = /^(.+) uses port (\d+) too: deactivate it first$/.exec(raw);
  if (port) return t('{app} uses port {port} too: deactivate it first', { app: port[1], port: port[2] });
  const own = /^port (\d+) is the Core's own$/.exec(raw);
  if (own) return t('Port {port} is the Core’s own', { port: own[1] });
  const apart = /^(.+) is started apart from the Core$/.exec(raw);
  if (apart) return t('{app} is started apart from the Core', { app: apart[1] });
  return t(raw);
}
