// Réglages › Ce serveur › Stockage : les serveurs où le Core range des fichiers (ses sauvegardes,
// et plus tard ce que les applis y déposent) — un partage d'un NAS (SMB), SFTP, FTP/FTPS, WebDAV,
// ou un dossier du serveur (un NAS monté là). Repris des serveurs de fichiers de Strata LiveCut.
import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, Check, ChevronLeft, Folder, FolderSearch, HardDrive, Pencil, Plug, Plus, Trash2, X } from 'lucide-react';
import { Badge, Card, Empty, Field, FieldGrid, Modal, PageHead, Segmented, confirmDialog, t, tn, toast } from '@strata/ui';
import { api } from '../api';
import { friendly } from '../errors';

export type StorageKind = 'smb' | 'sftp' | 'ftp' | 'webdav' | 'dir';

export interface StorageTarget {
  id: string;
  name: string;
  kind: StorageKind;
  host: string;
  port: number;
  share: string;
  path: string;
  user: string;
  password: string;
  tls: string;
  address?: string;
}

const MASKED = '••••••••';

const KINDS: { kind: StorageKind; label: string; sub: string }[] = [
  { kind: 'smb', label: 'NAS share', sub: 'SMB / Windows' },
  { kind: 'sftp', label: 'SFTP', sub: 'SSH' },
  { kind: 'ftp', label: 'FTP', sub: 'FTP / FTPS' },
  { kind: 'webdav', label: 'WebDAV', sub: 'Nextcloud…' },
  { kind: 'dir', label: 'Folder', sub: 'of the server' },
];
export const KIND_TAG: Record<StorageKind, string> = { smb: 'SMB', sftp: 'SFTP', ftp: 'FTP', webdav: 'WebDAV', dir: 'Folder' };

function blank(): StorageTarget {
  return { id: '', name: '', kind: 'smb', host: '', port: 0, share: '', path: '', user: '', password: '', tls: 'explicit' };
}

function defaultPort(x: StorageTarget) {
  if (x.kind === 'smb') return 445;
  if (x.kind === 'sftp') return 22;
  if (x.kind === 'ftp') return x.tls === 'implicit' ? 990 : 21;
  if (x.kind === 'webdav') return x.tls === 'none' ? 80 : 443;
  return 0;
}

