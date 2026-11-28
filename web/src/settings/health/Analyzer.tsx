// Santé › Mires et analyse › Analyseur · reçu : une source du serveur analysée par le Core
// (image, scopes, son, sonie R128, calage image/son, données) et écoutée dans le navigateur. Le
// casque d'un canal l'écoute seul (plusieurs possibles) ; sans solo, le mixage de tous.
import { useEffect, useRef, useState } from 'react';
import { Headphones, Play, RotateCcw, Square } from 'lucide-react';
import { Badge, Card, Dropdown, Empty, t } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { usePoll } from '../../poll';
import { Preview } from '../hardware/Inputs';

interface Source {
  key: string;
  label: string;
  kind: 'sdi' | '2110' | 'audio' | 'stream' | 'ndi' | 'mxl';
  way: 'in' | 'out';
  video?: string | null;
  audio: string[];
  data?: string | null;
}

interface Level {
  peak: number;
  rms: number;
}

interface State {
  starting?: boolean;
  source?: Source;
  format?: string | null;
  video?: { waveform: string; vector: string; luma: number; black: boolean; gamut: number } | null;
  frozen?: number | null;
  audio?: {
    levels: Level[];
    loudness: { momentary?: number | null; shortTerm?: number | null; integrated?: number | null; lra?: number | null; truePeak?: number | null; channels: number[] };
    phase?: number | null;
    silent: boolean[];
    clipping: boolean[];
  } | null;
  lipsync?: { offset?: number | null; history: number[]; detected: boolean };
  data?: { timecode?: string | null; captions: boolean; afd?: number | null; scte104?: string | null };
}

const KIND: Record<Source['kind'], string> = { sdi: 'SDI', '2110': 'ST 2110', audio: 'Sound only', stream: 'Network stream', ndi: 'NDI', mxl: 'MXL flow' };
const enc = encodeURIComponent;
const num = (v: number | null | undefined, digits = 1) => (v == null || !Number.isFinite(v) ? '—' : v.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits }));

