// Les réglages du serveur (administrateurs), rangés par objet comme dans la maquette C : Applis
// (la Bibliothèque, puis une page par appli installée) et Personnes. Les autres sections viendront
// avec leurs briques (Ce serveur, Matériel, Surveillance).
import { LayoutGrid, Users as UsersIcon } from 'lucide-react';
import { NavItem, NavSection, SidePage, t } from '@strata/ui';
import { appIcon } from '../icons';
import { navigate } from '../router';
import { AppPage, Library, useApps } from './Library';
import { Users } from './Users';

/** `path` : ce qui suit /reglages/ (« bibliotheque », « applis/livecut », « utilisateurs »). */
export function Settings({ path }: { path: string }) {
  const { apps, core, setActive } = useApps();
  const appId = path.startsWith('applis/') ? path.slice('applis/'.length) : null;
  const app = appId ? apps?.find((a) => a.id === appId) : undefined;

  let page: React.ReactNode = null;
  if (path === 'utilisateurs') page = <Users />;
  else if (apps && app) page = <AppPage app={app} core={core} setActive={setActive} />;
  else if (apps) page = <Library apps={apps} setActive={setActive} />;

  const nav = (
    <>
      <div>
        <NavSection title={t('Apps')} />
        <NavItem icon={LayoutGrid} label={t('Library')} on={!appId && path !== 'utilisateurs'} onClick={() => navigate('/reglages/bibliotheque')} count={apps?.length} />
        {apps?.map((a) => (
          <NavItem key={a.id} icon={appIcon(a.icon)} label={a.name} on={a.id === appId} onClick={() => navigate(`/reglages/applis/${a.id}`)} status={a.active ? 'ok' : null} />
        ))}
      </div>
      <div>
        <NavSection title={t('People')} />
        <NavItem icon={UsersIcon} label={t('Users')} on={path === 'utilisateurs'} onClick={() => navigate('/reglages/utilisateurs')} />
      </div>
    </>
  );
  return <SidePage nav={nav}>{page}</SidePage>;
}
