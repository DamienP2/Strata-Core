// Banc d'essai : chaque composant du paquet, dans une coquille d'appli, dans les deux thèmes et les deux langues.
import { StrictMode, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Bell, CircleHelp, Cloud, Copy, Inbox, Layers, LayoutGrid, LayoutList, ListTree, MousePointerClick, Palette, Pencil, Plus, Scissors, Search, Server, Settings as SettingsIcon, SlidersHorizontal, Trash2 } from 'lucide-react';
import {
  Badge, Brand, Card, Cockpit, ContextMenuHost, Launcher, Page, TopBar, instanceKey, nameTab, openAppTab, t, type CockpitApp, type CockpitData, type CockpitInstance, type CockpitServer, DialogHost, Dot, Dropdown, Empty, Field, FieldGrid, InfoTip, Login, NavItem, NavSection, NumberField,
  PageHead, SaveBar, Segmented, SidePage, StatusPill, SubTabs, Switch, TextInput, Toasts, UserMenu,
  alertDialog, confirmDialog, formatBytes, formatDuration, installEscape, installNumberWheel, installTooltips, openContextMenu, promptDialog, setLang, timeAgo, toast, useLang,
} from '../src';
import '../src/styles/fonts.css';
import './playground.css';

installTooltips();
installEscape();
installNumberWheel();

type PageId = 'buttons' | 'fields' | 'menus' | 'settings' | 'dialogs' | 'brand';
const PAGES: { id: PageId; icon: typeof Palette; label: string; group: string }[] = [
  { id: 'buttons', icon: MousePointerClick, label: 'Boutons et états', group: 'Contrôles' },
  { id: 'fields', icon: SlidersHorizontal, label: 'Champs', group: 'Contrôles' },
  { id: 'menus', icon: ListTree, label: 'Menus', group: 'Contrôles' },
  { id: 'settings', icon: Server, label: 'Réglages', group: 'Pages' },
  { id: 'dialogs', icon: Bell, label: 'Dialogues et messages', group: 'Pages' },
  { id: 'brand', icon: Palette, label: 'Marque', group: 'Identité' },
];

function Row({ children }: { children: React.ReactNode }) {
  return <div className="pg-row">{children}</div>;
}

function Buttons() {
  return (
    <>
      <PageHead title="Boutons et états" sub="st-btn, st-icon-btn, st-badge, st-dot" />
      <Card title="Boutons">
        <Row>
          <button className="st-btn primary"><Plus size={14} /> Nouveau</button>
          <button className="st-btn">Annuler</button>
          <button className="st-btn ghost">Fantôme</button>
          <button className="st-btn on">Actif</button>
          <button className="st-btn danger"><Trash2 size={14} /> Supprimer</button>
          <button className="st-btn danger-solid">Supprimer définitivement</button>
          <button className="st-btn warn">Attention</button>
          <button className="st-btn small">Petit</button>
          <button className="st-btn" disabled>Désactivé</button>
          <button className="st-icon-btn" title="Réglages"><SlidersHorizontal size={14} /></button>
          <button className="st-icon-btn ghost" title="Supprimer"><Trash2 size={14} /></button>
        </Row>
      </Card>
      <Card title="États">
        <Row>
          <Badge>Inactive</Badge>
          <Badge tone="ok">En marche</Badge>
          <Badge tone="warn">Mise à jour</Badge>
          <Badge tone="bad">Injoignable</Badge>
          <Badge tone="accent">Maître</Badge>
          <span className="pg-dots"><Dot tone="ok" title="Joignable" /><Dot tone="warn" title="1 alerte" /><Dot tone="bad" title="Injoignable" /><Dot title="Inactif" /></span>
          <span>Infobulle dessinée <InfoTip text="Le title de l’élément, dans une carte qui suit le pointeur." /></span>
        </Row>
      </Card>
      <Card title="Formats">
        <Row>
          <span className="st-mono">{formatBytes(2.6 * 1024 ** 4)}</span>
          <span className="st-mono">{formatDuration(12 * 86400 + 3 * 3600)}</span>
          <span className="st-mono">{timeAgo(Date.now() - 5 * 60000)}</span>
        </Row>
      </Card>
    </>
  );
}

