// Réglages › Matériel › Flux réseau, en deux onglets : les sources (SRT, RTMP, RTSP, HLS, UDP…) que
// le Core reçoit et met en MXL pour les applis, et les destinations qu'il encode et envoie (YouTube,
// un CDN SRT, du HLS ou un lecteur HTML5 servis par le Core…). Chaque flux a son aperçu et son état.
import { useRef, useState } from 'react';
import { ArrowDownToLine, ArrowUpFromLine, Cast, ExternalLink, Pencil, Plus, Trash2 } from 'lucide-react';
import { Badge, Dropdown, Empty, Field, FieldGrid, Modal, NumberField, PageHead, Segmented, SubTabs, Switch, confirmDialog, t, toast } from '@strata/ui';
import { api } from '../api';
import { friendly } from '../errors';
import { Preview, Tile } from './hardware/Inputs';
import { usePoll } from '../poll';

type State = 'starting' | 'running' | 'waiting' | 'error';

interface InputStatus {
  state: State;
  source: string[];
  format?: string | null;
  frames: number;
  fps: number;
  audio: boolean;
  restarts: number;
  error?: string | null;
}

interface Input {
  id: string;
  name: string;
  url: string;
  enabled: boolean;
  format: string;
  relay: boolean;
  relayPort: number;
  opts?: ProtoOpts;
  mxlFlow?: string;
  relayUrl?: string | null;
  status?: InputStatus | null;
}

/** Les réglages propres au protocole (vide ou 0 : par défaut). */
interface ProtoOpts {
  srtMode?: string;
  srtLatencyMs?: number;
  srtPassphrase?: string;
  srtKeyLen?: number;
  srtStreamId?: string;
  rtspTransport?: string;
  rtmpListen?: boolean;
  localAddress?: string;
  sources?: string;
  ttl?: number;
  bufferKb?: number;
  ristProfile?: string;
  ristBufferMs?: number;
  ristSecret?: string;
  ristEncryption?: number;
  hlsSegmentS?: number;
  hlsListSize?: number;
  timeoutS?: number;
}

interface DestStatus {
  id: string;
  state: State;
  kbps: number;
  restarts: number;
  error?: string | null;
}

interface OutputStatus {
  state: State;
  encoder?: string | null;
  format?: string | null;
  live: boolean;
  fps: number;
  kbps: number;
  error?: string | null;
  destinations: DestStatus[];
}

interface Output {
  id: string;
  name: string;
  enabled: boolean;
  source: string;
  format: string;
  videoKbps: number;
  audioKbps: number;
  encoder: 'auto' | 'nvenc' | 'vaapi' | 'x264';
  /** Où elle part : `rtmp://…`, `srt://…`, `hls`, `html5`… */
  url: string;
  key: string;
  opts?: ProtoOpts;
  app?: string | null;
  mxlFlow?: string;
  /** L'adresse d'un flux servi par le Core (HLS, HTML5). */
  servedUrl?: string | null;
  status?: OutputStatus | null;
}

interface StreamsView {
  ffmpeg?: string | null;
  problem?: string | null;
  inputs: Input[];
  outputs: Output[];
  /** `output` : le flux qu'une appli écrit pour cette destination (envoyé aussi ailleurs). */
  sources: { flow: string; label: string; kind: string; output?: string }[];
}

const MASK = '••••••••';

