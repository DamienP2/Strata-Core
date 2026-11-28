// Les réglages du serveur (administrateurs), rangés par objet comme dans la maquette C : Ce serveur
// (vue d'ensemble, nom et adresses, HTTPS), Applis (la Bibliothèque, puis une page par appli installée)
// et Personnes (Utilisateurs, Groupes, Annuaire). Les autres sections viendront avec leurs briques
// Matériel (Cartes SDI, NDI, ST 2110, Réseau, GPU, Horloge) ; Santé (Ressources, Journal, Alertes, SNMP).
import { Activity, Archive, BellRing, Cast, Clock, Gpu as GpuIcon, Server as ServersIcon, Network as SnmpIcon, CloudDownload, ScrollText, EthernetPort as Ethernet, Globe, HardDrive, KeyRound, LayoutDashboard, LayoutGrid, Lock, MonitorPlay, Radio, Users as UsersIcon, UsersRound, Waves } from 'lucide-react';
import { Suspense, lazy, useEffect, useState } from 'react';
import { NavItem, NavSection, SidePage, t } from '@strata/ui';
import { appIcon } from '../icons';
import { navigate } from '../router';
import { AppPage, Library, serviceTone, useApps } from './Library';
import { Directory } from './Directory';
import { Groups } from './Groups';
import { Users } from './Users';
import { Addresses, Https, Overview } from './Server';
import { Updates } from './Updates';
import { Backups } from './Backups';
import { StoragePage } from './Storage';
import { Servers } from './Servers';
// Le matériel (et three.js, pour ses cartes en 3D) n'est chargé qu'en ouvrant ses pages.
const hw = () => import('./hardware/Hardware');
const SdiPage = lazy(() => hw().then((m) => ({ default: m.SdiPage })));
const NdiPage = lazy(() => hw().then((m) => ({ default: m.NdiPage })));
const St2110Page = lazy(() => hw().then((m) => ({ default: m.St2110Page })));
const NetworkPage = lazy(() => hw().then((m) => ({ default: m.NetworkPage })));
const ClockPage = lazy(() => hw().then((m) => ({ default: m.ClockPage })));
const StreamsPage = lazy(() => import('./Streams').then((m) => ({ default: m.StreamsPage })));
const GpuPage = lazy(() => import('./hardware/Gpu').then((m) => ({ default: m.GpuPage })));
// La santé (et ses courbes) aussi.
const ResourcesPage = lazy(() => import('./health/Resources').then((m) => ({ default: m.ResourcesPage })));
const AlertsPage = lazy(() => import('./health/Alerts').then((m) => ({ default: m.AlertsPage })));
const SnmpPage = lazy(() => import('./health/Snmp').then((m) => ({ default: m.SnmpPage })));
const JournalPage = lazy(() => import('./health/Journal').then((m) => ({ default: m.JournalPage })));
import { api } from '../api';

