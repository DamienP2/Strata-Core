// Réglages › Applis : la Bibliothèque (toutes les applis du serveur) et la page de chaque appli.
import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowUpRight, Cloud, LayoutGrid, Power, RotateCw, ScrollText, Server } from 'lucide-react';
import { Badge, Card, Empty, Field, FieldGrid, PageHead, Switch, formatDuration, openAppTab, t, tn, toast } from '@strata/ui';
import { api } from '../api';
import { friendly } from '../errors';
import { appIcon } from '../icons';
import { navigate } from '../router';

export interface AppInfo {
  id: string;
  name: string;
  version: string;
  icon: string;
  port: number;
  requires: string;
  compatible: boolean;
  active: boolean;
  url: string;
  /** Les chemins servis sans connexion. */
  public: string[];
  run: RunInfo;
}

/** Le service de l'appli : lancé par le Core (`managed`) ou à part ; `answers` : son port répond. */
export interface RunInfo {
  managed: boolean;
  answers: boolean;
  state: 'stopped' | 'starting' | 'running' | 'crashed';
  pid?: number;
  restarts: number;
  /** Secondes Unix. */
  since: number;
  exit?: string;
}

interface LogLine {
  t: number;
  src: 'out' | 'err' | 'core';
  text: string;
}

/** L'état du service en un mot, pour la pastille de la liste de gauche. */
export function serviceTone(a: AppInfo): 'ok' | 'warn' | 'bad' | null {
  if (!a.active) return null;
  if (a.run.answers) return 'ok';
  if (a.run.managed && a.run.state === 'starting') return 'warn';
  return 'bad';
}

