// Les pages du Core : le cockpit (/) et les réglages (/reglages/…). L'adresse suit la page (historique,
// favoris) ; le serveur renvoie l'interface pour toute adresse qui n'est pas un fichier.
// Une page aux changements non enregistrés (useLeaveGuard) demande avant d'être quittée.
import { create } from 'zustand';
import { canLeave } from '@strata/ui';

export const useRoute = create<{ path: string }>(() => ({ path: location.pathname }));

/** L'adresse de la page affichée : celle qu'on remet si l'on refuse de quitter la page (retour arrière). */
let shown = location.pathname + location.search + location.hash;
/** Une question « abandonner les changements ? » est ouverte : pas une seconde par-dessus. */
let asking = false;

async function mayLeave(to: string): Promise<boolean> {
  if (asking) return false;
  asking = true;
  try {
    return await canLeave(to);
  } finally {
    asking = false;
  }
}

export function navigate(path: string, replace = false) {
  if (path === location.pathname) return;
  void mayLeave(path).then((ok) => {
    if (!ok || path === location.pathname) return;
    if (replace) history.replaceState(null, '', path);
    else history.pushState(null, '', path);
    shown = location.pathname + location.search + location.hash;
    useRoute.setState({ path: location.pathname });
  });
}

// Retour ou avance du navigateur : l'adresse a déjà changé ; refusé, on remet celle de la page.
addEventListener('popstate', () => {
  void mayLeave(location.pathname).then((ok) => {
    if (!ok) {
      history.pushState(null, '', shown);
      return;
    }
    shown = location.pathname + location.search + location.hash;
    useRoute.setState({ path: location.pathname });
  });
});
