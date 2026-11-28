// Réglages › Applis : la Bibliothèque (toutes les applis du serveur) et la page de chaque appli.
import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowUpRight, Cloud, Download, LayoutGrid, PackagePlus, Power, RefreshCw, RotateCw, ScrollText, Server, Store } from 'lucide-react';
import { Badge, Card, Empty, Field, FieldGrid, PageHead, Switch, formatDuration, formatTime, openAppTab, t, tn, toast } from '@strata/ui';
import { api } from '../api';
import { friendly, pollError } from '../errors';
import { appIcon } from '../icons';
import { navigate } from '../router';
import { useJsonState, useMounted, useRepeat } from '../poll';
import { InstallModal, JobLine, RemoveCard, SourceCard, running, type Job } from './Install';

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
  /** Son dépôt GitHub (et s'il a une clé), ou rien : installée depuis une archive, ou à la main. */
  source: { repo: string; key: boolean } | null;
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
  // Relues sans cesse : la page n'est redessinée que si elles ont changé.
  const [apps, setApps] = useJsonState<AppInfo[] | null>(null);
  const [core, setCore] = useState('');
  const [jobs, setJobs] = useJsonState<Job[]>([]);
  const mounted = useMounted();
  // L'étape vue de chaque installation : sa fin est dite une seule fois.
  const seen = useRef(new Map<string, Job['step']>());
  const reload = useCallback(async () => {
    try {
      const r = await api<{ core: string; apps: AppInfo[]; jobs: Job[] }>('GET', '/api/apps');
      if (!mounted()) return;
      for (const j of r.jobs) {
        const before = seen.current.get(j.repo);
        if (before && before !== j.step) {
          const name = r.apps.find((a) => a.id === j.app)?.name ?? j.app ?? j.repo;
          if (j.step === 'done') toast(t('{app} {version} installed', { app: name, version: j.version ?? '' }));
          if (j.step === 'failed') toast(friendly(j.error ?? ''), 'error');
        }
        seen.current.set(j.repo, j.step);
      }
      setApps(r.apps);
      setCore(r.core);
      setJobs(r.jobs);
    } catch (e) {
      if (mounted()) pollError(e);
    }
  }, [mounted, setApps, setJobs]);
  useEffect(() => {
    void reload();
  }, [reload]);
  // Pendant une installation : relu chaque seconde.
  const busy = jobs.some(running);
  useRepeat(busy && 'jobs', 1000, () => reload(), { now: false });
  const dismiss = async (j: Job) => {
    await api('POST', '/api/apps/jobs/dismiss', { repo: j.repo }).catch(() => {});
    await reload();
  };
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
  return { apps, core, jobs, reload, setActive, restart, dismiss };
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

export function Library({ apps, jobs, setActive, reload, dismiss }: {
  apps: AppInfo[];
  jobs: Job[];
  setActive: (a: AppInfo, on: boolean) => void;
  reload: () => Promise<void>;
  dismiss: (j: Job) => Promise<void>;
}) {
  const active = apps.filter((a) => a.active).length;
  const [installing, setInstalling] = useState(false);
  const shown = jobs.filter((j) => running(j) || j.step === 'failed');
  return (
    <>
      <PageHead title={t('Library')} sub={`${tn(apps.length, '{n} app', '{n} apps')} · ${tn(active, '{n} active', '{n} actives')}`}>
        <button type="button" className="st-btn primary" onClick={() => setInstalling(true)}>
          <PackagePlus size={14} /> {t('Install an app')}
        </button>
      </PageHead>
      {shown.length > 0 && (
        <div className="core-jobs">
          {shown.map((j) => (
            <JobLine key={j.repo} job={j} name={apps.find((a) => a.id === j.app)?.name} onDismiss={() => void dismiss(j)} />
          ))}
        </div>
      )}
      {installing && <InstallModal onClose={() => setInstalling(false)} onStarted={() => void reload()} />}
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
      <Catalog jobs={jobs} reload={reload} />
    </>
  );
}

interface CatalogApp {
  repo: string;
  name: string;
  icon: string;
  description: string;
  app: string | null;
  /** La dernière version qui accepte ce Core. */
  version: string | null;
  /** Pourquoi il n'y a rien à installer. */
  error: string | null;
  /** La version installée ici. */
  installed: string | null;
  active: boolean;
}

