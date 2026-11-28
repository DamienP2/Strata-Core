// Les pages qui se relisent toutes seules : une lecture à la fois (la suivante part `ms` après la
// fin de la précédente), rien tant que l'onglet est caché, rien après avoir quitté la page, et pas
// de nouveau rendu quand la réponse n'a pas changé.
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { api } from './api';

/** L'onglet est visible. */
export function usePageVisible(): boolean {
  const [v, setV] = useState(!document.hidden);
  useEffect(() => {
    const on = () => setV(!document.hidden);
    document.addEventListener('visibilitychange', on);
    return () => document.removeEventListener('visibilitychange', on);
  }, []);
  return v;
}

/** Vrai tant que le composant est affiché : pour ne plus rien changer une fois la page quittée. */
export function useMounted(): () => boolean {
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  return useCallback(() => mounted.current, []);
}

/**
 * Appelle `tick` puis, `ms` après la fin de chaque appel, encore, tant que `key` n'est ni null ni
 * false et que l'onglet est visible. Un autre `key` repart de zéro (et oublie l'appel en cours).
 * `alive()` dit faux dès que cet appel ne compte plus (page quittée, onglet caché, autre `key`).
 * `now: false` : le premier appel attend `ms` lui aussi.
 */
export function useRepeat(key: string | null | false, ms: number, tick: (alive: () => boolean) => Promise<unknown> | void, opts?: { now?: boolean }) {
  const visible = usePageVisible();
  // Toujours la dernière version de `tick` (elle voit l'état du dernier rendu), sans relancer la boucle.
  const fn = useRef(tick);
  useLayoutEffect(() => {
    fn.current = tick;
  });
  const now = opts?.now ?? true;
  useEffect(() => {
    if (key === null || key === false || !visible) return;
    let gone = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const alive = () => !gone;
    const run = async () => {
      try {
        await fn.current(alive);
      } catch {
        /* chaque page dit ses erreurs elle-même */
      }
      if (!gone) timer = setTimeout(() => void run(), ms);
    };
    if (now) void run();
    else timer = setTimeout(() => void run(), ms);
    return () => {
      gone = true;
      clearTimeout(timer);
    };
  }, [key, ms, visible, now]);
}

/** Un état qui ne change (et ne redessine la page) que si son JSON change : pour ce qui est relu sans cesse. */
export function useJsonState<T>(init: T): [T, (v: T | ((prev: T) => T)) => void] {
  const [value, setValue] = useState<T>(init);
  const set = useCallback(
    (v: T | ((prev: T) => T)) =>
      setValue((prev) => {
        const next = typeof v === 'function' ? (v as (prev: T) => T)(prev) : v;
        return JSON.stringify(next) === JSON.stringify(prev) ? prev : next;
      }),
    [],
  );
  return [value, set];
}

/** Relit `path` toutes les `ms` (1 s par défaut) tant que l'onglet est visible ; null : rien. */
export function usePoll<T>(path: string | null, ms = 1000, onError?: (e: unknown) => void): T | null {
  const [data, setData] = useJsonState<T | null>(null);
  useRepeat(path, ms, async (alive) => {
    try {
      const r = await api<T>('GET', path!);
      if (alive()) setData(r);
    } catch (e) {
      // Le serveur redémarre : on réessaie.
      if (alive()) onError?.(e);
    }
  });
  return data;
}
