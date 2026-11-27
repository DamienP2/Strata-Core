// Réglages › Matériel : Cartes SDI, NDI, ST 2110, Réseau, Horloge. Les réglages sont partagés par
// les cinq pages (store.ts) et enregistrés d'un coup.
import { useEffect, useState } from 'react';
import { Clock, MonitorPlay, Network as NetIcon, Pencil, Plus, Radio, RefreshCw, Search, Trash2, Waves } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, Field, FieldGrid, Modal, NumberField, PageHead, SaveBar, Segmented, Switch, confirmDialog, formatDateTime, t, tn, toast, type DropOption } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { DeckLink3D, Pcie3D, SDI_COLOR, nicLook, printedOf, type Jack } from './Card3D';
import { cardOf, discardHw, editCard, editHw, loadHw, saveHw, useHw, useHwDirty, type DetectedCard, type Flow, type Fn, type HwConfig, type Nic, type Port, type Snapshot } from './store';

/** Charge le matériel à la première page qui en a besoin ; rend l'instantané et les réglages. */
function useHardware(): [Snapshot, HwConfig] | null {
  const snap = useHw((s) => s.snap);
  const draft = useHw((s) => s.draft);
  useEffect(() => {
    if (!useHw.getState().snap) void loadHw();
  }, []);
  return snap && draft ? [snap, draft] : null;
}

function HwSaveBar() {
  const dirty = useHwDirty();
  const saving = useHw((s) => s.saving);
  return <SaveBar dirty={dirty} saving={saving} onSave={() => void saveHw()} onDiscard={discardHw} />;
}

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
                </span>
                <Segmented<Fn> value={k.function} options={fnOptions(d.ports[i] ?? 'both')} onChange={(f) => editCard(d, (x) => void (x.connectors[i].function = f))} />
                <input className="core-hw-wire" value={k.name} placeholder={t('Wired to…')} onChange={(e) => editCard(d, (x) => void (x.connectors[i].name = e.target.value))} />
              </div>
            );
          })}
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

export function NetworkPage() {
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
      </Card>
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
      <FieldGrid two>
        <Field label={t('Direction')} group>
          <Segmented<Flow['direction']> value={f.direction} onChange={(v) => set('direction', v)} options={[{ value: 'input', label: t('Received') }, { value: 'output', label: t('Sent') }]} />
        </Field>
        <Field label={t('Essence')} group>
          <Segmented<Flow['essence']> value={f.essence} onChange={(v) => set('essence', v)} options={(['video', 'audio', 'data'] as const).map((e) => ({ value: e, label: t(ESSENCE[e]) }))} />
        </Field>
      </FieldGrid>
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
  if (!hw) return null;
  const [, draft] = hw;
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
              <th className="r" />
            </tr>
          </thead>
          <tbody>
            {draft.flows.map((f, i) => (
              <tr key={f.id || i}>
                <td>
                  <strong>{f.name}</strong>
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

// ---------- Horloge ----------

interface ClockStatus {
  ntp: { tool?: string | null; synced?: boolean | null; enabled?: boolean | null; source?: string | null; offsetMs?: number | null };
  ptp: { running: boolean; message?: string | null; domain: number; portState?: string | null; grandmaster?: string | null; offsetNs?: number | null; jitterNs?: number | null; locked: boolean; setsClock: boolean };
  checkedAt: number;
}

export function ClockPage() {
  const hw = useHardware();
  const [st, setSt] = useState<ClockStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const read = async () => {
    setBusy(true);
    try {
      setSt(await api<ClockStatus>('GET', '/api/hardware/clock'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
    setBusy(false);
  };
  useEffect(() => {
    void read();
    const id = setInterval(() => void read(), 10000);
    return () => clearInterval(id);
  }, []);
  if (!hw) return null;
  const [, draft] = hw;
  const ptp = st?.ptp;
  const ntp = st?.ntp;
  const ptpBadge = !ptp ? null : ptp.locked ? <Badge tone="ok">{t('Locked')}</Badge> : ptp.running ? <Badge tone="warn">{t('Not locked')}</Badge> : <Badge>{t('Not running')}</Badge>;
  const line = (label: string, value: React.ReactNode) => (
    <div className="core-line">
      <span>{label}</span>
      <span className="st-muted st-mono">{value ?? '—'}</span>
    </div>
  );
  return (
    <>
      <PageHead title={t('Clock')} sub={ptp?.locked ? t('PTP locked') : ntp?.synced ? t('NTP synchronised') : t('Not synchronised')}>
        <button type="button" className="st-btn" disabled={busy} onClick={() => void read()}>
          <RefreshCw size={14} /> {t('Read again')}
        </button>
      </PageHead>
      <div className="core-cols">
        <Card icon={<Clock size={16} />} title="PTP" actions={ptpBadge}>
          {ptp && !ptp.running && <p className="st-muted core-note">{t('ptp4l does not answer (is linuxptp installed and running?)')}</p>}
          <div className="core-lines">
            {line(t('Domain'), ptp?.domain)}
            {line(t('Port state'), ptp?.portState)}
            {line(t('Grandmaster'), ptp?.grandmaster)}
            {line(t('Offset'), ptp?.offsetNs != null ? `${Math.round(ptp.offsetNs)} ns` : null)}
            {line(t('Jitter'), ptp?.jitterNs != null ? `± ${Math.round(ptp.jitterNs)} ns` : null)}
            {line(t('Sets the system clock (phc2sys)'), ptp ? (ptp.setsClock ? t('Yes') : t('No')) : null)}
            {line(t('Interface'), draft.network.ptp || null)}
          </div>
        </Card>
        <Card icon={<Clock size={16} />} title="NTP" actions={ntp?.synced == null ? null : ntp.synced ? <Badge tone="ok">{t('Synchronised')}</Badge> : <Badge tone="warn">{t('Not synchronised')}</Badge>}>
          <div className="core-lines">
            {line(t('Server'), ntp?.source)}
            {line(t('Offset'), ntp?.offsetMs != null ? `${ntp.offsetMs.toFixed(3)} ms` : null)}
            {line(t('Read with'), ntp?.tool)}
            {line(t('Read at'), st ? formatDateTime(st.checkedAt) : null)}
          </div>
        </Card>
      </div>
      <Card icon={<Clock size={16} />} title={t('Settings')}>
        <FieldGrid two>
          <Field label={t('PTP domain')} group>
            <NumberField value={draft.ptp.domain} min={0} max={127} onChange={(v) => editHw((x) => void (x.ptp.domain = Math.round(v)))} />
          </Field>
          <Field label={t('TAI − UTC (seconds)')} group>
            <NumberField value={draft.ptp.taiOffset} min={0} max={100} onChange={(v) => editHw((x) => void (x.ptp.taiOffset = Math.round(v)))} />
          </Field>
        </FieldGrid>
        <p className="st-muted core-note">{t('SMPTE ST 2059-2: domain 127. TAI − UTC is 37 s since 2017.')}</p>
      </Card>
      <HwSaveBar />
    </>
  );
}