/** Le catalogue des applis Strata : ce qui peut s'installer d'un clic. */
function Catalog({ jobs, reload }: { jobs: Job[]; reload: () => Promise<void> }) {
  const [list, setList] = useState<CatalogApp[] | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(async (refresh = false) => {
    setBusy(true);
    try {
      setList((await api<{ apps: CatalogApp[] }>('GET', `/api/apps/catalog${refresh ? '?refresh=true' : ''}`)).apps);
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  }, []);
  // Relu à l'ouverture, et quand une installation se termine.
  const done = jobs.filter((j) => !running(j)).length;
  useEffect(() => {
    void load();
  }, [load, done]);
  const install = async (c: CatalogApp) => {
    try {
      await api('POST', '/api/apps/install', { repo: c.repo });
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  return (
    <Card
      icon={<Store size={16} />}
      title={t('Strata apps')}
      actions={
        <button type="button" className="st-btn small" disabled={busy} onClick={() => void load(true)} data-tip={t('Ask GitHub again')}>
          <RefreshCw size={13} className={busy ? 'spin' : undefined} /> {t('Refresh')}
        </button>
      }
    >
      {list === null ? (
        <p className="st-muted core-note">{t('Asking GitHub…')}</p>
      ) : (
        <div className="core-catalog">
          {list.map((c) => {
            const Icon = appIcon(c.icon);
            const job = jobs.find((j) => j.repo.toLowerCase() === c.repo.toLowerCase() && running(j));
            const newer = !!(c.installed && c.version && c.version !== c.installed);
            return (
              <div key={c.repo} className="core-catalog-item">
                <span className="core-app-icon">
                  <Icon size={18} />
                </span>
                <div className="core-catalog-text">
                  <strong>{c.name}</strong>
                  {c.description && <span className="st-muted">{c.description}</span>}
                  <span className="st-mono st-muted core-catalog-repo">{c.repo}</span>
                </div>
                <div className="core-catalog-state">
                  {c.installed ? (
                    <Badge tone="ok">{newer ? t('Installed {v} · {n} available', { v: c.installed, n: c.version ?? '' }) : t('Installed {v}', { v: c.installed })}</Badge>
                  ) : c.version ? (
                    <button type="button" className="st-btn small primary" disabled={!!job} onClick={() => void install(c)}>
                      <Download size={13} /> {job ? t('Installing…') : t('Install {v}', { v: c.version })}
                    </button>
                  ) : (
                    <span className="st-muted core-catalog-why">{!c.error || / has no Strata release$/.test(c.error) ? t('No release yet') : friendly(new Error(c.error))}</span>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}

/** Les dernières lignes du journal d'une appli lancée par le Core, relues toutes les 2 s. */
function AppLog({ id }: { id: string }) {
  const [lines, setLines] = useJsonState<LogLine[] | null>(null);
  const box = useRef<HTMLDivElement>(null);
  useRepeat(id, 2000, (alive) =>
    api<{ lines: LogLine[] }>('GET', `/api/apps/${id}/log`)
      .then((r) => alive() && setLines(r.lines))
      .catch(() => {}),
  );
  // Collé en bas, sauf si on remonte lire.
  const atBottom = useRef(true);
  useEffect(() => {
    const el = box.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [lines]);
  if (!lines) return null;
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
          <span className="core-log-t">{formatTime(l.t)}</span>
          <span>{l.text}</span>
        </div>
      ))}
    </div>
  );
}

export function AppPage({ app: a, core, job, setActive, restart, reload }: {
  app: AppInfo;
  core: string;
  /** Sa mise à jour en cours, s'il y en a une. */
  job?: Job;
  setActive: (a: AppInfo, on: boolean) => void;
  restart: (a: AppInfo) => Promise<void>;
  reload: () => Promise<void>;
}) {
  const Icon = appIcon(a.icon);
  const [restarting, setRestarting] = useState(false);
  const [sending, setSending] = useState(false);
  // L'état du service bouge tout seul (démarrage, relance) : relu toutes les 2 s.
  useRepeat(a.id, 2000, () => reload(), { now: false });
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
      <SourceCard app={a} job={job} reload={reload} onUpload={() => setSending(true)} />
      <RemoveCard app={a} reload={reload} />
      {sending && <InstallModal from="file" onClose={() => setSending(false)} onStarted={() => void reload()} />}
    </>
  );
}
