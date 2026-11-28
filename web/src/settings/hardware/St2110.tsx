// Réglages › Matériel › ST 2110 : les flux reçus (entrées) et envoyés (sorties), chacun avec sa
// vidéo (2110-20), ses audios (2110-30) et ses données ANC (2110-40) ; les réglages des ports
// (SMPTE 2022-7) et de NMOS. Et la page Dante (flux AES67 annoncés en SAP).
//
// Dans les réglages, une essence est un `Flow` ; celles d'un même flux partagent son `group`.
import { useEffect, useState } from 'react';
import { Copy, Download, FileText, Pencil, Plus, Radio, Trash2, Waves, X } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, Field, FieldGrid, Modal, PageHead, Segmented, SubTabs, Switch, confirmDialog, t, timeAgo, tn, toast } from '@strata/ui';
import { ApiError } from '../../api';
import { friendly } from '../../errors';
import { usePoll } from '../../poll';
import { HwSaveBar, useHardware } from './common';
import { Preview } from './Inputs';
import { MediaPortsCard, NmosCard, PortsLoad, nicOptions } from './Hardware';
import { editHw, type Capacity, type Flow, type HwConfig, type NmosStatus } from './store';

type Dir = 'input' | 'output';

interface LegIo {
  packets: number;
  lost: number;
}

interface FlowIo {
  state: 'running' | 'idle' | 'error' | string;
  format?: string | null;
  frames: number;
  packets: number;
  lost: number;
  unrecovered: number;
  legs: LegIo[];
  error?: string | null;
}

interface FlowsState {
  flows: { id: string; io?: FlowIo | null }[];
  io?: { active: boolean; problem?: string | null };
}

interface ClockState {
  ptp: { running: boolean; locked: boolean; domain: number; grandmaster?: string | null; offsetNs?: number | null; message?: string | null };
}

const FORMATS = ['1080i50', '1080i59.94', '1080p25', '1080p50', '1080p59.94', '720p50', '720p59.94', '2160p25', '2160p50', '2160p59.94'];
const ORDER: Record<Flow['essence'], number> = { video: 0, audio: 1, data: 2 };

/** Le flux d'une essence. */
const groupOf = (f: Flow) => f.group || f.id;

/** Les flux d'un sens, dans l'ordre des réglages ; leurs essences : vidéo, audios, ANC. */
function groupsOf(flows: Flow[], dir: Dir): { id: string; flows: Flow[] }[] {
  const out: { id: string; flows: Flow[] }[] = [];
  for (const f of flows.filter((x) => x.direction === dir)) {
    const g = groupOf(f);
    const hit = out.find((x) => x.id === g);
    if (hit) hit.flows.push(f);
    else out.push({ id: g, flows: [f] });
  }
  for (const g of out) g.flows.sort((a, b) => ORDER[a.essence] - ORDER[b.essence]);
  return out;
}

/** « 239.10.1.1:5004 » → [ip, port]. */
function split(addr?: string | null): [string, string] {
  if (!addr) return ['', ''];
  const i = addr.lastIndexOf(':');
  return i < 0 ? [addr, ''] : [addr.slice(0, i), addr.slice(i + 1)];
}

const unset = (f: Flow) => !f.red || f.red.startsWith('0.0.0.0');

function essenceName(f: Flow, all: Flow[]): string {
  if (f.essence === 'video') return t('Video');
  if (f.essence === 'data') return 'ANC';
  const audios = all.filter((x) => x.essence === 'audio');
  return audios.length > 1 ? t('Audio {n}', { n: audios.indexOf(f) + 1 }) : t('Audio');
}

function formatOf(f: Flow, house: string): string {
  if (f.essence === 'audio') return t('{n} ch · L{b} · {p} ms', { n: f.channels ?? 2, b: f.bits ?? 24, p: (f.ptimeUs ?? 1000) / 1000 });
  return f.format || house;
}

// ---------- La page ----------

