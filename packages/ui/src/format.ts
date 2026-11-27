// Nombres, tailles, durées et dates dans la langue de l'interface. Repris de LiveCut ; le fuseau horaire
// du serveur est donné par l'appli (setTimezone), plus lu dans son store.
import { locale, t } from './i18n';

let timezone: string | undefined;
/** Le fuseau horaire du serveur (ex. « Europe/Paris ») : les dates s'affichent dans ce fuseau. */
export function setTimezone(tz: string | undefined) {
  timezone = tz || undefined;
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

/** Une taille en octets : « 12 Mo », « 1,5 Go » (base 1024). */
export function formatBytes(n: number | null | undefined): string {
  let v = Math.max(0, n ?? 0);
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  const num = v.toLocaleString(locale(), { maximumFractionDigits: i > 0 && v < 10 ? 1 : 0 });
  return `${num} ${t(UNITS[i])}`;
}

/** Un pourcentage arrondi : « 34 % ». */
export function formatPercent(n: number | null | undefined): string {
  return t('{n}%', { n: Math.round(n ?? 0) });
}

/** Une durée : « 2 j 3 h », « 3 h 12 min », « 5 min 10 s », « 8 s ». */
export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return t('{d}d {h}h', { d, h });
  if (h) return t('{h}h {m}m', { h, m });
  if (m) return t('{m}m {s}s', { m, s: s % 60 });
  return t('{s}s', { s });
}

/** Un horodatage (ms ou texte ISO) en Date. */
export function toDate(at: number | string | null | undefined): Date | null {
  if (at === null || at === undefined || at === '') return null;
  const d = new Date(at);
  return Number.isNaN(d.getTime()) ? null : d;
}

/** « 18 nov. 2026, 14:05 » dans le fuseau du serveur. */
export function formatDateTime(at: number | string | null | undefined): string {
  const d = toDate(at);
  if (!d) return '';
  try {
    return d.toLocaleString(locale(), { dateStyle: 'medium', timeStyle: 'short', timeZone: timezone });
  } catch {
    return d.toLocaleString(locale(), { dateStyle: 'medium', timeStyle: 'short' });
  }
}

/** « 18 nov. 2026 » dans le fuseau du serveur. */
export function formatDate(at: number | string | null | undefined): string {
  const d = toDate(at);
  if (!d) return '';
  try {
    return d.toLocaleDateString(locale(), { dateStyle: 'medium', timeZone: timezone });
  } catch {
    return d.toLocaleDateString(locale(), { dateStyle: 'medium' });
  }
}

/** « il y a 5 min ». */
export function timeAgo(at: number | string | null | undefined): string {
  const d = toDate(at);
  if (!d) return '';
  const s = Math.max(0, (Date.now() - d.getTime()) / 1000);
  if (s < 60) return t('just now');
  if (s < 3600) return t('{n} min ago', { n: Math.floor(s / 60) });
  if (s < 86400) return t('{n} h ago', { n: Math.floor(s / 3600) });
  return t('{n} d ago', { n: Math.floor(s / 86400) });
}

/** La couleur d'une charge : ok sous 60 %, attention sous 85 %, au-delà mauvais. */
export function loadTone(pct: number | null | undefined): 'ok' | 'warn' | 'bad' | 'idle' {
  if (pct === null || pct === undefined || Number.isNaN(pct)) return 'idle';
  return pct < 60 ? 'ok' : pct < 85 ? 'warn' : 'bad';
}
