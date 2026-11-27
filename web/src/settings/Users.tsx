// Réglages › Personnes › Utilisateurs : les comptes (administrateurs).
import { useCallback, useEffect, useState } from 'react';
import { Ellipsis, KeyRound, Pencil, Trash2, UserCheck, UserPlus, UserX } from 'lucide-react';
import { Badge, Field, Modal, PageHead, Switch, confirmDialog, initial, openContextMenu, promptDialog, t, tn, toast, type MenuEntry } from '@strata/ui';
import { api, useApp, type PublicUser } from '../api';
import { friendly } from '../errors';

function NewUser({ onClose, onCreated }: { onClose: () => void; onCreated: (u: PublicUser) => void }) {
  const [username, setUsername] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [admin, setAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (password !== repeat) return toast(t('The passwords do not match.'), 'error');
    setBusy(true);
    try {
      onCreated(await api<PublicUser>('POST', '/api/users', { username: username.trim(), displayName: displayName.trim(), password, admin }));
    } catch (e) {
      toast(friendly(e), 'error');
      setBusy(false);
    }
  };
  const empty = !username.trim() || !password || !repeat;
  return (
    <Modal
      title={t('New user')}
      icon={<UserPlus size={16} />}
      onClose={onClose}
      onSubmit={() => void submit()}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={empty || busy}>
            {t('Create')}
          </button>
        </>
      }
    >
      <Field label={t('User name')}>
        <input value={username} autoComplete="off" autoCapitalize="none" spellCheck={false} onChange={(e) => setUsername(e.target.value)} />
      </Field>
      <Field label={t('Display name')}>
        <input value={displayName} placeholder={t('Optional')} onChange={(e) => setDisplayName(e.target.value)} />
      </Field>
      <Field label={t('Password')}>
        <input type="password" value={password} autoComplete="new-password" onChange={(e) => setPassword(e.target.value)} />
      </Field>
      <Field label={t('Repeat the password')} hint={t('At least 8 characters.')}>
        <input type="password" value={repeat} autoComplete="new-password" onChange={(e) => setRepeat(e.target.value)} />
      </Field>
      <Field label={t('Role')} group>
        <Switch checked={admin} onChange={setAdmin} label={t('Server administrator')} />
      </Field>
    </Modal>
  );
}

function EditUser({ user, self, onClose, onSaved }: { user: PublicUser; self: boolean; onClose: () => void; onSaved: (u: PublicUser) => void }) {
  const [displayName, setDisplayName] = useState(user.displayName);
  const [admin, setAdmin] = useState(user.admin);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    try {
      onSaved(await api<PublicUser>('PATCH', `/api/users/${user.id}`, { displayName, admin }));
    } catch (e) {
      toast(friendly(e), 'error');
      setBusy(false);
    }
  };
  return (
    <Modal
      title={t('Edit {name}', { name: user.displayName })}
      icon={<Pencil size={16} />}
      onClose={onClose}
      onSubmit={() => void submit()}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={busy}>
            {t('Save')}
          </button>
        </>
      }
    >
      <Field label={t('User name')}>
        <input value={user.username} readOnly />
      </Field>
      <Field label={t('Display name')}>
        <input value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
      </Field>
      <Field label={t('Role')} group>
        <Switch checked={admin} disabled={self} onChange={setAdmin} label={t('Server administrator')} />
      </Field>
    </Modal>
  );
}

export function Users() {
  const me = useApp((s) => s.auth?.user);
  const [users, setUsers] = useState<PublicUser[] | null>(null);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<PublicUser | null>(null);

  const reload = useCallback(async () => {
    try {
      setUsers((await api<{ users: PublicUser[] }>('GET', '/api/users')).users);
    } catch (e) {
      toast(friendly(e), 'error');
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  const patch = async (u: PublicUser, body: Partial<Pick<PublicUser, 'disabled' | 'admin' | 'displayName'>>) => {
    try {
      await api('PATCH', `/api/users/${u.id}`, body);
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const changePassword = async (u: PublicUser) => {
    const password = await promptDialog({ title: t('Change the password'), message: u.displayName, label: t('New password'), password: true, confirm: t('Save') });
    if (!password) return;
    try {
      await api('POST', `/api/users/${u.id}/password`, { password });
      toast(t('Password changed'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const remove = async (u: PublicUser) => {
    if (!(await confirmDialog({ title: t('Delete {name}?', { name: u.displayName }), message: u.username, confirm: t('Delete the account'), danger: true }))) return;
    try {
      await api('DELETE', `/api/users/${u.id}`);
      toast(t('{name} deleted', { name: u.displayName }));
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const menu = (u: PublicUser): MenuEntry[] => {
    const self = u.id === me?.id;
    return [
      { label: t('Edit'), icon: <Pencil size={14} />, onClick: () => setEditing(u) },
      { label: t('Change the password'), icon: <KeyRound size={14} />, onClick: () => void changePassword(u) },
      u.disabled
        ? { label: t('Enable'), icon: <UserCheck size={14} />, onClick: () => void patch(u, { disabled: false }) }
        : { label: t('Disable'), icon: <UserX size={14} />, disabled: self, onClick: () => void patch(u, { disabled: true }) },
      'sep',
      { label: t('Delete'), icon: <Trash2 size={14} />, danger: true, disabled: self, onClick: () => void remove(u) },
    ];
  };

  if (!users) return null;
  const admins = users.filter((u) => u.admin).length;
  return (
    <>
      <PageHead title={t('Users')} sub={`${tn(users.length, '{n} user', '{n} users')} · ${tn(admins, '{n} administrator', '{n} administrators')}`}>
        <button type="button" className="st-btn primary" onClick={() => setCreating(true)}>
          <UserPlus size={14} /> {t('New user')}
        </button>
      </PageHead>
      <table className="st-table">
        <thead>
          <tr>
            <th>{t('User')}</th>
            <th>{t('Role')}</th>
            <th>{t('Groups')}</th>
            <th>{t('State')}</th>
            <th className="r" />
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <tr key={u.id} className={u.disabled ? 'dim' : ''} onContextMenu={(e) => openContextMenu(e, menu(u))}>
              <td>
                <div className="core-user">
                  <span className="st-avatar">{initial(u.displayName)}</span>
                  <div>
                    <strong>{u.displayName}</strong>
                    {u.id === me?.id && <span className="st-muted"> · {t('you')}</span>}
                    <div className="st-muted st-mono">{u.username}</div>
                  </div>
                </div>
              </td>
              <td>{u.admin ? <Badge tone="accent">{t('Server administrator')}</Badge> : <Badge>{t('User')}</Badge>}</td>
              <td className="st-muted">{u.groups.join(' · ') || '—'}</td>
              <td>
                {u.disabled && <Badge tone="bad">{t('Disabled')}</Badge>} {u.sso && <Badge>SSO</Badge>}
              </td>
              <td className="r">
                <button type="button" className="st-icon-btn ghost" title={t('More')} onClick={(e) => openContextMenu(e, menu(u))}>
                  <Ellipsis size={14} />
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {creating && (
        <NewUser
          onClose={() => setCreating(false)}
          onCreated={(u) => {
            setCreating(false);
            toast(t('{name} created', { name: u.displayName }));
            void reload();
          }}
        />
      )}
      {editing && (
        <EditUser
          user={editing}
          self={editing.id === me?.id}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void reload();
          }}
        />
      )}
    </>
  );
}
