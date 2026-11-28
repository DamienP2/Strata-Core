// Réglages › Ce serveur › Sauvegardes : les réglages du Core (comptes, matériel, flux, HTTPS,
// alertes…) dans une archive. Faite à la main, chaque jour, et avant chaque restauration ; à
// télécharger, ou à envoyer depuis un autre serveur. Une restauration redémarre le Core.
import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Archive, Check, Download, FileArchive, History, HardDrive, Plus, Trash2, Upload } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, Field, FieldGrid, InfoTip, Modal, PageHead, SaveBar, Switch, confirmDialog, t, timeAgo, tn, toast } from '@strata/ui';
import { navigate } from '../router';
import { KIND_TAG, useStorage } from './Storage';
import { api, expectRestart, upload } from '../api';
import { friendly } from '../errors';

interface Manifest {
  coreVersion: string;
  serverName: string;
  dnsName: string;
  createdAt: number;
  kind: 'manual' | 'daily' | 'before-restore' | 'uploaded' | string;
  apps: boolean;
}
interface CopySettings {
  target: string;
  folder: string;
  keep: number;
}
interface LastCopy {
  at: number;
  name: string;
  ok: boolean;
  error: string;
}
interface Backup {
  name: string;
  size: number;
  manifest: Manifest;
}

function size(n: number) {
  if (n >= 1 << 30) return t('{n} GB', { n: (n / (1 << 30)).toFixed(1) });
  if (n >= 1 << 20) return t('{n} MB', { n: (n / (1 << 20)).toFixed(1) });
  return t('{n} KB', { n: Math.max(1, Math.round(n / 1024)) });
}

function kindLabel(k: string) {
  return k === 'manual' ? t('By hand') : k === 'daily' ? t('Daily') : k === 'before-restore' ? t('Before a restore') : k === 'uploaded' ? t('Uploaded') : k;
}

