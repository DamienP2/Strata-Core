// Réglages › Ce serveur › Mises à jour : les versions du Core et des applis, la recherche (à la
// demande et chaque jour), la mise à jour du Core (depuis GitHub ou une archive), l'accès GitHub.
import { useCallback, useEffect, useRef, useState } from 'react';
import { CloudDownload, Download, FileArchive, KeyRound, RefreshCw } from 'lucide-react';
import { Badge, Card, Field, Modal, PageHead, Switch, confirmDialog, t, timeAgo, tn, toast } from '@strata/ui';
import { api, expectRestart, upload } from '../api';
import { friendly } from '../errors';
import { JobLine, running, type Job } from './Install';
import type { AppInfo } from './Library';

interface CoreCheck {
  latest?: string;
  newer: boolean;
  blocked: { id: string; name: string; version: string; needs: string }[];
  error?: string;
}

interface AppCheck {
  latest?: string;
  newer: boolean;
  error?: string;
}

export interface UpdatesInfo {
  core: { version: string; exe: string; repo: string; key: boolean };
  daily: boolean;
  count: number;
  checked: { at?: number; core?: CoreCheck; apps: Record<string, AppCheck> };
  job: Job | null;
}

/** L'état des mises à jour ; relu chaque seconde pendant une mise à jour du Core. */
export function useUpdates() {
  const [info, setInfo] = useState<UpdatesInfo | null>(null);
  const reload = useCallback(async () => {
    try {
      setInfo(await api<UpdatesInfo>('GET', '/api/updates'));
    } catch {
      /* pendant un redémarrage */
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  const busy = !!info?.job && running(info.job);
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => void reload(), 1000);
    return () => clearInterval(timer);
  }, [busy, reload]);
  return { info, setInfo, reload };
}

/** Une clé d'accès GitHub à remplacer ou retirer. */
function KeyDialog({ repo, has, onClose, onSave }: { repo: string; has: boolean; onClose: () => void; onSave: (key: string) => Promise<void> }) {
  const [key, setKey] = useState('');
  return (
    <Modal
      title={t('Access key of {repo}', { repo })}
      icon={<KeyRound size={16} />}
      onClose={onClose}
      onSubmit={() => void onSave(key.trim())}
      footer={
        <>
          {has && (
            <button type="button" className="st-btn danger" onClick={() => void onSave('')}>
              {t('Remove the key')}
            </button>
          )}
          <div className="st-spacer" />
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={!key.trim()}>
            {t('Save')}
          </button>
        </>
      }
    >
      <Field label={t('New key')} hint={t('A read-only GitHub token. Kept on the server, never shown again.')}>
        <input type="password" value={key} autoComplete="off" onChange={(e) => setKey(e.target.value)} />
      </Field>
    </Modal>
  );
}