export function St2110Page() {
  const hw = useHardware();
  const [tab, setTab] = useState<'input' | 'output' | 'settings'>('input');
  const [editing, setEditing] = useState<{ dir: Dir; group: string | null; add?: Flow['essence'] } | null>(null);
  const [sdpOf, setSdpOf] = useState<Flow[] | null>(null);
  const io = usePoll<FlowsState>('/api/hardware/flows');
  const nmos = usePoll<NmosStatus>('/api/hardware/nmos', 5000);
  const clock = usePoll<ClockState>('/api/hardware/clock', 5000);
  const cap = usePoll<Capacity>('/api/hardware/ports', tab === 'settings' ? 1000 : 5000);
  if (!hw) return null;
  const [snap, draft] = hw;
  const redundant = draft.network.mediaMode !== 'separate';
  const ins = groupsOf(draft.flows, 'input');
  const outs = groupsOf(draft.flows, 'output');
  const ioOf = (id: string) => (io?.io?.active ? io.flows.find((x) => x.id === id)?.io ?? null : null);
  const house = cap?.house ?? '';
  const remove = async (group: string, name: string) => {
    if (!(await confirmDialog({ title: t('Delete the flow {name}?', { name }), message: t('Its video, audios and ANC are removed.'), confirm: t('Delete'), danger: true }))) return;
    editHw((x) => void (x.flows = x.flows.filter((f) => groupOf(f) !== group)));
  };
  return (
    <>
      <PageHead title="ST 2110" />
      <Header cap={cap} clock={clock} redundant={redundant} flows={draft.flows} ioOf={ioOf} />
      {io?.io?.problem && <p className="core-warn">{friendly(io.io.problem)}</p>}
      <SubTabs<'input' | 'output' | 'settings'>
        items={[
          { id: 'input', label: t('Inputs · receivers'), count: ins.length },
          { id: 'output', label: t('Outputs · senders'), count: outs.length },
          { id: 'settings', label: t('Settings') },
        ]}
        value={tab}
        onChange={setTab}
      >
        {tab !== 'settings' && (
          <button type="button" className="st-btn primary" onClick={() => setEditing({ dir: tab, group: null })}>
            <Plus size={14} /> {tab === 'input' ? t('New received flow') : t('New sent flow')}
          </button>
        )}
      </SubTabs>
      {tab === 'settings' && (
        <>
          <MediaPortsCard draft={draft} />
          <PortsLoad cap={cap} />
          <NmosCard draft={draft} opts={nicOptions(snap.nics)} status={nmos} />
        </>
      )}
      {tab !== 'settings' &&
        ((tab === 'input' ? ins : outs).length === 0 ? (
          <div className="core-empty">
            <Empty icon={Waves} text={tab === 'input' ? t('No received flow') : t('No sent flow')} />
          </div>
        ) : (
          (tab === 'input' ? ins : outs).map((g) => (
            <GroupCard
              key={g.id}
              dir={tab}
              flows={g.flows}
              redundant={redundant}
              house={house}
              ioOf={ioOf}
              nmosOn={draft.nmos?.enabled ?? false}
              onEdit={(add) => setEditing({ dir: tab, group: g.id, add })}
              onDelete={() => void remove(g.id, g.flows[0].name)}
              onSdp={() => setSdpOf(g.flows)}
            />
          ))
        ))}
      {editing && (
        <FlowEditor
          dir={editing.dir}
          draft={draft}
          group={editing.group}
          add={editing.add}
          redundant={redundant}
          onClose={() => setEditing(null)}
        />
      )}
      {sdpOf && <SdpModal flows={sdpOf} onClose={() => setSdpOf(null)} />}
      <HwSaveBar />
    </>
  );
}

/** Sous le titre : les flux conseillés, l'horloge PTP, la redondance 2022-7. */
function Header({ cap, clock, redundant, flows, ioOf }: { cap: Capacity | null; clock: ClockState | null; redundant: boolean; flows: Flow[]; ioOf: (id: string) => FlowIo | null }) {
  const room = (name?: string | null) => cap?.formats.find((f) => f.name === name)?.total;
  const house = cap?.house;
  const uhd = house?.startsWith('1080') ? house.replace('1080', '2160') : null;
  const ptp = clock?.ptp;
  // 2022-7 : chaque flux reçu en marche a-t-il ses deux pattes ?
  const running = flows.filter((f) => f.direction === 'input' && f.blue && ioOf(f.id)?.state === 'running');
  const missing = running.filter((f) => (ioOf(f.id)?.legs ?? []).some((l) => l.packets === 0));
  return (
    <div className="core-2110-status">
      {room(house) != null && (
        <Badge title={t('Per direction, on the media ports (bit rate under the safety margin)')}>
          {uhd && room(uhd) != null ? t('Advised: {n} flows {f} at once · {m} in {u}', { n: room(house)!, f: house!, m: room(uhd)!, u: uhd }) : t('Advised: {n} flows {f} at once', { n: room(house)!, f: house! })}
        </Badge>
      )}
      {ptp &&
        (ptp.locked ? (
          <Badge tone="ok">
            {[t('PTP locked'), t('domain {n}', { n: ptp.domain }), ptp.grandmaster ? `GM ${ptp.grandmaster}` : null, ptp.offsetNs != null ? `±${Math.abs(ptp.offsetNs)} ns` : null].filter(Boolean).join(' · ')}
          </Badge>
        ) : (
          <Badge tone={ptp.running ? 'warn' : 'bad'}>{ptp.running ? t('PTP not locked · domain {n}', { n: ptp.domain }) : t('PTP stopped')}</Badge>
        ))}
      {redundant ? (
        <Badge tone={missing.length ? 'warn' : 'ok'}>{missing.length ? tn(missing.length, '2022-7: {n} flow on one network only', '2022-7: {n} flows on one network only') : t('2022-7: red and blue OK')}</Badge>
      ) : (
        <Badge>{t('Two separate media ports')}</Badge>
      )}
    </div>
  );
}

// ---------- Un flux ----------