/** `path` : ce qui suit /reglages/ (« serveur », « adresses », « https », « bibliotheque », « applis/livecut », « utilisateurs »…). */
export function Settings({ path }: { path: string }) {
  const { apps, core, jobs, reload, setActive, restart, dismiss } = useApps();
  // Le nombre de mises à jour qui attendent (dernière recherche), relu à chaque page.
  const [pending, setPending] = useState(0);
  // Les problèmes en cours (Santé › Alertes).
  const [problems, setProblems] = useState(0);
  useEffect(() => {
    api<{ count: number }>('GET', '/api/updates')
      .then((u) => setPending(u.count))
      .catch(() => {});
    api<{ active: unknown[] }>('GET', '/api/health/alerts')
      .then((a) => setProblems(a.active.length))
      .catch(() => {});
  }, [path]);
  const appId = path.startsWith('applis/') ? path.slice('applis/'.length) : null;
  const app = appId ? apps?.find((a) => a.id === appId) : undefined;

  let page: React.ReactNode = null;
  const hardware = ['sdi', 'ndi', 'st2110', 'flux', 'reseau', 'gpu', 'horloge'];
  const health = ['ressources', 'journal', 'alertes', 'snmp'];
  const fixed = [...hardware, ...health, 'serveurs', 'serveur', 'adresses', 'https', 'mises-a-jour', 'sauvegardes', 'stockage', 'utilisateurs', 'groupes', 'annuaire'].includes(path);
  if (path === 'serveur') page = <Overview apps={apps} />;
  else if (path === 'adresses') page = <Addresses />;
  else if (path === 'https') page = <Https />;
  else if (path === 'serveurs') page = <Servers />;
  else if (path === 'sdi') page = <SdiPage />;
  else if (path === 'ndi') page = <NdiPage />;
  else if (path === 'st2110') page = <St2110Page />;
  else if (path === 'flux') page = <StreamsPage />;
  else if (path === 'reseau') page = <NetworkPage />;
  else if (path === 'horloge') page = <ClockPage />;
  else if (path === 'gpu') page = <GpuPage />;
  else if (path === 'ressources') page = <ResourcesPage />;
  else if (path === 'journal') page = <JournalPage />;
  else if (path === 'alertes') page = <AlertsPage onCount={setProblems} />;
  else if (path === 'snmp') page = <SnmpPage />;
  else if (path === 'mises-a-jour') page = <Updates apps={apps} jobs={jobs} reloadApps={reload} onCount={setPending} />;
  else if (path === 'sauvegardes') page = <Backups />;
  else if (path === 'stockage') page = <StoragePage />;
  else if (path === 'utilisateurs') page = <Users />;
  else if (path === 'groupes') page = <Groups />;
  else if (path === 'annuaire') page = <Directory />;
  else if (apps && app) page = <AppPage app={app} core={core} job={jobs.find((j) => j.app === app.id)} setActive={setActive} restart={restart} reload={reload} />;
  else if (apps) page = <Library apps={apps} jobs={jobs} setActive={setActive} reload={reload} dismiss={dismiss} />;

  const nav = (
    <>
      <div>
        <NavSection title={t('This server')} />
        <NavItem icon={LayoutDashboard} label={t('Overview')} on={path === 'serveur'} onClick={() => navigate('/reglages/serveur')} />
        <NavItem icon={Globe} label={t('Name and addresses')} on={path === 'adresses'} onClick={() => navigate('/reglages/adresses')} />
        <NavItem icon={Lock} label="HTTPS" on={path === 'https'} onClick={() => navigate('/reglages/https')} />
        <NavItem icon={ServersIcon} label={t('Servers')} on={path === 'serveurs'} onClick={() => navigate('/reglages/serveurs')} />
        <NavItem icon={CloudDownload} label={t('Updates')} on={path === 'mises-a-jour'} onClick={() => navigate('/reglages/mises-a-jour')} count={pending || undefined} />
        <NavItem icon={Archive} label={t('Backups')} on={path === 'sauvegardes'} onClick={() => navigate('/reglages/sauvegardes')} />
        <NavItem icon={HardDrive} label={t('Storage')} on={path === 'stockage'} onClick={() => navigate('/reglages/stockage')} />
      </div>
      <div>
        <NavSection title={t('Apps')} />
        <NavItem icon={LayoutGrid} label={t('Library')} on={!appId && !fixed} onClick={() => navigate('/reglages/bibliotheque')} count={apps?.length} />
        {apps?.map((a) => (
          <NavItem key={a.id} icon={appIcon(a.icon)} label={a.name} on={a.id === appId} onClick={() => navigate(`/reglages/applis/${a.id}`)} status={serviceTone(a)} />
        ))}
      </div>
      <div>
        <NavSection title={t('Hardware')} />
        <NavItem icon={MonitorPlay} label={t('SDI cards')} on={path === 'sdi'} onClick={() => navigate('/reglages/sdi')} />
        <NavItem icon={Radio} label="NDI" on={path === 'ndi'} onClick={() => navigate('/reglages/ndi')} />
        <NavItem icon={Waves} label="ST 2110" on={path === 'st2110'} onClick={() => navigate('/reglages/st2110')} />
        <NavItem icon={Cast} label={t('Network streams')} on={path === 'flux'} onClick={() => navigate('/reglages/flux')} />
        <NavItem icon={Ethernet} label={t('Network')} on={path === 'reseau'} onClick={() => navigate('/reglages/reseau')} />
        <NavItem icon={GpuIcon} label="GPU" on={path === 'gpu'} onClick={() => navigate('/reglages/gpu')} />
        <NavItem icon={Clock} label={t('Clock')} on={path === 'horloge'} onClick={() => navigate('/reglages/horloge')} />
      </div>
      <div>
        <NavSection title={t('Health')} />
        <NavItem icon={Activity} label={t('Resources')} on={path === 'ressources'} onClick={() => navigate('/reglages/ressources')} />
        <NavItem icon={ScrollText} label={t('Journal')} on={path === 'journal'} onClick={() => navigate('/reglages/journal')} />
        <NavItem icon={BellRing} label={t('Alerts')} on={path === 'alertes'} onClick={() => navigate('/reglages/alertes')} count={problems || undefined} />
        <NavItem icon={SnmpIcon} label="SNMP" on={path === 'snmp'} onClick={() => navigate('/reglages/snmp')} />
      </div>
      <div>
        <NavSection title={t('People')} />
        <NavItem icon={UsersIcon} label={t('Users')} on={path === 'utilisateurs'} onClick={() => navigate('/reglages/utilisateurs')} />
        <NavItem icon={UsersRound} label={t('Groups')} on={path === 'groupes'} onClick={() => navigate('/reglages/groupes')} />
        <NavItem icon={KeyRound} label={t('Directory')} on={path === 'annuaire'} onClick={() => navigate('/reglages/annuaire')} />
      </div>
    </>
  );
  return (
    <SidePage nav={nav}>
      <Suspense fallback={null}>{page}</Suspense>
    </SidePage>
  );
}
