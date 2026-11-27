// Les réglages du serveur (administrateurs), rangés par objet comme dans la maquette C : Applis
// (la Bibliothèque, puis une page par appli installée) et Personnes (Utilisateurs, Groupes, Annuaire).
// Les autres sections viendront
// avec leurs briques (Ce serveur, Matériel, Surveillance).
import { KeyRound, LayoutGrid, Users as UsersIcon, UsersRound } from 'lucide-react';
import { NavItem, NavSection, SidePage, t } from '@strata/ui';
import { appIcon } from '../icons';
import { navigate } from '../router';
import { AppPage, Library, serviceTone, useApps } from './Library';
import { Directory } from './Directory';
import { Groups } from './Groups';
import { Users } from './Users';

/** `path` : ce qui suit /reglages/ (« bibliotheque », « applis/livecut », « utilisateurs », « groupes », « annuaire »). */
export function Settings({ path }: { path: string }) {
  const { apps, core, jobs, reload, setActive, restart, dismiss } = useApps();
  const appId = path.startsWith('applis/') ? path.slice('applis/'.length) : null;
  const app = appId ? apps?.find((a) => a.id === appId) : undefined;

  let page: React.ReactNode = null;
  const people = ['utilisateurs', 'groupes', 'annuaire'].includes(path);
  if (path === 'utilisateurs') page = <Users />;
  else if (path === 'groupes') page = <Groups />;
  else if (path === 'annuaire') page = <Directory />;
  else if (apps && app) page = <AppPage app={app} core={core} job={jobs.find((j) => j.app === app.id)} setActive={setActive} restart={restart} reload={reload} />;
  else if (apps) page = <Library apps={apps} jobs={jobs} setActive={setActive} reload={reload} dismiss={dismiss} />;

  const nav = (
    <>
      <div>
        <NavSection title={t('Apps')} />
        <NavItem icon={LayoutGrid} label={t('Library')} on={!appId && !people} onClick={() => navigate('/reglages/bibliotheque')} count={apps?.length} />
        {apps?.map((a) => (
          <NavItem key={a.id} icon={appIcon(a.icon)} label={a.name} on={a.id === appId} onClick={() => navigate(`/reglages/applis/${a.id}`)} status={serviceTone(a)} />
        ))}
      </div>
      <div>
        <NavSection title={t('People')} />
        <NavItem icon={UsersIcon} label={t('Users')} on={path === 'utilisateurs'} onClick={() => navigate('/reglages/utilisateurs')} />
        <NavItem icon={UsersRound} label={t('Groups')} on={path === 'groupes'} onClick={() => navigate('/reglages/groupes')} />
        <NavItem icon={KeyRound} label={t('Directory')} on={path === 'annuaire'} onClick={() => navigate('/reglages/annuaire')} />
      </div>
    </>
  );
  return <SidePage nav={nav}>{page}</SidePage>;
}
