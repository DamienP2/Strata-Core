// L'interface en anglais (les textes sources) ou en français. Un texte est cherché par sa forme anglaise :
// non traduit, il reste en anglais.
//
//   t('New project')                       → « Nouveau projet »
//   t('Delete “{name}”?', { name })        → « Supprimer « … » ? »
//   tn(n, '{n} project', '{n} projects')   → « 3 projets »
//
// Appeler t() au moment de dessiner, pas au chargement d'un module. Le Core apporte les textes de ses
// composants ; chaque appli ajoute les siens avec addTranslations().
import { create } from 'zustand';
import { core } from './fr';

export type Lang = 'en' | 'fr';
export const LANGS: { id: Lang; label: string; name: string }[] = [
  { id: 'en', label: 'EN', name: 'English' },
  { id: 'fr', label: 'FR', name: 'Français' },
];

/** La clé du navigateur, la même dans toutes les applis. */
export const LANG_KEY = 'strata.lang';

export const isLang = (l: unknown): l is Lang => l === 'en' || l === 'fr';

function first(): Lang {
  try {
    const saved = localStorage.getItem(LANG_KEY);
    if (isLang(saved)) return saved;
  } catch {
    /* navigation privée */
  }
  return typeof navigator !== 'undefined' && navigator.language?.toLowerCase().startsWith('fr') ? 'fr' : 'en';
}

let current: Lang = typeof document === 'undefined' ? 'fr' : first();
if (typeof document !== 'undefined') document.documentElement.lang = current;

const DICT: Record<Lang, Record<string, string>> = { en: {}, fr: { ...core } };

/** Ajoute les traductions d'une appli (elles l'emportent sur celles du Core en cas de doublon). */
export function addTranslations(lang: Lang, dict: Record<string, string>) {
  Object.assign(DICT[lang], dict);
}

/** La langue affichée ; les composants qui tiennent des textes se redessinent quand elle change. */
export const useLang = create<{ lang: Lang }>(() => ({ lang: current }));

/** Applique une langue et la garde dans ce navigateur ; une valeur inconnue est ignorée. */
export function setLang(l: string | null | undefined) {
  if (!isLang(l) || l === current) return;
  current = l;
  document.documentElement.lang = l;
  try {
    localStorage.setItem(LANG_KEY, l);
  } catch {
    /* navigation privée */
  }
  useLang.setState({ lang: l });
}

export const lang = () => current;

/** Pour les dates et les nombres. */
export const locale = () => (current === 'fr' ? 'fr-FR' : 'en-GB');

/** Le texte dans la langue de l'interface, `{nom}` remplacé par vars. */
export function t(s: string, vars?: Record<string, string | number>): string {
  const out = DICT[current][s] ?? s;
  return vars ? out.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : out;
}

/** Un mot court dont le français dépend du contexte : tc('theme', 'Light') cherche d'abord 'theme|Light'. */
export function tc(context: string, s: string, vars?: Record<string, string | number>): string {
  const key = `${context}|${s}`;
  return key in DICT[current] ? t(key, vars) : t(s, vars);
}

/** Singulier ou pluriel selon n (en français, 0 et 1 sont au singulier), {n} remplacé. */
export function tn(n: number, one: string, other: string, vars?: Record<string, string | number>): string {
  const single = current === 'fr' ? Math.abs(n) < 2 : n === 1;
  return t(single ? one : other, { n, ...vars });
}
