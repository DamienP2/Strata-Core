// Réglages › Matériel : Cartes SDI, NDI, ST 2110, Réseau, Horloge. Les réglages sont partagés par
// les cinq pages (store.ts) et enregistrés d'un coup.
import { useState } from 'react';
import { Cable, Gauge, MonitorPlay, Network as NetIcon, Pencil, Plus, Radio, RefreshCw, Search, Trash2, Waves } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, Field, FieldGrid, Modal, NumberField, PageHead, Segmented, Switch, confirmDialog, t, tn, toast, type DropOption } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { HwSaveBar, useHardware } from './common';
import { Meter } from '../health/Resources';
import { FlowsCard, InputsCard, OutputsCard, usePoll } from './Inputs';
import { DeckLink3D, Pcie3D, SDI_COLOR, nicLook, printedOf, type Jack } from './Card3D';
import { cardOf, editCard, editHw, loadHw, type DetectedCard, type Flow, type Fn, type Capacity, type HwConfig, type Nic, type NmosStatus, type Port, type Snapshot } from './store';

function Rescan() {
  const [busy, setBusy] = useState(false);
  return (
    <button
      type="button"
      className="st-btn"
      disabled={busy}
      onClick={async () => {
        setBusy(true);
        await loadHw();
        setBusy(false);
      }}
    >
      <RefreshCw size={14} /> {t('Look again')}
    </button>
  );
}

// ---------- Cartes SDI ----------

const FN_LABEL: Record<Fn, string> = { input: 'Input', output: 'Output', unused: 'Unused' };

/** La fonction d'un connecteur : ce qu'il sait faire seulement. */
function fnOptions(port: Port) {
  return (['input', 'output', 'unused'] as Fn[]).map((f) => ({
    value: f,
    label: t(FN_LABEL[f]),
    disabled: (f === 'input' && port === 'out') || (f === 'output' && port === 'in'),
  }));
}

/** Les modes des connecteurs (profils Desktop Video) : ce qu'ils veulent dire ici. */
function profileLabel(p: string, perPair: boolean): string {
  switch (p) {
    case '2dhd':
      return t('Independent');
    case '1dfd':
      return perPair ? t('In pairs') : t('1 pair');
    case '4dhd':
      return t('4 independent');
    case '2dfd':
      return t('2 pairs');
    case '1dhd':
      return t('1 connector');
    default:
      return p;
  }
}

/** Le mode des connecteurs d'une carte : chacun à part, ou par paires (une entrée et sa sortie). */
function DuplexMode({ d }: { d: DetectedCard }) {
  const [busy, setBusy] = useState(false);
  if (d.profiles.length < 2 || !d.profile) return null;
  const perPair = !/8k/i.test(d.modelName);
  const order = ['2dhd', '4dhd', '1dfd', '2dfd', '1dhd'];
  const profiles = [...d.profiles].sort((a, b) => order.indexOf(a) - order.indexOf(b));
  return (
    <div className="core-hw-genlock">
      <span className="core-service">
        <span className="st-muted">{t('Connectors')}</span>
        <Segmented<string>
          value={d.profile}
          options={profiles.map((p) => ({ value: p, label: profileLabel(p, perPair), disabled: busy }))}
          onChange={async (p) => {
            if (p === d.profile) return;
            const ok = await confirmDialog({
              title: t('Change the connector mode?'),
              message: t('The captures of this card stop for a few seconds. In pairs, each pair is one input and its output (loop-through, or fill and key): the second connector of a pair can no longer be an input.'),
              confirm: t('Change'),
            });
            if (!ok) return;
            setBusy(true);
            try {
              await api('POST', '/api/hardware/decklink/profile', { card: d.id, profile: p });
              await loadHw();
              toast(t('Connector mode changed'));
            } catch (e) {
              toast(friendly(e), 'error');
            }
            setBusy(false);
          }}
        />
      </span>
      <span className="st-muted">{d.fullDuplex ? t('Full duplex: one input and its output per pair') : t('Half duplex: each connector an input or an output')}</span>
    </div>
  );
}

function RefPill({ d, genlock }: { d: DetectedCard; genlock: boolean }) {
  if (!d.hasReference || d.referenceLocked == null) return null;
  if (d.referenceLocked)
    return <Badge tone="ok">{`${t('REF locked')}${d.referenceFormat ? ` · ${d.referenceFormat}` : ''}`}</Badge>;
  return <Badge tone={genlock ? 'bad' : 'warn'}>{genlock ? t('REF absent') : `${t('REF absent')} · ${t('internal clock')}`}</Badge>;
}