type Net = 'srt' | 'rtmp' | 'rtmps' | 'rtsp' | 'https' | 'udp' | 'rtp' | 'rist';
/** `hls` et `html5` : servis par le Core (pas d'adresse). */
type Kind = Net | 'hls' | 'html5';
const SERVED: Kind[] = ['hls', 'html5'];
const NET_LABEL: Record<Net, string> = { srt: 'SRT', rtmp: 'RTMP', rtmps: 'RTMPS', rtsp: 'RTSP', https: 'HLS', udp: 'UDP (TS)', rtp: 'RTP (TS)', rist: 'RIST' };
const kindLabel = (k: Kind) => (k === 'hls' ? t('HLS served by the Core') : k === 'html5' ? t('HTML5 (web player)') : NET_LABEL[k]);
const PLACEHOLDER: Record<Net, [string, string]> = {
  srt: ['srt://10.0.0.12:9000  ·  srt://:9000', 'srt://cdn.example:9000'],
  rtmp: ['rtmp://server/live/stream  ·  rtmp://0.0.0.0:1935/live/cam', 'rtmp://a.rtmp.youtube.com/live2'],
  rtmps: ['rtmps://server/live/stream', 'rtmps://live-api-s.facebook.com:443/rtmp'],
  rtsp: ['rtsp://user:password@camera/stream', ''],
  https: ['https://example.com/live/index.m3u8', ''],
  udp: ['udp://@239.1.1.1:5000', 'udp://239.1.1.1:5000'],
  rtp: ['rtp://@239.1.1.1:5004', 'rtp://239.1.1.1:5004'],
  rist: ['rist://@:8193  ·  rist://10.0.0.12:8193', 'rist://10.0.0.12:8193'],
};

function kindOf(url: string): Kind | null {
  if (url === 'hls' || url === 'html5') return url;
  const s = url.split('://')[0]?.toLowerCase();
  if (s === 'http') return 'https';
  return (Object.keys(NET_LABEL) as Net[]).includes(s as Net) ? (s as Net) : null;
}

/** Change le protocole d'une adresse (garde le reste). */
function withKind(url: string, k: Kind): string {
  if (SERVED.includes(k)) return k;
  const rest = url.includes('://') ? url.slice(url.indexOf('://') + 3) : '';
  return `${k}://${rest}`;
}

/** L'adresse complète d'un flux servi par le Core. */
const full = (path: string) => `${location.origin}${path}`;