function GroupCard({
  dir,
  flows,
  redundant,
  house,
  ioOf,
  nmosOn,
  onEdit,
  onDelete,
  onSdp,
}: {
  dir: Dir;
  flows: Flow[];
  redundant: boolean;
  house: string;
  ioOf: (id: string) => FlowIo | null;
  nmosOn: boolean;
  onEdit: (add?: Flow['essence']) => void;
  onDelete: () => void;
  onSdp: () => void;
}) {
  const head = flows[0];
  const input = dir === 'input';
  const waiting = input && flows.every(unset);
  const on = flows.some((f) => f.enabled !== false && !(input && unset(f)));
  const states = flows.map((f) => ioOf(f.id));
  const running = states.filter((s) => s?.state === 'running').length;
  const lostForGood = states.some((s) => (s?.unrecovered ?? 0) > 0);
  const oneLeg = redundant && flows.some((f, i) => f.blue && states[i]?.state === 'running' && (states[i]?.legs ?? []).some((l) => l.packets === 0));
  const errors = states.filter((s) => s?.state === 'error');
  const badge = waiting ? (
    <Badge tone="warn">{t('Not connected')}</Badge>
  ) : !on ? (
    <Badge>{t('Off')}</Badge>
  ) : errors.length ? (
    <Badge tone="bad">{t('Error')}</Badge>
  ) : running === 0 ? (
    <Badge tone="warn">{input ? t('Nothing received') : t('Black and silence')}</Badge>
  ) : lostForGood ? (
    <Badge tone="bad">{t('Packets lost')}</Badge>
  ) : oneLeg ? (
    <Badge tone="warn">{t('One network missing · picture intact')}</Badge>
  ) : (
    <Badge tone="ok">{input ? (redundant ? t('Received · 2022-7 complete') : t('Received')) : t('Sent')}</Badge>
  );
  const by = flows.find((f) => f.setBy)?.setBy;
  const at = flows.find((f) => f.setAt)?.setAt;
  const has = (e: Flow['essence']) => flows.some((f) => f.essence === e);
  return (
    <section className="core-flux">
      <div className="core-flux-head">
        <h2>{head.name}</h2>
        {input ? (
          waiting ? (
            <span className="st-muted">{nmosOn ? t('to connect: by an NMOS controller or here') : t('to connect')}</span>
          ) : by ? (
            <Badge tone={by === 'nmos' ? 'accent' : undefined}>
              {by === 'nmos' ? t('connected by NMOS') : t('set by hand · {who}', { who: by })}
              {at ? ` · ${timeAgo(at)}` : ''}
            </Badge>
          ) : null
        ) : (
          <>
            <span className="st-muted st-mono" title={t('The MXL flow an app writes to')}>
              ← 2110/{flows.find((f) => f.essence === 'video')?.id ?? head.id}
            </span>
            {nmosOn && <Badge>{head.nmosAddresses ? t('NMOS · addresses by the controller') : 'NMOS'}</Badge>}
          </>
        )}
        <span className="core-grow" />
        {badge}
        {waiting ? (
          <button type="button" className="st-btn small" onClick={() => onEdit()}>
            {t('Connect…')}
          </button>
        ) : (
          !input && (
            <button type="button" className="st-btn small" onClick={onSdp}>
              <FileText size={13} /> {t('SDP')}
            </button>
          )
        )}
        <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => onEdit()}>
          <Pencil size={14} />
        </button>
        <button type="button" className="st-icon-btn ghost" title={t('Delete')} onClick={onDelete}>
          <Trash2 size={14} />
        </button>
      </div>
      <div className="core-flux-scroll">
        <div className="core-flux-rows">
          <div className="core-flux-row head">
            <span>{t('Essence')}</span>
            <span>{t('Format')}</span>
            <span>{t('Networks: IP · port · state')}</span>
            <span />
          </div>
          {flows.map((f, i) => (
            <EssenceRow key={f.id} f={f} all={flows} io={states[i]} input={input} redundant={redundant} house={house} />
          ))}
        </div>
      </div>
      <div className="core-flux-foot">
        <span className="st-muted">{[!has('video') && t('no video'), !has('audio') && t('no audio'), !has('data') && t('no ANC')].filter(Boolean).join(' · ')}</span>
        <span className="core-grow" />
        {!has('video') && (
          <button type="button" className="st-btn small" onClick={() => onEdit('video')}>
            <Plus size={13} /> {t('Video')}
          </button>
        )}
        <button type="button" className="st-btn small" onClick={() => onEdit('audio')}>
          <Plus size={13} /> {t('Audio')}
        </button>
        <button type="button" className="st-btn small" onClick={() => onEdit('data')}>
          <Plus size={13} /> ANC
        </button>
      </div>
    </section>
  );
}

