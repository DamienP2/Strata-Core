// Les erreurs du serveur (texte court en anglais) dans la langue de l'interface.
import { t } from '@strata/ui';

export function friendly(e: unknown): string {
  const raw = (e as Error)?.message ?? String(e);
  const m = /^(.+) needs the Core (.+) \(this one is (.+)\)$/.exec(raw);
  if (m) return t('{app} needs the Core {range} (this one is {core})', { app: m[1], range: m[2], core: m[3] });
  return t(raw);
}