export function Analyzer() {
  const list = usePoll<{ sources: Source[]; mxl: boolean }>('/api/analyze/sources', 10000);
  const [key, setKey] = useState<string>('');
  const sources = list?.sources ?? [];
  const src = sources.find((s) => s.key === key) ?? sources[0];
  const st = usePoll<State>(src ? `/api/analyze/state?source=${enc(src.key)}` : null, 250);
  const [solo, setSolo] = useState<number[]>([]);
  useEffect(() => setSolo([]), [src?.key]);
  if (!list) return null;
  if (!list.mxl) return <p className="core-warn">{t('MXL is not available: the analyzer needs it.')}</p>;
  if (!src) return <Empty icon={Headphones} text={t('No source on this server')} />;
  const a = st?.audio;
  const v = st?.video;
  const live = !!v;
  const option = (s: Source) => ({ value: s.key, label: `${s.way === 'in' ? t('Received') : t('Sent')} · ${t(KIND[s.kind])} · ${s.label}` });
  const toggle = (c: number) => setSolo((cur) => (cur.includes(c) ? cur.filter((x) => x !== c) : [...cur, c].sort((x, y) => x - y)));
  return (
    <>
      <div className="core-an-source">
        <span className="core-an-label">{t('Source')}</span>
        <Dropdown value={src.key} options={sources.map(option)} onChange={setKey} />
        {st?.format && <Badge>{st.format}</Badge>}
        {src.video && (live ? <Badge tone="ok">{t('Picture')}</Badge> : <Badge tone="warn">{t('No picture')}</Badge>)}
        {src.audio.length > 0 && (a ? <Badge tone="ok">{t('{n} audio channels', { n: a.levels.length })}</Badge> : <Badge tone="warn">{t('No sound')}</Badge>)}
      </div>
      <div className="core-an">
        <div className="core-an-main">
          {src.video && (
            <Card title={t('Picture')}>
              <div className="core-an-pic">
                <Preview src={`/api/analyze/preview.jpg?source=${enc(src.key)}`} on={live} empty={t('No picture')}>
                  {st?.data?.timecode && <span className="core-input-tc">{st.data.timecode}</span>}
                </Preview>
              </div>
            </Card>
          )}
          {src.video && (
            <div className="core-an-scopes">
              <Card title={t('Waveform (luma)')}>
                <Scope data={v?.waveform} w={96} h={64} columns />
              </Card>
              <Card title={t('Vectorscope')}>
                <Scope data={v?.vector} w={64} h={64} />
              </Card>
            </div>
          )}
          <Card title={t('Automatic checks')}>
            <div className="core-an-checks">
              {src.video && <Check label={t('Black')} bad={!!v?.black} />}
              {src.video && <Check label={t('Frozen picture')} bad={st?.frozen != null} detail={st?.frozen != null ? t('for {n} s', { n: Math.round(st.frozen) }) : undefined} />}
              {src.video && <Check label={t('Out of gamut')} bad={(v?.gamut ?? 0) > 1} detail={v ? `${num(v.gamut)} %` : undefined} />}
              {a && <Check label={t('Silence')} bad={a.silent.some(Boolean)} detail={a.silent.some(Boolean) ? t('channels {list}', { list: a.silent.map((x, i) => (x ? i + 1 : 0)).filter(Boolean).join(', ') }) : undefined} />}
              {a && <Check label={t('Clipping')} bad={a.clipping.some(Boolean)} />}
            </div>
          </Card>
        </div>
        <div className="core-an-side">
          {src.audio.length > 0 && <Listen source={src.key} solo={solo} channels={a?.levels.length ?? 0} />}
          {a && (
            <Card title={t('Sound · {n} channels', { n: a.levels.length })}>
              <Meters levels={a.levels} solo={solo} onSolo={toggle} />
              {a.phase != null && (
                <div className="core-an-phase">
                  <span>{t('Phase 1-2')}</span>
                  <div className="core-an-phase-bar">
                    <span style={{ left: `${((a.phase + 1) / 2) * 100}%` }} />
                  </div>
                  <b>{a.phase >= 0 ? '+' : ''}{num(a.phase, 2)}</b>
                </div>
              )}
            </Card>
          )}
          {a && (
            <Card
              title={t('Loudness · EBU R128')}
              actions={
                <button type="button" className="st-btn small" onClick={() => void api('POST', `/api/analyze/reset?source=${enc(src.key)}`).catch((e) => console.warn(friendly(e)))}>
                  <RotateCcw size={13} /> {t('Reset')}
                </button>
              }
            >
              <div className="core-an-loud">
                <Big label={t('Momentary')} value={num(a.loudness.momentary)} unit="LUFS" />
                <Big label={t('Short term')} value={num(a.loudness.shortTerm)} unit="LUFS" />
                <Big label={t('Integrated')} value={num(a.loudness.integrated)} unit="LUFS" />
                <Big label={t('Range (LRA)')} value={num(a.loudness.lra)} unit="LU" small />
                <Big label={t('True peak')} value={num(a.loudness.truePeak)} unit="dBTP" small />
                <span className="st-muted core-an-note">{t('Channels {list}', { list: a.loudness.channels.join('-') })}</span>
              </div>
            </Card>
          )}
          {src.video && src.audio.length > 0 && (
            <Card title={t('Lip sync')} actions={st?.lipsync?.detected ? <Badge tone="ok">{t('Test pattern seen')}</Badge> : <Badge>{t('Waiting for the test pattern')}</Badge>}>
              <div className="core-an-lip">
                <div>
                  <span className="core-an-lipv">{st?.lipsync?.offset != null ? `${st.lipsync.offset >= 0 ? '+' : ''}${num(st.lipsync.offset)} ms` : '—'}</span>
                  <span className="st-muted">{t('sound against the picture')}</span>
                </div>
                <Spark values={st?.lipsync?.history ?? []} />
              </div>
              <p className="st-muted core-an-note">{t('Measured on the Lip sync test pattern of the generator (white flash and cut tone), read like an app reads it.')}</p>
            </Card>
          )}
          <Card title={t('Data')}>
            <dl className="core-an-dl">
              <dt>{t('Timecode')}</dt>
              <dd>{st?.data?.timecode ?? '—'}</dd>
              <dt>{t('Format')}</dt>
              <dd>{st?.format ?? '—'}</dd>
              {src.data && (
                <>
                  <dt>{t('Captions')}</dt>
                  <dd>{st?.data?.captions ? t('present') : t('none')}</dd>
                  <dt>AFD</dt>
                  <dd>{st?.data?.afd ?? '—'}</dd>
                  <dt>SCTE-104</dt>
                  <dd>{st?.data?.scte104 ?? '—'}</dd>
                </>
              )}
            </dl>
          </Card>
        </div>
      </div>
    </>
  );
}