function SdiCard({ d, snap, draft }: { d: DetectedCard; snap: Snapshot; draft: HwConfig }) {
  const [sel, setSel] = useState<number | null>(null);
  const c = cardOf(draft, d);
  const m = snap.models.find((x) => x.name.toLowerCase() === d.modelName.toLowerCase());
  const order = m && m.order.length === d.connectors ? m.order : Array.from({ length: d.connectors }, (_, i) => i + 1);
  const jack = m?.jack ?? (d.connectors > 4 ? 'din' : 'bnc');
  const reference = m?.reference ?? d.hasReference;
  const functions = c.connectors.map((k) => k.function);
  const used = functions.filter((f) => f !== 'unused').length;
  return (
    <Card
      icon={<MonitorPlay size={16} />}
      title={c.name || d.modelName}
      actions={
        <span className="core-service">
          <RefPill d={d} genlock={c.genlock} />
          <span className="st-muted">{[d.modelName, d.interface === 'pcie' ? 'PCIe' : d.interface, tn(used, '{n} connector used', '{n} connectors used')].filter(Boolean).join(' · ')}</span>
        </span>
      }
    >
      <div className="core-hw-body">
        <div className="core-hw-figure">
          <DeckLink3D model={d.modelName} jack={jack} order={order} reference={reference} functions={functions} genlock={c.genlock} sel={sel} onPick={setSel} />
          <div className="core-hw-legend">
            <span style={{ ['--c' as string]: SDI_COLOR.input }}>{t('Input')}</span>
            <span style={{ ['--c' as string]: SDI_COLOR.output }}>{t('Output')}</span>
            {reference && <span style={{ ['--c' as string]: SDI_COLOR.ref }}>REF</span>}
          </div>
        </div>
        <div className="core-hw-ports">
          <Field label={t('Card name')}>
            <input value={c.name} placeholder={d.modelName} onChange={(e) => editCard(d, (x) => void (x.name = e.target.value))} />
          </Field>
          {c.connectors.map((k, i) => {
            const printed = printedOf(d.modelName, order, i + 1);
            return (
              <div key={i} className={`core-hw-port ${sel === i ? 'sel' : ''}`} onClick={() => setSel(i)}>
                <span className="core-hw-dot" style={{ background: SDI_COLOR[k.function] || 'var(--line-2)' }} />
                <span className="core-hw-port-name">
                  <strong>SDI {i + 1}</strong>
                  {printed !== null && <span className="st-muted">{t('n° {n} on the bracket', { n: printed })}</span>}
                  {d.paired?.[i] && <span className="st-muted">{t('output of its pair')}</span>}
                </span>
                <Segmented<Fn> value={k.function} options={fnOptions(d.ports[i] ?? 'both')} onChange={(f) => editCard(d, (x) => void (x.connectors[i].function = f))} />
                <input className="core-hw-wire" value={k.name} placeholder={t('Wired to…')} onChange={(e) => editCard(d, (x) => void (x.connectors[i].name = e.target.value))} />
              </div>
            );
          })}
          <DuplexMode d={d} />
          {reference && (
            <div className="core-hw-genlock">
              <Switch checked={c.genlock} onChange={(v) => editCard(d, (x) => void (x.genlock = v))} label={t('Genlock: the outputs follow the reference input')} />
              {c.genlock && (
                <span className="core-service">
                  <span className="st-muted">{t('Offset (lines)')}</span>
                  <NumberField value={c.genlockOffset} min={-2000} max={2000} onChange={(v) => editCard(d, (x) => void (x.genlockOffset = Math.round(v)))} />
                </span>
              )}
            </div>
          )}
        </div>
      </div>
    </Card>
  );
}

export function SdiPage() {
  const hw = useHardware();
  if (!hw) return null;
  const [snap, draft] = hw;
  const dl = snap.decklink;
  const connectors = dl.cards.reduce((n, c) => n + c.connectors, 0);
  const absent = draft.cards.filter((c) => !dl.cards.some((d) => d.id === c.id));
  const sub = !dl.built
    ? t('DeckLink support is not built into this Core')
    : !dl.driver
      ? t('Desktop Video not found')
      : `${tn(dl.cards.length, '{n} card', '{n} cards')} · ${tn(connectors, '{n} connector', '{n} connectors')}${dl.version ? ` · Desktop Video ${dl.version}` : ''}${dl.fake ? ` · ${t('simulated')}` : ''}`;
  return (
    <>
      <PageHead title={t('SDI cards')} sub={sub}>
        <Rescan />
      </PageHead>
      {dl.cards.length === 0 ? (
        <div className="core-empty">
          <Empty icon={MonitorPlay} text={dl.driver ? t('No DeckLink card') : t('No DeckLink card: Blackmagic Desktop Video is not installed (the installer can set it up).')} />
        </div>
      ) : (
        dl.cards.map((d) => <SdiCard key={d.id} d={d} snap={snap} draft={draft} />)
      )}
      <InputsCard cfg={snap.config} />
      <OutputsCard cfg={snap.config} />
      {absent.map((c) => (
        <Card
          key={c.id}
          icon={<MonitorPlay size={16} />}
          title={c.name || c.modelName}
          actions={
            <button type="button" className="st-btn small danger" onClick={() => editHw((x) => void (x.cards = x.cards.filter((y) => y.id !== c.id)))}>
              <Trash2 size={13} /> {t('Forget')}
            </button>
          }
        >
          <p className="core-warn">{t('{model} is not seen any more (removed, or Desktop Video stopped). Its settings are kept.', { model: c.modelName || c.id })}</p>
        </Card>
      ))}
      <HwSaveBar />
    </>
  );
}

