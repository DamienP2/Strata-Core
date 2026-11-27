// Réglages › Applis : la Bibliothèque (toutes les applis du serveur) et la page de chaque appli.
import { useCallback, useEffect, useState } from 'react';
import { ArrowUpRight, Cloud, LayoutGrid, Power, Server } from 'lucide-react';
import { Badge, Card, Empty, Field, FieldGrid, PageHead, Switch, openAppTab, t, tn, toast } from '@strata/ui';
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
  return { apps, core, reload, setActive };
}

const open = (a: AppInfo) => openAppTab(a.url, `${a.id}@local`);

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
                  {a.active && (
                    <button type="button" className="st-btn small" onClick={() => open(a)}>
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

export function AppPage({ app: a, core, setActive }: { app: AppInfo; core: string; setActive: (a: AppInfo, on: boolean) => void }) {
  const Icon = appIcon(a.icon);
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
          <button type="button" className="st-btn primary" onClick={() => open(a)}>
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
      <Card icon={<Server size={16} />} title={t('Service')}>
        <FieldGrid two>
          <Field label={t('Address')}>
            <input value={a.url} readOnly />
          </Field>
          <Field label={t('Internal port')}>
            <input value={String(a.port)} readOnly />
          </Field>
        </FieldGrid>
      </Card>
    </>
  );
}
