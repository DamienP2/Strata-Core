// L'interface du Core : la connexion (et le premier administrateur), puis le cockpit.
import { StrictMode, Suspense, lazy, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { CircleHelp, CloudDownload, LoaderCircle, RotateCw, Settings as SettingsIcon, Unplug } from 'lucide-react';
import '@strata/ui/styles.css';
import './core.css';
import {
  Cockpit, ContextMenuHost, DialogHost, Launcher, Login, Toasts, TopBar, UserMenu,
  addTranslations, installEscape, installTooltips, instanceKey, nameTab, openAppTab, setLang, setTheme, setTimezone, t, toast, useLang,
  type CockpitData, type CockpitInstance, type Lang, type ThemePref,
} from '@strata/ui';
import { api, ApiError, expectRestart, movedUrl, useApp, type AuthState, type PublicUser, type Unavailable } from './api';
import { fr } from './fr';
import { appIcon } from './icons';
import { navigate, useRoute } from './router';
import { useRepeat } from './poll';
// Les réglages (administrateurs seulement) : chargés à part, en les ouvrant.
const Settings = lazy(() => import('./settings/Settings').then((m) => ({ default: m.Settings })));

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

/** La liaison : /api/health toutes les 5 s (2 s quand l'écran est bloqué). Une mise à jour ou un
 *  redémarrage en cours, ou deux appels sans réponse : l'écran se bloque ; le Core revenu, la page
 *  se recharge. */
function useHeartbeat() {
  useEffect(() => {
    let fails = 0;
    let timer = 0;
    let stop = false;
    const beat = async () => {
      const down = useApp.getState().down;
      try {
        // Un serveur figé ne répond pas sans échouer : 4 s au plus.
        const res = await fetch('/api/health', { credentials: 'same-origin', signal: AbortSignal.timeout(4000) });
        if (!res.ok) throw new Error(res.statusText);
        const h = (await res.json()) as { updating?: boolean; restarting?: boolean; safeMode?: boolean; unavailable?: Unavailable[] };
        useApp.setState({ unavailable: h.unavailable ?? [], safeMode: !!h.safeMode });
        fails = 0;
        if (h.updating) useApp.setState({ down: 'update', connected: true });
        else if (h.restarting) useApp.setState({ down: down === 'update' ? 'update' : 'restart', connected: true });
        else if (down === 'moved' ? movedLost : down && (down === 'lost' || sawDown)) {
          // L'adresse a changé : l'écran garde son lien tant que l'ancienne adresse répond ; seule
          // l'ancienne adresse revenue après une coupure (changement non confirmé) recharge la page.
          location.reload();
          return;
        } else useApp.setState({ connected: true });
        if (useApp.getState().down) sawDown = true;
      } catch {
        fails++;
        if (down === 'moved') movedLost = true;
        if (down || fails >= 2) {
          sawDown = true;
          useApp.setState({ down: down === 'update' || down === 'restart' || down === 'moved' ? down : 'lost', connected: false });
        }
      }
      if (!stop) timer = window.setTimeout(() => void beat(), useApp.getState().down || fails > 0 ? 2000 : 5000);
    };
    // Le Core a dit (ou on lui a demandé) qu'il allait redémarrer : vu une fois bloqué, son retour
    // sans mise à jour ni redémarrage recharge la page.
    let sawDown = false;
    // Adresse changée (`moved`) : l'ancienne adresse a cessé de répondre au moins une fois.
    let movedLost = false;
    const unsub = useApp.subscribe((s, prev) => {
      if (s.down && !prev.down) {
        window.clearTimeout(timer);
        timer = window.setTimeout(() => void beat(), 2000);
      }
    });
    timer = window.setTimeout(() => void beat(), 300);
    return () => {
      stop = true;
      unsub();
      window.clearTimeout(timer);
    };
  }, []);
}

/** Ce qui ne marche pas faute d'une bibliothèque : le Core tourne sans, on dit ce qui manque. */
function Degraded() {
  useLang((s) => s.lang);
  const { unavailable, safeMode } = useApp();
  if (!unavailable.length && !safeMode) return null;
  const what: Record<string, string> = {
    mxl: t('MXL does not load: the pictures are not shared with the apps, and the network streams and the SDI outputs written by the apps are not available.'),
    mtl: t('MTL does not load: the ST 2110 flows are neither received nor sent by the Core.'),
    ffmpeg: t('ffmpeg is missing: the network streams (SRT, RTMP, HLS…) are not available.'),
  };
  return (
    <div className="core-degraded">
      {safeMode && (
        <div className="core-degraded-line">
          <strong>{t('Safe mode: the Core stopped several times at start, so it runs without MXL and MTL.')}</strong>
          <button
            type="button"
            className="st-btn small"
            onClick={() => void api('POST', '/api/server/restart').then(() => expectRestart('restart'), (e) => toast(message(e), 'error'))}
          >
            {t('Restart normally')}
          </button>
        </div>
      )}
      {unavailable.map((u) => (
        <div key={u.lib} className="core-degraded-line">
          <span>{what[u.features] ?? `${u.lib}: ${t('not available')}`}</span>
          <span className="st-muted st-mono">{u.why}</span>
        </div>
      ))}
    </div>
  );
}

/** L'écran bloqué : mise à jour, redémarrage, ou serveur injoignable. */
function DownOverlay() {
  useLang((s) => s.lang);
  const down = useApp((s) => s.down);
  const [since, setSince] = useState(0);
  useEffect(() => {
    if (!down) return;
    const start = Date.now();
    setSince(0);
    const id = setInterval(() => setSince(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => clearInterval(id);
  }, [down]);
  if (!down) return null;
  const Icon = down === 'update' ? CloudDownload : down === 'restart' ? RotateCw : Unplug;
  const title =
    down === 'update'
      ? t('The server is being updated')
      : down === 'restart'
        ? t('The server is restarting')
        : down === 'moved'
          ? t('The address of the server changes')
          : t('The server does not answer');
  const text =
    down === 'moved'
      ? t('Open the new address and confirm the change within 2 minutes; otherwise the previous address comes back and this page reloads by itself.')
      : down === 'lost'
        ? t('Trying again every 2 seconds. This page reloads by itself as soon as the server answers.')
        : t('Please wait: the Core restarts. This page reloads by itself as soon as it answers.');
  const elapsed = `${Math.floor(since / 60)}:${String(since % 60).padStart(2, '0')}`;
  return (
    <div className="core-down" role="alertdialog" aria-modal="true" aria-label={title}>
      <div className={`core-down-box ${down}`}>
        <span className="core-down-icon">
          <Icon size={26} />
        </span>
        <h2>{title}</h2>
        <p>{text}</p>
        {down === 'moved' && movedUrl && (
          <p>
            <a className="st-btn primary" href={movedUrl}>
              {movedUrl}
            </a>
          </p>
        )}
        <p className="core-down-wait">
          <LoaderCircle size={15} className="st-spin" /> {t('For {time}', { time: elapsed })}
        </p>
      </div>
    </div>
  );
}

/** Le cockpit, relu toutes les 15 s ; la page n'est redessinée que s'il a changé. */
function useCockpit(signedIn: boolean) {
  const [data, setData] = useState<CockpitData | null>(null);
  const last = useRef('');
  useRepeat(signedIn && 'cockpit', 15000, (alive) =>
    api<CockpitWire>('GET', '/api/cockpit')
      .then((w) => {
        const text = JSON.stringify(w);
        if (!alive() || text === last.current) return;
        last.current = text;
        setData({ ...w, apps: w.apps.map((a) => ({ id: a.id, name: a.name, icon: appIcon(a.icon) })) });
      })
      .catch((e) => alive() && toast(message(e), 'error')),
  );
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
      {user.admin && <Degraded />}
      <div className="st-content">
        {inSettings ? (
          <Suspense fallback={null}>
            <Settings path={path.slice('/reglages/'.length)} />
          </Suspense>
        ) : (
          data && <Cockpit data={data} onOpen={open} onServer={user.admin ? (id) => openServer(data, id) : undefined} />
        )}
      </div>
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
    <DownOverlay />
  </StrictMode>,
);
