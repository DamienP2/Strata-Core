// Le thème de l'interface : sombre (par défaut), clair, ou celui de l'ordinateur ("system", suivi en direct).
//
// Le choix de référence est celui gardé sur le serveur pour l'utilisateur (le même pour toutes les applis
// et tous les serveurs) : l'appli l'applique avec setTheme() dès qu'elle le reçoit. Le navigateur en garde
// une copie, par adresse, pour que le premier affichage soit déjà le bon (voir themeBootScript).
//
// Le clair pose data-theme="light" sur <html> ; ses jetons sont dans tokens.css. Les pages qui passent à
// l'antenne (sorties, intégrations) restent sombres : voir setAlwaysDark().
import { create } from 'zustand';

export type ThemePref = 'dark' | 'light' | 'system';
export const THEMES: ThemePref[] = ['dark', 'light', 'system'];

/** La clé du navigateur, la même dans toutes les applis. */
export const THEME_KEY = 'strata.theme';

export const isTheme = (v: unknown): v is ThemePref => v === 'dark' || v === 'light' || v === 'system';

function first(): ThemePref {
  try {
    const saved = localStorage.getItem(THEME_KEY);
    if (isTheme(saved)) return saved;
  } catch {
    /* navigation privée */
  }
  return 'dark';
}

const media = typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: light)') : null;
let pref: ThemePref = typeof document === 'undefined' ? 'dark' : first();
let alwaysDark: RegExp | null = null;

function apply() {
  if (typeof document === 'undefined') return;
  const dark = alwaysDark?.test(location.pathname) ?? false;
  const light = !dark && (pref === 'light' || (pref === 'system' && !!media?.matches));
  const root = document.documentElement;
  if (light) root.dataset.theme = 'light';
  else delete root.dataset.theme;
}

apply();
media?.addEventListener('change', () => pref === 'system' && apply());

/** Le thème choisi ; le menu utilisateur se redessine quand il change. */
export const useTheme = create<{ theme: ThemePref }>(() => ({ theme: pref }));

/** Applique un thème et le garde dans ce navigateur ; une valeur vide ou inconnue est ignorée. */
export function setTheme(t: string | null | undefined) {
  if (!isTheme(t)) return;
  try {
    localStorage.setItem(THEME_KEY, t);
  } catch {
    /* navigation privée */
  }
  if (t === pref) return;
  pref = t;
  apply();
  useTheme.setState({ theme: t });
}

/** Les chemins qui restent sombres quel que soit le choix (ex. /^\/(output|embed)(\/|$)/). */
export function setAlwaysDark(paths: RegExp | null) {
  alwaysDark = paths;
  apply();
}

/**
 * À mettre dans un <script> de index.html, avant les styles : le thème gardé, avant le premier
 * affichage (pas d'éclair de l'autre thème). `alwaysDark` : la source d'une expression régulière.
 */
export function themeBootScript(alwaysDarkSource?: string): string {
  const guard = alwaysDarkSource ? `!/${alwaysDarkSource}/.test(location.pathname) && ` : '';
  return `try{var t=localStorage.getItem('${THEME_KEY}');if(${guard}(t==='light'||(t==='system'&&matchMedia('(prefers-color-scheme: light)').matches)))document.documentElement.dataset.theme='light';}catch(e){}`;
}