function Check({ label, bad, detail }: { label: string; bad: boolean; detail?: string }) {
  return (
    <div className="core-an-check">
      <span className="st-muted">{label}</span>
      <Badge tone={bad ? 'warn' : 'ok'}>{detail ?? (bad ? t('Yes') : t('No'))}</Badge>
    </div>
  );
}

function Big({ label, value, unit, small }: { label: string; value: string; unit: string; small?: boolean }) {
  return (
    <div className="core-an-big">
      <span className="st-muted">{label}</span>
      <b className={small ? 'small' : undefined}>{value}</b>
      <span className="st-muted">{unit}</span>
    </div>
  );
}

/** Une forme d'onde ou un vecteurscope : une grille d'intensités (0 à 255) en base64. */
function Scope({ data, w, h, columns }: { data?: string; w: number; h: number; columns?: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = ref.current;
    const ctx = c?.getContext('2d');
    if (!c || !ctx) return;
    ctx.fillStyle = '#0b0c12';
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.strokeStyle = 'rgba(255,255,255,0.08)';
    if (columns) {
      for (const p of [0, 0.25, 0.5, 0.75, 1]) {
        const y = c.height - p * c.height;
        ctx.beginPath();
        ctx.moveTo(0, y);
        ctx.lineTo(c.width, y);
        ctx.stroke();
      }
    } else {
      ctx.beginPath();
      ctx.arc(c.width / 2, c.height / 2, c.width * 0.45, 0, Math.PI * 2);
      ctx.moveTo(c.width / 2, 0);
      ctx.lineTo(c.width / 2, c.height);
      ctx.moveTo(0, c.height / 2);
      ctx.lineTo(c.width, c.height / 2);
      ctx.stroke();
    }
    if (!data) return;
    const bytes = Uint8Array.from(atob(data), (ch) => ch.charCodeAt(0));
    const cw = c.width / w;
    const chh = c.height / h;
    for (let i = 0; i < bytes.length; i++) {
      const v = bytes[i];
      if (!v) continue;
      // Forme d'onde : colonne par colonne, niveaux du bas vers le haut. Vecteurscope : ligne par ligne.
      const [x, y] = columns ? [Math.floor(i / h), h - 1 - (i % h)] : [i % w, Math.floor(i / w)];
      ctx.fillStyle = `rgba(74, 222, 128, ${v / 255})`;
      ctx.fillRect(x * cw, y * chh, Math.ceil(cw), Math.ceil(chh));
    }
  }, [data, w, h, columns]);
  return <canvas ref={ref} className={columns ? 'core-an-wave' : 'core-an-vec'} width={columns ? 384 : 192} height={192} role="img" aria-label={columns ? t('Waveform (luma)') : t('Vectorscope')} />;
}

/** −60 à 0 dBFS en hauteur (%). */
const pct = (db: number) => Math.max(0, Math.min(100, ((db + 60) / 60) * 100));

