// Les pages du Core : le cockpit (/) et les réglages (/reglages/…). L'adresse suit la page (historique,
// favoris) ; le serveur renvoie l'interface pour toute adresse qui n'est pas un fichier.
import { create } from 'zustand';

export const useRoute = create<{ path: string }>(() => ({ path: location.pathname }));

export function navigate(path: string, replace = false) {
  if (path === location.pathname) return;
  if (replace) history.replaceState(null, '', path);
  else history.pushState(null, '', path);
  useRoute.setState({ path });
}

addEventListener('popstate', () => useRoute.setState({ path: location.pathname }));
