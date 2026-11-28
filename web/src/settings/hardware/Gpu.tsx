// Réglages › Matériel › GPU : chaque GPU (NVIDIA, Intel, AMD), son pilote, son nœud de rendu, ce
// qu'il encode et décode, sa charge. Les applis lisent les mêmes informations pour choisir leur GPU.
import { useState } from 'react';
import { Gpu as GpuIcon, RefreshCw } from 'lucide-react';
import { Badge, Card, Empty, PageHead, formatBytes, t, tn } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import { usePoll } from '../../poll';

export interface GpuUsage {
  gpu: number;
  encoder: number;
  decoder: number;
  memoryUsedMb: number;
  temperature?: number | null;
}

export interface Gpu {
  pci: string;
  vendor: 'nvidia' | 'intel' | 'amd' | 'other';
  name: string;
  driver?: string | null;
  driverVersion?: string | null;
  renderNode?: string | null;
  memoryMb?: number | null;
  api?: 'nvenc' | 'vaapi' | null;
  encode: string[];
  decode: string[];
  ready: boolean;
  note?: string | null;
  usage?: GpuUsage | null;
}

const VENDOR: Record<Gpu['vendor'], string> = { nvidia: 'NVIDIA', intel: 'Intel', amd: 'AMD', other: '—' };
const API: Record<string, string> = { nvenc: 'NVENC / NVDEC', vaapi: 'VA-API' };

/** Une barre de charge, avec sa valeur. */
export function UseBar({ label, value, text }: { label: string; value: number; text?: string }) {
  return (
    <div className="core-line">
      <span>{label}</span>
      <span className="core-use">
        <span className="core-meter">
          <i style={{ width: `${Math.min(100, Math.max(0, value))}%` }} />
        </span>
        <span className="st-mono st-muted">{text ?? `${Math.round(value)} %`}</span>
      </span>
    </div>
  );
}

export function GpuCard({ g }: { g: Gpu }) {
  const line = (label: string, value: React.ReactNode) => (
    <div className="core-line">
      <span>{label}</span>
      <span className="st-muted st-mono">{value || '—'}</span>
    </div>
  );
  const codecs = (v: string[]) => (v.length ? v.map((c) => c.toUpperCase()).join(' · ') : null);
  return (
    <Card icon={<GpuIcon size={16} />} title={g.name} actions={g.ready ? <Badge tone="ok">{t('Ready')}</Badge> : <Badge tone="warn">{t('Driver to install')}</Badge>}>
      {g.note && <p className="core-warn">{friendly(g.note)}</p>}
      <div className="core-cols">
        <div className="core-lines">
          {line(t('Maker'), VENDOR[g.vendor])}
          {line(t('Driver'), [g.driver, g.driverVersion].filter(Boolean).join(' · '))}
          {line(t('Render node'), g.renderNode)}
          {line(t('Memory'), g.memoryMb ? formatBytes(g.memoryMb * 1048576) : null)}
          {line('PCI', g.pci)}
        </div>
        <div className="core-lines">
          {line(t('Interface'), g.api ? API[g.api] : null)}
          {line(t('Encodes'), codecs(g.encode))}
          {line(t('Decodes'), codecs(g.decode))}
          {g.usage && (
            <>
              <UseBar label="GPU" value={g.usage.gpu} />
              <UseBar label={t('Encoder')} value={g.usage.encoder} />
              <UseBar label={t('Decoder')} value={g.usage.decoder} />
              {g.memoryMb ? <UseBar label={t('Memory')} value={(g.usage.memoryUsedMb / g.memoryMb) * 100} text={`${formatBytes(g.usage.memoryUsedMb * 1048576)} / ${formatBytes(g.memoryMb * 1048576)}`} /> : null}
              {g.usage.temperature != null && line(t('Temperature'), `${Math.round(g.usage.temperature)} °C`)}
            </>
          )}
        </div>
      </div>
    </Card>
  );
}

export function GpuPage() {
  // Les GPU seulement (pas tout le matériel), et seulement quand l'onglet est visible.
  const polled = usePoll<{ gpus: Gpu[] }>('/api/hardware/gpus');
  // « Regarder à nouveau » : sa réponse sert jusqu'à la lecture régulière suivante (sans rendu de plus).
  const [fresh, setFresh] = useState<{ gpus: Gpu[]; after: typeof polled } | null>(null);
  const gpus = (fresh && fresh.after === polled ? fresh.gpus : null) ?? polled?.gpus ?? null;
  const read = () =>
    api<{ gpus: Gpu[] }>('GET', '/api/hardware/gpus')
      .then((r) => setFresh({ gpus: r.gpus, after: polled }))
      .catch(() => {});
  return (
    <>
      <PageHead title="GPU" sub={gpus ? tn(gpus.length, '{n} GPU', '{n} GPUs') : undefined}>
        <button type="button" className="st-btn" onClick={() => void read()}>
          <RefreshCw size={14} /> {t('Look again')}
        </button>
      </PageHead>
      {gpus && gpus.length === 0 && (
        <div className="core-empty">
          <Empty icon={GpuIcon} text={t('No GPU: the apps encode and render with the processors.')} />
        </div>
      )}
      {gpus?.map((g) => <GpuCard key={g.pci} g={g} />)}
    </>
  );
}