/** Les applis du serveur, relues à la demande ; partagées par la liste de gauche et les pages. */
export function useApps() {
  const [apps, setApps] = useState<AppInfo[] | null>(null);
  const [core, setCore] = useState('');
  const reload = useCallback(async () => {
    try {
      const r = await api<{ core: string; apps: AppInfo[] }>('GET', '/api/apps');
      setApps(r.apps);
      setCore(r.core);
    } catch (e) {
      toast(friendly(e), 'error');
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  /** Active ou désactive une appli ; la liste suit. */
  const setActive = async (a: AppInfo, on: boolean) => {
    try {
      const next = await api<AppInfo>('PATCH', `/api/apps/${a.id}`, { active: on });
      setApps((list) => list?.map((x) => (x.id === a.id ? next : x)) ?? null);
      toast(t(on ? '{name} activated' : '{name} deactivated', { name: a.name }));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  /** Relance une appli lancée par le Core. */
  const restart = async (a: AppInfo) => {
    try {
      const next = await api<AppInfo>('POST', `/api/apps/${a.id}/restart`);
      setApps((list) => list?.map((x) => (x.id === a.id ? next : x)) ?? null);
      toast(t('{name} restarted', { name: a.name }));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  return { apps, core, reload, setActive, restart };
}

const open = (a: AppInfo) => openAppTab(a.url, `${a.id}@local`);

/** Ce que fait son service, en une pastille. */
function ServiceBadge({ a }: { a: AppInfo }) {
  const r = a.run;
  if (!a.active) return <Badge>{t('Stopped')}</Badge>;
  if (r.answers) return <Badge tone="ok">{t('Running')}</Badge>;
  if (!r.managed) return <Badge tone="bad">{t('Does not answer')}</Badge>;
  if (r.state === 'starting') return <Badge tone="warn">{t('Starting')}</Badge>;
  if (r.state === 'crashed') return <Badge tone="bad">{t('Stopped by itself, restarting')}</Badge>;
  return <Badge tone="bad">{t('Does not answer')}</Badge>;
}

function StateBadge({ a }: { a: AppInfo }) {
  if (!a.compatible) return <Badge tone="warn">{t('Needs the Core {range}', { range: a.requires })}</Badge>;
  return a.active ? <Badge tone="ok">{t('Active')}</Badge> : <Badge>{t('Inactive')}</Badge>;
}

export function Library({ apps, setActive }: { apps: AppInfo[]; setActive: (a: AppInfo, on: boolean) => void }) {
  const active = apps.filter((a) => a.active).length;
  return (
    <>
      <PageHead title={t('Library')} sub={`${tn(apps.length, '{n} app', '{n} apps')} · ${tn(active, '{n} active', '{n} actives')}`} />
      {apps.length === 0 ? (
        <div className="core-empty">
          <Empty icon={LayoutGrid} text={t('No app installed')} />
        </div>
      ) : (
        <div className="core-cards">
          {apps.map((a) => {
            const Icon = appIcon(a.icon);
            return (
              <div key={a.id} className={`core-libcard ${a.active ? '' : 'off'}`}>
                <button type="button" className="core-libcard-h" onClick={() => navigate(`/reglages/applis/${a.id}`)}>
                  <span className="core-app-icon">
                    <Icon size={18} />
                  </span>
                  <span className="core-libcard-name">
                    <strong>{a.name}</strong>
                    <span className="st-muted st-mono">{a.version}</span>
                  </span>
                  {!a.compatible && <StateBadge a={a} />}
                </button>
                <div className="core-libcard-f">
                  <Switch checked={a.active} disabled={!a.compatible} onChange={(on) => setActive(a, on)} label={t('Active')} />
                  <div className="st-spacer" />
                  {a.active && <ServiceBadge a={a} />}
                  {a.active && (
                    <button type="button" className="st-btn small" disabled={!a.run.answers} onClick={() => open(a)}>
                      <ArrowUpRight size={13} /> {t('Open')}
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </>
  );
}

/** Les dernières lignes du journal d'une appli lancée par le Core, relues toutes les 2 s. */
function AppLog({ id }: { id: string }) {
  const [lines, setLines] = useState<LogLine[] | null>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true;
    const load = () =>
      api<{ lines: LogLine[] }>('GET', `/api/apps/${id}/log`)
        .then((r) => live && setLines(r.lines))
        .catch(() => {});
    void load();
    const timer = setInterval(load, 2000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [id]);
  // Collé en bas, sauf si on remonte lire.
  const atBottom = useRef(true);
  useEffect(() => {
    const el = box.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [lines]);
  if (!lines) return null;
  const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  return (
    <div
      ref={box}
      className="core-log"
      onScroll={(e) => {
        const el = e.currentTarget;
        atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      }}
    >
      {lines.length === 0 && <span className="st-muted">{t('Nothing yet')}</span>}
      {lines.map((l, i) => (
        <div key={i} className={`core-log-line ${l.src}`}>
          <span className="core-log-t">{time(l.t)}</span>
          <span>{l.text}</span>
        </div>
      ))}
    </div>
  );
}

export function AppPage({ app: a, core, setActive, restart, reload }: {
  app: AppInfo;
  core: string;
  setActive: (a: AppInfo, on: boolean) => void;
  restart: (a: AppInfo) => Promise<void>;
  reload: () => Promise<void>;
}) {
  const Icon = appIcon(a.icon);
  const [restarting, setRestarting] = useState(false);
  // L'état du service bouge tout seul (démarrage, relance) : relu toutes les 2 s.
  useEffect(() => {
    const timer = setInterval(() => void reload(), 2000);
    return () => clearInterval(timer);
  }, [reload]);
  const r = a.run;
  const since = r.since ? formatDuration(Date.now() / 1000 - r.since) : '';
  return (
    <>
      <div className="core-apphead">
        <span className={`core-app-icon big ${a.active ? '' : 'off'}`}>
          <Icon size={26} />
        </span>
        <div>
          <h1>{a.name}</h1>
          <p className="st-muted st-mono">{a.url}</p>
        </div>
        <div className="st-spacer" />
        {a.active && (
          <button type="button" className="st-btn primary" disabled={!r.answers} onClick={() => open(a)}>
            <ArrowUpRight size={14} /> {t('Open')}
          </button>
        )}
      </div>
      <Card icon={<Power size={16} />} title={t('State')}>
        <div className="st-row">
          <Switch checked={a.active} disabled={!a.compatible} onChange={(on) => setActive(a, on)} label={t('Active')} />
          {!a.compatible && <StateBadge a={a} />}
        </div>
      </Card>
      <Card icon={<Server size={16} />} title={t('Service')}>
        <div className="core-service">
          <ServiceBadge a={a} />
          {a.active && r.managed && r.pid && <span className="st-muted">{t('Process {pid} · for {time}', { pid: r.pid, time: since })}</span>}
          {a.active && r.managed && r.restarts > 0 && <span className="st-muted">{tn(r.restarts, '{n} restart', '{n} restarts')}</span>}
          {a.active && r.exit && r.state !== 'running' && <span className="st-muted">{t('Last stop: {why}', { why: r.exit })}</span>}
          <div className="st-spacer" />
          {a.active && r.managed && (
            <button
              type="button"
              className="st-btn"
              disabled={restarting}
              onClick={async () => {
                setRestarting(true);
                await restart(a);
                setRestarting(false);
              }}
            >
              <RotateCw size={14} /> {t('Restart')}
            </button>
          )}
        </div>
        {!r.managed && <p className="st-muted core-note">{t('This app is started apart from the Core (no [run] in its manifest): the Core only relays it.')}</p>}
        <FieldGrid two>
          <Field label={t('Address')}>
            <input value={a.url} readOnly />
          </Field>
          <Field label={t('Internal port')}>
            <input value={String(a.port)} readOnly />
          </Field>
        </FieldGrid>
        {a.public.length > 0 && (
          <Field label={t('Open without signing in')} group>
            <div className="core-chips">
              {a.public.map((p) => (
                <span key={p} className="core-tag st-mono">
                  {p}
                </span>
              ))}
            </div>
          </Field>
        )}
      </Card>
      {r.managed && (
        <Card icon={<ScrollText size={16} />} title={t('Log')}>
          <AppLog id={a.id} />
        </Card>
      )}
      <Card icon={<Cloud size={16} />} title={t('Version')}>
        <FieldGrid>
          <Field label={t('Installed version')}>
            <input value={a.version} readOnly />
          </Field>
          <Field label={t('Accepted Core versions')}>
            <input value={a.requires} readOnly />
          </Field>
          <Field label={t('This Core')}>
            <input value={core} readOnly />
          </Field>
        </FieldGrid>
      </Card>
    </>
  );
}
