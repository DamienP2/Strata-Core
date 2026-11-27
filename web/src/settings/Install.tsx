// Réglages › Applis : installer une appli (depuis GitHub ou une archive), suivre l'installation, sa
// source et ses mises à jour, la désinstaller.
import { useRef, useState } from 'react';
import { Download, FileArchive, GitBranch, KeyRound, PackagePlus, RefreshCw, Trash2, X } from 'lucide-react';
import { Badge, Card, Field, Modal, Segmented, Switch, t, toast } from '@strata/ui';
import { api, upload } from '../api';
import { friendly } from '../errors';
import { navigate } from '../router';
import type { AppInfo } from './Library';

/** Une installation depuis GitHub, en cours ou finie. */
export interface Job {
  repo: string;
  step: 'looking' | 'downloading' | 'installing' | 'done' | 'failed';
  done: number;
  total: number;
  version?: string;
  app?: string;
  error?: string;
}

export const running = (j: Job) => j.step !== 'done' && j.step !== 'failed';

function Progress({ part }: { part: number | null }) {
  return (
    <div className={`core-progress ${part === null ? 'busy' : ''}`}>
      <span style={part === null ? undefined : { width: `${Math.round(part * 100)}%` }} />
    </div>
  );
}

/** Où en est une installation, en une ligne et une barre. */
export function JobLine({ job, name, onDismiss }: { job: Job; name?: string; onDismiss?: () => void }) {
  const what = name ?? job.app ?? job.repo;
  const v = job.version ?? '';
  const pct = job.total ? Math.min(1, job.done / job.total) : null;
  const text =
    job.step === 'looking'
      ? t('Looking for the latest release of {repo}…', { repo: job.repo })
      : job.step === 'downloading'
        ? `${t('Downloading {app} {version}…', { app: what, version: v })}${pct === null ? '' : ` ${Math.round(pct * 100)} %`}`
        : job.step === 'installing'
          ? t('Installing {app} {version}…', { app: what, version: v })
          : job.step === 'done'
            ? t('{app} {version} installed', { app: what, version: v })
            : friendly(job.error ?? '');
  return (
    <div className={`core-job ${job.step}`}>
      <div className="core-job-row">
        <span>{text}</span>
        {job.step === 'failed' && onDismiss && (
          <button type="button" className="st-icon-btn ghost small" title={t('Close')} onClick={onDismiss}>
            <X size={13} />
          </button>
        )}
      </div>
      {running(job) && <Progress part={job.step === 'downloading' ? pct : null} />}
    </div>
  );
}

/** Installer une appli : depuis son dépôt GitHub, ou depuis une archive (serveur sans internet). */
export function InstallModal({ from: start = 'github', onClose, onStarted }: { from?: 'github' | 'file'; onClose: () => void; onStarted: () => void }) {
  const [from, setFrom] = useState(start);
  const [repo, setRepo] = useState('');
  const [key, setKey] = useState('');
  const [file, setFile] = useState<File | null>(null);
  const [sent, setSent] = useState<number | null>(null);
  const picker = useRef<HTMLInputElement>(null);
  const busy = sent !== null;

  const submit = async () => {
    if (from === 'github') {
      try {
        await api('POST', '/api/apps/install', { repo: repo.trim(), key: key.trim() || undefined });
        onStarted();
        onClose();
      } catch (e) {
        toast(friendly(e), 'error');
      }
      return;
    }
    if (!file) return;
    setSent(0);
    try {
      const app = await upload<AppInfo>('/api/apps/upload', file, setSent);
      toast(t('{app} {version} installed', { app: app.name, version: app.version }));
      onStarted();
      onClose();
      navigate(`/reglages/applis/${app.id}`);
    } catch (e) {
      toast(friendly(e), 'error');
      setSent(null);
    }
  };

  return (
    <Modal
      title={t('Install an app')}
      icon={<PackagePlus size={16} />}
      onClose={busy ? () => {} : onClose}
      onSubmit={() => void submit()}
      footer={
        <>
          <button type="button" className="st-btn" disabled={busy} onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={busy || (from === 'github' ? !repo.trim() : !file)}>
            <Download size={14} /> {t('Install')}
          </button>
        </>
      }
    >
      <Segmented<'github' | 'file'>
        value={from}
        onChange={(v) => !busy && setFrom(v)}
        options={[
          { value: 'github', label: t('From GitHub') },
          { value: 'file', label: t('From a file') },
        ]}
      />
      {from === 'github' ? (
        <>
          <Field label={t('Repository')} hint={t('The latest release that accepts this Core is installed, after checking its fingerprint.')}>
            <input value={repo} placeholder="DamienP2/Strata-Cut" spellCheck={false} autoCapitalize="none" onChange={(e) => setRepo(e.target.value)} />
          </Field>
          <Field label={t('Access key')} hint={t('For a private repository: a read-only GitHub token. Kept on the server, never shown again.')}>
            <input type="password" value={key} autoComplete="off" placeholder={t('Optional')} onChange={(e) => setKey(e.target.value)} />
          </Field>
        </>
      ) : (
        <Field label={t('Release archive')} hint={t('The app’s .tar.gz archive, taken from its release (for a server without internet).')} group>
          <div className="core-file">
            <input ref={picker} type="file" accept=".gz,.tgz,application/gzip" hidden onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
            <button type="button" className="st-btn" disabled={busy} onClick={() => picker.current?.click()}>
              <FileArchive size={14} /> {t('Choose a file')}
            </button>
            <span className="st-muted st-mono">{file ? file.name : t('No file')}</span>
          </div>
          {busy && <Progress part={sent} />}
        </Field>
      )}
    </Modal>
  );
}