/** Le type de flux, puis l'adresse et les réglages de ce type. */
function ProtoFields({ url, opts, output, kinds, served, onUrl, onOpts, extra }: {
  url: string;
  opts: ProtoOpts;
  output: boolean;
  kinds: Kind[];
  /** L'adresse d'un flux servi par le Core (connue une fois la sortie enregistrée). */
  served?: string | null;
  onUrl: (u: string) => void;
  onOpts: (o: ProtoOpts) => void;
  extra?: React.ReactNode;
}) {
  const kind = kindOf(url);
  const set = <K extends keyof ProtoOpts>(k: K, x: ProtoOpts[K]) => onOpts({ ...opts, [k]: x });
  const num = (k: keyof ProtoOpts, label: string, suffix: string, max: number, placeholder: string) => (
    <Field label={suffix ? `${label} (${suffix})` : label}>
      <input
        className="st-mono"
        inputMode="numeric"
        value={opts[k] ? String(opts[k]) : ''}
        placeholder={placeholder}
        onChange={(e) => set(k, Math.min(max, Number(e.target.value.replace(/[^0-9]/g, '')) || 0) as never)}
      />
    </Field>
  );
  const text = (k: keyof ProtoOpts, label: string, placeholder = '', secret = false) => (
    <Field label={label}>
      <input
        className="st-mono"
        type={secret ? 'password' : 'text'}
        autoComplete="off"
        value={(opts[k] as string | undefined) ?? ''}
        placeholder={placeholder}
        onFocus={(e) => secret && opts[k] === MASK && e.target.select()}
        onChange={(e) => set(k, e.target.value as never)}
      />
    </Field>
  );
  return (
    <>
      <FieldGrid two>
        <Field label={t('Type')} group>
          <Dropdown
            value={kind ?? ''}
            options={kinds.map((k) => ({ value: k, label: kindLabel(k) }))}
            onChange={(k) => onUrl(withKind(url, k as Kind))}
          />
        </Field>
        {kind && SERVED.includes(kind) ? (
          <Field label={t('Address')}>
            {served ? (
              <a className="st-mono core-served" href={served} target="_blank" rel="noreferrer">
                {full(served)} <ExternalLink size={12} />
              </a>
            ) : (
              <input className="st-mono" value={t('given once saved')} readOnly disabled />
            )}
          </Field>
        ) : (
          <Field label={t('Address')}>
            <input className="st-mono" value={url} placeholder={kind ? PLACEHOLDER[kind as Net][output ? 1 : 0] : 'srt://…'} onChange={(e) => onUrl(e.target.value)} />
          </Field>
        )}
      </FieldGrid>
      {kind === 'hls' && (
        <FieldGrid two>
          {num('hlsSegmentS', t('Segment length'), 's', 30, '2')}
          {num('hlsListSize', t('Segments in the playlist'), '', 60, '6')}
        </FieldGrid>
      )}
      {kind === 'srt' && (
        <>
          <FieldGrid two>
            <Field label={t('SRT mode')} group>
              <Dropdown
                value={opts.srtMode ?? ''}
                options={[
                  { value: '', label: t('Auto (listener without a host)') },
                  { value: 'caller', label: t('Caller') },
                  { value: 'listener', label: t('Listener') },
                  { value: 'rendezvous', label: t('Rendezvous') },
                ]}
                onChange={(x) => set('srtMode', x)}
              />
            </Field>
            {num('srtLatencyMs', t('Latency'), 'ms', 10000, '120')}
          </FieldGrid>
          <FieldGrid two>
            {text('srtPassphrase', t('Passphrase (encryption)'), t('none'), true)}
            <Field label={t('Key length')} group>
              <Dropdown
                value={String(opts.srtKeyLen ?? 0)}
                options={[{ value: '0', label: t('Default') }, { value: '16', label: 'AES-128' }, { value: '24', label: 'AES-192' }, { value: '32', label: 'AES-256' }]}
                onChange={(x) => set('srtKeyLen', Number(x))}
              />
            </Field>
          </FieldGrid>
          {text('srtStreamId', t('Stream ID'), output ? t('the stream key, if empty') : '')}
        </>
      )}
      {kind === 'rtsp' && (
        <FieldGrid two>
          <Field label={t('Transport')} group>
            <Segmented value={opts.rtspTransport || 'tcp'} onChange={(x) => set('rtspTransport', x)} options={[{ value: 'tcp', label: 'TCP' }, { value: 'udp', label: 'UDP' }, { value: 'http', label: 'HTTP' }]} />
          </Field>
          {num('timeoutS', t('Give up after'), 's', 600, '10')}
        </FieldGrid>
      )}
      {(kind === 'rtmp' || kind === 'rtmps') && !output && (
        <FieldGrid two>
          <Field label={t('Connection')} group>
            <Switch checked={!!opts.rtmpListen} onChange={(x) => set('rtmpListen', x)} label={t('The stream is pushed to the server')} />
          </Field>
          {num('timeoutS', t('Give up after'), 's', 600, '10')}
        </FieldGrid>
      )}
      {kind === 'https' && !output && <FieldGrid two>{num('timeoutS', t('Give up after'), 's', 600, '10')}</FieldGrid>}
      {(kind === 'udp' || kind === 'rtp') && (
        <FieldGrid two>
          {text('localAddress', t('Local address (interface)'), t('automatic'))}
          {output ? num('ttl', 'TTL', '', 255, '1') : text('sources', t('Accepted sources (SSM)'), '10.0.0.12')}
          {!output && num('bufferKb', t('Receive buffer'), 'KB', 524288, '8192')}
        </FieldGrid>
      )}
      {kind === 'rist' && (
        <>
          <FieldGrid two>
            <Field label={t('RIST profile')} group>
              <Segmented value={opts.ristProfile || 'main'} onChange={(x) => set('ristProfile', x)} options={[{ value: 'simple', label: 'Simple' }, { value: 'main', label: 'Main' }, { value: 'advanced', label: 'Advanced' }]} />
            </Field>
            {num('ristBufferMs', t('Recovery buffer'), 'ms', 30000, '1000')}
          </FieldGrid>
          <FieldGrid two>
            {text('ristSecret', t('Secret (encryption)'), t('none'), true)}
            <Field label={t('Encryption')} group>
              <Dropdown value={String(opts.ristEncryption || 128)} options={[{ value: '128', label: 'AES-128' }, { value: '256', label: 'AES-256' }]} onChange={(x) => set('ristEncryption', Number(x))} />
            </Field>
          </FieldGrid>
          {text('localAddress', t('Local address (interface)'), t('automatic'))}
        </>
      )}
      {extra}
    </>
  );
}

