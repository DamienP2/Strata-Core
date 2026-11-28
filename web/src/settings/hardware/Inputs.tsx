// Les images des réglages du matériel : entrées et sorties SDI, flux ST 2110 vidéo. Une petite
// image par seconde, demandée seulement quand la vignette est à l'écran et l'onglet visible : le
// serveur ne fabrique rien (et ne reçoit aucun flux) pour une page fermée ou cachée.
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Cable, MonitorUp, Waves } from 'lucide-react';
import { Badge, Card, t, tn } from '@strata/ui';
import { usePageVisible, usePoll } from '../../poll';
import { friendly } from '../../errors';
import type { Flow, HwConfig } from './store';

export interface InputStatus {
  card: string;
  connector: number;
  simulated: boolean;
  state: 'opening' | 'capturing' | 'error';
  error?: string | null;
  signal?: boolean | null;
  format?: string | null;
  fps: number;
  frames: number;
  timecode?: string | null;
  readers: { app: string; pid: number; name?: string }[];
  share: string;
  /** Le flux MXL de l'entrée (ce que lisent les applis), quand MXL est là. */
  mxlFlow?: string | null;
}

/** Ce qu'une appli envoie sur une sortie (SDI ou flux ST 2110 sortant). */
interface Sent {
  sending: boolean;
  share: string;
  app?: string | null;
  appName?: string | null;
  format?: string | null;
  timecode?: string | null;
}

interface OutputStatus extends Sent {
  card: string;
  connector: number;
  /** Avec MXL, le Core tient la sortie : ce qui part sur le câble. */
  playout?: { state: 'opening' | 'playing' | 'black' | 'error'; format?: string | null; error?: string | null; simulated: boolean; audio?: boolean } | null;
}

interface FlowStatus extends Partial<Sent> {
  id: string;
  /** Tenu par le Core (MTL) : ce qui arrive ou part vraiment. */
  io?: { state: 'opening' | 'running' | 'idle' | 'error'; format?: string | null; frames: number; packets: number; lost: number; error?: string | null } | null;
  /** Un flux entrant regardé : la réception de l'aperçu. */
  reception?: { receiving: boolean; leg?: string | null; format?: string | null; error?: string | null } | null;
}

/** Une image par seconde de `src`, seulement quand la vignette est à l'écran et l'onglet visible. */
export function Preview({ src, on, empty, children }: { src: string; on: boolean; empty: string; children?: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  const [seen, setSeen] = useState(false);
  const visible = usePageVisible();
  const [img, setImg] = useState<string | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const io = new IntersectionObserver(([e]) => setSeen(e.isIntersecting));
    io.observe(el);
    return () => io.disconnect();
  }, []);
  useEffect(() => {
    if (!on) setImg(null);
    if (!on || !seen || !visible) return;
    let gone = false;
    let n = 0;
    // La réponse attendue : une image en retard (d'une lecture précédente) est ignorée.
    let seq = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let im: HTMLImageElement | null = null;
    // L'image suivante est demandée une fois la précédente arrivée (ou ratée), au moins 1 s après
    // le début de la précédente : jamais d'images qui s'empilent sur un serveur lent.
    const load = () => {
      const mine = ++seq;
      const start = Date.now();
      const url = `${src}?n=${start}-${n++}`;
      const next = () => {
        if (gone || mine !== seq) return;
        timer = setTimeout(load, Math.max(0, 1000 - (Date.now() - start)));
      };
      im = new Image();
      im.onload = () => {
        if (gone || mine !== seq) return;
        setImg(url);
        next();
      };
      im.onerror = () => {
        if (gone || mine !== seq) return;
        setImg(null);
        next();
      };
      im.src = url;
    };
    load();
    return () => {
      gone = true;
      clearTimeout(timer);
      if (im) {
        im.onload = im.onerror = null;
        im.src = '';
      }
    };
  }, [src, on, seen, visible]);
  return (
    <div className="core-input-pic" ref={box}>
      {img ? <img src={img} alt="" /> : <span>{empty}</span>}
      {img && children}
    </div>
  );
}

function Apps({ label, none, names }: { label: string; none: string; names: string[] }) {
  return (
    <div className="core-input-readers">
      {names.length === 0 ? (
        <span className="st-muted">{none}</span>
      ) : (
        <>
          <span className="st-muted">{label}</span>
          {names.map((a) => (
            <span key={a} className="core-input-app">
              {a}
            </span>
          ))}
        </>
      )}
    </div>
  );
}