// ---------- Réseau ----------

const ROLE_COLOR = { management: '#a1a7b3', ndi: '#c084fc', media1: '#2dd4bf', media2: '#f59e0b', ptp: '#f5c542', red: '#f87171', blue: '#60a5fa' };

/** Les rôles d'une interface, avec leur nom et leur couleur. */
function rolesOf(cfg: HwConfig, name: string): { label: string; color: string }[] {
  const n = cfg.network;
  const redundant = n.mediaMode !== 'separate';
  const out: { label: string; color: string }[] = [];
  if (n.media1 === name) out.push(redundant ? { label: t('ST 2110 red'), color: ROLE_COLOR.red } : { label: t('Media 1'), color: ROLE_COLOR.media1 });
  if (n.media2 === name) out.push(redundant ? { label: t('ST 2110 blue'), color: ROLE_COLOR.blue } : { label: t('Media 2'), color: ROLE_COLOR.media2 });
  if (n.ptp === name) out.push({ label: 'PTP', color: ROLE_COLOR.ptp });
  if (n.ndi === name) out.push({ label: 'NDI', color: ROLE_COLOR.ndi });
  if (n.management === name) out.push({ label: t('Management'), color: ROLE_COLOR.management });
  return out;
}

function speed(mb?: number | null) {
  if (!mb) return '—';
  return mb >= 1000 ? `${mb / 1000} Gb/s` : `${mb} Mb/s`;
}

function nicOptions(nics: Nic[]): DropOption<string>[] {
  return [{ value: '', label: t('None') }, ...nics.map((n) => ({ value: n.name, label: n.name, note: n.ipv4[0] ?? '' }))];
}

/** Une carte réseau ajoutée (plusieurs ports sur une même carte PCI), en 3D. */
function NicCard({ ports, cfg }: { ports: Nic[]; cfg: HwConfig }) {
  const [sel, setSel] = useState<number | null>(null);
  const sfp = ports.some((p) => p.connector === 'sfp');
  const model = ports[0].model ?? t('Network card');
  const jacks: Jack[] = ports.map((p, i) => {
    const roles = rolesOf(cfg, p.name);
    return { k: i, etched: String((p.port ?? i) + 1), label: p.name, sub: roles.map((r) => r.label).join(' · ') || undefined, color: roles[0]?.color ?? '' };
  });
  return (
    <Card icon={<NetIcon size={16} />} title={model} actions={<span className="st-muted st-mono">{ports[0].pci}</span>}>
      <div className="core-hw-body">
        <div className="core-hw-figure">
          <Pcie3D look={nicLook(ports[0].model, ports.length, sfp)} kind={sfp ? 'sfp' : 'rj45'} jacks={jacks} sel={sel} onPick={setSel} title={model} />
        </div>
        <div className="core-hw-ports">
          {ports.map((p, i) => (
            <NicRow key={p.name} n={p} cfg={cfg} sel={sel === i} onClick={() => setSel(i)} />
          ))}
        </div>
      </div>
    </Card>
  );
}

function NicRow({ n, cfg, sel, onClick }: { n: Nic; cfg: HwConfig; sel?: boolean; onClick?: () => void }) {
  return (
    <div className={`core-hw-port nic ${sel ? 'sel' : ''}`} onClick={onClick}>
      <span className={`st-dot ${n.up ? 'ok' : n.up === false ? 'bad' : ''}`} />
      <span className="core-hw-port-name">
        <strong className="st-mono">{n.name}</strong>
        <span className="st-muted st-mono">{n.ipv4.join(', ') || t('no address')}</span>
      </span>
      <span className="st-muted">{speed(n.speed)}</span>
      <span className="core-chips">
        {rolesOf(cfg, n.name).map((r) => (
          <span key={r.label} className="core-hw-role" style={{ ['--c' as string]: r.color }}>
            {r.label}
          </span>
        ))}
      </span>
    </div>
  );
}