function Fields() {
  const [on, setOn] = useState(true);
  const [mode, setMode] = useState<'master' | 'member'>('master');
  const [okta, setOkta] = useState('okta');
  const [role, setRole] = useState('st2110-red');
  const [delay, setDelay] = useState(10);
  const [vol, setVol] = useState(0.8);
  const [name, setName] = useState('Régie A');
  return (
    <>
      <PageHead title="Champs" sub="Dropdown, NumberField, TextInput, Switch, Segmented" />
      <Card title="Choix">
        <FieldGrid>
          <Field label="Fournisseur" group>
            <Dropdown value={okta} onChange={setOkta} options={[{ value: 'okta', label: 'Okta' }, { value: 'entra', label: 'Microsoft Entra' }, { value: 'keycloak', label: 'Keycloak' }, { value: 'google', label: 'Google' }, { value: 'adfs', label: 'ADFS', disabled: true }]} />
          </Field>
          <Field label="Rôle de la carte" group>
            <Dropdown
              value={role}
              onChange={setRole}
              options={[
                { value: 'mgmt', label: 'Gestion', sub: 'Interface web, SSH, mises à jour', group: 'Réseau', note: 'eno1' },
                { value: 'st2110-red', label: 'ST 2110 · rouge', sub: 'Flux vidéo, voie principale', group: 'ST 2110', note: 'ens2f0' },
                { value: 'st2110-blue', label: 'ST 2110 · bleu', sub: 'Flux vidéo, voie de secours', group: 'ST 2110', note: 'ens2f1' },
                { value: 'none', label: 'Aucun', sub: 'Débranchée', group: 'Autre', disabled: true },
              ]}
            />
          </Field>
          <Field label="Rôle" group>
            <Segmented value={mode} onChange={setMode} options={[{ value: 'master', label: 'Maître' }, { value: 'member', label: 'Membre' }]} />
          </Field>
        </FieldGrid>
      </Card>
      <Card title="Saisie">
        <FieldGrid>
          <Field label="Nom" hint={`Envoyé en quittant le champ : ${name}`}>
            <TextInput value={name} onChange={setName} commitOnBlur />
          </Field>
          <Field label="Prévenir après">
            <NumberField value={delay} onChange={setDelay} min={0} max={600} suffix="s" />
          </Field>
          <Field label="Volume" group>
            <NumberField label="Niveau" value={vol} onChange={setVol} display={100} min={0} max={1} suffix="%" />
          </Field>
          <Field label="État" group>
            <Switch checked={on} onChange={setOn} label="Active" />
          </Field>
          <Field label="Recherche" group>
            <label className="st-search"><Search size={14} /><input placeholder="Rechercher" /></label>
          </Field>
        </FieldGrid>
      </Card>
    </>
  );
}

function Menus() {
  const items = [
    { label: 'Renommer', icon: <Pencil size={14} />, onClick: () => toast('Renommer'), keys: 'F2' },
    { label: 'Dupliquer', icon: <Copy size={14} />, onClick: () => toast('Dupliquer'), side: '1,2 Go' },
    'sep' as const,
    { note: 'Studio · strata03' },
    { label: 'Supprimer', icon: <Trash2 size={14} />, danger: true, onClick: () => toast('Supprimer') },
  ];
  return (
    <>
      <PageHead title="Menus" sub="ContextMenu, SubTabs">
        <button className="st-icon-btn" title="Plus" onClick={(e) => openContextMenu(e, items)}>
          <LayoutList size={14} />
        </button>
      </PageHead>
      <SubTabsDemo />
      <div className="pg-zone" onContextMenu={(e) => openContextMenu(e, items)}>
        Clic droit ici
      </div>
    </>
  );
}

function SubTabsDemo() {
  const [tab, setTab] = useState<'sdi' | 'ndi' | 'st2110'>('sdi');
  return <SubTabs value={tab} onChange={setTab} items={[{ id: 'sdi', label: 'SDI', count: 4 }, { id: 'ndi', label: 'NDI', count: 3 }, { id: 'st2110', label: 'ST 2110', count: 1 }]} />;
}