export function Updates({ apps, jobs, reloadApps, onCount }: { apps: AppInfo[] | null; jobs: Job[]; reloadApps: () => Promise<void>; onCount: (n: number) => void }) {
  const { info, setInfo } = useUpdates();
  // Le compteur du menu suit chaque recherche.
  const count = info?.count ?? 0;
  useEffect(() => onCount(count), [count, onCount]);
  const [checking, setChecking] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [sent, setSent] = useState<number | null>(null);
  const [keyFor, setKeyFor] = useState<{ repo: string; has: boolean; app?: string } | null>(null);
  const picker = useRef<HTMLInputElement>(null);
  const done = info?.job?.step === 'done';

  // La mise à jour du Core est en place : il redémarre, la page suit.
  useEffect(() => {
    if (done && !restarting) {
      setRestarting(true);
      expectRestart('update');
    }
  }, [done, restarting]);

  if (!info) return null;
  const c = info.checked;

  const check = async () => {
    setChecking(true);
    try {
      setInfo(await api<UpdatesInfo>('POST', '/api/updates/check'));
      await reloadApps();
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setChecking(false);
  };
  const updateCore = async () => {
    const blocked = c.core?.blocked ?? [];
    const lines = blocked.map((b) => t('{app} {version} accepts only the Core {range}', { app: b.name, version: b.version, range: b.needs }));
    const ok = await confirmDialog({
      title: t('Update the Core to {version}?', { version: c.core?.latest ?? '' }),
      message: [t('The Core restarts at the end, with the apps it starts.'), ...lines].join('\n'),
      confirm: t('Update'),
      danger: blocked.length > 0,
    });
    if (!ok) return;
    try {
      setInfo(await api<UpdatesInfo>('POST', '/api/updates/core'));
      expectRestart('update');
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const updateApp = async (a: AppInfo) => {
    try {
      await api('POST', `/api/apps/${a.id}/update`);
      await reloadApps();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const sendArchive = async (file: File) => {
    setSent(0);
    try {
      const r = await upload<{ version: string }>('/api/updates/core/upload', file, setSent);
      toast(t('Core {version} installed; restarting…', { version: r.version }));
      setRestarting(true);
      expectRestart('update');
    } catch (e) {
      toast(friendly(e), 'error');
      setSent(null);
    }
  };
  const saveKey = async (key: string) => {
    if (!keyFor) return;
    try {
      if (keyFor.app) {
        await api('PUT', `/api/apps/${keyFor.app}/key`, { key });
        await reloadApps();
      } else {
        setInfo(await api<UpdatesInfo>('PUT', '/api/updates/core/key', { key }));
      }
      toast(key ? t('Key saved') : t('Key removed'));
      setKeyFor(null);
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };

  const coreCell = () => {
    if (!c.core) return <span className="st-muted">—</span>;
    if (c.core.error) return <Badge tone="bad">{friendly(c.core.error)}</Badge>;
    if (!c.core.newer) return <Badge tone="ok">{t('Up to date')}</Badge>;
    return (
      <span className="core-service">
        <span className="st-mono">{c.core.latest}</span>
        {c.core.blocked.length > 0 && <Badge tone="warn">{tn(c.core.blocked.length, '{n} app would not accept it', '{n} apps would not accept it')}</Badge>}
      </span>
    );
  };
  const appCell = (a: AppInfo) => {
    if (!a.source) return <span className="st-muted">{t('Installed from an archive')}</span>;
    const x = c.apps[a.id];
    if (!x) return <span className="st-muted">—</span>;
    if (x.error) return <Badge tone="bad">{friendly(x.error)}</Badge>;
    return x.newer ? <span className="st-mono">{x.latest}</span> : <Badge tone="ok">{t('Up to date')}</Badge>;
  };
  const coreJob = info.job;
  const sub = `Core ${info.core.version} · ${
    !c.at ? t('Not checked yet') : info.count ? tn(info.count, '{n} update available', '{n} updates available') : t('Everything is up to date')
  }${c.at ? ` · ${t('checked {when}', { when: timeAgo(c.at * 1000) })}` : ''}`;

  return (
    <>
      <PageHead title={t('Updates')} sub={sub}>
        <button type="button" className="st-btn" disabled={checking} onClick={() => void check()}>
          <RefreshCw size={14} /> {checking ? t('Checking…') : t('Check now')}
        </button>
      </PageHead>
      {restarting && <div className="core-job">{t('The Core is restarting; this page reloads by itself.')}</div>}
      <Card icon={<CloudDownload size={16} />} title={t('Versions')}>
        <table className="st-table">
          <thead>
            <tr>
              <th>{t('Component')}</th>
              <th>{t('Installed')}</th>
              <th>{t('Available')}</th>
              <th className="r" />
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>
                <strong>Core</strong>
              </td>
              <td className="st-mono">{info.core.version}</td>
              <td>{coreCell()}</td>
              <td className="r">
                {c.core?.newer && !(coreJob && running(coreJob)) && !restarting && (
                  <button type="button" className="st-btn small primary" onClick={() => void updateCore()}>
                    <Download size={13} /> {t('Update')}
                  </button>
                )}
              </td>
            </tr>
            {apps?.map((a) => {
              const job = jobs.find((j) => j.app === a.id && running(j));
              return (
                <tr key={a.id}>
                  <td>
                    <strong>{a.name}</strong> {!a.active && <Badge>{t('Inactive')}</Badge>}
                  </td>
                  <td className="st-mono">{a.version}</td>
                  <td>{appCell(a)}</td>
                  <td className="r">
                    {c.apps[a.id]?.newer && !job && a.version !== c.apps[a.id]?.latest && (
                      <button type="button" className="st-btn small primary" onClick={() => void updateApp(a)}>
                        <Download size={13} /> {t('Update')}
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        {coreJob && (running(coreJob) || coreJob.step === 'failed') && <JobLine job={coreJob} name="Core" />}
        {jobs.filter((j) => j.app && running(j)).map((j) => (
          <JobLine key={j.repo} job={j} name={apps?.find((a) => a.id === j.app)?.name} />
        ))}
        <Switch
          checked={info.daily}
          onChange={async (daily) => {
            try {
              setInfo(await api<UpdatesInfo>('PUT', '/api/updates/settings', { daily }));
            } catch (e) {
              toast(friendly(e), 'error');
            }
          }}
          label={t('Look for updates every day (they are never installed by themselves)')}
        />
      </Card>
      <Card icon={<FileArchive size={16} />} title={t('Offline update')}>
        <div className="core-service">
          <span className="st-muted">{t('The Core’s .tar.gz archive, taken from its release (for a server without internet).')}</span>
          <div className="st-spacer" />
          <input ref={picker} type="file" accept=".gz,.tgz,application/gzip" hidden onChange={(e) => e.target.files?.[0] && void sendArchive(e.target.files[0])} />
          <button type="button" className="st-btn" disabled={sent !== null || restarting} onClick={() => picker.current?.click()}>
            <FileArchive size={14} /> {t('Send an archive')}
          </button>
        </div>
        {sent !== null && !restarting && (
          <div className="core-progress">
            <span style={{ width: `${Math.round(sent * 100)}%` }} />
          </div>
        )}
        <p className="st-muted core-note st-mono">{info.core.exe}</p>
      </Card>
      <Card icon={<KeyRound size={16} />} title={t('GitHub access')}>
        <table className="st-table">
          <tbody>
            <tr>
              <td className="st-mono">{info.core.repo}</td>
              <td>Core</td>
              <td>{info.core.key ? <Badge tone="ok">{t('Key saved')}</Badge> : <Badge>{t('No key')}</Badge>}</td>
              <td className="r">
                <button type="button" className="st-btn small" onClick={() => setKeyFor({ repo: info.core.repo, has: info.core.key })}>
                  {t('Replace')}
                </button>
              </td>
            </tr>
            {apps
              ?.filter((a) => a.source)
              .map((a) => (
                <tr key={a.id}>
                  <td className="st-mono">{a.source!.repo}</td>
                  <td>{a.name}</td>
                  <td>{a.source!.key ? <Badge tone="ok">{t('Key saved')}</Badge> : <Badge>{t('No key')}</Badge>}</td>
                  <td className="r">
                    <button type="button" className="st-btn small" onClick={() => setKeyFor({ repo: a.source!.repo, has: a.source!.key, app: a.id })}>
                      {t('Replace')}
                    </button>
                  </td>
                </tr>
              ))}
          </tbody>
        </table>
        <p className="st-muted core-note">{t('A private repository needs a read-only key. Public repositories need none.')}</p>
      </Card>
      {keyFor && <KeyDialog repo={keyFor.repo} has={keyFor.has} onClose={() => setKeyFor(null)} onSave={saveKey} />}
    </>
  );
}