/** Les vumètres, crête maintenue 2 s, avec le casque de chaque canal. */
function Meters({ levels, solo, onSolo }: { levels: Level[]; solo: number[]; onSolo: (c: number) => void }) {
  const hold = useRef<{ v: number; at: number }[]>([]);
  const now = Date.now();
  levels.forEach((l, i) => {
    const h = hold.current[i];
    if (!h || l.peak >= h.v || now - h.at > 2000) hold.current[i] = { v: l.peak, at: now };
  });
  return (
    <div className="core-an-meters" style={{ gridTemplateColumns: `28px repeat(${levels.length}, minmax(0, 1fr))` }}>
      <div className="core-an-scale">
        {[0, -9, -18, -27, -40, -60].map((d) => (
          <span key={d} style={{ bottom: `${pct(d)}%` }}>
            {d}
          </span>
        ))}
      </div>
      {levels.map((l, i) => (
        <div key={i} className="core-an-ch">
          <div className="core-an-bar" title={`${num(l.peak)} dBFS`}>
            <div className="core-an-fill" style={{ height: `${pct(l.peak)}%` }} />
            <div className="core-an-hold" style={{ bottom: `${pct(hold.current[i]?.v ?? -120)}%` }} />
          </div>
          <span>{i + 1}</span>
          <button
            type="button"
            className={`core-an-solo${solo.includes(i + 1) ? ' on' : ''}`}
            aria-pressed={solo.includes(i + 1)}
            title={solo.includes(i + 1) ? t('Back to the mix') : t('Hear alone in the headphones')}
            aria-label={t('Channel {n}: hear alone in the headphones', { n: i + 1 })}
            onClick={() => onSolo(i + 1)}
          >
            <Headphones size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}

function Spark({ values }: { values: number[] }) {
  if (values.length < 2) return <svg className="core-an-spark" viewBox="0 0 160 48" aria-hidden="true" />;
  const max = Math.max(5, ...values.map((x) => Math.abs(x)));
  const pts = values.map((x, i) => `${(i / (values.length - 1)) * 160},${24 - (x / max) * 20}`).join(' ');
  return (
    <svg className="core-an-spark" viewBox="0 0 160 48" role="img" aria-label={t('Lip sync, last measures')}>
      <path d="M0 24H160" stroke="var(--line-2)" strokeDasharray="3 3" />
      <polyline points={pts} fill="none" stroke="var(--accent)" strokeWidth="2" />
    </svg>
  );
}

/** L'écoute : le PCM du Core joué par un AudioWorklet ; un solo changé relance le flux. */
function Listen({ source, solo, channels }: { source: string; solo: number[]; channels: number }) {
  const [on, setOn] = useState(false);
  const [vol, setVol] = useState(0.7);
  const ctx = useRef<{ ac: AudioContext; node: AudioWorkletNode; gain: GainNode } | null>(null);
  const abort = useRef<AbortController | null>(null);
  useEffect(() => {
    if (ctx.current) ctx.current.gain.gain.value = vol;
  }, [vol]);
  useEffect(() => {
    if (!on) return;
    let stopped = false;
    const ac0 = new AudioContext({ sampleRate: 48000 });
    const start = async () => {
      await ac0.audioWorklet.addModule('/pcm-player.js');
      const node = new AudioWorkletNode(ac0, 'pcm-player', { outputChannelCount: [2] });
      const gain = ac0.createGain();
      gain.gain.value = vol;
      node.connect(gain).connect(ac0.destination);
      ctx.current = { ac: ac0, node, gain };
      const ctl = new AbortController();
      abort.current = ctl;
      const res = await fetch(`/api/analyze/listen?source=${enc(source)}&solo=${solo.join(',')}`, { credentials: 'same-origin', signal: ctl.signal });
      const reader = res.body?.getReader();
      let rest = new Uint8Array(0);
      while (reader && !stopped) {
        const { value, done } = await reader.read();
        if (done || !value) break;
        // Des échantillons entiers (4 octets : gauche et droite en 16 bits).
        const all = new Uint8Array(rest.length + value.length);
        all.set(rest);
        all.set(value, rest.length);
        const n = all.length - (all.length % 4);
        const chunk = all.slice(0, n);
        node.port.postMessage(chunk.buffer, [chunk.buffer]);
        rest = all.slice(n);
      }
    };
    start().catch(() => {});
    return () => {
      stopped = true;
      abort.current?.abort();
      void ac0.close();
      ctx.current = null;
    };
    // `vol` vit dans son propre effet.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [on, source, solo.join(',')]);
  useEffect(() => setOn(false), [source]);
  const what = solo.length === 0 ? t('Mix of the {n} channels', { n: channels }) : solo.length === 1 ? t('Channel {n} alone', { n: solo[0] }) : t('Channels {list}', { list: solo.join(', ') });
  return (
    <Card title={t('Listen in the browser')} icon={<Headphones size={16} />}>
      <div className="core-an-listen">
        <button type="button" className="st-btn primary" onClick={() => setOn(!on)}>
          {on ? <Square size={14} /> : <Play size={14} />} {on ? t('Stop') : t('Listen')}
        </button>
        <Badge tone={solo.length ? 'warn' : undefined}>{what}</Badge>
        <label className="core-an-vol">
          {t('Volume')}
          <input type="range" min={0} max={1} step={0.01} value={vol} onChange={(e) => setVol(Number(e.target.value))} />
        </label>
      </div>
      <p className="st-muted core-an-note">{t('No solo: the mix of all the channels. The headphones on one or several channels: only those. The sound comes from the Core; the source is not touched.')}</p>
    </Card>
  );
}