function Settings() {
  const [name, setName] = useState('Régie A');
  const [mode, setMode] = useState<'master' | 'member'>('master');
  const dirty = name !== 'Régie A';
  return (
    <>
      <PageHead title="Réglages" sub="strata01.regie.exemple.fr">
        <button className="st-btn primary"><Plus size={14} /> Ajouter un serveur</button>
      </PageHead>
      <Card icon={<Server size={16} />} title="Serveur" actions={<button className="st-btn small"><Cloud size={13} /> Vérifier</button>}>
        <FieldGrid>
          <Field label="Nom">
            <input value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Nom DNS">
            <input value="strata01.regie.exemple.fr" readOnly />
          </Field>
          <Field label="Rôle" group>
            <Segmented value={mode} onChange={setMode} options={[{ value: 'master', label: 'Maître' }, { value: 'member', label: 'Membre' }]} />
          </Field>
        </FieldGrid>
      </Card>
      <Card icon={<Inbox size={16} />} title="Serveurs rattachés">
        <div className="pg-empty">
          <Empty icon={Inbox} text="Aucun serveur rattaché">
            <button className="st-btn small"><Plus size={13} /> Ajouter un serveur</button>
          </Empty>
        </div>
      </Card>
      <SaveBar dirty={dirty} saving={false} onSave={() => toast('Enregistré')} onDiscard={() => setName('Régie A')} />
    </>
  );
}

function Dialogs() {
  return (
    <>
      <PageHead title="Dialogues et messages" sub="confirmDialog, promptDialog, alertDialog, toast" />
      <Row>
        <button className="st-btn" onClick={async () => toast((await confirmDialog({ title: 'Détacher Régie B ?', message: 'Son cockpit se rallume.', confirm: 'Détacher', danger: true })) ? 'Détaché' : 'Annulé')}>Confirmer</button>
        <button className="st-btn" onClick={async () => { const v = await promptDialog({ title: 'Renommer le serveur', label: 'Nom', value: 'Régie B' }); if (v) toast(`Renommé en ${v}`); }}>Demander</button>
        <button className="st-btn" onClick={() => alertDialog({ title: 'Code d’invitation', message: 'K7Q-4XM-92P\nValable 10 min' })}>Informer</button>
        <button className="st-btn" onClick={() => toast('Mise à jour installée')}>Toast</button>
        <button className="st-btn" onClick={() => toast('Studio injoignable', 'error')}>Toast d’erreur</button>
      </Row>
    </>
  );
}

function BrandPage() {
  return (
    <>
      <PageHead title="Marque" sub="Brand" />
      <Row>
        <Brand module="LiveCut" />
        <Brand module="Graphics" />
        <Brand module="Multiview" />
      </Row>
      <Row>
        <Brand module="Core" stacked />
      </Row>
    </>
  );
}

// ---------------------------------------------------------------- données de démonstration du cockpit
const APPS: CockpitApp[] = [
  { id: 'livecut', name: 'LiveCut', icon: Scissors },
  { id: 'graphics', name: 'Graphics', icon: Layers },
  { id: 'multiview', name: 'Multiview', icon: LayoutGrid },
];
const SERVERS: CockpitServer[] = [
  { id: 'strata01', name: 'Régie A', host: 'strata01', up: true, alerts: 1, master: true },
  { id: 'strata02', name: 'Régie B', host: 'strata02', up: true, alerts: 0 },
  { id: 'strata03', name: 'Studio', host: 'strata03', up: false },
];
// Une instance de démonstration s'ouvre dans ce même banc d'essai (?app=…), dans son onglet.
const inst = (app: string, server: string): CockpitInstance => ({ app, server, url: `${location.origin}${location.pathname}?app=${app}@${server}` });
const ALL = [inst('livecut', 'strata01'), inst('graphics', 'strata01'), inst('livecut', 'strata02'), inst('multiview', 'strata03')];
const USER_CAN = new Set(['livecut@strata01', 'livecut@strata02', 'multiview@strata03']);
const cockpitData = (admin: boolean): CockpitData => ({ apps: APPS, servers: SERVERS, instances: admin ? ALL : ALL.filter((i) => USER_CAN.has(instanceKey(i))), admin });
const open = (i: CockpitInstance) => openAppTab(i.url, instanceKey(i));
const APP_KEY = new URLSearchParams(location.search).get('app');
nameTab(APP_KEY ?? 'cockpit');

