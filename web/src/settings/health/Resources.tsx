// Réglages › Santé › Ressources : processeurs, mémoire, disques, réseau, ce que prend chaque
// programme, ce qui reste, et les 10 dernières minutes. Relu toutes les 2 s.
import { useEffect, useState } from 'react';
import { Activity, Cpu, HardDrive, Network } from 'lucide-react';
import { Badge, Card, PageHead, formatBytes, formatDuration, locale, t } from '@strata/ui';
import { api } from '../../api';
import { LineChart } from './LineChart';

interface Use {
  id: string;
  cores: number;
  memory: number;
  processes: number;
}

interface Now {
  cpu: number;
  perCore: number[];
  load: [number, number, number];
  memUsed: number;
  memTotal: number;
  memAvailable: number;
  swapUsed: number;
  swapTotal: number;
  disks: { mount: string; fs: string; total: number; free: number; data: boolean }[];
  nets: { name: string; rx: number; tx: number }[];
  uses: Use[];
}

interface Res {
  now: Now | null;
  history: { t: number; cpu: number; mem: number; rx: number; tx: number; apps: Record<string, number> }[];
  names: Record<string, string>;
  machine: { host?: string; os: string; kernel?: string; cpu?: string; cores: number; uptimeSecs: number; coreUptimeSecs: number };
}

/** Série 1 et 2 de la palette catégorielle (sombre). */
const SERIES = ['#338ced', '#e76624'];
const ACCENT = 'var(--accent)';

export function bits(v: number): string {
  if (v >= 1e9) return `${num(v / 1e9, 1)} Gb/s`;
  if (v >= 1e6) return `${num(v / 1e6, 1)} Mb/s`;
  if (v >= 1e3) return `${Math.round(v / 1e3)} kb/s`;
  return `${Math.round(v)} b/s`;
}

const pct = (v: number) => `${Math.round(v)} %`;
/** Un nombre à `d` décimales, dans la langue de l'interface (3,8). */
const num = (v: number, d: number) => v.toLocaleString(locale(), { minimumFractionDigits: d, maximumFractionDigits: d });

/** Une barre de remplissage (couleur d'état seulement au-delà des seuils, avec le texte à côté). */
function Meter({ value, warn = 80, bad = 95 }: { value: number; warn?: number; bad?: number }) {
  const tone = value >= bad ? 'bad' : value >= warn ? 'warn' : '';
  return (
    <span className={`core-meter ${tone}`}>
      <i style={{ width: `${Math.min(100, Math.max(0, value))}%` }} />
    </span>
  );
}

