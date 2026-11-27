// L'interface du Core : la connexion (et le premier administrateur), puis le cockpit.
import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { CircleHelp, Settings as SettingsIcon } from 'lucide-react';
import '@strata/ui/styles.css';
import './core.css';
import {
  Cockpit, ContextMenuHost, DialogHost, Launcher, Login, StatusPill, Toasts, TopBar, UserMenu,
  addTranslations, installEscape, installTooltips, instanceKey, nameTab, openAppTab, setLang, setTheme, setTimezone, t, toast, useLang,
  type CockpitData, type CockpitInstance, type Lang, type ThemePref,
} from '@strata/ui';
import { api, ApiError, useApp, type AuthState, type PublicUser } from './api';
import { fr } from './fr';
import { appIcon } from './icons';
import { navigate, useRoute } from './router';
import { Settings } from './settings/Settings';

addTranslations('fr', fr);
installTooltips();
installEscape();
nameTab('cockpit');

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
        .then((w) => live && setData({ ...w, apps: w.apps.map((a) => ({ id: a.id, name: a.name, icon: appIcon(a.icon) })) }))
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

/** Les réglages d'un serveur : ceux-ci ici, ceux d'un membre dans son propre onglet. */
function openServer(data: CockpitData, id: string) {
  const s = data.servers.find((x) => x.id === id) as (CockpitData['servers'][number] & { url?: string }) | undefined;
  if (!s || s.master) navigate('/reglages/serveur');
  else if (s.url) window.open(`${s.url.replace(/\/$/, '')}/reglages/serveur`, `strata-settings-${id}`);
}

/** `?next=` : où aller une fois connecté (le serveur vérifie aussi celui du SSO). */
const nextParam = new URLSearchParams(location.search).get('next');

/**
 * Où revenir après la connexion : une page du cockpit, ou une appli de ce serveur (le relais y
 * envoie quand on ouvre une appli sans être connecté). Autre chose : nulle part.
 */
function returnTo(): { local: string } | { app: string } | null {
  if (!nextParam) return null;
  try {
    const u = new URL(nextParam, location.origin);
    if (u.origin === location.origin) return { local: u.pathname + u.search };
    const ours = u.hostname.endsWith(`.${location.hostname}`) && !u.hostname.slice(0, -location.hostname.length - 1).includes('.');
    if (ours && (u.protocol === 'https:' || u.protocol === 'http:') && !u.username && !u.password) return { app: u.href };
  } catch {
    /* adresse illisible */
  }
  return null;
}

function App() {
  useLang((s) => s.lang);
  const auth = useApp((s) => s.auth);
  const connected = useApp((s) => s.connected);
  const path = useRoute((s) => s.path);
  const user = auth?.user ?? null;
  const data = useCockpit(!!user);
  useHeartbeat();
  const [back, setBack] = useState(returnTo);
  // Connecté avec un `?next=` : vers l'appli (cet onglet devient le sien) ou la page demandée.
  useEffect(() => {
    if (!user || !back) return;
    if ('app' in back) {
      location.replace(back.app);
      return;
    }
    // Une adresse du serveur (le maître qui connecte un membre) : chargée telle quelle.
    if (back.local.startsWith('/api/')) {
      location.replace(back.local);
      return;
    }
    setBack(null);
    history.replaceState(null, '', location.pathname);
    navigate(back.local, true);
  }, [user, back]);
  // Les réglages : administrateurs seulement ; toute autre adresse revient au cockpit.
  useEffect(() => {
    if (!user || back) return;
    if (user.admin && path === '/reglages') navigate('/reglages/serveur', true);
    else if (path !== '/' && !(user.admin && path.startsWith('/reglages/'))) navigate('/', true);
  }, [user, path, back]);

  // Membre d'un cockpit : le cockpit est celui du maître, la connexion aussi (sauf `?local=1`, ou
  // un maître qui ne répond pas : les comptes locaux de secours).
  const cluster = auth?.cluster;
  const viaMaster = !!cluster?.masterUp && !new URLSearchParams(location.search).has('local');
  useEffect(() => {
    if (!cluster || !viaMaster) return;
    if (!user && !auth?.needsSetup) location.replace(`/api/cluster/login?next=${encodeURIComponent(back && 'app' in back ? back.app : location.pathname)}`);
    else if (user && path === '/' && !back) location.replace(cluster.masterUrl);
  }, [cluster, viaMaster, user, path, back, auth?.needsSetup]);

  if (!auth) return null;
  if (user && back && 'app' in back) return null;
  if (cluster && viaMaster && (!user || path === '/')) return null;
  if (!user)
    return (
      <Login
        module="Core"
        serverName={auth.serverName}
        setup={auth.needsSetup}
        sso={cluster ? cluster.masterName : auth.sso}
        ssoHref={
          cluster
            ? `/api/cluster/login?next=${encodeURIComponent(back && 'app' in back ? back.app : location.pathname)}`
            : `/api/auth/sso/start?next=${encodeURIComponent(back && 'app' in back ? back.app : location.pathname)}`
        }
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
  const inSettings = user.admin && path.startsWith('/reglages/');
  return (
    <div className="st-app">
      <TopBar
        module={inSettings ? t('Settings') : 'Core'}
        start={<Launcher data={data ?? empty} onOpen={open} onCockpit={() => navigate('/')} />}
        end={
          <>
            <StatusPill connected={connected} serverName={auth.serverName} onLost={() => void loadAuth().catch(() => {})} />
            <button type="button" className="st-help" title={t('Help')}>
              <CircleHelp size={17} />
            </button>
            {user.admin && (
              <button type="button" className={`st-help ${inSettings ? 'on' : ''}`} title={t('Server settings')} onClick={() => navigate(inSettings ? '/' : '/reglages/serveur')}>
                <SettingsIcon size={17} />
              </button>
            )}
            <UserMenu
              name={user.displayName}
              role={user.admin ? t('Server administrator') : user.groupNames.join(' · ') || t('User')}
              named
              onLang={(lang) => void savePrefs({ lang })}
              onTheme={(theme) => void savePrefs({ theme })}
              onSignOut={() => void api('POST', '/api/auth/logout').finally(() => void loadAuth())}
            />
          </>
        }
      />
      <div className="st-content">{inSettings ? <Settings path={path.slice('/reglages/'.length)} /> : data && <Cockpit data={data} onOpen={open} onServer={user.admin ? (id) => openServer(data, id) : undefined} />}</div>
      <ContextMenuHost />
      <DialogHost />
      <Toasts />
    </div>
  );
}

loadAuth().catch((e) => {
  useApp.setState({ auth: { needsSetup: false, user: null, serverName: '', timezone: 'Europe/Paris', version: '', sso: null, localAccounts: 'all' }, connected: false });
  if (e instanceof ApiError) toast(message(e), 'error');
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
