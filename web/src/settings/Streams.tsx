// Réglages › Matériel › Flux réseau : les entrées (SRT, RTMP, RTSP, HLS, UDP…) que le Core reçoit et
// met en MXL pour les applis, et les sorties qu'il encode et envoie à leurs destinations (YouTube,
// un CDN SRT, du HLS servi par le Core…). Chaque flux a son aperçu et son état en direct.
import { useState } from 'react';
import { ArrowDownToLine, ArrowUpFromLine, Cast, Pencil, Plus, Trash2 } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, Field, FieldGrid, Modal, NumberField, PageHead, Segmented, Switch, confirmDialog, t, tn, toast } from '@strata/ui';
import { api } from '../api';
import { friendly } from '../errors';
import { Preview, Tile, usePoll } from './hardware/Inputs';

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

interface Destination {
  id: string;
  name: string;
  url: string;
  key: string;
  enabled: boolean;
  opts?: ProtoOpts;
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
  destinations: Destination[];
  app?: string | null;
  mxlFlow?: string;
  hlsUrl?: string | null;
  status?: OutputStatus | null;
}

interface StreamsView {
  ffmpeg?: string | null;
  problem?: string | null;
  inputs: Input[];
  outputs: Output[];
  sources: { flow: string; label: string; kind: string }[];
}

const MASK = '••••••••';

type Kind = 'srt' | 'rtmp' | 'rtmps' | 'rtsp' | 'https' | 'udp' | 'rtp' | 'rist';
const KIND_LABEL: Record<Kind, string> = { srt: 'SRT', rtmp: 'RTMP', rtmps: 'RTMPS', rtsp: 'RTSP', https: 'HLS', udp: 'UDP (TS)', rtp: 'RTP (TS)', rist: 'RIST' };
const PLACEHOLDER: Record<Kind, [string, string]> = {
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
  const s = url.split('://')[0]?.toLowerCase();
  if (s === 'http') return 'https';
  return (Object.keys(KIND_LABEL) as Kind[]).includes(s as Kind) ? (s as Kind) : null;
}

/** Change le protocole d'une adresse (garde le reste). */
function withKind(url: string, k: Kind): string {
  const rest = url.includes('://') ? url.slice(url.indexOf('://') + 3) : '';
  return `${k}://${rest}`;
}

