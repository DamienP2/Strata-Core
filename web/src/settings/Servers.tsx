// Réglages › Serveurs : un seul cockpit pour plusieurs serveurs (brief § 4 bis). Sur le maître : ses
// membres, en ajouter (un code d'invitation), en détacher. Sur un membre : son maître, s'en détacher.
// Un serveur seul peut rejoindre un cockpit.
import { useEffect, useState } from 'react';
import { ExternalLink, Link2, Plus, Server as ServerIcon, Unlink } from 'lucide-react';
import { Badge, Card, Empty, Field, Modal, PageHead, confirmDialog, t, timeAgo, tn, toast } from '@strata/ui';
import { api } from '../api';
import { friendly, pollError } from '../errors';
import { useJsonState, useMounted, useRepeat } from '../poll';

interface Member {
  id: string;
  name: string;
  url: string;
  joined: number;
  up: boolean;
  version?: string | null;
  apps: number;
  problems: number;
}

interface ClusterState {
  id: string;
  role: 'master' | 'member';
  master?: { id: string; name: string; url: string; joined: number; up: boolean; error?: string | null } | null;
  members: Member[];
}

const settingsOf = (url: string) => `${url.replace(/\/$/, '')}/reglages/serveur`;

function Invite({ onClose }: { onClose: () => void }) {
  const [inv, setInv] = useState<{ code: string; expiresAt: number } | null>(null);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    api<{ code: string; expiresAt: number }>('POST', '/api/cluster/invite')
      .then(setInv)
      .catch((e) => {
        toast(friendly(e), 'error');
        onClose();
      });
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const left = inv ? Math.max(0, Math.round((inv.expiresAt - now) / 1000)) : 0;
  return (
    <Modal title={t('Add a server')} icon={<Plus size={16} />} onClose={onClose} footer={<button type="button" className="st-btn" onClick={onClose}>{t('Close')}</button>}>
      {inv && (
        <div className="core-invite">
          <p>{t('On the new server: Settings › Servers › Join a cockpit, with this address and this code.')}</p>
          <div className="core-line">
            <span>{t('Address of this cockpit')}</span>
            <span className="st-mono">{location.origin}</span>
          </div>
          <div className="core-invite-code st-mono">{left > 0 ? inv.code : '— — —'}</div>
          <p className="st-muted core-note">{left > 0 ? t('Valid {m} min {s} s, once.', { m: Math.floor(left / 60), s: left % 60 }) : t('This code has expired.')}</p>
        </div>
      )}
    </Modal>
  );
}

