// Une carte PCIe en 3D réaliste (render3d.ts, three.js), selon son modèle, et par-dessus, pour
// chaque prise, un trait de rappel vers son nom tel que le logiciel le voit (SDI n, ens2f0), à la
// couleur de sa fonction ; un clic sur une prise (ou son nom) la choisit.
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { buildScene, type Built, type JackKind, type Look } from './render3d';

export type { JackKind, Look };

/** Une prise, de haut en bas de l'équerre. */
export interface Jack {
  /** Ce que renvoie un clic (−1 : pas cliquable, la REF). */
  k: number;
  /** Ce qui est imprimé sur l'équerre. */
  etched: string;
  /** Le nom au bout du trait (« 4 », « ens2f0 »). */
  label: string;
  /** Une seconde ligne (le rôle d'un port réseau). */
  sub?: string;
  /** La couleur de sa fonction ; vide : rien. */
  color: string;
  reference?: boolean;
}

/** La largeur d'un nom (px), pour les écarter. */
const textWidth = (j: Jack) => Math.max(j.label.length * 8.2, (j.sub ?? '').length * 6) + 14;

export function Pcie3D({ look, kind, jacks, sel, onPick, title }: {
  look: Look;
  kind: JackKind;
  jacks: Jack[];
  sel: number | null;
  onPick: (k: number) => void;
  title: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [built, setBuilt] = useState<Built | null>(null);
  const hasSub = jacks.some((j) => j.sub);
  const labelsHeight = hasSub ? 56 : 40;
  // La largeur disponible (le rendu est refait quand elle change de plus de 40 px).
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => setWidth((w) => {
      const now = Math.round(el.clientWidth);
      return Math.abs(now - w) > 40 ? now : w;
    });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // Le rendu : refait quand la carte, ses prises ou leurs couleurs changent.
  const key = JSON.stringify([look, kind, jacks.map((j) => [j.etched, j.color, !!j.reference])]);
  useEffect(() => {
    if (width < 100) return;
    let gone = false;
    let b: Built | null = null;
    // Après l'affichage : le rendu prend quelques dizaines de millisecondes.
    const id = requestAnimationFrame(() => {
      if (gone) return;
      try {
        b = buildScene(look, kind, jacks, width, labelsHeight);
        setBuilt(b);
      } catch (e) {
        console.warn('3D card', e);
      }
    });
    return () => {
      gone = true;
      cancelAnimationFrame(id);
      b?.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, width, labelsHeight]);

  // Les noms au bout des traits : sous la carte, écartés quand ils se toucheraient.
  const base = built ? built.height - labelsHeight + 8 : 0;
  const xs = built ? built.tips.map((t) => t.x) : [];
  for (let i = 1; i < xs.length; i++) {
    const gap = (textWidth(jacks[i - 1]) + textWidth(jacks[i])) / 2;
    if (xs[i] - xs[i - 1] < gap) xs[i] = xs[i - 1] + gap;
  }
  if (built && xs.length) {
    const shift = (built.tips.reduce((n, t) => n + t.x, 0) - xs.reduce((n, x) => n + x, 0)) / xs.length;
    for (let i = 0; i < xs.length; i++) xs[i] = Math.min(Math.max(xs[i] + shift, textWidth(jacks[i]) / 2), built.width - textWidth(jacks[i]) / 2);
  }
  return (
    <div ref={box} className="core-hw-card3d" style={built ? { height: built.height } : { aspectRatio: '1.6' }}>
      {built && (
        <>
          <img
            src={built.image}
            alt={title}
            width={built.width}
            height={built.height}
            draggable={false}
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              const k = built.pick(e.clientX - r.left, e.clientY - r.top);
              if (k >= 0) onPick(k);
            }}
          />
          <svg width={built.width} height={built.height} className="core-hw-leaders">
            {jacks.map((j, i) => {
              const t = built.tips[i];
              const c = j.color || 'var(--text-3)';
              const on = sel === j.k && j.k >= 0;
              return (
                <g key={`${j.k}-${i}`} className={`core-hw-leader ${on ? 'sel' : ''}`} onClick={() => j.k >= 0 && onPick(j.k)}>
                  <circle cx={t.x} cy={t.y} r={on ? 4 : 2.5} fill={c} />
                  {on && <circle cx={t.x} cy={t.y} r={9} fill="none" stroke="var(--text)" strokeWidth="1.2" strokeDasharray="3 2.5" />}
                  <polyline points={`${t.x},${t.y} ${t.x},${base - 12} ${xs[i]},${base - 2}`} fill="none" stroke={c} strokeWidth={on ? 2 : 1.2} strokeLinejoin="round" />
                  <text x={xs[i]} y={base + 12} textAnchor="middle" className="core-hw-num" fill={j.color || 'currentColor'}>
                    {j.label}
                  </text>
                  {j.sub && (
                    <text x={xs[i]} y={base + 28} textAnchor="middle" className="core-hw-sub">
                      {j.sub}
                    </text>
                  )}
                </g>
              );
            })}
          </svg>
        </>
      )}
    </div>
  );
}