const IN_KINDS: Kind[] = ['srt', 'rtmp', 'rtmps', 'rtsp', 'https', 'udp', 'rtp', 'rist'];
const OUT_KINDS: Kind[] = ['srt', 'rtmp', 'rtmps', 'udp', 'rtp', 'rist', 'hls', 'html5'];
const enc = encodeURIComponent;
const FORMATS = ['1080i50', '1080i59.94', '1080p25', '1080p50', '1080p59.94', '720p50', '720p59.94', '2160p25', '2160p50'];
const OUT_FORMATS = ['1080p25', '1080p50', '1080p59.94', '720p50', '720p59.94', '2160p25', '2160p50'];

/** Ce qui est enregistré : les réglages seuls, sans l'état. */
function clean(v: StreamsView) {
  return {
    inputs: v.inputs.map(({ id, name, url, enabled, format, relay, relayPort }) => ({ id, name, url, enabled, format, relay, relayPort })),
    outputs: v.outputs.map(({ id, name, enabled, source, format, videoKbps, audioKbps, encoder, url, key, opts, app }) => ({ id, name, enabled, source, format, videoKbps, audioKbps, encoder, url, key, opts, app })),
  };
}

/** Un identifiant d'après le nom : « Caméra 2 » → « camera-2 ». */
function slug(name: string, taken: string[]): string {
  const base =
    name
      .normalize('NFD')
      .replace(/[\u0300-\u036f]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '')
      .slice(0, 40) || 'flux';
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

function stateBadge(s: State | undefined, running: string) {
  switch (s) {
    case 'running':
      return <Badge tone="ok">{running}</Badge>;
    case 'waiting':
      return <Badge tone="warn">{t('Waiting for the source')}</Badge>;
    case 'error':
      return <Badge tone="bad">{t('Error')}</Badge>;
    default:
      return <Badge>{t('Starting…')}</Badge>;
  }
}

/** « Video: h264 (High) ([27][0][0][0] / 0x001B), yuv420p(tv), 1920x1080, 25 fps » → « h264 (High), 1920x1080, 25 fps ». */
function codec(desc: string): string {
  const parts = desc
    .replace(/^(Video|Audio):\s*/, '')
    .replace(/\s*\(\[[^)]*\)/g, '')
    .replace(/\s*\[[^\]]*\]/g, '')
    .split(/,\s*(?![^(]*\))/);
  return parts.filter((p, i) => i === 0 || /\d+x\d+|fps|Hz|stereo|mono|5\.1/.test(p)).join(', ');
}

/** Le protocole d'une adresse, pour la montrer en clair. */
function proto(url: string): string {
  if (url === 'hls') return 'HLS';
  if (url === 'html5') return 'HTML5';
  const s = url.split('://')[0]?.toUpperCase() ?? '';
  return s === 'HTTPS' || s === 'HTTP' ? 'HLS / HTTP' : s;
}

