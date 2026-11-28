// Une petite courbe des 10 dernières minutes : une ou deux séries, une seule échelle, un repère
// vertical qui suit le pointeur et donne les valeurs de toutes les séries à cet instant.
import { useRef, useState } from 'react';
import { formatTime } from '@strata/ui';

export interface Series {
  name: string;
  /** Une couleur de série (jamais une couleur d'état). */
  color: string;
  values: number[];
}

/** Le haut de l'échelle : 1, 2 ou 5 × 10ⁿ. */
function nice(v: number): number {
  const p = 10 ** Math.floor(Math.log10(v));
  return ([1, 2, 5, 10].find((m) => m * p >= v) ?? 10) * p;
}

const W = 600;
const H = 120;
const PAD = { l: 44, r: 8, t: 8, b: 18 };

export function LineChart({ times, series, max, format, label }: { times: number[]; series: Series[]; max?: number; format: (v: number) => string; label: string }) {
  const [hover, setHover] = useState<number | null>(null);
  const box = useRef<SVGSVGElement>(null);
  const n = times.length;
  const top = max ?? nice(Math.max(1, ...series.flatMap((s) => s.values)) * 1.1);
  const x = (i: number) => PAD.l + (n <= 1 ? 0 : (i / (n - 1)) * (W - PAD.l - PAD.r));
  const y = (v: number) => PAD.t + (1 - Math.min(v, top) / top) * (H - PAD.t - PAD.b);
  const path = (vals: number[]) => vals.map((v, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join('');
  const ticks = [0, top / 2, top];
  const move = (e: React.PointerEvent) => {
    const r = box.current?.getBoundingClientRect();
    if (!r || n < 2) return;
    const px = ((e.clientX - r.left) / r.width) * W;
    setHover(Math.max(0, Math.min(n - 1, Math.round(((px - PAD.l) / (W - PAD.l - PAD.r)) * (n - 1)))));
  };
  return (
    <div className="core-chart">
      <div className="core-chart-head">
        <span>{label}</span>
        {series.length > 1 && (
          <span className="core-chart-legend">
            {series.map((s) => (
              <span key={s.name}>
                <i style={{ background: s.color }} />
                {s.name}
              </span>
            ))}
          </span>
        )}
      </div>
      <svg ref={box} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" onPointerMove={move} onPointerLeave={() => setHover(null)} role="img" aria-label={label}>
        {ticks.map((v, i) => (
          <g key={i}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(v)} y2={y(v)} className="core-chart-grid" />
            <text x={PAD.l - 6} y={y(v) + 3} textAnchor="end" className="core-chart-tick">
              {format(v)}
            </text>
          </g>
        ))}
        {series.map((s) => (
          <path key={s.name} d={path(s.values)} fill="none" stroke={s.color} strokeWidth={2} vectorEffect="non-scaling-stroke" strokeLinejoin="round" />
        ))}
        {hover !== null && <line x1={x(hover)} x2={x(hover)} y1={PAD.t} y2={H - PAD.b} className="core-chart-cross" />}
      </svg>
      {hover !== null && (
        <div className="core-chart-tip" style={{ left: `${(x(hover) / W) * 100}%` }}>
          <div className="st-muted">{formatTime(times[hover])}</div>
          {series.map((s) => (
            <div key={s.name}>
              <i style={{ background: s.color }} />
              <strong>{format(s.values[hover])}</strong> {series.length > 1 && <span className="st-muted">{s.name}</span>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