export function Backups() {
  const [list, setList] = useState<Backup[] | null>(null);
  const [copy, setCopy] = useState<CopySettings | null>(null);
  const [last, setLast] = useState<LastCopy | null>(null);
  const [defaultFolder, setDefaultFolder] = useState('');
  const [making, setMaking] = useState(false);
  const [withApps, setWithApps] = useState(false);
  const [sent, setSent] = useState<number | null>(null);
  const [restoring, setRestoring] = useState<Backup | null>(null);
  const [restarting, setRestarting] = useState(false);
  const picker = useRef<HTMLInputElement>(null);

  const reload = useCallback(async () => {
    try {
      const r = await api<{ backups: Backup[]; copy: CopySettings; lastCopy: LastCopy; defaultFolder: string }>('GET', '/api/backups');
      setDefaultFolder(r.defaultFolder);
      setList(r.backups);
      setCopy(r.copy);
      setLast(r.lastCopy.at ? r.lastCopy : null);
    } catch (e) {
      toast(friendly(e), 'error');
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  const make = async () => {
    setMaking(true);
    try {
      await api('POST', '/api/backups', { apps: withApps });
      toast(t('Backup made'));
      await reload();
      // Sa copie vers le serveur de stockage se fait derrière : son résultat, relu un peu après.
      if (copy?.target) window.setTimeout(() => void reload(), 4000);
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setMaking(false);
  };

  const send = async (file: File) => {
    setSent(0);
    try {
      await upload('/api/backups/upload', file, setSent);
      toast(t('Backup received'));
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setSent(null);
    if (picker.current) picker.current.value = '';
  };

  const remove = async (b: Backup) => {
    if (!(await confirmDialog({ title: t('Delete this backup?'), message: b.name, confirm: t('Delete'), danger: true }))) return;
    try {
      await api('DELETE', `/api/backups/${encodeURIComponent(b.name)}`);
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };

  return (
    <>
      <PageHead title={t('Backups')} sub={list ? tn(list.length, '{n} backup', '{n} backups') : undefined}>
        <input ref={picker} type="file" accept=".gz,.tgz,application/gzip" hidden onChange={(e) => e.target.files?.[0] && void send(e.target.files[0])} />
        <button type="button" className="st-btn" disabled={sent !== null || restarting} onClick={() => picker.current?.click()}>
          <FileArchive size={14} /> {t('Send a backup')}
        </button>
        <button type="button" className="st-btn primary" disabled={making || restarting} onClick={() => void make()}>
          <Plus size={14} /> {making ? t('Backing up…') : t('Back up now')}
        </button>
      </PageHead>
      {restarting && <div className="core-job">{t('The Core is restarting; this page reloads by itself.')}</div>}
      {sent !== null && (
        <div className="core-progress">
          <span style={{ width: `${Math.round(sent * 100)}%` }} />
        </div>
      )}
      {copy && <CopyCard saved={copy} last={last} defaultFolder={defaultFolder} onSaved={(c) => setCopy(c)} reload={reload} />}
      <Card
        icon={<Archive size={16} />}
        title={t('Backups of this server')}
        actions={
          <InfoTip
            text={t(
              'One a day (the last 14 are kept), and one just before each restore. They hold secrets (password hashes, keys, SSO and e-mail passwords): keep the downloaded files safe.',
            )}
          />
        }
      >
        <Switch checked={withApps} onChange={setWithApps} label={t('With the installed apps and their data (bigger)')} />
        {list === null ? null : list.length === 0 ? (
          <Empty icon={Archive} text={t('No backup yet')} />
        ) : (
          <table className="st-table">
            <thead>
              <tr>
                <th>{t('Made')}</th>
                <th>{t('Server')}</th>
                <th>{t('Content')}</th>
                <th className="r">{t('Size')}</th>
                <th className="r" />
              </tr>
            </thead>
            <tbody>
              {list.map((b) => (
                <tr key={b.name}>
                  <td>
                    <strong>{new Date(b.manifest.createdAt).toLocaleString()}</strong> <span className="st-muted">{timeAgo(b.manifest.createdAt)}</span>
                    <div>
                      <Badge>{kindLabel(b.manifest.kind)}</Badge>
                    </div>
                  </td>
                  <td>
                    {b.manifest.serverName || '—'}
                    <div className="st-muted st-mono">Core {b.manifest.coreVersion}</div>
                  </td>
                  <td>{b.manifest.apps ? t('Settings and apps') : t('Settings')}</td>
                  <td className="r st-mono">{size(b.size)}</td>
                  <td className="r">
                    <div className="core-stream-actions">
                      <a className="st-btn small" href={`/api/backups/${encodeURIComponent(b.name)}`} download title={t('Download')}>
                        <Download size={13} />
                      </a>
                      <button type="button" className="st-btn small" disabled={restarting} onClick={() => setRestoring(b)}>
                        <History size={13} /> {t('Restore')}
                      </button>
                      <button type="button" className="st-btn small" title={t('Delete')} onClick={() => void remove(b)}>
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      {restoring && (
        <RestoreModal
          backup={restoring}
          onClose={() => setRestoring(null)}
          onDone={() => {
            setRestoring(null);
            setRestarting(true);
            expectRestart('restart');
          }}
        />
      )}
    </>
  );
}

function RestoreModal({ backup, onClose, onDone }: { backup: Backup; onClose: () => void; onDone: () => void }) {
  const [apps, setApps] = useState(false);
  const [identity, setIdentity] = useState(false);
  const [busy, setBusy] = useState(false);
  const m = backup.manifest;
  const go = async () => {
    setBusy(true);
    try {
      await api('POST', `/api/backups/${encodeURIComponent(backup.name)}/restore`, { apps, identity });
      toast(t('Settings restored; restarting…'));
      onDone();
    } catch (e) {
      toast(friendly(e), 'error');
      setBusy(false);
    }
  };
  return (
    <Modal
      title={t('Restore this backup?')}
      icon={<History size={16} />}
      onClose={onClose}
      onSubmit={() => void go()}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn danger-solid" disabled={busy}>
            {busy ? t('Restoring…') : t('Restore and restart')}
          </button>
        </>
      }
    >
      <p>
        {t('{server}, {when}', { server: m.serverName || '—', when: new Date(m.createdAt).toLocaleString() })} · Core {m.coreVersion}
      </p>
      <p className="st-muted">
        {t('The settings of this server are replaced by those of the backup (the current ones are backed up first), then the Core restarts. Everyone signs in again.')}
      </p>
      {m.apps && <Switch checked={apps} onChange={setApps} label={t('The apps too (replaces the installed apps and their data)')} />}
      <Switch checked={identity} onChange={setIdentity} label={t('The identity of the server too (the same server, reinstalled)')} />
      {identity && <p className="st-muted core-note">{t('Only if this backup comes from this very server: two servers with the same identity confuse the cockpit.')}</p>}
    </Modal>
  );
}

/** La copie des sauvegardes vers un serveur de stockage (Ce serveur › Stockage). */
function CopyCard({ saved, last, defaultFolder, onSaved, reload }: { saved: CopySettings; last: LastCopy | null; defaultFolder: string; onSaved: (c: CopySettings) => void; reload: () => Promise<void> }) {
  const [storage] = useStorage();
  const [c, setC] = useState<CopySettings>(saved);
  const [busy, setBusy] = useState(false);
  const [copying, setCopying] = useState(false);
  useEffect(() => setC(saved), [saved]);
  const dirty = JSON.stringify(c) !== JSON.stringify(saved);
  const save = async () => {
    setBusy(true);
    try {
      onSaved(await api<CopySettings>('PUT', '/api/backups/copy', c));
      toast(t('Saved'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  };
  const now = async () => {
    setCopying(true);
    try {
      const r = await api<{ name: string }>('POST', '/api/backups/copy', {});
      toast(t('{name} copied to the storage server', { name: r.name }));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setCopying(false);
    await reload();
  };
  const target = storage?.find((s) => s.id === saved.target);
  return (
    <Card
      icon={<HardDrive size={16} />}
      title={t('Copy to a storage server')}
      actions={
        saved.target ? (
          <button type="button" className="st-btn small" disabled={copying || dirty} onClick={() => void now()}>
            <Upload size={13} /> {copying ? t('Copying…') : t('Copy the latest now')}
          </button>
        ) : undefined
      }
    >
      <p className="st-muted core-note">
        {t('Each backup (the daily one, and those made by hand) is copied there, in a folder of this server; only the latest are kept there. A copy that failed is tried again every hour.')}
      </p>
      {storage && storage.length === 0 ? (
        <p className="core-note">
          {t('No storage server yet.')}{' '}
          <button type="button" className="st-btn small" onClick={() => navigate('/reglages/stockage')}>
            <Plus size={13} /> {t('Add one')}
          </button>
        </p>
      ) : (
        <FieldGrid>
          <Field label={t('Storage server')} group>
            <Dropdown
              value={c.target}
              options={[{ value: '', label: t('No copy') }, ...(storage ?? []).map((s) => ({ value: s.id, label: `${s.name} · ${t(KIND_TAG[s.kind])}` }))]}
              onChange={(target) => setC({ ...c, target })}
            />
          </Field>
          <Field label={t('Folder there')} hint={c.target ? `→ ${(storage?.find((s) => s.id === c.target)?.address ?? '').replace(/\/$/, '')}/${c.folder.trim() || defaultFolder}/` : undefined}>
            <input className="st-mono" value={c.folder} disabled={!c.target} placeholder={defaultFolder} onChange={(e) => setC({ ...c, folder: e.target.value })} />
          </Field>
          <Field label={t('Kept there')}>
            <input
              className="st-mono"
              inputMode="numeric"
              disabled={!c.target}
              value={c.keep ? String(c.keep) : ''}
              placeholder={t('all')}
              onChange={(e) => setC({ ...c, keep: Math.min(1000, Number(e.target.value.replace(/[^0-9]/g, '')) || 0) })}
            />
          </Field>
        </FieldGrid>
      )}
      {last && saved.target && (
        <div className={`core-check ${last.ok ? 'ok' : 'bad'}`}>
          {last.ok ? <Check size={14} /> : <AlertTriangle size={14} />}
          <span>
            {last.ok ? t('Last copy: {name}, {when}', { name: last.name, when: timeAgo(last.at) }) : t('Last copy failed ({when}): {why}', { when: timeAgo(last.at), why: friendly(new Error(last.error)) })}
            {target && <span className="st-muted"> · {target.name}</span>}
          </span>
        </div>
      )}
      <SaveBar dirty={dirty} saving={busy} onSave={() => void save()} onDiscard={() => setC(saved)} />
    </Card>
  );
}