export function StreamsPage() {
  const data = usePoll<StreamsView>('/api/streams');
  // Les flux à éditer, par leur id (vide : un nouveau) : la liste est relue chaque seconde, ses
  // positions peuvent changer pendant qu'un dialogue est ouvert.
  const [editIn, setEditIn] = useState<Input | null>(null);
  const [editOut, setEditOut] = useState<Output | null>(null);
  const [saving, setSaving] = useState(false);
  const [tab, setTab] = useState<'sources' | 'destinations'>('sources');
  // La dernière lecture : ce qu'on enregistre après un dialogue (pas celle d'avant le dialogue).
  const latest = useRef(data);
  latest.current = data;
  if (!data) return null;

  const save = async (next: StreamsView) => {
    setSaving(true);
    try {
      await api('PUT', '/api/streams', clean(next));
      toast(t('Saved'));
      return true;
    } catch (e) {
      toast(friendly(e), 'error');
      return false;
    } finally {
      setSaving(false);
    }
  };
  const idsOf = (d: StreamsView) => [...d.inputs.map((x) => x.id), ...d.outputs.map((x) => x.id)];
  const newInput: Input = { id: '', name: '', url: '', enabled: true, format: '', relay: false, relayPort: 0 };
  const newOutput: Output = { id: '', name: '', enabled: true, source: '', format: '', videoKbps: 6000, audioKbps: 192, encoder: 'auto', url: 'rtmp://', key: '' };

  return (
    <>
      <PageHead title={t('Network streams')} sub={data.ffmpeg ? `ffmpeg ${data.ffmpeg}` : undefined} />
      {data.problem && <p className="core-warn">{friendly(data.problem)}</p>}

      <SubTabs<'sources' | 'destinations'>
        items={[
          { id: 'sources', label: t('Sources · received'), count: data.inputs.length },
          { id: 'destinations', label: t('Destinations · sent'), count: data.outputs.length },
        ]}
        value={tab}
        onChange={setTab}
      >
        {tab === 'sources' ? (
          <button type="button" className="st-btn primary" onClick={() => setEditIn(newInput)}>
            <Plus size={14} /> {t('New source')}
          </button>
        ) : (
          <button type="button" className="st-btn primary" onClick={() => setEditOut(newOutput)}>
            <Plus size={14} /> {t('New destination')}
          </button>
        )}
      </SubTabs>

      {tab === 'sources' &&
        (data.inputs.length === 0 ? (
          <Empty icon={Cast} text={t('No network input')} />
        ) : (
          <div className="core-inputs">
            {data.inputs.map((x) => {
              const st = x.status;
              const sub = [proto(x.url), st?.format, st?.state === 'running' ? `${st.fps} ${t('fps')}` : null, st?.state === 'running' ? (st.audio ? t('with sound') : t('no sound')) : null].filter(Boolean).join(' · ');
              return (
                <Tile
                  key={x.id}
                  pic={<Preview src={`/api/streams/inputs/${enc(x.id)}/preview.jpg`} on={st?.state === 'running'} empty={x.enabled ? t('No picture') : t('Off')} />}
                  head={
                    <>
                      <strong className="core-input-wire">{x.name}</strong>
                      {x.enabled ? stateBadge(st?.state, t('Receiving')) : <Badge>{t('Off')}</Badge>}
                    </>
                  }
                  sub={sub}
                  error={x.enabled ? st?.error : null}
                  foot={
                    <div className="core-stream-foot">
                      <span className="st-mono st-muted" title={x.url}>
                        {x.url}
                      </span>
                      {st?.source?.[0] && <span className="st-muted">{st.source.map(codec).join(' · ')}</span>}
                      {x.relayUrl && (
                        <span className="st-muted">
                          {t('Original stream:')} <span className="st-mono">{x.relayUrl}</span>
                        </span>
                      )}
                      <span className="core-stream-actions">
                        <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditIn(x)}>
                          <Pencil size={14} />
                        </button>
                        <button
                          type="button"
                          className="st-icon-btn ghost"
                          title={t('Delete')}
                          onClick={async () => {
                            if (await confirmDialog({ title: t('Delete {name}?', { name: x.name }), confirm: t('Delete'), danger: true })) {
                              const cur = latest.current ?? data;
                              await save({ ...cur, inputs: cur.inputs.filter((y) => y.id !== x.id) });
                            }
                          }}
                        >
                          <Trash2 size={14} />
                        </button>
                      </span>
                    </div>
                  }
                />
              );
            })}
          </div>
        ))}

      {tab === 'destinations' &&
        (data.outputs.length === 0 ? (
          <Empty icon={Cast} text={t('No network output')} />
        ) : (
          <div className="core-inputs">
            {data.outputs.map((o) => {
              const st = o.status;
              const src = o.source ? data.sources.find((s) => s.flow === o.source)?.label ?? o.source : o.app ? t('App {name}', { name: o.app }) : t('An app');
              const ds = st?.destinations[0];
              const sub = [proto(o.url), src, st?.format, st?.encoder?.toUpperCase()].filter(Boolean).join(' · ');
              return (
                <Tile
                  key={o.id}
                  pic={<Preview src={`/api/streams/outputs/${enc(o.id)}/preview.jpg`} on={!!st?.live} empty={st && !st.live ? t('Black and silence') : t('No picture')} />}
                  head={
                    <>
                      <strong className="core-input-wire">{o.name}</strong>
                      {!o.enabled ? (
                        <Badge>{t('Off')}</Badge>
                      ) : ds && ds.state !== 'running' ? (
                        stateBadge(ds.state, '')
                      ) : (
                        stateBadge(st?.state, st?.live ? (ds && ds.kbps > 0 ? `${t('On air')} · ${Math.round(ds.kbps)} kb/s` : t('On air')) : t('Black on air'))
                      )}
                    </>
                  }
                  sub={sub}
                  error={o.enabled ? (ds?.error ? friendly(ds.error) : st?.error) : null}
                  foot={
                    <div className="core-stream-foot">
                      {o.servedUrl ? (
                        <a className="st-mono core-served" href={o.servedUrl} target="_blank" rel="noreferrer">
                          {full(o.servedUrl)} <ExternalLink size={12} />
                        </a>
                      ) : (
                        <span className="st-mono st-muted" title={o.url}>
                          {o.url}
                        </span>
                      )}
                      <span className="core-stream-actions">
                        <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditOut(o)}>
                          <Pencil size={14} />
                        </button>
                        <button
                          type="button"
                          className="st-icon-btn ghost"
                          title={t('Delete')}
                          onClick={async () => {
                            if (await confirmDialog({ title: t('Delete {name}?', { name: o.name }), confirm: t('Delete'), danger: true })) {
                              const cur = latest.current ?? data;
                              await save({ ...cur, outputs: cur.outputs.filter((y) => y.id !== o.id) });
                            }
                          }}
                        >
                          <Trash2 size={14} />
                        </button>
                      </span>
                    </div>
                  }
                />
              );
            })}
          </div>
        ))}

      {editIn && (
        <InputEditor
          value={editIn}
          busy={saving}
          onClose={() => setEditIn(null)}
          onSave={async (v) => {
            const cur = latest.current ?? data;
            const inputs = [...cur.inputs];
            const item = { ...v, id: v.id || slug(v.name, idsOf(cur)) };
            const k = editIn.id ? inputs.findIndex((y) => y.id === editIn.id) : -1;
            if (k < 0) inputs.push(item);
            else inputs[k] = item;
            if (await save({ ...cur, inputs })) setEditIn(null);
          }}
        />
      )}
      {editOut && (
        <OutputEditor
          value={editOut}
          sources={data.sources}
          busy={saving}
          onClose={() => setEditOut(null)}
          onSave={async (v) => {
            const cur = latest.current ?? data;
            const outputs = [...cur.outputs];
            const item = { ...v, id: v.id || slug(v.name, idsOf(cur)) };
            const k = editOut.id ? outputs.findIndex((y) => y.id === editOut.id) : -1;
            if (k < 0) outputs.push(item);
            else outputs[k] = item;
            if (await save({ ...cur, outputs })) setEditOut(null);
          }}
        />
      )}
    </>
  );
}

