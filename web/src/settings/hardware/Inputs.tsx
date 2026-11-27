// Les entrées SDI captées par le Core et partagées avec les applis : pour chacune, son image (mise
// à jour chaque seconde), son format, son timecode et les applis qui la lisent.
import { useEffect, useState } from 'react';
import { Cable } from 'lucide-react';
import { Badge, Card, t } from '@strata/ui';
import { api } from '../../api';
import { friendly } from '../../errors';
import type { HwConfig } from './store';

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
}

/** L'état des entrées, relu chaque seconde tant que la page est ouverte. */
function useInputs(): [InputStatus[] | null, number] {
  const [list, setList] = useState<InputStatus[] | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let gone = false;
    const read = async () => {
      try {
        const r = await api<{ inputs: InputStatus[] }>('GET', '/api/hardware/inputs');
        if (!gone) {
          setList(r.inputs);
          setTick((n) => n + 1);
        }
      } catch {
        /* le serveur redémarre : on réessaie */
      }
    };
    void read();
    const id = setInterval(read, 1000);
    return () => {
      gone = true;
      clearInterval(id);
    };
  }, []);
  return [list, tick];
}

function Tile({ i, cfg, tick }: { i: InputStatus; cfg: HwConfig; tick: number }) {
  const card = cfg.cards.find((c) => c.id === i.card);
  const wire = card?.connectors[i.connector - 1]?.name;
  const [broken, setBroken] = useState(false);
  const live = i.state === 'capturing' && i.signal !== false && i.frames > 0;
  useEffect(() => setBroken(false), [live]);
  const src = `/api/hardware/inputs/${encodeURIComponent(i.card)}/${i.connector}/preview.jpg?n=${tick}`;
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
    <div className="core-input">
      <div className="core-input-pic">
        {live && !broken ? <img src={src} alt="" onError={() => setBroken(true)} /> : <span>{i.state === 'error' ? t('Error') : t('No picture')}</span>}
        {i.timecode && live && <span className="core-input-tc">{i.timecode}</span>}
        {i.simulated && <span className="core-input-sim">{t('test pattern')}</span>}
      </div>
      <div className="core-input-head">
        <strong>SDI {i.connector}</strong>
        <span className="core-input-wire">{wire || card?.name || card?.modelName || ''}</span>
        {status}
      </div>
      {wire && <div className="st-muted core-input-card">{card?.name || card?.modelName}</div>}
      {i.error && <div className="core-warn">{friendly(i.error)}</div>}
      <div className="core-input-readers">
        {i.readers.length === 0 ? (
          <span className="st-muted">{t('No app reads it')}</span>
        ) : (
          <>
            <span className="st-muted">{t('Read by')}</span>
            {[...new Set(i.readers.map((r) => r.name || r.app || t('a program')))].map((a) => (
              <span key={a} className="core-input-app">
                {a}
              </span>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

/** Les entrées partagées (rien quand aucun connecteur n'est réglé en entrée). */
export function InputsCard({ cfg }: { cfg: HwConfig }) {
  const [list, tick] = useInputs();
  if (!list || list.length === 0) return null;
  return (
    <Card icon={<Cable size={16} />} title={t('Shared inputs')} actions={<span className="st-muted">{t('Captured once by the Core, read by the apps')}</span>}>
      <div className="core-inputs">
        {list.map((i) => (
          <Tile key={`${i.card}-${i.connector}`} i={i} cfg={cfg} tick={tick} />
        ))}
      </div>
    </Card>
  );
}
