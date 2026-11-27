/*
 * La coquille du Core (proposition C, validée avec la maquette docs/maquettes/cockpit.html) :
 *   - le cockpit : une tuile par appli ; une appli présente sur plusieurs serveurs porte un bouton par
 *     serveur, au nom d'usage du serveur. Les utilisateurs voient seulement « disponible » ou
 *     « indisponible » ; les administrateurs voient en plus l'état de chaque serveur ;
 *   - le lanceur ⊞, le même dans la barre du haut du Core et de chaque appli ;
 *   - chaque appli s'ouvre dans son propre onglet, réutilisé (openAppTab).
 * Les données (applis, serveurs, instances où l'utilisateur a des droits) viennent du Core.
 */
import { useRef, useState, type ComponentType, type ReactNode } from 'react';
import { Grid3x3, LayoutGrid } from 'lucide-react';
import { t, tn } from '../i18n';
import { useMenuKeys } from '../menuKeys';
import { Brand } from './Brand';
import { Empty } from './basics';

type Icon = ComponentType<{ size?: number }>;

/** Une appli Strata (Graphics, LiveCut…). */
export interface CockpitApp {
  id: string;
  name: string;
  icon: Icon;
}

/** Un serveur physique. `name` : son nom d'usage (« Régie A ») ; `host` : son nom de machine (administrateurs). */
export interface CockpitServer {
  id: string;
  name: string;
  host: string;
  up: boolean;
  /** Alertes en cours (administrateurs). */
  alerts?: number;
  master?: boolean;
}

/** Une appli active sur un serveur, que l'utilisateur a le droit d'ouvrir. */
export interface CockpitInstance {
  app: string;
  server: string;
  url: string;
  /** L'appli répond-elle sur ce serveur ? (absent : oui) */
  up?: boolean;
}

export interface CockpitData {
  apps: CockpitApp[];
  servers: CockpitServer[];
  instances: CockpitInstance[];
  /** Administrateur des serveurs : voit l'état des serveurs. */
  admin: boolean;
}

/** La clé d'une instance : `livecut@strata02`. */
export const instanceKey = (i: CockpitInstance) => `${i.app}@${i.server}`;

/** Les instances regroupées par appli, dans l'ordre des applis. */
export function groupByApp({ apps, instances }: Pick<CockpitData, 'apps' | 'instances'>) {
  return apps.map((app) => ({ app, list: instances.filter((i) => i.app === app.id) })).filter((g) => g.list.length > 0);
}

/**
 * Ouvre une appli dans son onglet attitré : le même onglet à chaque fois (au lieu d'en ouvrir un
 * autre), sans le recharger s'il est déjà ouvert. Doit être appelé dans un clic.
 */
export function openAppTab(url: string, key: string) {
  const w = window.open('', `strata-${key}`);
  if (!w) return false;
  try {
    // Nouvel onglet (vide, même origine) : on y charge l'appli.
    if (w.location.href === 'about:blank') w.location.href = url;
  } catch {
    // Autre origine : l'appli y est déjà.
  }
  w.focus();
  return true;
}

/**
 * Donne son nom à l'onglet courant (`strata-cockpit`, `strata-livecut@strata02`) : le cockpit et le
 * lanceur retrouvent ainsi un onglet ouvert à la main (favori, adresse tapée) au lieu d'en ouvrir un autre.
 */
export function nameTab(key: string) {
  window.name = `strata-${key}`;
}

function serverOf(data: CockpitData, id: string) {
  return data.servers.find((s) => s.id === id);
}

/** L'instance peut-elle s'ouvrir : son serveur répond, et l'appli aussi. */
const live = (data: CockpitData, i: CockpitInstance) => !!serverOf(data, i.server)?.up && i.up !== false;

/** Le bouton d'une instance : le nom d'usage du serveur ; son état en plus pour les administrateurs. */
function InstanceButton({ data, inst, onOpen, menu }: { data: CockpitData; inst: CockpitInstance; onOpen: (i: CockpitInstance) => void; menu?: boolean }) {
  const role = menu ? 'menuitem' : undefined;
  const s = serverOf(data, inst.server);
  const name = s?.name ?? inst.server;
  if (!live(data, inst)) {
    const why = !s?.up ? t('{host} does not answer', { host: s?.host ?? inst.server }) : t('The app does not answer on {host}', { host: s.host });
    return (
      <button type="button" role={role} className="st-inst" disabled title={data.admin ? why : t('Unavailable')}>
        {data.admin ? (
          <>
            <span className="st-dot bad" />
            {name}
          </>
        ) : (
          t('{name} · unavailable', { name })
        )}
      </button>
    );
  }
  const alerts = s?.alerts ?? 0;
  const title = data.admin ? [s?.host, alerts ? tn(alerts, '{n} alert', '{n} alerts') : null].filter(Boolean).join(' · ') : undefined;
  return (
    <button type="button" role={role} className="st-inst" title={title} onClick={() => onOpen(inst)}>
      {data.admin && <span className={`st-dot ${alerts ? 'warn' : 'ok'}`} />}
      {name}
    </button>
  );
}