function InputEditor({ value, busy, onClose, onSave }: { value: Input; busy: boolean; onClose: () => void; onSave: (v: Input) => void }) {
  const [v, setV] = useState<Input>(value);
  const set = <K extends keyof Input>(k: K, x: Input[K]) => setV({ ...v, [k]: x });
  return (
    <Modal
      title={value.name ? t('Edit {name}', { name: value.name }) : t('New input')}
      icon={<ArrowDownToLine size={16} />}
      onClose={onClose}
      onSubmit={() => onSave(v)}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={busy || !v.name.trim() || !v.url.trim()}>
            {t('OK')}
          </button>
        </>
      }
    >
      <Field label={t('Name')}>
        <input value={v.name} placeholder={t('Studio camera')} onChange={(e) => set('name', e.target.value)} />
      </Field>
      <ProtoFields url={v.url} opts={v.opts ?? {}} output={false} kinds={IN_KINDS} onUrl={(u) => set('url', u)} onOpts={(o) => set('opts', o)} />
      <FieldGrid two>
        <Field label={t('Video written in MXL')} group>
          <Dropdown
            value={v.format}
            options={[{ value: '', label: t('As the source (in 1080)') }, { value: 'house', label: t('House format') }, ...FORMATS.map((x) => ({ value: x, label: x }))]}
            onChange={(x) => set('format', x)}
          />
        </Field>
        <Field label={t('State')} group>
          <Switch checked={v.enabled} onChange={(x) => set('enabled', x)} label={t('Received')} />
        </Field>
      </FieldGrid>
      <Switch checked={v.relay} onChange={(x) => set('relay', x)} label={t('Also pass the original stream on, locally (for an app that records or relays it without decoding)')} />
    </Modal>
  );
}