function EssenceRow({ f, all, io, input, redundant, house }: { f: Flow; all: Flow[]; io: FlowIo | null; input: boolean; redundant: boolean; house: string }) {
  const off = f.enabled === false || (input && unset(f));
  const legs: { color: 'r' | 'b'; addr?: string | null }[] = redundant ? [{ color: 'r', addr: f.red }, { color: 'b', addr: f.blue }] : [{ color: f.port === 2 ? 'b' : 'r', addr: f.red }];
  const legState = (i: number): { text: string; tone: string } => {
    if (off) return { text: input && unset(f) ? t('not connected') : t('off'), tone: 'muted' };
    if (!io) return { text: t('opening…'), tone: 'muted' };
    if (io.state === 'error') return { text: t('error'), tone: 'bad' };
    if (!input) return io.state === 'running' ? { text: t('sent'), tone: 'ok' } : { text: f.essence === 'audio' ? t('silence sent') : f.essence === 'data' ? t('nothing to send') : t('black sent'), tone: 'warn' };
    const l = io.legs[i];
    if (!l || l.packets === 0) return { text: t('nothing received'), tone: io.state === 'running' ? 'warn' : 'bad' };
    return { text: l.lost ? tn(l.lost, 'received · {n} loss', 'received · {n} losses') : t('received'), tone: l.lost ? 'warn' : 'ok' };
  };
  return (
    <div className={`core-flux-row ${off ? 'off' : ''}`}>
      <span className="core-flux-kind">
        <b>{essenceName(f, all)}</b>
        <span className="st-muted">
          {f.essence === 'video' ? '2110-20' : f.essence === 'audio' ? '2110-30' : '2110-40'}
          {f.label ? ` · ${f.label}` : ''}
        </span>
      </span>
      <span className="st-mono">
        {io?.format && f.essence !== 'audio' ? io.format : formatOf(f, house)}
        {!input && f.essence === 'video' && <span className="st-muted"> · {f.pacing || 'N'}</span>}
      </span>
      <span className="core-flux-legs">
        {legs.map((l, i) => {
          const [ip, port] = split(l.addr);
          const st = legState(i);
          return (
            <span key={l.color} className="core-flux-leg">
              <span className={`core-flux-dot ${l.color}`} aria-label={l.color === 'r' ? t('Red network') : t('Blue network')}>
                ●
              </span>
              <span className="st-mono">{ip && !ip.startsWith('0.0.0.0') ? ip : '—'}</span>
              <span className="st-mono core-flux-port">{ip && !ip.startsWith('0.0.0.0') ? port : ''}</span>
              <span className={`core-flux-st ${st.tone}`}>{st.text}</span>
            </span>
          );
        })}
      </span>
      <span>
        {f.essence === 'video' && input && !unset(f) && (
          <div className="core-flux-pic">
            <Preview src={`/api/hardware/flows/${encodeURIComponent(f.id)}/preview.jpg`} on={io ? io.state === 'running' : true} empty="—" />
          </div>
        )}
      </span>
    </div>
  );
}

// ---------- Le SDP des flux envoyés ----------

function SdpModal({ flows, onClose }: { flows: Flow[]; onClose: () => void }) {
  const [texts, setTexts] = useState<Record<string, string | null>>({});
  useEffect(() => {
    let gone = false;
    for (const f of flows) {
      fetch(`/api/hardware/flows/${encodeURIComponent(f.id)}/sdp`, { credentials: 'same-origin' })
        .then((r) => (r.ok ? r.text() : null))
        .catch(() => null)
        .then((x) => !gone && setTexts((m) => ({ ...m, [f.id]: x })));
    }
    return () => {
      gone = true;
    };
  }, [flows]);
  return (
    <Modal wide title={t('SDP of {name}', { name: flows[0].name })} icon={<FileText size={16} />} onClose={onClose}>
      {flows.map((f) => {
        const text = texts[f.id];
        return (
          <div key={f.id} className="core-sdp">
            <div className="core-sdp-head">
              <b>{essenceName(f, flows)}</b>
              <span className="core-grow" />
              {text && (
                <>
                  <button
                    type="button"
                    className="st-btn small"
                    onClick={() => {
                      void navigator.clipboard?.writeText(text);
                      toast(t('Copied'));
                    }}
                  >
                    <Copy size={13} /> {t('Copy')}
                  </button>
                  <a className="st-btn small" href={`/api/hardware/flows/${encodeURIComponent(f.id)}/sdp`} download={`${f.id}.sdp`}>
                    <Download size={13} /> {t('Download')}
                  </a>
                </>
              )}
            </div>
            {text === undefined ? <p className="st-muted">…</p> : text === null ? <p className="st-muted">{t('Save the settings first: the SDP is that of the saved flow.')}</p> : <pre className="core-sdp-text">{text}</pre>}
          </div>
        );
      })}
    </Modal>
  );
}

// ---------- Créer ou modifier un flux ----------

interface Leg {
  ip: string;
  port: string;
}

interface Ess {
  id: string;
  essence: Flow['essence'];
  label: string;
  red: Leg;
  blue: Leg;
  channels: number;
  bits: number;
  ptimeUs: number;
  payloadType: number;
  source: string;
  sourceBlue: string;
  /** Ce que le Core avait (le reste des réglages, gardé tel quel). */
  base?: Flow;
}

const legOf = (addr?: string | null): Leg => {
  const [ip, port] = split(addr);
  return ip && !ip.startsWith('0.0.0.0') ? { ip, port } : { ip: '', port: '' };
};