/** Un débit en Mb/s, lisible. */
function rate(mbps: number): string {
  return mbps >= 1000 ? `${(mbps / 1000).toLocaleString(undefined, { maximumFractionDigits: 1 })} Gb/s` : `${Math.round(mbps)} Mb/s`;
}

/** La charge des ports média en direct, et les flux qui y tiennent encore. */
function PortsLoadCard({ cap }: { cap: Capacity | null }) {
  if (!cap || cap.ports.length === 0) return null;
  const margin = Math.round(cap.headroom * 100);
  const side = (p: Capacity['ports'][number], dir: 'rx' | 'tx') => {
    const used = p.used[dir];
    const pctOf = p.speed ? (used / p.speed) * 100 : 0;
    return (
      <div className="core-use" title={p.measured ? t('Measured {m} · planned {p}', { m: rate(p.measured[dir]), p: rate(p.planned[dir]) }) : t('Planned {p}', { p: rate(p.planned[dir]) })}>
        {p.speed ? <Meter value={pctOf} warn={margin - 10} bad={margin} /> : null}
        <span className="st-mono">{rate(used)}</span>
        {p.speed ? <span className="st-muted">{Math.round(pctOf)} %</span> : null}
      </div>
    );
  };
  return (
    <Card icon={<Gauge size={16} />} title={t('Media ports load')}>
      <table className="st-table">
        <thead>
          <tr>
            <th>{t('Port')}</th>
            <th>{t('Link')}</th>
            <th>{t('Received')}</th>
            <th>{t('Sent')}</th>
          </tr>
        </thead>
        <tbody>
          {cap.ports.map((p) => (
            <tr key={p.iface}>
              <td>
                <strong>{cap.redundant ? (p.n === 1 ? t('Port 1 · red') : t('Port 2 · blue')) : t('Port {n}', { n: p.n })}</strong> <span className="st-mono st-muted">{p.iface}</span>
              </td>
              <td>
                {p.up === false ? <Badge tone="bad">{t('Down')}</Badge> : p.speed ? rate(p.speed) : <span className="st-muted">{t('Unknown')}</span>}
                {p.dropped ? <span className="st-muted"> · {tn(Math.round(p.dropped), '{n} packet dropped/s', '{n} packets dropped/s')}</span> : null}
              </td>
              <td>{side(p, 'rx')}</td>
              <td>{side(p, 'tx')}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="st-muted core-note">
        {cap.redundant
          ? t('Used: the larger of the measured rate and what the flows set need. Margin: {m} % of the link. In SMPTE 2022-7 each flow uses both ports.', { m: margin })
          : t('Used: the larger of the measured rate and what the flows set need. Margin: {m} % of the link.', { m: margin })}
      </p>
      <table className="st-table">
        <thead>
          <tr>
            <th>{t('Format')}</th>
            <th>{t('Rate')}</th>
            <th>{t('Still room to receive')}</th>
            <th>{t('Still room to send')}</th>
          </tr>
        </thead>
        <tbody>
          {cap.formats.map((f) => (
            <tr key={f.name}>
              <td>{f.name}</td>
              <td className="st-mono">{rate(f.mbps)}</td>
              <td>{f.rx ?? '—'}</td>
              <td>{f.tx ?? '—'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Card>
  );
}

/** NMOS : le Core annoncé en mDNS (sans registre) et branché par un contrôleur (Buttons…). */
function NmosCard({ draft, opts, status }: { draft: HwConfig; opts: DropOption<string>[]; status: NmosStatus | null }) {
  const n = draft.nmos ?? { enabled: false, port: 3212, interface: '' };
  const set = (fn: (x: NonNullable<HwConfig['nmos']>) => void) =>
    editHw((x) => {
      x.nmos = { ...(x.nmos ?? { enabled: false, port: 3212, interface: '' }) };
      fn(x.nmos);
    });
  return (
    <Card icon={<Cable size={16} />} title="NMOS (IS-04 · IS-05)">
      <Switch checked={n.enabled} onChange={(v) => set((x) => void (x.enabled = v))} label={t('Announce the ST 2110 flows to an NMOS controller')} />
      <p className="st-muted core-note">
        {t('Without a registry: the Core announces itself in mDNS (peer-to-peer) and the controller connects the receivers. The addresses of the flows sent stay those set here.')}
      </p>
      {n.enabled && (
        <>
          <FieldGrid two>
            <Field label={t('Interface')} hint={t('The controller reaches the Core through it.')} group>
              <Dropdown value={n.interface} options={[{ value: '', label: t('Management interface') }, ...opts.filter((o) => o.value !== '')]} onChange={(v) => set((x) => void (x.interface = v))} />
            </Field>
            <Field label={t('Port')}>
              <NumberField value={n.port} min={1} max={65535} onChange={(v) => set((x) => void (x.port = v))} />
            </Field>
          </FieldGrid>
          {status?.error ? (
            <p className="core-note">
              <Badge tone="bad">{t('Error')}</Badge> {friendly(status.error)}
            </p>
          ) : status?.running && status.url ? (
            <p className="core-note">
              <Badge tone="ok">{t('Announced')}</Badge> <span className="st-mono">{status.url}</span>
            </p>
          ) : null}
        </>
      )}
    </Card>
  );
}

export function NetworkPage() {
  const nmos = usePoll<NmosStatus>('/api/hardware/nmos');
  const cap = usePoll<Capacity>('/api/hardware/ports');
  const hw = useHardware();
  if (!hw) return null;
  const [snap, draft] = hw;
  const r = draft.network;
  const opts = nicOptions(snap.nics);
  const setRole = (k: keyof HwConfig['network'], v: string) => editHw((x) => void ((x.network as Record<string, string>)[k] = v));
  // Les cartes ajoutées, par carte PCI ; le reste (carte mère, virtuelles) dans un tableau.
  const cards = new Map<string, Nic[]>();
  const rest: Nic[] = [];
  for (const n of snap.nics) {
    if (n.pci && !n.onboard && n.kind === 'ethernet') cards.set(n.pci, [...(cards.get(n.pci) ?? []), n]);
    else rest.push(n);
  }
  const redundant = r.mediaMode !== 'separate';
  return (
    <>
      <PageHead title={t('Network')} sub={tn(snap.nics.length, '{n} interface', '{n} interfaces')}>
        <Rescan />
      </PageHead>
      <Card icon={<Waves size={16} />} title={t('ST 2110 media ports')}>
        <Segmented<'redundant' | 'separate'>
          value={redundant ? 'redundant' : 'separate'}
          onChange={(v) => setRole('mediaMode', v)}
          options={[
            { value: 'redundant', label: t('SMPTE 2022-7 (red + blue)') },
            { value: 'separate', label: t('Two separate ports') },
          ]}
        />
        <p className="st-muted core-note">
          {redundant ? t('Each flow goes through both ports at once, on two networks: if one fails, the other carries on.') : t('Each flow goes through one port or the other: twice the bandwidth, no redundancy.')}
        </p>
        <FieldGrid two>
          <Field label={redundant ? t('Port 1 · red network') : t('Media port 1')} group>
            <Dropdown value={r.media1} options={opts} onChange={(v) => setRole('media1', v)} />
          </Field>
          <Field label={redundant ? t('Port 2 · blue network') : t('Media port 2')} group>
            <Dropdown value={r.media2} options={opts} onChange={(v) => setRole('media2', v)} />
          </Field>
        </FieldGrid>
        <Field label={t('Flows received and sent by the Core (MTL)')} group>
          <Segmented<'off' | 'kernel' | 'afXdp' | 'dpdk'>
            value={r.mediaIo ?? 'off'}
            onChange={(v) => setRole('mediaIo', v)}
            options={[
              { value: 'off', label: t('No') },
              { value: 'kernel', label: t('Kernel') },
              { value: 'afXdp', label: 'AF_XDP' },
              { value: 'dpdk', label: 'DPDK' },
            ]}
          />
        </Field>
        <p className="st-muted core-note">
          {
            {
              off: t('The flows are only set: the apps receive and send them themselves.'),
              kernel: t('Through the kernel: simple, for a few HD flows.'),
              afXdp: t('AF_XDP: the port stays with the kernel, much faster.'),
              dpdk: t('DPDK: the fastest (UHD, many flows); the ports are reserved for ST 2110 and need their own address.'),
            }[r.mediaIo ?? 'off']
          }
        </p>
        {r.mediaIo === 'dpdk' && (
          <FieldGrid two>
            <Field label={t('Address of port 1')}>
              <input className="st-mono" value={r.media1Address ?? ''} placeholder="10.10.1.21/24" onChange={(e) => setRole('media1Address', e.target.value)} />
            </Field>
            <Field label={t('Address of port 2')}>
              <input className="st-mono" value={r.media2Address ?? ''} placeholder="10.20.1.21/24" onChange={(e) => setRole('media2Address', e.target.value)} />
            </Field>
          </FieldGrid>
        )}
      </Card>
      <PortsLoadCard cap={cap} />
      <NmosCard draft={draft} opts={opts} status={nmos} />
      <Card icon={<NetIcon size={16} />} title={t('Roles')}>
        <FieldGrid>
          <Field label={t('Management')} hint={t('The control room reaches the cockpit through it.')} group>
            <Dropdown value={r.management} options={opts} onChange={(v) => setRole('management', v)} />
          </Field>
          <Field label="NDI" hint={t('NDI sends and looks for sources on it only.')} group>
            <Dropdown value={r.ndi} options={opts} onChange={(v) => setRole('ndi', v)} />
          </Field>
          <Field label="PTP" hint={t('ptp4l runs on it.')} group>
            <Dropdown value={r.ptp} options={opts} onChange={(v) => setRole('ptp', v)} />
          </Field>
        </FieldGrid>
      </Card>
      {[...cards.values()].map((ports) => (
        <NicCard key={ports[0].pci} ports={ports.sort((a, b) => (a.port ?? 0) - (b.port ?? 0))} cfg={draft} />
      ))}
      {rest.length > 0 && (
        <Card icon={<NetIcon size={16} />} title={t('Motherboard and others')}>
          <div className="core-hw-ports">
            {rest.map((n) => (
              <NicRow key={n.name} n={n} cfg={draft} />
            ))}
          </div>
        </Card>
      )}
      <HwSaveBar />
    </>
  );
}

// ---------- ST 2110 ----------

const ESSENCE: Record<Flow['essence'], string> = { video: 'Video', audio: 'Audio', data: 'Data' };

function FlowEditor({ flow, redundant, onClose, onSave }: { flow: Flow; redundant: boolean; onClose: () => void; onSave: (f: Flow) => void }) {
  const [f, setF] = useState<Flow>(flow);
  const set = <K extends keyof Flow>(k: K, v: Flow[K]) => setF({ ...f, [k]: v });
  return (
    <Modal
      title={flow.name ? t('Edit {name}', { name: flow.name }) : t('New flow')}
      icon={<Waves size={16} />}
      onClose={onClose}
      onSubmit={() => onSave({ ...f, blue: redundant ? f.blue : null })}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={!f.name.trim() || !f.red.trim() || (redundant && !f.blue?.trim())}>
            {t('OK')}
          </button>
        </>
      }
    >
      <Field label={t('Name')}>
        <input value={f.name} onChange={(e) => set('name', e.target.value)} />
      </Field>
      <Switch checked={f.enabled !== false} onChange={(v) => set('enabled', v)} label={t('On: received or sent by the Core')} />
      <FieldGrid two>
        <Field label={t('Direction')} group>
          <Segmented<Flow['direction']> value={f.direction} onChange={(v) => set('direction', v)} options={[{ value: 'input', label: t('Received') }, { value: 'output', label: t('Sent') }]} />
        </Field>
        <Field label={t('Essence')} group>
          <Segmented<Flow['essence']> value={f.essence} onChange={(v) => set('essence', v)} options={(['video', 'audio', 'data'] as const).map((e) => ({ value: e, label: t(ESSENCE[e]) }))} />
        </Field>
      </FieldGrid>
      {f.essence === 'video' && (
        <Field label={t('Video format')} group>
          <Dropdown
            value={f.format ?? ''}
            options={[
              { value: '', label: t('House format') },
              ...['1080i50', '1080i59.94', '1080p25', '1080p50', '1080p59.94', '720p50', '720p59.94', '2160p25', '2160p50', '2160p59.94'].map((x) => ({ value: x, label: x })),
            ]}
            onChange={(v) => set('format', v)}
          />
        </Field>
      )}
      {f.essence === 'audio' && (
        <FieldGrid two>
          <Field label={t('Channels')} group>
            <Dropdown value={String(f.channels ?? 2)} options={['1', '2', '4', '6', '8', '16'].map((x) => ({ value: x, label: x }))} onChange={(v) => set('channels', Number(v))} />
          </Field>
          <Field label={t('Samples and packets')} group>
            <Dropdown
              value={`${f.bits ?? 24}/${f.ptimeUs ?? 1000}`}
              options={[
                { value: '24/1000', label: t('L24 · 1 ms (AES67)') },
                { value: '24/125', label: t('L24 · 125 µs') },
                { value: '16/1000', label: t('L16 · 1 ms') },
              ]}
              onChange={(v) => {
                const [b, p] = v.split('/').map(Number);
                setF({ ...f, bits: b, ptimeUs: p });
              }}
            />
          </Field>
        </FieldGrid>
      )}
      {redundant ? (
        <FieldGrid two>
          <Field label={t('Red address (port 1)')}>
            <input className="st-mono" value={f.red} placeholder="239.10.2.1:5004" onChange={(e) => set('red', e.target.value)} />
          </Field>
          <Field label={t('Blue address (port 2)')}>
            <input className="st-mono" value={f.blue ?? ''} placeholder="239.20.2.1:5004" onChange={(e) => set('blue', e.target.value)} />
          </Field>
        </FieldGrid>
      ) : (
        <FieldGrid two>
          <Field label={t('Multicast address')}>
            <input className="st-mono" value={f.red} placeholder="239.10.2.1:5004" onChange={(e) => set('red', e.target.value)} />
          </Field>
          <Field label={t('Media port')} group>
            <Segmented<'1' | '2'> value={String(f.port ?? 1) as '1' | '2'} onChange={(v) => set('port', Number(v))} options={[{ value: '1', label: t('Port 1') }, { value: '2', label: t('Port 2') }]} />
          </Field>
        </FieldGrid>
      )}
    </Modal>
  );
}

export function St2110Page() {
  const hw = useHardware();
  const [editing, setEditing] = useState<{ flow: Flow; index: number } | null>(null);
  // Le Core tient les flux (MTL) : ce qui passe vraiment, flux par flux.
  const io = usePoll<{ flows: { id: string; io?: { state: string; format?: string | null; lost: number; error?: string | null } | null }[]; io?: { active: boolean } }>('/api/hardware/flows');
  const ioOf = (id: string) => (io?.io?.active ? io.flows.find((x) => x.id === id)?.io : undefined);
  const nmos = usePoll<NmosStatus>('/api/hardware/nmos');
  if (!hw) return null;
  const [snap, draft] = hw;
  const redundant = draft.network.mediaMode !== 'separate';
  const n = draft.network;
  const blank: Flow = { id: '', name: '', direction: 'input', essence: 'video', red: '', blue: '', port: 1 };
  return (
    <>
      <PageHead title="ST 2110" sub={`${tn(draft.flows.length, '{n} flow', '{n} flows')} · ${redundant ? t('SMPTE 2022-7 (red + blue)') : t('Two separate ports')}`}>
        <button type="button" className="st-btn primary" onClick={() => setEditing({ flow: blank, index: -1 })}>
          <Plus size={14} /> {t('New flow')}
        </button>
      </PageHead>
      <p className="st-muted core-note">
        {redundant
          ? t('Port 1 (red): {a} · port 2 (blue): {b} — set in Network.', { a: n.media1 || '—', b: n.media2 || '—' })
          : t('Port 1: {a} · port 2: {b} — set in Network.', { a: n.media1 || '—', b: n.media2 || '—' })}
      </p>
      {draft.flows.length === 0 ? (
        <div className="core-empty">
          <Empty icon={Waves} text={t('No ST 2110 flow')} />
        </div>
      ) : (
        <table className="st-table">
          <thead>
            <tr>
              <th>{t('Flow')}</th>
              <th>{t('Direction')}</th>
              <th>{t('Essence')}</th>
              {redundant ? (
                <>
                  <th>{t('Red')}</th>
                  <th>{t('Blue')}</th>
                </>
              ) : (
                <>
                  <th>{t('Multicast')}</th>
                  <th>{t('Port')}</th>
                </>
              )}
              {io?.io?.active && <th>{t('In the Core')}</th>}
              <th className="r" />
            </tr>
          </thead>
          <tbody>
            {draft.flows.map((f, i) => (
              <tr key={f.id || i}>
                <td>
                  <strong>{f.name}</strong>
                  {f.enabled === false && (
                    <>
                      {' '}
                      <Badge>{t('Off')}</Badge>
                    </>
                  )}
                  {f.direction === 'input' && nmos?.links[f.id]?.peer && (
                    <>
                      {' '}
                      <span title={t('Connected by the NMOS controller to sender {id}', { id: nmos.links[f.id].peer ?? '' })}>
                        <Badge tone="accent">NMOS</Badge>
                      </span>
                    </>
                  )}
                </td>
                <td>{f.direction === 'input' ? t('Received') : t('Sent')}</td>
                <td>{t(ESSENCE[f.essence])}</td>
                {redundant ? (
                  <>
                    <td className="st-mono">{f.red}</td>
                    <td className="st-mono">{f.blue || '—'}</td>
                  </>
                ) : (
                  <>
                    <td className="st-mono">{f.red}</td>
                    <td>{f.port ?? 1}</td>
                  </>
                )}
                {io?.io?.active && (
                  <td>
                    {(() => {
                      const x = ioOf(f.id);
                      if (f.essence === 'data' || f.enabled === false) return <span className="st-muted">—</span>;
                      if (!x) return <Badge>{t('Opening…')}</Badge>;
                      if (x.state === 'error') return <span title={x.error ? friendly(x.error) : undefined}><Badge tone="bad">{t('Error')}</Badge></span>;
                      if (x.state === 'running')
                        return (
                          <span title={x.format ?? undefined}>
                            <Badge tone="ok">{f.direction === 'input' ? t('Receiving') : t('Sending')}</Badge>
                            {f.direction === 'input' && x.lost > 0 ? <span className="st-muted"> {tn(x.lost, '{n} packet lost', '{n} packets lost')}</span> : null}
                          </span>
                        );
                      return <Badge tone="warn">{f.direction === 'input' ? t('Nothing received') : f.essence === 'audio' ? t('silence on the output') : t('black on the output')}</Badge>;
                    })()}
                  </td>
                )}
                <td className="r">
                  <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditing({ flow: f, index: i })}>
                    <Pencil size={14} />
                  </button>
                  <button
                    type="button"
                    className="st-icon-btn ghost"
                    title={t('Delete')}
                    onClick={async () => {
                      if (await confirmDialog({ title: t('Delete {name}?', { name: f.name }), confirm: t('Delete'), danger: true })) editHw((x) => void x.flows.splice(i, 1));
                    }}
                  >
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <FlowsCard flows={snap.config.flows} />
      {editing && (
        <FlowEditor
          flow={editing.flow}
          redundant={redundant}
          onClose={() => setEditing(null)}
          onSave={(f) => {
            editHw((x) => {
              if (editing.index < 0) x.flows.push(f);
              else x.flows[editing.index] = f;
            });
            setEditing(null);
          }}
        />
      )}
      <HwSaveBar />
    </>
  );
}

// ---------- NDI ----------

export function NdiPage() {
  const hw = useHardware();
  const [sources, setSources] = useState<string[] | null>(null);
  const [looking, setLooking] = useState(false);
  if (!hw) return null;
  const [snap, draft] = hw;
  const look = async () => {
    setLooking(true);
    try {
      setSources((await api<{ sources: string[] }>('GET', '/api/hardware/ndi/sources')).sources);
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setLooking(false);
  };
  return (
    <>
      <PageHead title="NDI" sub={snap.ndi.path ? `${t('NDI library')}${snap.ndi.version ? ` · ${snap.ndi.version}` : ''}` : t('NDI library not installed')} />
      <Card icon={<Radio size={16} />} title={t('NDI library')}>
        {snap.ndi.path ? (
          <div className="core-service">
            <Badge tone="ok">{t('Installed')}</Badge>
            <span className="st-mono st-muted">{snap.ndi.path}</span>
          </div>
        ) : (
          <p className="st-muted core-note">{t('The NDI library (libndi) is not installed: the installer sets it up, after its licence is accepted.')}</p>
        )}
      </Card>
      <Card icon={<NetIcon size={16} />} title={t('NDI network')}>
        <FieldGrid>
          <Field label={t('Interface')} hint={t('Also set in Network.')} group>
            <Dropdown value={draft.network.ndi} options={nicOptions(snap.nics)} onChange={(v) => editHw((x) => void (x.network.ndi = v))} />
          </Field>
          <Field label={t('Discovery server')} hint={t('Empty: the sources announce themselves (mDNS).')}>
            <input className="st-mono" value={draft.ndi.discovery} placeholder="10.0.0.2" onChange={(e) => editHw((x) => void (x.ndi.discovery = e.target.value))} />
          </Field>
          <Field label={t('Machine name')} hint={t('Receivers see “NAME (source)”.')}>
            <input value={draft.ndi.machine} placeholder={t('The computer’s')} onChange={(e) => editHw((x) => void (x.ndi.machine = e.target.value))} />
          </Field>
        </FieldGrid>
        <p className="st-muted core-note">{t('These settings are the same for every app; an app reads them when it starts.')}</p>
      </Card>
      <Card icon={<Search size={16} />} title={t('Visible sources')}>
        <div className="st-row">
          <button type="button" className="st-btn" disabled={looking || !snap.ndi.path} onClick={() => void look()}>
            <Search size={14} /> {looking ? t('Looking…') : t('Look for sources')}
          </button>
        </div>
        {sources &&
          (sources.length ? (
            <div className="core-lines">
              {sources.map((s) => (
                <div key={s} className="core-line">
                  <strong>{s}</strong>
                </div>
              ))}
            </div>
          ) : (
            <span className="st-muted">{t('No source seen')}</span>
          ))}
      </Card>
      <HwSaveBar />
    </>
  );
}

export { ClockPage } from './Clock';