function App() {
  useLang((s) => s.lang);
  const appKey = new URLSearchParams(location.search).get('app');
  const [view, setView] = useState<'cockpit' | 'components'>('cockpit');
  const [admin, setAdmin] = useState(true);
  const [page, setPage] = useState<PageId>('buttons');
  const [signedIn, setSignedIn] = useState(true);
  const [connected, setConnected] = useState(true);
  const [load, setLoad] = useState(34);
  const data = cockpitData(admin);

  if (!signedIn)
    return (
      <Login
        module="Core"
        serverName="Régie A"
        sso="Okta"
        ssoHref="#"
        onLogin={async (_u, p) => {
          await new Promise((r) => setTimeout(r, 300));
          if (p !== 'strata') throw Object.assign(new Error('wrong'), { status: 401 });
          setSignedIn(true);
        }}
      />
    );

  const launcher = <Launcher data={data} current={appKey ?? undefined} onOpen={open} onCockpit={() => (appKey ? openAppTab(`${location.origin}${location.pathname}`, 'cockpit') : setView('cockpit'))} />;
  const end = (
    <>
      <label className="st-switch pg-ctl" title="Banc d’essai : administrateur ou utilisateur">
        <input type="checkbox" checked={admin} onChange={(e) => setAdmin(e.target.checked)} /> Admin
      </label>
      <label className="st-switch pg-ctl" title="Banc d’essai : couper la liaison">
        <input type="checkbox" checked={connected} onChange={(e) => setConnected(e.target.checked)} /> Liaison
      </label>
      <label className="st-switch pg-ctl" title="Banc d’essai : charger la machine">
        <input type="checkbox" checked={load > 85} onChange={(e) => setLoad(e.target.checked ? 92 : 34)} /> Charge
      </label>
      <StatusPill connected={connected} serverName="Régie A" machine={{ cpu: load, cores: 32, memUsed: 21 * 1024 ** 3, memTotal: 64 * 1024 ** 3, hostname: 'strata01' }} onClick={admin ? () => toast('Santé du serveur') : undefined} />
      <button className="st-help" title={t('Help')}>
        <CircleHelp size={17} />
      </button>
      {admin && (
        <button className="st-help" title={t('Server settings')} onClick={() => toast('Réglages du serveur')}>
          <SettingsIcon size={17} />
        </button>
      )}
      <UserMenu name={admin ? 'Damien' : 'Claire Martin'} role={admin ? 'Administrateur des serveurs' : 'Utilisateur'} named onShortcuts={() => toast('Raccourcis')} onSignOut={() => setSignedIn(false)} />
    </>
  );

  // Un onglet d'appli (démonstration).
  if (appKey) {
    const [a, srv] = appKey.split('@');
    const app = APPS.find((x) => x.id === a);
    const server = SERVERS.find((x) => x.id === srv);
    document.title = `${app?.name} · ${server?.name}`;
    return (
      <div className="st-app">
        <TopBar module={app?.name ?? a} start={launcher} end={end}>
          <div className="st-tabs">
            <button className="on">Accueil</button>
            <button>Réglages</button>
          </div>
        </TopBar>
        <div className="st-content">
          <Page>
            <PageHead title={app?.name ?? a} sub={server?.name} />
            <Card title="Onglet de l’appli">
              <span className="st-mono">{location.href}</span>
            </Card>
          </Page>
        </div>
        <ContextMenuHost />
        <DialogHost />
        <Toasts />
      </div>
    );
  }

  const body = { buttons: <Buttons />, fields: <Fields />, menus: <Menus />, settings: <Settings />, dialogs: <Dialogs />, brand: <BrandPage /> }[page];
  const groups = [...new Set(PAGES.map((p) => p.group))];
  return (
    <div className="st-app">
      <TopBar module="Core" start={launcher} end={end}>
        <div className="st-tabs" title="Banc d’essai">
          <button className={view === 'cockpit' ? 'on' : ''} onClick={() => setView('cockpit')}>Cockpit</button>
          <button className={view === 'components' ? 'on' : ''} onClick={() => setView('components')}>Composants</button>
        </div>
      </TopBar>
      <div className="st-content">
        {view === 'cockpit' ? (
          <Cockpit data={data} onOpen={open} onServer={(id) => toast(`Réglages de ${SERVERS.find((s) => s.id === id)?.name}`)} />
        ) : (
          <SidePage
            nav={groups.map((g) => (
              <div key={g}>
                <NavSection title={g} />
                {PAGES.filter((p) => p.group === g).map((p) => (
                  <NavItem key={p.id} icon={p.icon} label={p.label} on={page === p.id} onClick={() => setPage(p.id)} status={p.id === 'dialogs' ? 'warn' : null} count={p.id === 'menus' ? 3 : undefined} />
                ))}
              </div>
            ))}
          >
            {body}
          </SidePage>
        )}
      </div>
      <ContextMenuHost />
      <DialogHost />
      <Toasts />
    </div>
  );
}

setLang('fr');
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