export function ResourcesPage() {
  const [r, setR] = useState<Res | null>(null);
  useEffect(() => {
    let gone = false;
    const read = () =>
      api<Res>('GET', '/api/health/resources')
        .then((v) => !gone && setR(v))
        .catch(() => {});
    void read();
    const id = setInterval(read, 2000);
    return () => {
      gone = true;
      clearInterval(id);
    };
  }, []);
  if (!r?.now) return <PageHead title={t('Resources')} />;
  const { now, machine, history } = r;
  const times = history.map((p) => p.t);
  const memPct = now.memTotal ? (now.memUsed / now.memTotal) * 100 : 0;
  const freeCores = Math.max(0, machine.cores * (1 - now.cpu / 100));
  const name = (id: string) => (id === 'core' ? 'Strata Core' : (r.names[id] ?? id));
  const dataDisk = now.disks.find((d) => d.data);
  const rx = now.nets.reduce((n, x) => n + x.rx, 0);
  const tx = now.nets.reduce((n, x) => n + x.tx, 0);
  return (
    <>
      <PageHead
        title={t('Resources')}
        sub={[machine.cpu ? `${machine.cpu} × ${machine.cores}` : null, machine.os, t('machine up for {time}', { time: formatDuration(machine.uptimeSecs) })].filter(Boolean).join(' · ')}
      />

      <div className="core-tiles">
        <div className="core-tile">
          <span>{t('Processors')}</span>
          <strong>{pct(now.cpu)}</strong>
          <span className="st-muted">{t('{n} cores free', { n: num(freeCores, 1) })}</span>
        </div>
        <div className="core-tile">
          <span>{t('Memory')}</span>
          <strong>{pct(memPct)}</strong>
          <span className="st-muted">{t('{free} available of {total}', { free: formatBytes(now.memAvailable), total: formatBytes(now.memTotal) })}</span>
        </div>
        <div className="core-tile">
          <span>{t('Data disk')}</span>
          <strong>{dataDisk ? pct(((dataDisk.total - dataDisk.free) / dataDisk.total) * 100) : '—'}</strong>
          <span className="st-muted">{dataDisk ? t('{free} free', { free: formatBytes(dataDisk.free) }) : ''}</span>
        </div>
        <div className="core-tile">
          <span>{t('Network')}</span>
          <strong className="core-tile-small">↓ {bits(rx)}</strong>
          <span className="st-muted">↑ {bits(tx)}</span>
        </div>
      </div>

      <Card icon={<Activity size={16} />} title={t('Last 10 minutes')}>
        <div className="core-charts">
          <LineChart label={t('Processors')} times={times} max={100} format={pct} series={[{ name: t('Processors'), color: ACCENT, values: history.map((p) => p.cpu) }]} />
          <LineChart label={t('Memory')} times={times} max={100} format={pct} series={[{ name: t('Memory'), color: ACCENT, values: history.map((p) => p.mem) }]} />
          <LineChart
            label={t('Network')}
            times={times}
            format={bits}
            series={[
              { name: t('Received'), color: SERIES[0], values: history.map((p) => p.rx) },
              { name: t('Sent'), color: SERIES[1], values: history.map((p) => p.tx) },
            ]}
          />
        </div>
      </Card>

      <Card icon={<Cpu size={16} />} title={t('What each program takes')} actions={<span className="st-muted">{t('{cores} cores and {mem} still free', { cores: num(freeCores, 1), mem: formatBytes(now.memAvailable) })}</span>}>
        <table className="core-table">
          <thead>
            <tr>
              <th>{t('Program')}</th>
              <th>{t('Processors')}</th>
              <th className="num">{t('Memory')}</th>
              <th className="num">{t('Processes')}</th>
            </tr>
          </thead>
          <tbody>
            {now.uses.map((u) => (
              <tr key={u.id}>
                <td>{name(u.id)}</td>
                <td>
                  <span className="core-use">
                    <Meter value={(u.cores / machine.cores) * 100} warn={101} bad={101} />
                    <span className="st-mono">{t('{n} cores', { n: num(u.cores, 2) })}</span>
                  </span>
                </td>
                <td className="num st-mono">{formatBytes(u.memory)}</td>
                <td className="num st-mono">{u.processes}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <div className="core-cores" aria-label={t('Each core')}>
          {now.perCore.map((c, i) => (
            <span key={i} title={`${t('Core {n}', { n: i })} : ${pct(c)}`}>
              <i style={{ height: `${Math.max(2, c)}%` }} />
            </span>
          ))}
        </div>
        <p className="st-muted core-note">{t('Load: {a} · {b} · {c} (1, 5, 15 min)', { a: num(now.load[0], 2), b: num(now.load[1], 2), c: num(now.load[2], 2) })}</p>
      </Card>

      <div className="core-cols">
        <Card icon={<HardDrive size={16} />} title={t('Disks')}>
          <div className="core-lines">
            {now.disks.map((d) => {
              const used = d.total ? ((d.total - d.free) / d.total) * 100 : 0;
              return (
                <div key={d.mount} className="core-line">
                  <span>
                    <span className="st-mono">{d.mount}</span>
                    {d.data && <Badge>{t('data')}</Badge>}
                  </span>
                  <span className="core-use">
                    <Meter value={used} />
                    <span className="st-muted st-mono">{t('{free} free of {total}', { free: formatBytes(d.free), total: formatBytes(d.total) })}</span>
                  </span>
                </div>
              );
            })}
            {now.swapTotal > 0 && (
              <div className="core-line">
                <span>{t('Swap')}</span>
                <span className="st-muted st-mono">{t('{used} used of {total}', { used: formatBytes(now.swapUsed), total: formatBytes(now.swapTotal) })}</span>
              </div>
            )}
          </div>
        </Card>
        <Card icon={<Network size={16} />} title={t('Network cards')}>
          <div className="core-lines">
            {now.nets.map((n) => (
              <div key={n.name} className="core-line">
                <span className="st-mono">{n.name}</span>
                <span className="st-muted st-mono">
                  ↓ {bits(n.rx)} · ↑ {bits(n.tx)}
                </span>
              </div>
            ))}
          </div>
        </Card>
      </div>
    </>
  );
}
