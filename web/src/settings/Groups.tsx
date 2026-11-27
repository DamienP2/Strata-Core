// Réglages › Personnes › Groupes : les groupes du Core, et les groupes de l'annuaire qui les remplissent.
import { useCallback, useEffect, useState } from 'react';
import { Ellipsis, Pencil, Plus, Trash2, UsersRound } from 'lucide-react';
import { Empty, Field, Modal, PageHead, confirmDialog, openContextMenu, t, tn, toast, type MenuEntry } from '@strata/ui';
import { api } from '../api';
import { friendly } from '../errors';
import { TagInput } from './TagInput';

export interface GroupInfo {
  id: string;
  name: string;
  directory: string[];
  members: number;
}

/** Les groupes du Core (administrateurs). */
export function useGroups() {
  const [groups, setGroups] = useState<GroupInfo[] | null>(null);
  const reload = useCallback(async () => {
    try {
      setGroups((await api<{ groups: GroupInfo[] }>('GET', '/api/groups')).groups);
    } catch (e) {
      toast(friendly(e), 'error');
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { groups, reload };
}

function GroupForm({ group, onClose, onSaved }: { group: GroupInfo | null; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(group?.name ?? '');
  const [directory, setDirectory] = useState<string[]>(group?.directory ?? []);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    try {
      if (group) await api('PATCH', `/api/groups/${group.id}`, { name, directory });
      else await api('POST', '/api/groups', { name, directory });
      onSaved();
    } catch (e) {
      toast(friendly(e), 'error');
      setBusy(false);
    }
  };
  return (
    <Modal
      title={group ? t('Edit {name}', { name: group.name }) : t('New group')}
      icon={group ? <Pencil size={16} /> : <Plus size={16} />}
      onClose={onClose}
      onSubmit={() => void submit()}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={!name.trim() || busy}>
            {group ? t('Save') : t('Create')}
          </button>
        </>
      }
    >
      <Field label={t('Name')}>
        <input value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field label={t('Directory groups')} group>
        <TagInput value={directory} onChange={setDirectory} placeholder="/Realisateurs" />
      </Field>
    </Modal>
  );
}

export function Groups() {
  const { groups, reload } = useGroups();
  const [editing, setEditing] = useState<GroupInfo | 'new' | null>(null);

  const remove = async (g: GroupInfo) => {
    const message = g.members ? tn(g.members, '{n} member leaves it', '{n} members leave it') : undefined;
    if (!(await confirmDialog({ title: t('Delete {name}?', { name: g.name }), message, confirm: t('Delete the group'), danger: true }))) return;
    try {
      await api('DELETE', `/api/groups/${g.id}`);
      toast(t('{name} deleted', { name: g.name }));
      await reload();
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const menu = (g: GroupInfo): MenuEntry[] => [
    { label: t('Edit'), icon: <Pencil size={14} />, onClick: () => setEditing(g) },
    'sep',
    { label: t('Delete'), icon: <Trash2 size={14} />, danger: true, onClick: () => void remove(g) },
  ];

  if (!groups) return null;
  return (
    <>
      <PageHead title={t('Groups')} sub={tn(groups.length, '{n} group', '{n} groups')}>
        <button type="button" className="st-btn primary" onClick={() => setEditing('new')}>
          <Plus size={14} /> {t('New group')}
        </button>
      </PageHead>
      {groups.length === 0 ? (
        <div className="core-empty">
          <Empty icon={UsersRound} text={t('No group yet')} />
        </div>
      ) : (
        <table className="st-table">
          <thead>
            <tr>
              <th>{t('Group')}</th>
              <th>{t('Directory groups')}</th>
              <th>{t('Members')}</th>
              <th className="r" />
            </tr>
          </thead>
          <tbody>
            {groups.map((g) => (
              <tr key={g.id} onContextMenu={(e) => openContextMenu(e, menu(g))}>
                <td>
                  <strong>{g.name}</strong>
                </td>
                <td>
                  {g.directory.length ? (
                    <div className="core-chips">
                      {g.directory.map((d) => (
                        <span key={d} className="st-badge accent st-mono">
                          {d}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <span className="st-muted">—</span>
                  )}
                </td>
                <td className="st-num">{g.members}</td>
                <td className="r">
                  <button type="button" className="st-icon-btn ghost" title={t('More')} onClick={(e) => openContextMenu(e, menu(g))}>
                    <Ellipsis size={14} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {editing && (
        <GroupForm
          group={editing === 'new' ? null : editing}
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