/** Les serveurs de stockage (administrateurs). */
export function useStorage() {
  const [list, setList] = useState<StorageTarget[] | null>(null);
  const reload = useCallback(async () => {
    try {
      setList(await api<StorageTarget[]>('GET', '/api/storage'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  return [list, reload] as const;
}

type Test = { busy: boolean; ok?: boolean; text?: string };

async function runTest(x: StorageTarget, setTest: (v: Test) => void) {
  setTest({ busy: true });
  try {
    const r = await api<{ ms: number }>('POST', '/api/storage/test', x);
    setTest({ busy: false, ok: true, text: t('Written then removed in {ms} ms', { ms: r.ms }) });
  } catch (e) {
    setTest({ busy: false, ok: false, text: friendly(e) });
  }
}

export function StoragePage() {
  const [list, reload] = useStorage();
  const [edit, setEdit] = useState<StorageTarget | null>(null);
  return (
    <>
      <PageHead title={t('Storage')} sub={list ? tn(list.length, '{n} storage server', '{n} storage servers') : undefined}>
        <button type="button" className="st-btn primary" onClick={() => setEdit(blank())}>
          <Plus size={14} /> {t('New storage server')}
        </button>
      </PageHead>
      <Card icon={<HardDrive size={16} />} title={t('Storage servers')}>
        {list === null ? null : list.length === 0 ? (
          <Empty icon={HardDrive} text={t('No storage server yet: a NAS share, an SFTP, FTP or WebDAV server, or a folder of this server. The backups can be copied there.')} />
        ) : (
          <table className="st-table">
            <thead>
              <tr>
                <th>{t('Name')}</th>
                <th>{t('Type')}</th>
                <th>{t('Address')}</th>
                <th>{t('Test')}</th>
                <th className="r" />
              </tr>
            </thead>
            <tbody>
              {list.map((x) => (
                <Row key={x.id} x={x} onEdit={() => setEdit(x)} reload={reload} />
              ))}
            </tbody>
          </table>
        )}
      </Card>
      {edit && (
        <StorageModal
          target={edit}
          onClose={() => setEdit(null)}
          onSaved={() => {
            setEdit(null);
            void reload();
          }}
        />
      )}
    </>
  );
}

function TestLine({ test }: { test: Test }) {
  if (test.busy) return <span className="st-muted">{t('Testing…')}</span>;
  if (!test.text) return null;
  return (
    <span className={`core-storage-test ${test.ok ? 'ok' : 'bad'}`}>
      {test.ok ? <Check size={12} /> : <AlertTriangle size={12} />} {test.text}
    </span>
  );
}

function Row({ x, onEdit, reload }: { x: StorageTarget; onEdit: () => void; reload: () => Promise<void> }) {
  const [test, setTest] = useState<Test>({ busy: false });
  const remove = async () => {
    if (!(await confirmDialog({ title: t('Delete the storage server “{name}”?', { name: x.name }), message: t('The files already there stay.'), confirm: t('Delete'), danger: true }))) return;
    try {
      await api('DELETE', `/api/storage/${x.id}`);
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  return (
    <tr>
      <td>
        <strong>{x.name}</strong>
      </td>
      <td>
        <Badge>{t(KIND_TAG[x.kind])}</Badge>
      </td>
      <td className="st-mono">{x.address}</td>
      <td>
        <TestLine test={test} />
      </td>
      <td className="r">
        <div className="core-stream-actions">
          <button type="button" className="st-btn small" disabled={test.busy} onClick={() => void runTest(x, setTest)} data-tip={t('Writes a small file there, then removes it')}>
            <Plug size={13} /> {t('Test')}
          </button>
          <button type="button" className="st-btn small" title={t('Edit')} onClick={onEdit}>
            <Pencil size={13} />
          </button>
          <button type="button" className="st-btn small" title={t('Delete')} onClick={() => void remove()}>
            <Trash2 size={13} />
          </button>
        </div>
      </td>
    </tr>
  );
}

function StorageModal({ target, onClose, onSaved }: { target: StorageTarget; onClose: () => void; onSaved: () => void }) {
  const [x, setX] = useState<StorageTarget>(target);
  const [busy, setBusy] = useState(false);
  const [test, setTest] = useState<Test>({ busy: false });
  const set = (p: Partial<StorageTarget>) => {
    setX((v) => ({ ...v, ...p }));
    setTest({ busy: false });
  };
  const remote = x.kind !== 'dir';
  const ready = !!x.name.trim() && (remote ? !!x.host.trim() && (x.kind !== 'smb' || !!x.share.trim()) : x.path.trim().startsWith('/'));
  const save = async () => {
    if (!ready) return;
    setBusy(true);
    try {
      if (x.id) await api('PUT', `/api/storage/${x.id}`, x);
      else await api('POST', '/api/storage', x);
      toast(t('Storage server saved'));
      onSaved();
    } catch (e) {
      toast(friendly(e), 'error');
      setBusy(false);
    }
  };
  return (
    <Modal
      wide
      title={x.id ? t('Edit {name}', { name: target.name }) : t('New storage server')}
      icon={<HardDrive size={16} />}
      onClose={onClose}
      onSubmit={() => void save()}
      footer={
        <>
          <button type="button" className="st-btn" disabled={test.busy || !ready} onClick={() => void runTest(x, setTest)}>
            <Plug size={13} /> {test.busy ? t('Testing…') : t('Test')}
          </button>
          <span className="core-storage-foot">
            <TestLine test={test} />
          </span>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={busy || !ready}>
            {x.id ? t('Save') : t('Add')}
          </button>
        </>
      }
    >
      <Field label={t('Type')} group>
        <div className="core-storage-kinds">
          {KINDS.map((k) => (
            <button key={k.kind} type="button" className={x.kind === k.kind ? 'on' : ''} onClick={() => set({ kind: k.kind, port: 0, tls: k.kind === 'webdav' ? 'tls' : 'explicit' })}>
              <b>{t(k.label)}</b>
              <small>{t(k.sub)}</small>
            </button>
          ))}
        </div>
      </Field>
      <FieldGrid two>
        <Field label={t('Name')}>
          <input value={x.name} placeholder={t('Backup NAS')} onChange={(e) => set({ name: e.target.value })} />
        </Field>
        {remote && (
          <Field label={t('Server')}>
            <input className="st-mono" value={x.host} placeholder={x.kind === 'smb' ? 'nas-regie' : 'server.example.org'} spellCheck={false} onChange={(e) => set({ host: e.target.value })} />
          </Field>
        )}
        {x.kind === 'smb' && (
          <Field label={t('Share (SMB)')}>
            <input className="st-mono" value={x.share} placeholder="Sauvegardes" spellCheck={false} onChange={(e) => set({ share: e.target.value })} />
          </Field>
        )}
        {remote && (
          <Field label={t('Port')}>
            <input
              className="st-mono"
              inputMode="numeric"
              value={x.port ? String(x.port) : ''}
              placeholder={String(defaultPort(x))}
              onChange={(e) => set({ port: Math.min(65535, Number(e.target.value.replace(/[^0-9]/g, '')) || 0) })}
            />
          </Field>
        )}
      </FieldGrid>
      {x.kind === 'ftp' && (
        <Field label={t('Encryption')} group>
          <Segmented value={x.tls} options={[{ value: 'explicit', label: t('Explicit FTPS') }, { value: 'implicit', label: t('Implicit FTPS') }, { value: 'none', label: t('No encryption') }]} onChange={(tls) => set({ tls, port: x.port === defaultPort(x) ? 0 : x.port })} />
        </Field>
      )}
      {x.kind === 'webdav' && (
        <Field label={t('Encryption')} group>
          <Segmented value={x.tls} options={[{ value: 'tls', label: 'HTTPS' }, { value: 'none', label: 'HTTP' }]} onChange={(tls) => set({ tls, port: x.port === defaultPort(x) ? 0 : x.port })} />
        </Field>
      )}
      {remote && (
        <FieldGrid two>
          <Field label={t('User')}>
            <input value={x.user} autoComplete="off" spellCheck={false} onChange={(e) => set({ user: e.target.value })} />
          </Field>
          <Field label={t('Password')}>
            <input type="password" value={x.password} autoComplete="new-password" onFocus={() => x.password === MASKED && set({ password: '' })} onChange={(e) => set({ password: e.target.value })} />
          </Field>
        </FieldGrid>
      )}
      <FolderField x={x} set={set} can={remote ? !!x.host.trim() && (x.kind !== 'smb' || !!x.share.trim()) : x.path.trim().startsWith('/')} />
    </Modal>
  );
}

/** Un dossier tel qu'écrit : des « / », jamais doublés, ni à la fin (sauf la racine). */
function tidy(p: string, kind: StorageKind): string {
  let v = p.trim().replace(/\\/g, '/').replace(/\/{2,}/g, '/');
  if (v.length > 1) v = v.replace(/\/$/, '');
  if (kind === 'dir' && v && !v.startsWith('/')) v = `/${v}`;
  return v;
}

interface Look {
  busy: boolean;
  exists?: boolean;
  folders?: string[];
  error?: string;
}

/** Le dossier là-bas : vérifié tel qu'écrit, et choisi parmi ceux qui y sont (« Parcourir »). */
function FolderField({ x, set, can }: { x: StorageTarget; set: (p: Partial<StorageTarget>) => void; can: boolean }) {
  const [look, setLook] = useState<Look>({ busy: false });
  const [browse, setBrowse] = useState<(Look & { path: string }) | null>(null);
  const ask = (path: string) => api<{ exists: boolean; folders: string[] }>('POST', '/api/storage/browse', { target: x, path });
  const key = JSON.stringify([x.kind, x.host, x.port, x.share, x.user, x.password, x.tls, x.path]);
  useEffect(() => {
    if (!can) return setLook({ busy: false });
    let dead = false;
    setLook({ busy: true });
    const timer = window.setTimeout(() => {
      ask(x.path).then(
        (r) => !dead && setLook({ busy: false, exists: r.exists, folders: r.folders }),
        (e) => !dead && setLook({ busy: false, error: friendly(e) }),
      );
    }, 800);
    return () => {
      dead = true;
      clearTimeout(timer);
    };
  }, [key, can]); // eslint-disable-line react-hooks/exhaustive-deps
  const open = (path: string) => {
    setBrowse((b) => ({ path, busy: true, folders: b?.path === path ? b.folders : [] }));
    ask(path).then(
      (r) => setBrowse({ path, busy: false, exists: r.exists, folders: r.folders }),
      (e) => setBrowse({ path, busy: false, error: friendly(e) }),
    );
  };
  const root = x.kind === 'dir' ? '/' : '';
  const join = (a: string, n: string) => (a && a !== '/' ? `${a.replace(/\/$/, '')}/${n}` : `${a}${n}`);
  const up = (p: string) => {
    const i = p.replace(/\/$/, '').lastIndexOf('/');
    return i <= 0 ? (p.startsWith('/') ? '/' : root) : p.slice(0, i);
  };
  return (
    <Field label={x.kind === 'dir' ? t('Folder of the server') : t('Folder there')} group>
      <div className="core-storage-folder">
        <input
          className="st-mono"
          value={x.path}
          placeholder={x.kind === 'dir' ? '/mnt/nas/strata' : x.kind === 'sftp' ? '/srv/backups' : 'Strata'}
          spellCheck={false}
          onChange={(e) => set({ path: e.target.value })}
          onBlur={() => x.path !== tidy(x.path, x.kind) && set({ path: tidy(x.path, x.kind) })}
        />
        <button type="button" className="st-btn" disabled={!can} onClick={() => (browse ? setBrowse(null) : open(x.path || root))}>
          <FolderSearch size={14} /> {t('Browse')}
        </button>
      </div>
      {can && (
        <span className={`core-storage-test ${look.busy ? '' : look.error ? 'bad' : look.exists ? 'ok' : look.exists === false ? 'warn' : ''}`}>
          {look.busy ? (
            t('Checking the folder…')
          ) : look.error ? (
            <>
              <X size={12} /> {look.error}
            </>
          ) : look.exists ? (
            <>
              <Check size={12} /> {t('Folder found')} · {tn(look.folders?.length ?? 0, '{n} folder in it', '{n} folders in it')}
            </>
          ) : look.exists === false ? (
            <>
              <AlertTriangle size={12} /> {t('This folder is not there yet: it is made at the first copy.')}
            </>
          ) : null}
        </span>
      )}
      {browse && (
        <div className="core-storage-browse">
          <div className="core-storage-browse-head">
            <button type="button" className="st-btn small" disabled={browse.path === root || browse.path === '' || browse.busy} title={t('Folder above')} onClick={() => open(up(browse.path))}>
              <ChevronLeft size={14} />
            </button>
            <code>{browse.path || t('(the start folder)')}</code>
            <span className="core-grow" />
            <button
              type="button"
              className="st-btn small primary"
              disabled={browse.busy || !!browse.error}
              onClick={() => {
                set({ path: browse.path });
                setBrowse(null);
              }}
            >
              {t('Choose this folder')}
            </button>
          </div>
          <div className="core-storage-browse-list">
            {browse.busy && browse.folders?.length === 0 && <span className="st-muted">{t('Reading the folder…')}</span>}
            {browse.error && <span className="core-storage-test bad">{browse.error}</span>}
            {!browse.busy && !browse.error && browse.exists === false && <span className="core-storage-test warn">{t('This folder is not there yet: it is made at the first copy.')}</span>}
            {!browse.busy && !browse.error && browse.exists && browse.folders?.length === 0 && <span className="st-muted">{t('No folder in it.')}</span>}
            {browse.folders?.map((n) => (
              <button key={n} type="button" className="core-storage-dir" onClick={() => open(join(browse.path, n))}>
                <Folder size={14} /> {n}
              </button>
            ))}
          </div>
        </div>
      )}
    </Field>
  );
}