function essOf(f: Flow): Ess {
  return {
    id: f.id,
    essence: f.essence,
    label: f.label ?? '',
    red: legOf(f.red),
    blue: legOf(f.blue),
    channels: f.channels ?? 2,
    bits: f.bits ?? 24,
    ptimeUs: f.ptimeUs ?? 1000,
    payloadType: f.payloadType ?? 0,
    source: f.source ?? '',
    sourceBlue: f.sourceBlue ?? '',
    base: f,
  };
}

const blankEss = (essence: Flow['essence']): Ess => ({
  id: '',
  essence,
  label: '',
  red: { ip: '', port: '5004' },
  blue: { ip: '', port: '5004' },
  channels: essence === 'audio' ? 2 : 0,
  bits: 24,
  ptimeUs: 1000,
  payloadType: 0,
  source: '',
  sourceBlue: '',
});

/** Un nom de flux en identifiant : « CAM 1 — Plateau » → « cam-1-plateau ». */
function slug(name: string): string {
  return (
    name
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 40) || 'flux'
  );
}

/** La prochaine adresse multicast libre après `ip` (dernier octet +1). */
function nextFree(ip: string, taken: Set<string>): string {
  const p = ip.split('.').map(Number);
  if (p.length !== 4 || p.some((x) => !(x >= 0 && x <= 255))) return ip;
  for (let k = 0; k < 4096; k++) {
    p[3] += 1;
    if (p[3] > 254) {
      p[3] = 1;
      p[2] = (p[2] + 1) % 256;
    }
    const s = p.join('.');
    if (!taken.has(s)) return s;
  }
  return ip;
}

