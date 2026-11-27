// L'interface du Core : la connexion (et le premier administrateur), puis le cockpit.
import { StrictMode, useEffect, useState, type ComponentType } from 'react';
import { createRoot } from 'react-dom/client';
import { AppWindow, CircleHelp, Film, Layers, LayoutGrid, Network, Phone, Scissors } from 'lucide-react';
import '@strata/ui/styles.css';
import {
  Cockpit, ContextMenuHost, DialogHost, Launcher, Login, StatusPill, Toasts, TopBar, UserMenu,
  addTranslations, installEscape, installTooltips, instanceKey, nameTab, openAppTab, setLang, setTheme, setTimezone, t, toast, useLang,
  type CockpitData, type CockpitInstance, type Lang, type ThemePref,
} from '@strata/ui';
import { api, ApiError, useApp, type AuthState, type PublicUser } from './api';
import { fr } from './fr';

addTranslations('fr', fr);
installTooltips();
installEscape();
nameTab('cockpit');

/** Les icônes des applis Strata, d'après le nom donné par leur fiche ; une appli inconnue : une fenêtre. */
const ICONS: Record<string, ComponentType<{ size?: number }>> = { scissors: Scissors, layers: Layers, 'layout-grid': LayoutGrid, network: Network, film: Film, phone: Phone };

interface CockpitWire extends Omit<CockpitData, 'apps'> {
  apps: { id: string; name: string; icon: string }[];
}

/** Relit l'état de la connexion, et applique les choix de l'utilisateur (langue, thème, fuseau). */
async function loadAuth() {
  const auth = await api<AuthState>('GET', '/api/auth/state');
  useApp.setState({ auth, connected: true });
  setTimezone(auth.timezone);
  if (auth.user) {
    if (auth.user.lang) setLang(auth.user.lang);
    if (auth.user.theme) setTheme(auth.user.theme);
  }
}

const message = (e: unknown) => t((e as Error).message);

/** Garde un choix de l'utilisateur sur le serveur. */
async function savePrefs(prefs: { lang?: Lang; theme?: ThemePref }) {
  try {
    const user = await api<PublicUser>('PUT', '/api/me/prefs', prefs);
    const a = useApp.getState().auth;
    if (a) useApp.setState({ auth: { ...a, user } });
  } catch (e) {
    toast(message(e), 'error');
  }
}

/** La liaison : /api/health toutes les 5 s. */
function useHeartbeat() {
  useEffect(() => {
    const beat = () =>
      api('GET', '/api/health')
        .then(() => useApp.setState({ connected: true }))
        .catch(() => useApp.setState({ connected: false }));
    const id = setInterval(beat, 5000);
    return () => clearInterval(id);
  }, []);
}

function useCockpit(signedIn: boolean) {
  const [data, setData] = useState<CockpitData | null>(null);
  useEffect(() => {
    if (!signedIn) return;
    let live = true;
    const load = () =>
      api<CockpitWire>('GET', '/api/cockpit')
        .then((w) => live && setData({ ...w, apps: w.apps.map((a) => ({ id: a.id, name: a.name, icon: ICONS[a.icon] ?? AppWindow })) }))
        .catch((e) => live && toast(message(e), 'error'));
    void load();
    const id = setInterval(load, 15000);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [signedIn]);
  return data;
}

const open = (i: CockpitInstance) => openAppTab(i.url, instanceKey(i));

function App() {
  useLang((s) => s.lang);
  const auth = useApp((s) => s.auth);
  const connected = useApp((s) => s.connected);
  const user = auth?.user ?? null;
  const data = useCockpit(!!user);
  useHeartbeat();

  if (!auth) return null;
  if (!user)
    return (
      <Login
        module="Core"
        serverName={auth.serverName}
        setup={auth.needsSetup}
        sso={auth.sso}
        cliHint="sudo strata-core user add admin"
        onLogin={async (username, password) => {
          await api('POST', '/api/auth/login', { username, password });
          await loadAuth();
        }}
        onSetup={async (username, password, displayName) => {
          await api('POST', '/api/auth/setup', { username, password, displayName });
          await loadAuth();
        }}
      />
    );

  const empty: CockpitData = { apps: [], servers: [], instances: [], admin: user.admin };
  return (
    <div className="st-app">
      <TopBar
        module="Core"
        start={<Launcher data={data ?? empty} onOpen={open} onCockpit={() => window.scrollTo(0, 0)} />}
        end={
          <>
            <StatusPill connected={connected} serverName={auth.serverName} onLost={() => void loadAuth().catch(() => {})} />
            <button type="button" className="st-help" title={t('Help')}>
              <CircleHelp size={17} />
            </button>
            <UserMenu
              name={user.displayName}
              role={user.admin ? t('Server administrator') : user.groups.join(' · ') || t('User')}
              named
              onLang={(lang) => void savePrefs({ lang })}
              onTheme={(theme) => void savePrefs({ theme })}
              onSignOut={() => void api('POST', '/api/auth/logout').finally(() => void loadAuth())}
            />
          </>
        }
      />
      <div className="st-content">{data && <Cockpit data={data} onOpen={open} />}</div>
      <ContextMenuHost />
      <DialogHost />
      <Toasts />
    </div>
  );
}

loadAuth().catch((e) => {
  useApp.setState({ auth: { needsSetup: false, user: null, serverName: '', timezone: 'Europe/Paris', version: '', sso: null }, connected: false });
  if (e instanceof ApiError) toast(message(e), 'error');
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