/** Le cockpit : les tuiles des applis, et pour les administrateurs la liste des serveurs. */
export function Cockpit({ data, title, onOpen, onServer }: {
  data: CockpitData;
  /** Le titre (« Applis »). */
  title?: string;
  onOpen: (i: CockpitInstance) => void;
  /** Administrateurs : ouvre les réglages d'un serveur. */
  onServer?: (serverId: string) => void;
}) {
  const groups = groupByApp(data);
  return (
    <section className="st-cockpit st-page">
      <div className="st-cockpit-head">
        <h1>{title ?? t('Apps')}</h1>
      </div>
      {groups.length === 0 && (
        <div className="st-cockpit-empty">
          <Empty icon={LayoutGrid} text={t('No app yet')} />
        </div>
      )}
      <div className="st-tiles">
        {groups.map(({ app, list }) => {
          const Icon = app.icon;
          const up = list.filter((i) => live(data, i));
          if (!up.length)
            return (
              <div key={app.id} className="st-tile off" aria-disabled="true">
                <span className="st-tile-icon">
                  <Icon size={28} />
                </span>
                <strong>{app.name}</strong>
                <span className="st-muted">{t('Unavailable')}</span>
              </div>
            );
          if (list.length === 1) {
            const s = serverOf(data, list[0].server);
            return (
              <button key={app.id} type="button" className="st-tile" onClick={() => onOpen(list[0])} title={data.admin ? s?.host : undefined}>
                <span className="st-tile-icon">
                  <Icon size={28} />
                </span>
                <strong>{app.name}</strong>
                {data.admin && s && <span className="st-muted">{s.name}</span>}
              </button>
            );
          }
          return (
            <div key={app.id} className="st-tile multi">
              <span className="st-tile-icon">
                <Icon size={28} />
              </span>
              <strong>{app.name}</strong>
              <div className="st-insts">
                {list.map((i) => (
                  <InstanceButton key={instanceKey(i)} data={data} inst={i} onOpen={onOpen} />
                ))}
              </div>
            </div>
          );
        })}
      </div>
      {data.admin && data.servers.length > 0 && (
        <div className="st-servers">
          {data.servers.map((s) => (
            <button key={s.id} type="button" className="st-server" title={t('Settings of {name}', { name: s.name })} onClick={() => onServer?.(s.id)}>
              <span className={`st-dot ${!s.up ? 'bad' : s.alerts ? 'warn' : 'ok'}`} />
              <b>{s.name}</b>
              <span>{s.host}</span>
              {!s.up ? <span className="st-server-bad">{t('does not answer')}</span> : s.alerts ? <span className="st-server-warn">{tn(s.alerts, '{n} alert', '{n} alerts')}</span> : null}
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

/** Le lanceur ⊞ : retour au cockpit, et les applis pour passer de l'une à l'autre. */
export function Launcher({ data, current, onOpen, onCockpit }: {
  data: CockpitData;
  /** L'instance ouverte dans cet onglet (`livecut@strata02`), mise en avant. */
  current?: string;
  onOpen: (i: CockpitInstance) => void;
  onCockpit: () => void;
}) {
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const close = useRef((focus: boolean) => {
    setOpen(false);
    if (focus) btn.current?.focus();
  }).current;
  useMenuKeys(open, menu, close);
  const pick = (i: CockpitInstance) => {
    setOpen(false);
    onOpen(i);
  };

  return (
    <div className="st-launcher">
      <button ref={btn} type="button" className={`st-launcher-btn ${open ? 'on' : ''}`} title={t('Apps')} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Grid3x3 size={17} />
      </button>
      {open && (
        <div ref={menu} className="st-menu st-launcher-menu" role="menu">
          <div className="st-launcher-head">
            <span>{t('Apps')}</span>
            <button
              type="button"
              role="menuitem"
              className="st-btn small ghost"
              onClick={() => {
                setOpen(false);
                onCockpit();
              }}
            >
              <Grid3x3 size={13} /> {t('Cockpit')}
            </button>
          </div>
          {groupByApp(data).map(({ app, list }) => {
            const Icon = app.icon;
            const one = list.length === 1 && live(data, list[0]);
            const here = list.some((i) => instanceKey(i) === current);
            const row = (
              <>
                <span className="st-launcher-icon">
                  <Icon size={15} />
                </span>
                <strong>{app.name}</strong>
              </>
            );
            if (one)
              return (
                <button key={app.id} type="button" role="menuitem" className={`st-launcher-row click ${here ? 'on' : ''}`} onClick={() => pick(list[0])}>
                  {row}
                  {data.admin && <span className="st-muted">{serverOf(data, list[0].server)?.name}</span>}
                </button>
              );
            return (
              <div key={app.id} className={`st-launcher-row ${here ? 'on' : ''}`}>
                {row}
                {list.map((i) => (
                  <InstanceButton key={instanceKey(i)} data={data} inst={i} onOpen={pick} menu />
                ))}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/**
 * La barre du haut, la même dans le Core et dans chaque appli : `start` (le lanceur), la marque,
 * `children` (les onglets de l'appli), puis `end` (pastille, aide, réglages, utilisateur).
 */
export function TopBar({ module, start, end, children }: { module: string; start?: ReactNode; end?: ReactNode; children?: ReactNode }) {
  return (
    <nav className="st-topbar">
      {start}
      <Brand module={module} />
      {children}
      <div className="st-spacer" />
      {end}
    </nav>
  );
}