export function Tile({ head, sub, pic, error, foot }: { head: ReactNode; sub?: string; pic: ReactNode; error?: string | null; foot: ReactNode }) {
  return (
    <div className="core-input">
      {pic}
      <div className="core-input-head">{head}</div>
      {sub && <div className="st-muted core-input-card">{sub}</div>}
      {error && <div className="core-warn">{friendly(error)}</div>}
      {foot}
    </div>
  );
}

function Tc({ tc }: { tc?: string | null }) {
  return tc ? <span className="core-input-tc">{tc}</span> : null;
}

const enc = encodeURIComponent;

// ---------- SDI ----------

function wireOf(cfg: HwConfig, card: string, n: number) {
  const c = cfg.cards.find((x) => x.id === card);
  return { wire: c?.connectors[n - 1]?.name, card: c?.name || c?.modelName || '' };
}

function InputTile({ i, cfg }: { i: InputStatus; cfg: HwConfig }) {
  const { wire, card } = wireOf(cfg, i.card, i.connector);
  const live = i.state === 'capturing' && i.signal !== false && i.frames > 0;
  const status =
    i.state === 'error' ? (
      <Badge tone="bad">{t('Error')}</Badge>
    ) : i.signal === false ? (
      <Badge tone="warn">{t('No signal')}</Badge>
    ) : i.state === 'opening' || i.signal == null ? (
      <Badge>{t('Waiting for a signal')}</Badge>
    ) : (
      <Badge tone="ok">{[i.format, i.fps ? `${Math.round(i.fps * 100) / 100} ${t('fps')}` : ''].filter(Boolean).join(' · ')}</Badge>
    );
  return (
    <Tile
      pic={
        <Preview src={`/api/hardware/inputs/${enc(i.card)}/${i.connector}/preview.jpg`} on={live} empty={i.state === 'error' ? t('Error') : t('No picture')}>
          <Tc tc={i.timecode} />
          {i.simulated && <span className="core-input-sim">{t('test pattern')}</span>}
        </Preview>
      }
      head={
        <>
          <strong>SDI {i.connector}</strong>
          <span className="core-input-wire">{wire || card}</span>
          {status}
        </>
      }
      sub={wire ? card : undefined}
      error={i.error}
      foot={
        i.mxlFlow ? (
          <div className="core-input-readers">
            <span className="core-input-app">MXL</span>
            <span className="st-muted st-mono core-input-flow" title={i.mxlFlow}>
              {i.mxlFlow}
            </span>
          </div>
        ) : (
          <Apps label={t('Read by')} none={t('No app reads it')} names={[...new Set(i.readers.map((r) => r.name || r.app || t('a program')))]} />
        )
      }
    />
  );
}

/** Les entrées partagées (rien quand aucun connecteur n'est réglé en entrée). */
export function InputsCard({ cfg }: { cfg: HwConfig }) {
  const r = usePoll<{ inputs: InputStatus[] }>('/api/hardware/inputs');
  if (!r || r.inputs.length === 0) return null;
  return (
    <Card icon={<Cable size={16} />} title={t('Shared inputs')}>
      <div className="core-inputs">
        {r.inputs.map((i) => (
          <InputTile key={`${i.card}-${i.connector}`} i={i} cfg={cfg} />
        ))}
      </div>
    </Card>
  );
}

function sentBadge(s: Partial<Sent>) {
  return s.sending ? <Badge tone="ok">{s.format || t('Sending')}</Badge> : <Badge>{t('Nothing sent')}</Badge>;
}

function sentApps(s: Partial<Sent>) {
  return <Apps label={t('Sent by')} none={s.sending ? t('a program') : t('No app sends to it')} names={s.sending && (s.appName || s.app) ? [s.appName || s.app || ''] : []} />;
}

/** Les sorties SDI : ce que les applis y envoient. */
export function OutputsCard({ cfg }: { cfg: HwConfig }) {
  const r = usePoll<{ outputs: OutputStatus[] }>('/api/hardware/outputs');
  if (!r || r.outputs.length === 0) return null;
  return (
    <Card icon={<MonitorUp size={16} />} title={t('Outputs')}>
      <div className="core-inputs">
        {r.outputs.map((o) => {
          const { wire, card } = wireOf(cfg, o.card, o.connector);
          return (
            <Tile
              key={`${o.card}-${o.connector}`}
              pic={
                <Preview src={`/api/hardware/outputs/${enc(o.card)}/${o.connector}/preview.jpg`} on={o.sending} empty={o.sending ? t('No picture') : t('Nothing sent')}>
                  <Tc tc={o.timecode} />
                </Preview>
              }
              head={
                <>
                  <strong>SDI {o.connector}</strong>
                  <span className="core-input-wire">{wire || card}</span>
                  {sentBadge(o)}
                </>
              }
              sub={[wire ? card : '', o.playout?.state === 'black' ? t('black on the output') : '', o.playout?.state === 'playing' ? (o.playout.audio ? t('with sound') : t('no sound')) : ''].filter(Boolean).join(' · ') || undefined}
              error={o.playout?.error}
              foot={sentApps(o)}
            />
          );
        })}
      </div>
    </Card>
  );
}