/** Changer la clé du dépôt d'une appli, ou la retirer. */
function KeyModal({ app, onClose, onSaved }: { app: AppInfo; onClose: () => void; onSaved: () => void }) {
  const [key, setKey] = useState('');
  const save = async (value: string) => {
    try {
      await api('PUT', `/api/apps/${app.id}/key`, { key: value });
      toast(value ? t('Key saved') : t('Key removed'));
      onSaved();
      onClose();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  return (
    <Modal
      title={t('Access key of {repo}', { repo: app.source?.repo ?? '' })}
      icon={<KeyRound size={16} />}
      onClose={onClose}
      onSubmit={() => void save(key.trim())}
      footer={
        <>
          {app.source?.key && (
            <button type="button" className="st-btn danger" onClick={() => void save('')}>
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

/** D'où vient l'appli, et ses mises à jour. */
export function SourceCard({ app, job, reload, onUpload }: { app: AppInfo; job?: Job; reload: () => Promise<void>; onUpload: () => void }) {
  const [check, setCheck] = useState<{ latest: string; newer: boolean } | null>(null);
  const [checking, setChecking] = useState(false);
  const [keying, setKeying] = useState(false);
  const src = app.source;

  const look = async () => {
    setChecking(true);
    try {
      setCheck(await api<{ latest: string; newer: boolean }>('GET', `/api/apps/${app.id}/update`));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setChecking(false);
  };
  const update = async () => {
    try {
      await api('POST', `/api/apps/${app.id}/update`);
      setCheck(null);
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };

  return (
    <Card icon={<GitBranch size={16} />} title={t('Source and updates')}>
      {src ? (
        <>
          <div className="core-service">
            <a className="st-mono core-link" href={`https://github.com/${src.repo}`} target="_blank" rel="noreferrer noopener">
              github.com/{src.repo}
            </a>
            {src.key ? <Badge tone="ok">{t('Key saved')}</Badge> : <Badge>{t('No key')}</Badge>}
            <div className="st-spacer" />
            <button type="button" className="st-btn small" onClick={() => setKeying(true)}>
              <KeyRound size={13} /> {t('Change the key')}
            </button>
          </div>
          {job && (running(job) || job.step === 'failed') ? (
            <JobLine job={job} name={app.name} />
          ) : (
            <div className="core-service">
              <button type="button" className="st-btn" disabled={checking} onClick={() => void look()}>
                <RefreshCw size={14} /> {t('Look for an update')}
              </button>
              {check &&
                (check.newer ? (
                  <>
                    <span>{t('Version {version} is available', { version: check.latest })}</span>
                    <button type="button" className="st-btn primary" onClick={() => void update()}>
                      <Download size={14} /> {t('Update')}
                    </button>
                  </>
                ) : (
                  <span className="st-muted">{t('Up to date ({version})', { version: app.version })}</span>
                ))}
            </div>
          )}
        </>
      ) : (
        <div className="core-service">
          <span className="st-muted">{t('Installed from an archive: send a newer one to update it.')}</span>
          <div className="st-spacer" />
          <button type="button" className="st-btn" onClick={onUpload}>
            <FileArchive size={14} /> {t('Send an archive')}
          </button>
        </div>
      )}
      {keying && <KeyModal app={app} onClose={() => setKeying(false)} onSaved={() => void reload()} />}
    </Card>
  );
}

/** Désinstaller : l'appli doit être désactivée ; ses données restent, sauf demande. */
export function RemoveCard({ app, reload }: { app: AppInfo; reload: () => Promise<void> }) {
  const [asking, setAsking] = useState(false);
  const [data, setData] = useState(false);
  const [busy, setBusy] = useState(false);
  const remove = async () => {
    setBusy(true);
    try {
      await api('DELETE', `/api/apps/${app.id}${data ? '?data=true' : ''}`);
      toast(t('{app} uninstalled', { app: app.name }));
      navigate('/reglages/bibliotheque');
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
      setBusy(false);
    }
  };
  return (
    <Card icon={<Trash2 size={16} />} title={t('Uninstall')}>
      <div className="core-service">
        <span className="st-muted">{app.active ? t('Deactivate the app to uninstall it.') : t('Its data is kept, unless you ask otherwise.')}</span>
        <div className="st-spacer" />
        <button type="button" className="st-btn danger" disabled={app.active} onClick={() => setAsking(true)}>
          <Trash2 size={14} /> {t('Uninstall')}
        </button>
      </div>
      {asking && (
        <Modal
          title={t('Uninstall {app}?', { app: app.name })}
          icon={<Trash2 size={16} />}
          onClose={() => setAsking(false)}
          onSubmit={() => void remove()}
          footer={
            <>
              <button type="button" className="st-btn" onClick={() => setAsking(false)}>
                {t('Cancel')}
              </button>
              <button type="submit" className="st-btn danger-solid" disabled={busy}>
                {t('Uninstall')}
              </button>
            </>
          }
        >
          <p className="st-muted core-note">{t('{app} {version} is removed from this server.', { app: app.name, version: app.version })}</p>
          <Switch checked={data} onChange={setData} label={t('Also erase its data')} />
        </Modal>
      )}
    </Card>
  );
}