function JoinCard({ onDone }: { onDone: () => void }) {
  const [url, setUrl] = useState('');
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const join = async () => {
    setBusy(true);
    try {
      const r = await api<{ master: { name: string } }>('POST', '/api/cluster/join', { masterUrl: url, code });
      toast(t('This server joined the cockpit of {name}', { name: r.master.name }));
      onDone();
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  };
  return (
    <Card icon={<Link2 size={16} />} title={t('Join a cockpit')}>
      <div className="core-clock-form">
        <p className="st-muted core-note">{t('This server then shows its apps in the cockpit of the master; people sign in there.')}</p>
        <Field label={t('Address of the master')}>
          <input value={url} placeholder="https://strata01.regie.exemple.fr" onChange={(e) => setUrl(e.target.value)} />
        </Field>
        <Field label={t('Invitation code')}>
          <input className="st-mono" value={code} placeholder="ABCD-EFGH-JK" onChange={(e) => setCode(e.target.value)} />
        </Field>
        <div>
          <button type="button" className="st-btn primary" disabled={busy || !url.trim() || !code.trim()} onClick={() => void join()}>
            <Link2 size={14} /> {t('Join')}
          </button>
        </div>
      </div>
    </Card>
  );
}

export function Servers() {
  const [st, setSt] = useJsonState<ClusterState | null>(null);
  const [inviting, setInviting] = useState(false);
  const mounted = useMounted();
  // `alive` : faux une fois la page quittée ; rien ne change alors.
  const read = (alive: () => boolean = mounted) =>
    api<ClusterState>('GET', '/api/cluster')
      .then((r) => alive() && setSt(r))
      .catch((e) => alive() && pollError(e));
  useRepeat('cluster', 3000, (alive) => read(alive));
  if (!st) return <PageHead title={t('Servers')} />;

  const detachMember = async (m: Member) => {
    const ok = await confirmDialog({ title: t('Detach {name}?', { name: m.name }), message: t('Its apps leave this cockpit; it becomes a server on its own again.'), confirm: t('Detach'), danger: true });
    if (!ok) return;
    try {
      await api('DELETE', `/api/cluster/members/${m.id}`);
      void read();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const leave = async () => {
    const ok = await confirmDialog({ title: t('Detach this server?'), message: t('It shows its own cockpit again, and people sign in here.'), confirm: t('Detach'), danger: true });
    if (!ok) return;
    try {
      await api('POST', '/api/cluster/detach');
      void read();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };

  if (st.role === 'member' && st.master) {
    const m = st.master;
    return (
      <>
        <PageHead title={t('Servers')} sub={t('Member of the cockpit of {name}', { name: m.name })} />
        <Card
          icon={<ServerIcon size={16} />}
          title={m.name}
          actions={m.up ? <Badge tone="ok">{t('Reachable')}</Badge> : <Badge tone="bad">{t('Unreachable')}</Badge>}
        >
          <div className="core-lines">
            <div className="core-line">
              <span>{t('Master')}</span>
              <a className="st-mono" href={m.url} target="_blank" rel="noreferrer">
                {m.url}
              </a>
            </div>
            <div className="core-line">
              <span>{t('Joined')}</span>
              <span className="st-muted">{timeAgo(m.joined)}</span>
            </div>
            {m.error && (
              <div className="core-line">
                <span>{t('Last error')}</span>
                <span className="core-warn">{friendly(m.error)}</span>
              </div>
            )}
          </div>
          <p className="st-muted core-note">{t('If the master does not answer, the apps keep running and the local accounts of this server still sign in.')}</p>
          <div>
            <button type="button" className="st-btn danger" onClick={() => void leave()}>
              <Unlink size={14} /> {t('Detach from this cockpit')}
            </button>
          </div>
        </Card>
      </>
    );
  }

  return (
    <>
      <PageHead title={t('Servers')} sub={st.members.length ? tn(st.members.length, '{n} member server', '{n} member servers') : t('This server on its own')}>
        <button type="button" className="st-btn primary" onClick={() => setInviting(true)}>
          <Plus size={14} /> {t('Add a server')}
        </button>
      </PageHead>
      <Card icon={<ServerIcon size={16} />} title={t('Servers of this cockpit')}>
        {st.members.length === 0 ? (
          <Empty text={t('No other server yet')} />
        ) : (
          <table className="core-table">
            <thead>
              <tr>
                <th>{t('Name')}</th>
                <th>{t('Address')}</th>
                <th>{t('State')}</th>
                <th className="num">{t('Apps')}</th>
                <th className="num">{t('Problems')}</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {st.members.map((m) => (
                <tr key={m.id}>
                  <td>
                    {m.name}
                    {m.version && <span className="st-muted"> · {m.version}</span>}
                  </td>
                  <td className="st-mono st-muted">{m.url}</td>
                  <td>{m.up ? <Badge tone="ok">{t('Reachable')}</Badge> : <Badge tone="bad">{t('Unreachable')}</Badge>}</td>
                  <td className="num">{m.up ? m.apps : '—'}</td>
                  <td className="num">{m.up ? m.problems : '—'}</td>
                  <td className="num">
                    <span className="core-service">
                      <a className="st-btn small ghost" href={settingsOf(m.url)} target={`strata-settings-${m.id}`} rel="noreferrer" title={t('Settings of {name}', { name: m.name })}>
                        <ExternalLink size={13} />
                      </a>
                      <button type="button" className="st-btn small ghost" title={t('Detach')} onClick={() => void detachMember(m)}>
                        <Unlink size={13} />
                      </button>
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      {st.members.length === 0 && <JoinCard onDone={() => void read()} />}
      {inviting && <Invite onClose={() => setInviting(false)} />}
    </>
  );
}