// ---------- ST 2110 ----------

const LEG: Record<string, string> = { red: 'red network', blue: 'blue network', port1: 'port 1', port2: 'port 2' };

/** Les flux vidéo ST 2110 : reçus (le Core les rejoint seulement pendant qu'on regarde) ou envoyés. */
type FlowsState = { flows: FlowStatus[]; io?: { active: boolean; problem?: string | null } };

/** `state` : déjà relu par la page (une seule lecture par seconde pour les deux). */
export function FlowsCard({ flows, state }: { flows: Flow[]; state?: FlowsState | null }) {
  const video = flows.filter((f) => f.essence === 'video' && f.id);
  const polled = usePoll<FlowsState>(state === undefined ? '/api/hardware/flows' : null);
  const r = state === undefined ? polled : state;
  if (video.length === 0) return null;
  const engine = r?.io;
  const st = (id: string) => r?.flows.find((x) => x.id === id);
  return (
    <Card
      icon={<Waves size={16} />}
      title={t('Video flows')}
    >
      {engine?.problem && <p className="core-warn">{friendly(engine.problem)}</p>}
      <div className="core-inputs">
        {video.map((f) => {
          const s = st(f.id);
          const src = `/api/hardware/flows/${enc(f.id)}/preview.jpg`;
          if (engine?.active) {
            const io = s?.io;
            const badge = !io ? (
              <Badge>{t('Opening…')}</Badge>
            ) : io.state === 'error' ? (
              <Badge tone="bad">{t('Error')}</Badge>
            ) : io.state === 'running' ? (
              <Badge tone="ok">{io.format || t('Receiving')}</Badge>
            ) : (
              <Badge tone="warn">{f.direction === 'input' ? t('Nothing received') : t('black on the output')}</Badge>
            );
            const stats = f.direction === 'input' && io ? tn(io.lost, '{n} packet lost', '{n} packets lost') : '';
            return (
              <Tile
                key={f.id}
                pic={<Preview src={src} on={io?.state === 'running'} empty={t('No picture')} />}
                head={
                  <>
                    <strong className="core-input-wire">{f.name}</strong>
                    {badge}
                  </>
                }
                sub={[f.direction === 'input' ? t('Received by the Core') : t('Sent by the Core'), f.red, stats].filter(Boolean).join(' · ')}
                error={io?.error}
                foot={null}
              />
            );
          }
          if (f.direction === 'input') {
            const rx = s?.reception;
            const badge = rx?.error ? (
              <Badge tone="bad">{t('Error')}</Badge>
            ) : !rx ? (
              <Badge>{t('Joining…')}</Badge>
            ) : rx.receiving ? (
              <Badge tone="ok">{rx.format || t('Receiving')}</Badge>
            ) : (
              <Badge tone="warn">{t('Nothing received')}</Badge>
            );
            return (
              <Tile
                key={f.id}
                pic={<Preview src={src} on empty={rx && !rx.receiving ? t('Nothing received') : t('No picture')} />}
                head={
                  <>
                    <strong className="core-input-wire">{f.name}</strong>
                    {badge}
                  </>
                }
                sub={[t('Received'), rx?.leg ? t(LEG[rx.leg] ?? rx.leg) : '', f.red].filter(Boolean).join(' · ')}
                error={rx?.error}
                foot={null}
              />
            );
          }
          return (
            <Tile
              key={f.id}
              pic={
                <Preview src={src} on={!!s?.sending} empty={s?.sending ? t('No picture') : t('Nothing sent')}>
                  <Tc tc={s?.timecode} />
                </Preview>
              }
              head={
                <>
                  <strong className="core-input-wire">{f.name}</strong>
                  {sentBadge(s ?? {})}
                </>
              }
              sub={[t('Sent'), f.red].join(' · ')}
              foot={sentApps(s ?? {})}
            />
          );
        })}
      </div>
    </Card>
  );
}