function OutputEditor({ value, sources, busy, onClose, onSave }: { value: Output; sources: StreamsView['sources']; busy: boolean; onClose: () => void; onSave: (v: Output) => void }) {
  const [v, setV] = useState<Output>(value);
  const set = <K extends keyof Output>(k: K, x: Output[K]) => setV({ ...v, [k]: x });
  const kind = kindOf(v.url);
  const valid = v.name.trim() && kind && (SERVED.includes(kind) || v.url.split('://')[1]?.trim());
  return (
    <Modal
      title={value.name ? t('Edit {name}', { name: value.name }) : t('New destination')}
      icon={<ArrowUpFromLine size={16} />}
      wide
      onClose={onClose}
      onSubmit={() => onSave(v)}
      footer={
        <>
          <button type="button" className="st-btn" onClick={onClose}>
            {t('Cancel')}
          </button>
          <button type="submit" className="st-btn primary" disabled={busy || !valid}>
            {t('OK')}
          </button>
        </>
      }
    >
      <FieldGrid two>
        <Field label={t('Name')}>
          <input value={v.name} placeholder={t('YouTube, CDN…')} onChange={(e) => set('name', e.target.value)} />
        </Field>
        <Field label={t('What is sent')} group>
          <Dropdown
            value={v.source}
            options={[
              { value: '', label: v.app ? t('App {name}', { name: v.app }) : t('An app (writes the stream in MXL)') },
              ...sources.filter((s) => !s.output || s.output !== value.id).map((s) => ({ value: s.flow, label: s.kind === 'app' ? t('Same stream as {name}', { name: s.label }) : s.label })),
            ]}
            onChange={(x) => set('source', x)}
          />
        </Field>
      </FieldGrid>
      <ProtoFields
        url={v.url}
        opts={v.opts ?? {}}
        output
        kinds={OUT_KINDS}
        served={value.url === v.url ? value.servedUrl : null}
        onUrl={(u) => set('url', u)}
        onOpts={(o) => set('opts', o)}
        extra={
          (v.url.startsWith('rtmp') || v.key) && (
            <Field label={t('Stream key')}>
              <input className="st-mono" type="password" autoComplete="off" value={v.key} onFocus={(e) => v.key === MASK && e.target.select()} onChange={(e) => set('key', e.target.value)} />
            </Field>
          )
        }
      />
      <FieldGrid two>
        <Field label={t('Video sent')} group>
          <Dropdown value={v.format} options={[{ value: '', label: t('As the source') }, ...OUT_FORMATS.map((x) => ({ value: x, label: x }))]} onChange={(x) => set('format', x)} />
        </Field>
        <Field label={t('Encoder')} group>
          <Segmented<Output['encoder']>
            value={v.encoder}
            onChange={(x) => set('encoder', x)}
            options={[
              { value: 'auto', label: t('Auto (GPU)') },
              { value: 'nvenc', label: 'NVENC' },
              { value: 'vaapi', label: 'VA-API' },
              { value: 'x264', label: 'x264' },
            ]}
          />
        </Field>
      </FieldGrid>
      <FieldGrid two>
        <NumberField label={t('Video bitrate')} value={v.videoKbps} onChange={(x) => set('videoKbps', x)} step={500} min={300} max={100000} suffix="kb/s" />
        <NumberField label={t('Audio bitrate')} value={v.audioKbps} onChange={(x) => set('audioKbps', x)} step={32} min={32} max={512} suffix="kb/s" />
      </FieldGrid>
      <Switch checked={v.enabled} onChange={(x) => set('enabled', x)} label={t('On air')} />
    </Modal>
  );
}