// ---------- DeckLink ----------

/** Les couleurs des fonctions des connecteurs SDI. */
export const SDI_COLOR = { input: '#22c55e', output: '#8fa3ff', unused: '', ref: '#f5c542' } as const;

interface DeckLinkLook extends Look {
  /** Les numéros imprimés sur l'équerre, de haut en bas (sinon : les numéros logiciels). */
  printed?: number[];
}

const DECKLINK: Record<string, DeckLinkLook> = {
  // Imprimé 8 → 1 de haut en bas ; le logiciel (Desktop Video, vMix) : 8, 4, 7, 3, 6, 2, 5, 1.
  'decklink quad 2': { length: 168, fans: 2, lanes: 8, silk: 'DeckLink Quad 2', printed: [8, 7, 6, 5, 4, 3, 2, 1] },
  'decklink 8k pro': { length: 168, fans: 1, lanes: 8, silk: 'DeckLink 8K Pro' },
  'decklink duo 2': { length: 168, fans: 0, sink: true, lanes: 4, silk: 'DeckLink Duo 2' },
  'decklink sdi 4k': { length: 125, fans: 0, sink: true, lanes: 4, silk: 'DeckLink SDI 4K' },
  'decklink mini monitor': { length: 102, low: true, fans: 0, lanes: 1, hdmi: true, silk: 'DeckLink Mini Monitor' },
  'decklink mini monitor 4k': { length: 120, low: true, fans: 0, sink: true, lanes: 4, hdmi: true, silk: 'DeckLink Mini Monitor 4K' },
  'decklink mini recorder': { length: 102, low: true, fans: 0, lanes: 1, hdmi: true, silk: 'DeckLink Mini Recorder' },
  'decklink mini recorder 4k': { length: 120, low: true, fans: 0, sink: true, lanes: 4, hdmi: true, silk: 'DeckLink Mini Recorder 4K' },
};

export function decklinkLook(model: string, connectors: number): DeckLinkLook {
  return DECKLINK[model.trim().toLowerCase()] ?? { length: connectors > 2 ? 160 : 120, low: connectors <= 1, fans: connectors > 4 ? 1 : 0, sink: connectors > 1, lanes: 4, silk: model };
}

/** Le numéro imprimé d'un connecteur (SDI n), quand il diffère. */
export function printedOf(model: string, order: number[], n: number): number | null {
  const look = decklinkLook(model, order.length);
  const i = order.indexOf(n);
  const p = look.printed && look.printed.length === order.length && i >= 0 ? look.printed[i] : n;
  return p === n ? null : p;
}

export function DeckLink3D({ model, jack, order, reference, functions, genlock, sel, onPick }: {
  model: string;
  jack: 'din' | 'bnc';
  /** Les numéros logiciels (SDI n), de haut en bas de l'équerre. */
  order: number[];
  reference: boolean;
  functions: ('input' | 'output' | 'unused')[];
  genlock: boolean;
  sel: number | null;
  onPick: (k: number) => void;
}) {
  const look = decklinkLook(model, order.length);
  const printed = look.printed && look.printed.length === order.length ? look.printed : order;
  const jacks: Jack[] = order.map((n, i) => ({ k: n - 1, etched: String(printed[i]), label: String(n), color: SDI_COLOR[functions[n - 1] ?? 'unused'] }));
  if (reference) jacks.push({ k: -1, etched: 'REF', label: 'REF', color: genlock ? SDI_COLOR.ref : '', reference: true });
  return <Pcie3D look={look} kind={jack} jacks={jacks} sel={sel} onPick={onPick} title={model} />;
}
