// Le matériel du serveur : ce que voit la machine (relu à la demande) et les réglages en cours
// d'édition, partagés par les pages Cartes SDI, NDI, ST 2110, Réseau et Horloge (un seul
// enregistrement pour tout).
import { create } from 'zustand';
import { t, toast } from '@strata/ui';
import { api, ApiError } from '../../api';
import { friendly } from '../../errors';

export type Port = 'in' | 'out' | 'both';
export type Fn = 'unused' | 'input' | 'output';

export interface DetectedCard {
  id: string;
  modelName: string;
  legacyModel: string;
  connectors: number;
  /** L'appareil Desktop Video de chaque connecteur. */
  devices: number[];
  ports: Port[];
  hasReference: boolean;
  referenceLocked?: boolean | null;
  referenceKind?: 'blackBurst' | 'triLevel' | null;
  referenceFormat?: string | null;
  interface?: string | null;
  profile?: string | null;
  profiles: string[];
  internalKeyer: boolean;
  externalKeyer: boolean;
  /** Chaque connecteur qui est la sortie d'une paire en full duplex (jamais une entrée). */
  paired: boolean[];
  fullDuplex: boolean;
}

export interface Model {
  name: string;
  legacy: string;
  jack: 'din' | 'bnc';
  connectors: number;
  order: number[];
  reference: boolean | null;
  ports: Port[];
}

export interface Card {
  id: string;
  name: string;
  modelName: string;
  connectors: { name: string; function: Fn }[];
  genlock: boolean;
  genlockOffset: number;
}

export interface Nic {
  name: string;
  ipv4: string[];
  mac?: string | null;
  mtu?: number | null;
  speed?: number | null;
  up?: boolean | null;
  kind?: string | null;
  model?: string | null;
  /** La carte PCI (`0000:3b:00`) : les ports d'une même carte la partagent. */
  pci?: string | null;
  port?: number | null;
  onboard: boolean;
  connector?: 'sfp' | 'rj45' | null;
  /** Son horloge matérielle PTP (`ptp0`). */
  phc?: string | null;
}

export interface Flow {
  id: string;
  name: string;
  direction: 'input' | 'output';
  essence: 'video' | 'audio' | 'data';
  red: string;
  blue?: string | null;
  /** Le port média (1 ou 2), avec deux ports indépendants. */
  port?: number;
  /** La vidéo (« 1080i50 ») ; vide : le format maison. */
  format?: string;
  payloadType?: number;
  /** Son (ST 2110-30) : canaux, bits par échantillon, durée d'un paquet (µs). */
  channels?: number;
  bits?: number;
  ptimeUs?: number;
  /** Reçu ou envoyé par le Core ; coupé : réglé seulement (aussi par NMOS). */
  enabled?: boolean;
}

export type Role = 'management' | 'ndi' | 'media1' | 'media2' | 'ptp';

export interface HwConfig {
  cards: Card[];
  network: Record<Role, string> & {
    mediaMode: 'redundant' | 'separate';
    /** Comment le Core tient les flux ST 2110 (MTL). */
    mediaIo?: 'off' | 'kernel' | 'afXdp' | 'dpdk';
    media1Address?: string;
    media2Address?: string;
  };
  ndi: { discovery: string; machine: string };
  flows: Flow[];
  ptp: { domain: number; taiOffset: number };
  clock: ClockSettings;
  /** NMOS : IS-04 en pair-à-pair (mDNS) et IS-05. */
  nmos?: NmosSettings;
}

export interface NmosSettings {
  enabled: boolean;
  port: number;
  /** L'interface où le Core s'annonce ; vide : celle de gestion. */
  interface: string;
}

/** L'état de NMOS (`GET /api/hardware/nmos`). */
export interface NmosStatus {
  running: boolean;
  url?: string | null;
  interface?: string | null;
  nodeId?: string | null;
  error?: string | null;
  /** Par flux : l'émetteur (ou le récepteur) que NMOS y a branché. */
  links: Record<string, { peer?: string | null }>;
}

export type HouseRate = '25' | '29.97' | '30' | '24' | '23.98';

export interface ClockSettings {
  source: 'ptp' | 'ntp';
  ntpServers: string[];
  house: { rate: HouseRate; offset: 'local' | 'utc' | 'fixed'; fixedMinutes: number; dropFrame: boolean; dailyJam: string };
}

export interface Snapshot {
  config: HwConfig;
  /** La révision de hardware.json lue : renvoyée en enregistrant (409 si un autre a écrit). */
  revision?: string;
  decklink: { built: boolean; driver: boolean; driverPath?: string | null; version?: string | null; fake: boolean; cards: DetectedCard[] };
  models: Model[];
  nics: Nic[];
  ndi: { path?: string; version?: string | null; error?: string };
}