/** Le type de flux, puis l'adresse et les réglages de ce type. */
function ProtoFields({ url, opts, output, kinds, onUrl, onOpts, extra }: {
  url: string;
  opts: ProtoOpts;
  output: boolean;
  kinds: Kind[];
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
          <Dropdown value={kind ?? ''} options={kinds.map((k) => ({ value: k, label: KIND_LABEL[k] }))} onChange={(k) => onUrl(withKind(url, k as Kind))} />
        </Field>
        <Field label={t('Address')}>
          <input className="st-mono" value={url} placeholder={kind ? PLACEHOLDER[kind][output ? 1 : 0] : 'srt://…'} onChange={(e) => onUrl(e.target.value)} />
        </Field>
      </FieldGrid>
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
const OUT_KINDS: Kind[] = ['srt', 'rtmp', 'rtmps', 'udp', 'rtp', 'rist'];
const enc = encodeURIComponent;
const FORMATS = ['1080i50', '1080i59.94', '1080p25', '1080p50', '1080p59.94', '720p50', '720p59.94', '2160p25', '2160p50'];
const OUT_FORMATS = ['1080p25', '1080p50', '1080p59.94', '720p50', '720p59.94', '2160p25', '2160p50'];

/** Ce qui est enregistré : les réglages seuls, sans l'état. */
function clean(v: StreamsView) {
  return {
    inputs: v.inputs.map(({ id, name, url, enabled, format, relay, relayPort }) => ({ id, name, url, enabled, format, relay, relayPort })),
    outputs: v.outputs.map(({ id, name, enabled, source, format, videoKbps, audioKbps, encoder, destinations, app }) => ({ id, name, enabled, source, format, videoKbps, audioKbps, encoder, destinations, app })),
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
  const s = url.split('://')[0]?.toUpperCase() ?? '';
  return s === 'HTTPS' || s === 'HTTP' ? 'HLS / HTTP' : s;
}

export function StreamsPage() {
  const data = usePoll<StreamsView>('/api/streams');
  const [editIn, setEditIn] = useState<{ v: Input; i: number } | null>(null);
  const [editOut, setEditOut] = useState<{ v: Output; i: number } | null>(null);
  const [saving, setSaving] = useState(false);
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
  const ids = [...data.inputs.map((x) => x.id), ...data.outputs.map((x) => x.id)];
  const newInput: Input = { id: '', name: '', url: '', enabled: true, format: '', relay: false, relayPort: 0 };
  const newOutput: Output = { id: '', name: '', enabled: true, source: '', format: '', videoKbps: 6000, audioKbps: 192, encoder: 'auto', destinations: [] };

  return (
    <>
      <PageHead title={t('Network streams')} sub={`${tn(data.inputs.length, '{n} input', '{n} inputs')} · ${tn(data.outputs.length, '{n} output', '{n} outputs')}${data.ffmpeg ? ` · ffmpeg ${data.ffmpeg}` : ''}`}>
        <button type="button" className="st-btn" onClick={() => setEditIn({ v: newInput, i: -1 })}>
          <Plus size={14} /> {t('New input')}
        </button>
        <button type="button" className="st-btn primary" onClick={() => setEditOut({ v: newOutput, i: -1 })}>
          <Plus size={14} /> {t('New output')}
        </button>
      </PageHead>
      {data.problem && <p className="core-warn">{friendly(data.problem)}</p>}

      <Card icon={<ArrowDownToLine size={16} />} title={t('Inputs')}>
        {data.inputs.length === 0 ? (
          <Empty icon={Cast} text={t('No network input')} />
        ) : (
          <div className="core-inputs">
            {data.inputs.map((x, i) => {
              const st = x.status;
              const sub = [proto(x.url), st?.format, st?.state === 'running' ? `${st.fps} i/s` : null, st?.state === 'running' ? (st.audio ? t('with sound') : t('no sound')) : null].filter(Boolean).join(' · ');
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
                          {t('Original stream')} : <span className="st-mono">{x.relayUrl}</span>
                        </span>
                      )}
                      <span className="core-stream-actions">
                        <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditIn({ v: x, i })}>
                          <Pencil size={14} />
                        </button>
                        <button
                          type="button"
                          className="st-icon-btn ghost"
                          title={t('Delete')}
                          onClick={async () => {
                            if (await confirmDialog({ title: t('Delete {name}?', { name: x.name }), confirm: t('Delete'), danger: true })) {
                              await save({ ...data, inputs: data.inputs.filter((_, k) => k !== i) });
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
        )}
      </Card>

      <Card icon={<ArrowUpFromLine size={16} />} title={t('Outputs')}>
        {data.outputs.length === 0 ? (
          <Empty icon={Cast} text={t('No network output')} />
        ) : (
          <div className="core-inputs">
            {data.outputs.map((o, i) => {
              const st = o.status;
              const src = o.source ? data.sources.find((s) => s.flow === o.source)?.label ?? o.source : o.app ? t('App {name}', { name: o.app }) : t('An app');
              const sub = [src, st?.format, st?.encoder?.toUpperCase(), st && st.kbps > 0 ? `${Math.round(st.kbps)} kb/s` : null].filter(Boolean).join(' · ');
              return (
                <Tile
                  key={o.id}
                  pic={<Preview src={`/api/streams/outputs/${enc(o.id)}/preview.jpg`} on={!!st?.live} empty={st && !st.live ? t('Black and silence') : t('No picture')} />}
                  head={
                    <>
                      <strong className="core-input-wire">{o.name}</strong>
                      {o.enabled ? stateBadge(st?.state, st?.live ? t('On air') : t('Black on air')) : <Badge>{t('Off')}</Badge>}
                    </>
                  }
                  sub={sub}
                  error={o.enabled ? st?.error : null}
                  foot={
                    <div className="core-stream-foot">
                      {o.destinations.map((d) => {
                        const ds = st?.destinations.find((x) => x.id === d.id);
                        return (
                          <span key={d.id} className="core-stream-dest" title={ds?.error ? friendly(ds.error) : d.url}>
                            {!d.enabled ? <Badge>{t('Off')}</Badge> : ds ? stateBadge(ds.state, ds.kbps > 0 ? `${Math.round(ds.kbps)} kb/s` : t('Sending')) : <Badge>—</Badge>}
                            <span>{d.name || proto(d.url)}</span>
                            <span className="st-mono st-muted">{d.url === 'hls' ? o.hlsUrl : d.url}</span>
                          </span>
                        );
                      })}
                      <span className="core-stream-actions">
                        <button type="button" className="st-icon-btn ghost" title={t('Edit')} onClick={() => setEditOut({ v: o, i })}>
                          <Pencil size={14} />
                        </button>
                        <button
                          type="button"
                          className="st-icon-btn ghost"
                          title={t('Delete')}
                          onClick={async () => {
                            if (await confirmDialog({ title: t('Delete {name}?', { name: o.name }), confirm: t('Delete'), danger: true })) {
                              await save({ ...data, outputs: data.outputs.filter((_, k) => k !== i) });
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
        )}
      </Card>

      {editIn && (
        <InputEditor
          value={editIn.v}
          busy={saving}
          onClose={() => setEditIn(null)}
          onSave={async (v) => {
            const inputs = [...data.inputs];
            const item = { ...v, id: v.id || slug(v.name, ids) };
            if (editIn.i < 0) inputs.push(item);
            else inputs[editIn.i] = item;
            if (await save({ ...data, inputs })) setEditIn(null);
          }}
        />
      )}
      {editOut && (
        <OutputEditor
          value={editOut.v}
          sources={data.sources}
          busy={saving}
          onClose={() => setEditOut(null)}
          onSave={async (v) => {
            const outputs = [...data.outputs];
            const item = { ...v, id: v.id || slug(v.name, ids) };
            if (editOut.i < 0) outputs.push(item);
            else outputs[editOut.i] = item;
            if (await save({ ...data, outputs })) setEditOut(null);
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
  const setDest = (i: number, d: Partial<Destination>) => set('destinations', v.destinations.map((x, k) => (k === i ? { ...x, ...d } : x)));
  const addDest = () => set('destinations', [...v.destinations, { id: `d${Date.now().toString(36)}`, name: '', url: 'rtmp://', key: '', enabled: true }]);
  const valid = v.name.trim() && v.destinations.length > 0 && v.destinations.every((d) => d.url.trim());
  return (
    <Modal
      title={value.name ? t('Edit {name}', { name: value.name }) : t('New output')}
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
          <input value={v.name} placeholder={t('Program')} onChange={(e) => set('name', e.target.value)} />
        </Field>
        <Field label={t('What is sent')} group>
          <Dropdown
            value={v.source}
            options={[{ value: '', label: v.app ? t('App {name}', { name: v.app }) : t('An app (writes the stream in MXL)') }, ...sources.map((s) => ({ value: s.flow, label: s.label }))]}
            onChange={(x) => set('source', x)}
          />
        </Field>
      </FieldGrid>
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
      <h4 className="core-subhead">{t('Destinations')}</h4>
      {v.destinations.map((d, i) => (
        <div key={d.id} className="core-dest-row">
          {d.url === 'hls' ? (
            <FieldGrid two>
              <Field label={`${t('Segment length')} (s)`}>
                <input
                  className="st-mono"
                  value={d.opts?.hlsSegmentS ? String(d.opts.hlsSegmentS) : ''}
                  placeholder="2"
                  onChange={(e) => setDest(i, { opts: { ...d.opts, hlsSegmentS: Math.min(30, Number(e.target.value.replace(/[^0-9]/g, '')) || 0) } })}
                />
              </Field>
              <Field label={t('Segments in the playlist')}>
                <input
                  className="st-mono"
                  value={d.opts?.hlsListSize ? String(d.opts.hlsListSize) : ''}
                  placeholder="6"
                  onChange={(e) => setDest(i, { opts: { ...d.opts, hlsListSize: Math.min(60, Number(e.target.value.replace(/[^0-9]/g, '')) || 0) } })}
                />
              </Field>
            </FieldGrid>
          ) : (
            <ProtoFields
              url={d.url}
              opts={d.opts ?? {}}
              output
              kinds={OUT_KINDS}
              onUrl={(u) => setDest(i, { url: u })}
              onOpts={(o) => setDest(i, { opts: o })}
              extra={
                (d.url.startsWith('rtmp') || d.key) && (
                  <Field label={t('Stream key')}>
                    <input
                      className="st-mono"
                      type="password"
                      autoComplete="off"
                      value={d.key}
                      onFocus={(e) => d.key === MASK && e.target.select()}
                      onChange={(e) => setDest(i, { key: e.target.value })}
                    />
                  </Field>
                )
              }
            />
          )}
          <div className="core-dest-tools">
            <input value={d.name} placeholder={t('Name (YouTube, CDN…)')} onChange={(e) => setDest(i, { name: e.target.value })} />
            <Switch checked={d.enabled} onChange={(x) => setDest(i, { enabled: x })} label={t('On')} />
            <button type="button" className="st-icon-btn ghost" title={t('Delete')} onClick={() => set('destinations', v.destinations.filter((_, k) => k !== i))}>
              <Trash2 size={14} />
            </button>
          </div>
        </div>
      ))}
      <div className="core-dest-add">
        <button type="button" className="st-btn" onClick={addDest}>
          <Plus size={14} /> {t('Add a destination')}
        </button>
        <button
          type="button"
          className="st-btn"
          disabled={v.destinations.some((d) => d.url === 'hls')}
          onClick={() => set('destinations', [...v.destinations, { id: 'hls', name: 'HLS', url: 'hls', key: '', enabled: true }])}
        >
          <Plus size={14} /> {t('HLS served by the Core')}
        </button>
      </div>
    </Modal>
  );
}