function FlowEditor({ dir, draft, group, add, redundant, onClose }: { dir: Dir; draft: HwConfig; group: string | null; add?: Flow['essence']; redundant: boolean; onClose: () => void }) {
  const existing = group ? draft.flows.filter((f) => groupOf(f) === group).sort((a, b) => ORDER[a.essence] - ORDER[b.essence]) : [];
  const head = existing[0];
  const input = dir === 'input';
  const [name, setName] = useState(head?.name ?? '');
  const [enabled, setEnabled] = useState(head ? existing.some((f) => f.enabled !== false) || existing.every(unset) : true);
  const [format, setFormat] = useState(existing.find((f) => f.essence === 'video')?.format ?? head?.format ?? '');
  const [port, setPort] = useState(head?.port ?? 1);
  const [pacing, setPacing] = useState<NonNullable<Flow['pacing']>>(existing.find((f) => f.essence === 'video')?.pacing ?? '');
  const [nmosAddresses, setNmosAddresses] = useState(head?.nmosAddresses ?? false);
  const [list, setList] = useState<Ess[]>(() => {
    const l = head ? existing.map(essOf) : input ? [blankEss('video'), blankEss('audio')] : [blankEss('video'), blankEss('audio')];
    if (add) l.push(blankEss(add));
    return l;
  });
  const [sdp, setSdp] = useState<string | null>(null);
  const setEss = (i: number, p: Partial<Ess>) => setList(list.map((e, k) => (k === i ? { ...e, ...p } : e)));
  const addEss = (e: Flow['essence']) => setList([...list, blankEss(e)].sort((a, b) => ORDER[a.essence] - ORDER[b.essence]));
  const others = draft.flows.filter((f) => !group || groupOf(f) !== group);
  const fillNext = () => {
    const taken = new Set<string>();
    for (const f of draft.flows) for (const a of [f.red, f.blue]) if (a) taken.add(split(a)[0]);
    const lastOut = [...others].reverse().find((f) => f.direction === dir && !unset(f));
    let red = lastOut ? split(lastOut.red)[0] : input ? '239.10.0.0' : '239.30.0.0';
    let blue = lastOut?.blue ? split(lastOut.blue)[0] : input ? '239.20.0.0' : '239.40.0.0';
    setList(
      list.map((e) => {
        red = nextFree(red, taken);
        taken.add(red);
        const b = redundant ? nextFree(blue, taken) : '';
        if (b) {
          taken.add(b);
          blue = b;
        }
        return { ...e, red: { ip: red, port: e.red.port || '5004' }, blue: { ip: b, port: e.blue.port || '5004' } };
      }),
    );
  };
  const readSdp = async (text: string) => {
    try {
      const res = await fetch('/api/hardware/sdp', { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/sdp' }, body: text });
      if (!res.ok) throw new ApiError(res.status, await res.text());
      const r = (await res.json()) as { media: { essence: Flow['essence']; format?: string | null; channels?: number; bits?: number; ptimeUs?: number }; legs: { group: string; port: number; source?: string | null }[]; payloadType: number };
      const e: Ess = {
        ...blankEss(r.media.essence),
        red: { ip: r.legs[0]?.group ?? '', port: String(r.legs[0]?.port ?? 5004) },
        blue: { ip: r.legs[1]?.group ?? '', port: String(r.legs[1]?.port ?? 5004) },
        source: r.legs[0]?.source ?? '',
        sourceBlue: r.legs[1]?.source ?? '',
        payloadType: r.payloadType,
        channels: r.media.channels ?? 2,
        bits: r.media.bits ?? 24,
        ptimeUs: r.media.ptimeUs ?? 1000,
      };
      if (r.media.essence === 'video' && r.media.format) setFormat(r.media.format);
      // Remplace l'essence de même genre encore vide, sinon s'ajoute.
      const i = list.findIndex((x) => x.essence === r.media.essence && !x.red.ip);
      setList(i >= 0 ? list.map((x, k) => (k === i ? { ...e, id: x.id, base: x.base, label: x.label } : x)) : [...list, e].sort((a, b) => ORDER[a.essence] - ORDER[b.essence]));
      setSdp(null);
      toast(t('SDP read'));
    } catch (e) {
      toast(friendly(e), 'error');
    }
  };
  const videos = list.filter((e) => e.essence === 'video').length;
  const okLeg = (l: Leg) => /^\d+\.\d+\.\d+\.\d+$/.test(l.ip) && Number(l.port) > 0 && Number(l.port) < 65536;
  const complete = (e: Ess) => okLeg(e.red) && (!redundant || okLeg(e.blue));
  // Reçu : sans aucune adresse, le flux attend une connexion (NMOS) ; avec, toutes complètes.
  const anyAddr = list.some((e) => e.red.ip || e.blue.ip);
  const valid = name.trim() && list.length > 0 && videos <= 1 && (input && !anyAddr ? true : list.every(complete));
  const save = () => {
    const gid = (() => {
      if (group) return group;
      const taken = new Set(draft.flows.map(groupOf).concat(draft.flows.map((f) => f.id)));
      let g = slug(name);
      for (let n = 2; taken.has(g); n++) g = `${slug(name)}-${n}`;
      return g;
    })();
    const ids = new Set(others.map((f) => f.id));
    const idFor = (e: Ess) => {
      if (e.id) return e.id;
      const stem = e.essence === 'video' ? gid : `${gid}-${e.essence === 'audio' ? 'audio' : 'anc'}`;
      let id = stem;
      for (let n = 2; ids.has(id); n++) id = `${stem}${n}`;
      ids.add(id);
      return id;
    };
    for (const e of list) if (e.id) ids.add(e.id);
    const addr = (l: Leg) => (l.ip ? `${l.ip}:${l.port || '5004'}` : null);
    const flows: Flow[] = list.map((e) => ({
      ...(e.base ?? {}),
      id: idFor(e),
      group: gid,
      name: name.trim(),
      label: e.label.trim(),
      direction: dir,
      essence: e.essence,
      red: addr(e.red) ?? '0.0.0.0:0',
      blue: redundant ? addr(e.blue) : null,
      port: redundant ? 1 : port,
      format: e.essence === 'audio' ? '' : format,
      channels: e.channels || 2,
      bits: e.bits,
      ptimeUs: e.ptimeUs,
      payloadType: e.payloadType,
      enabled,
      source: input ? e.source.trim() || null : null,
      sourceBlue: input ? e.sourceBlue.trim() || null : null,
      pacing: !input && e.essence === 'video' ? pacing : '',
      nmosAddresses: !input && nmosAddresses,
    }));
    editHw((x) => {
      const at = x.flows.findIndex((f) => groupOf(f) === gid);
      const rest = x.flows.filter((f) => groupOf(f) !== gid);
      rest.splice(at < 0 ? rest.length : at, 0, ...flows);
      x.flows = rest;
    });
    onClose();
  };
  const formats = [{ value: '', label: t('House format') }, ...FORMATS.map((x) => ({ value: x, label: x }))];
  return (
    <Modal
      wide
      title={head ? t('Edit {name}', { name: head.name }) : input ? t('New received flow') : t('New sent flow')}
      icon={<Waves size={16} />}
      onClose={onClose}
      onSubmit={save}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={!valid}>
            {head ? t('OK') : t('Add the flow')}
          </button>
        </>
      }
    >
      <FieldGrid two>
        <Field label={t('Name')}>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder={input ? 'CAM 3 — Plateau' : 'PGM 2'} />
        </Field>
        <Field label={t('Video format')} group>
          <Dropdown value={format} options={formats} onChange={setFormat} />
        </Field>
      </FieldGrid>
      <FieldGrid two>
        <Switch checked={enabled} onChange={setEnabled} label={input ? t('Received by the Core') : t('Sent by the Core')} />
        {!redundant && (
          <Field label={t('Media port')} group>
            <Segmented<'1' | '2'> value={String(port) as '1' | '2'} onChange={(v) => setPort(Number(v))} options={[{ value: '1', label: t('Port 1') }, { value: '2', label: t('Port 2') }]} />
          </Field>
        )}
      </FieldGrid>
      <div className="core-ess-tools">
        {input && (
          <button type="button" className="st-btn small" onClick={() => setSdp('')}>
            <FileText size={13} /> {t('Paste an SDP…')}
          </button>
        )}
        {!input && (
          <button type="button" className="st-btn small" onClick={fillNext}>
            {t('Next free addresses')}
          </button>
        )}
      </div>
      {sdp !== null && (
        <div className="core-sdp-paste">
          <textarea rows={8} className="st-mono" value={sdp} onChange={(e) => setSdp(e.target.value)} placeholder="v=0…" aria-label={t('SDP')} />
          <div className="core-ess-tools">
            <button type="button" className="st-btn small primary" disabled={!sdp.trim()} onClick={() => void readSdp(sdp)}>
              {t('Read the SDP')}
            </button>
            <button type="button" className="st-btn small" onClick={() => setSdp(null)}>
              {t('Cancel')}
            </button>
          </div>
        </div>
      )}
      <div className="core-ess">
        <div className={`core-ess-row head ${redundant ? '' : 'one'}`}>
          <span>{t('Essence')}</span>
          <span>{redundant ? t('Red · IP · port') : t('IP · port')}</span>
          {redundant && <span>{t('Blue (2022-7) · IP · port')}</span>}
          <span />
        </div>
        {list.map((e, i) => (
          <div key={i} className={`core-ess-row ${redundant ? '' : 'one'}`}>
            <span className="core-flux-kind">
              <b>{e.essence === 'video' ? t('Video') : e.essence === 'data' ? 'ANC' : t('Audio')}</b>
              {e.essence === 'audio' ? (
                <span className="core-ess-audio">
                  <Dropdown value={String(e.channels)} options={['1', '2', '4', '6', '8', '16'].map((x) => ({ value: x, label: t('{n} ch', { n: x }) }))} onChange={(v) => setEss(i, { channels: Number(v) })} />
                  <Dropdown
                    value={`${e.bits}/${e.ptimeUs}`}
                    options={[
                      { value: '24/1000', label: 'L24 · 1 ms' },
                      { value: '24/125', label: 'L24 · 125 µs' },
                      { value: '16/1000', label: 'L16 · 1 ms' },
                    ]}
                    onChange={(v) => {
                      const [b, p] = v.split('/').map(Number);
                      setEss(i, { bits: b, ptimeUs: p });
                    }}
                  />
                </span>
              ) : (
                <span className="st-muted">{e.essence === 'video' ? '2110-20' : '2110-40'}</span>
              )}
              {e.essence !== 'video' && <input className="core-ess-label" value={e.label} onChange={(x) => setEss(i, { label: x.target.value })} placeholder={t('name (optional)')} aria-label={t('Name of this essence')} />}
            </span>
            <span className="core-ess-ipp">
              <input className="st-mono" value={e.red.ip} onChange={(x) => setEss(i, { red: { ...e.red, ip: x.target.value.trim() } })} placeholder={t('IP')} aria-label={t('IP')} />
              <input className="st-mono" value={e.red.port} onChange={(x) => setEss(i, { red: { ...e.red, port: x.target.value.replace(/\D/g, '') } })} placeholder="5004" aria-label={t('Port')} />
            </span>
            {redundant && (
              <span className="core-ess-ipp">
                <input className="st-mono" value={e.blue.ip} onChange={(x) => setEss(i, { blue: { ...e.blue, ip: x.target.value.trim() } })} placeholder={t('IP')} aria-label={t('IP')} />
                <input className="st-mono" value={e.blue.port} onChange={(x) => setEss(i, { blue: { ...e.blue, port: x.target.value.replace(/\D/g, '') } })} placeholder="5004" aria-label={t('Port')} />
              </span>
            )}
            <button type="button" className="st-icon-btn ghost" title={t('Remove')} onClick={() => setList(list.filter((_, k) => k !== i))}>
              <X size={14} />
            </button>
          </div>
        ))}
        <div className="core-ess-tools">
          {videos === 0 && (
            <button type="button" className="st-btn small" onClick={() => addEss('video')}>
              <Plus size={13} /> {t('Video')}
            </button>
          )}
          <button type="button" className="st-btn small" onClick={() => addEss('audio')}>
            <Plus size={13} /> {t('Audio')}
          </button>
          <button type="button" className="st-btn small" onClick={() => addEss('data')}>
            <Plus size={13} /> ANC
          </button>
        </div>
      </div>
      {!input && <Switch checked={nmosAddresses} onChange={setNmosAddresses} label={t('Let the NMOS controller choose the addresses (address plan of the orchestrator)')} />}
      <details className="core-adv">
        <summary>{t('Advanced')}</summary>
        <div className="core-adv-body">
          {!input && list.some((e) => e.essence === 'video') && (
            <Field label={t('Video pacing (ST 2110-21)')} group>
              <Segmented<'N' | 'NL' | 'W'> value={(pacing || 'N') as 'N' | 'NL' | 'W'} onChange={(v) => setPacing(v)} options={[{ value: 'N', label: 'N' }, { value: 'NL', label: 'NL' }, { value: 'W', label: 'W' }]} />
            </Field>
          )}
          {list.map((e, i) => (
            <FieldGrid key={i} two={!input || !redundant}>
              <Field label={t('{what} · RTP payload type', { what: e.essence === 'video' ? t('Video') : e.essence === 'data' ? 'ANC' : t('Audio') })}>
                <input className="st-mono" inputMode="numeric" value={e.payloadType || ''} placeholder={e.essence === 'video' ? '96' : e.essence === 'audio' ? '97' : '100'} onChange={(x) => setEss(i, { payloadType: Math.min(127, Number(x.target.value.replace(/\D/g, '')) || 0) })} />
              </Field>
              {input && (
                <Field label={t('Source only (SSM), red')}>
                  <input className="st-mono" value={e.source} placeholder={t('any source')} onChange={(x) => setEss(i, { source: x.target.value.trim() })} />
                </Field>
              )}
              {input && redundant && (
                <Field label={t('Source only (SSM), blue')}>
                  <input className="st-mono" value={e.sourceBlue} placeholder={t('any source')} onChange={(x) => setEss(i, { sourceBlue: x.target.value.trim() })} />
                </Field>
              )}
            </FieldGrid>
          ))}
        </div>
      </details>
    </Modal>
  );
}

// ---------- Dante (AES67, SAP) ----------

interface SapHeard {
  name: string;
  origin: string;
  group: string;
  port: number;
  source?: string | null;
  channels?: number | null;
  bits?: number | null;
  ptimeUs?: number | null;
  clock?: string | null;
  seenSecs: number;
}

/** Matériel › Dante : les flux audio AES67 annoncés (Dante, consoles), à recevoir d'un clic ;
 *  l'annonce des nôtres pour Dante Controller. */
export function DantePage() {
  const hw = useHardware();
  const sap = usePoll<{ heard: SapHeard[]; announced: string[]; listening: string[]; error?: string | null }>('/api/hardware/sap', 2000);
  if (!hw) return null;
  const [, draft] = hw;
  const announce = draft.sap?.announce ?? false;
  const taken = (h: SapHeard) => draft.flows.some((f) => f.direction === 'input' && f.red === `${h.group}:${h.port}`);
  const receive = (h: SapHeard) => {
    editHw((x) => {
      const base = slug(h.name || `dante-${h.group}`);
      let id = base;
      for (let n = 2; x.flows.some((f) => f.id === id || groupOf(f) === id); n++) id = `${base}-${n}`;
      x.flows.push({
        id,
        group: id,
        name: h.name || `${h.origin} ${h.group}`,
        direction: 'input',
        essence: 'audio',
        red: `${h.group}:${h.port}`,
        blue: null,
        port: 1,
        channels: h.channels ?? 2,
        bits: h.bits ?? 24,
        ptimeUs: h.ptimeUs ?? 1000,
        source: h.source ?? null,
        enabled: true,
      } as Flow);
    });
    toast(t('Flow added (ST 2110 › Inputs): save to receive it'));
  };
  return (
    <>
      <PageHead title="Dante" />
      <Card icon={<Radio size={16} />} title={t('Flows announced on the network')} actions={<Switch checked={announce} onChange={(v) => editHw((x) => void (x.sap = { ...(x.sap ?? {}), announce: v }))} label={t('Announce our audio flows (SAP)')} />}>
        {sap?.error && <p className="core-warn">{friendly(sap.error)}</p>}
        {announce && sap && <p className="st-muted core-note">{sap.announced.length ? t('Announced: {list}', { list: sap.announced.join(', ') }) : t('No audio flow sent to announce.')}</p>}
        {!sap ? null : sap.heard.length === 0 ? (
          <p className="st-muted">{t('No flow announced on the network yet (Dante devices in AES67 mode announce theirs every 30 s).')}</p>
        ) : (
          <table className="st-table">
            <thead>
              <tr>
                <th>{t('Flow')}</th>
                <th>IP</th>
                <th>{t('Port')}</th>
                <th>{t('Format')}</th>
                <th>{t('Clock')}</th>
                <th className="r" />
              </tr>
            </thead>
            <tbody>
              {sap.heard.map((h) => (
                <tr key={`${h.origin}-${h.group}-${h.port}`}>
                  <td>
                    <strong>{h.name || '—'}</strong>
                    <div className="st-muted st-mono">{h.origin}</div>
                  </td>
                  <td className="st-mono">{h.group}</td>
                  <td className="st-mono">{h.port}</td>
                  <td>{h.channels ? t('{n} ch · L{b} · {p} ms', { n: h.channels, b: h.bits ?? 24, p: (h.ptimeUs ?? 1000) / 1000 }) : <span className="st-muted">{t('not audio')}</span>}</td>
                  <td className="st-mono st-muted" title={h.clock ?? undefined}>
                    {h.clock?.replace('ptp=IEEE1588-2008:', 'PTP ') ?? '—'}
                  </td>
                  <td className="r">
                    {taken(h) ? (
                      <Badge tone="ok">{t('Received')}</Badge>
                    ) : h.channels ? (
                      <button type="button" className="st-btn small" onClick={() => receive(h)}>
                        {t('Receive')}
                      </button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      <HwSaveBar />
    </>
  );
}