/** Une étape de la mise en place de l'heure (chrony, ptp4l). */
export interface ApplyStep {
  what: 'chrony' | 'ptp4l' | 'unit' | 'restartChrony' | 'startPtp4l' | 'stopPtp4l';
  ok: boolean;
  message?: string | null;
}

export const useHw = create<{ snap: Snapshot | null; draft: HwConfig | null; saving: boolean }>(() => ({ snap: null, draft: null, saving: false }));

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** Relit ce que voit la machine ; les réglages en cours d'édition restent. */
export async function loadHw() {
  try {
    const snap = await api<Snapshot>('GET', '/api/hardware');
    const { snap: before, draft } = useHw.getState();
    const keep = before && draft && !same(draft, before.config);
    useHw.setState({ snap, draft: keep ? draft : structuredClone(snap.config) });
  } catch (e) {
    toast(friendly(e), 'error');
  }
}

/** Change les réglages en cours d'édition. */
export function editHw(fn: (d: HwConfig) => void) {
  const d = useHw.getState().draft;
  if (!d) return;
  const next = structuredClone(d);
  fn(next);
  useHw.setState({ draft: next });
}

export function useHwDirty() {
  return useHw((s) => !!s.snap && !!s.draft && !same(s.draft, s.snap.config));
}

export async function saveHw() {
  const { draft, snap } = useHw.getState();
  if (!draft || !snap) return;
  useHw.setState({ saving: true });
  const put = (body: HwConfig, rev?: string) =>
    api<Snapshot & { ndiChanged?: boolean; clockApplied?: ApplyStep[] | null }>('PUT', `/api/hardware${rev ? `?rev=${encodeURIComponent(rev)}` : ''}`, body);
  try {
    let saved;
    try {
      saved = await put(draft, snap.revision);
    } catch (e) {
      if (!(e instanceof ApiError) || e.status !== 409) throw e;
      // Écrits entre-temps (NMOS a branché un récepteur, un autre admin) : relus. Si les flux n'ont
      // pas été touchés ici, ceux du serveur sont gardés et l'enregistrement repart ; sinon on
      // laisse choisir.
      const fresh = await api<Snapshot>('GET', '/api/hardware');
      if (!same(draft.flows, snap.config.flows)) {
        useHw.setState({ snap: fresh });
        toast(t('The ST 2110 flows changed meanwhile (NMOS or another administrator): check them, then save again.'), 'error');
        return;
      }
      const merged = { ...structuredClone(draft), flows: structuredClone(fresh.config.flows) };
      saved = await put(merged, fresh.revision);
    }
    useHw.setState({ snap: saved, draft: structuredClone(saved.config) });
    toast(t('Settings saved'));
    const failed = saved.clockApplied?.find((s) => !s.ok);
    if (failed) toast(t('The server time could not be set up: {why}', { why: friendly(failed.message ?? '') }), 'error');
    if (saved.ndiChanged) toast(t('Restart the apps that use NDI: they read these settings when they start.'), 'info');
  } catch (e) {
    toast(friendly(e), 'error');
  } finally {
    useHw.setState({ saving: false });
  }
}

export function discardHw() {
  const snap = useHw.getState().snap;
  if (snap) useHw.setState({ draft: structuredClone(snap.config) });
}

/** Ce qu'un connecteur fait par défaut : ce que seul il sait faire, sinon rien. */
export const defaultFn = (p: Port | undefined): Fn => (p === 'in' ? 'input' : p === 'out' ? 'output' : 'unused');

/** Les réglages d'une carte détectée (ceux par défaut si elle n'en a pas encore). */
export function cardOf(draft: HwConfig, d: DetectedCard): Card {
  const c = draft.cards.find((x) => x.id === d.id);
  const connectors = Array.from({ length: d.connectors }, (_, i) => c?.connectors[i] ?? { name: '', function: defaultFn(d.ports[i]) });
  return { id: d.id, name: c?.name ?? '', modelName: d.modelName, genlock: c?.genlock ?? false, genlockOffset: c?.genlockOffset ?? 0, connectors };
}

/** Change les réglages d'une carte détectée (créés au premier changement). */
export function editCard(d: DetectedCard, fn: (c: Card) => void) {
  editHw((draft) => {
    const c = cardOf(draft, d);
    fn(c);
    const i = draft.cards.findIndex((x) => x.id === d.id);
    if (i >= 0) draft.cards[i] = c;
    else draft.cards.push(c);
  });
}

/** L'évaluation des ports média (`GET /api/hardware/ports`), en Mb/s. */
export interface Both {
  rx: number;
  tx: number;
}

export interface PortLoad {
  n: number;
  iface: string;
  speed?: number | null;
  up?: boolean | null;
  measured?: Both | null;
  dropped?: number | null;
  planned: Both;
  used: Both;
  free?: Both | null;
  percent?: number | null;
}

export interface Capacity {
  redundant: boolean;
  headroom: number;
  ports: PortLoad[];
  formats: { name: string; mbps: number; rx?: number | null; tx?: number | null }[];
}
