/*
 * Le serveur dans la barre du haut, en une pastille : « Connecté » avec un point vert ; le processeur
 * quand la machine est chargée (orange) ; « Serveur perdu » quand la liaison est coupée (rouge).
 * Au survol : la machine. Repris de CG (seuil de charge) et LiveCut ; l'état arrive par les props.
 */
import { useEffect, useRef, useState } from 'react';
import { TriangleAlert, WifiOff } from 'lucide-react';
import { t, tn } from '../i18n';
import { formatBytes, formatPercent } from '../format';

/** Au-dessus de cette charge (%), la pastille montre le processeur au lieu de « Connecté »… */
const HOT = 85;
/** … et elle revient en dessous de celle-ci. */
const COOL = 80;

export interface MachineLoad {
  cpu: number;
  cores: number;
  memUsed: number;
  memTotal: number;
  hostname?: string;
}

/** Perdu : la liaison s'est coupée après avoir été ouverte (800 ms), ou ne s'est pas ouverte en 4 s. */
function useLost(connected: boolean, onLost?: () => void): boolean {
  const [lost, setLost] = useState(false);
  const ever = useRef(false);
  const tick = useRef(onLost);
  tick.current = onLost;
  useEffect(() => {
    if (connected) {
      ever.current = true;
      setLost(false);
      return;
    }
    const h = setTimeout(() => setLost(true), ever.current ? 800 : 4000);
    // Revenu mais redémarré (les sessions ne sont pas gardées) : l'appli revérifie sa session.
    const ask = setInterval(() => tick.current?.(), 3000);
    return () => {
      clearTimeout(h);
      clearInterval(ask);
    };
  }, [connected]);
  return lost && !connected;
}

export function StatusPill({ connected, serverName, machine, onClick, onLost }: {
  connected: boolean;
  serverName?: string;
  machine?: MachineLoad | null;
  /** Administrateurs : ouvre la Santé du serveur. Sans : une simple pastille. */
  onClick?: () => void;
  /** Appelé toutes les 3 s tant que la liaison est perdue. */
  onLost?: () => void;
}) {
  const lost = useLost(connected, onLost);
  const cpu = machine?.cpu;
  // Tenu un peu : ne clignote pas autour du seuil.
  const [hot, setHot] = useState(false);
  if (cpu !== undefined) {
    const next = hot ? cpu >= COOL : cpu > HOT;
    if (next !== hot) setHot(next);
  }

  const state = lost ? 'lost' : !connected ? 'wait' : hot ? 'hot' : 'ok';
  const label =
    state === 'lost' ? (
      <>
        <WifiOff size={13} />
        <span className="st-status-txt">
          <b>{t('Server lost')}</b>
          <span className="st-status-sep"> · </span>
          <span>{t('reconnecting…')}</span>
        </span>
      </>
    ) : state === 'hot' ? (
      <>
        <TriangleAlert size={13} /> {t('CPU {n}', { n: formatPercent(cpu) })}
      </>
    ) : (
      <>
        <i className="st-status-dot" /> {state === 'ok' ? t('Connected') : t('Connecting…')}
      </>
    );

  const lines = [serverName || 'Strata', state === 'lost' ? t('Server lost · reconnecting…') : connected ? t('Connected') : t('Connecting…')];
  if (machine && !lost) {
    lines.push(`${t('CPU {n}', { n: formatPercent(machine.cpu) })} · ${tn(machine.cores, '{n} core', '{n} cores')}`);
    lines.push(t('Memory {used} of {total}', { used: formatBytes(machine.memUsed), total: formatBytes(machine.memTotal) }));
    if (machine.hostname && machine.hostname !== serverName) lines.push(machine.hostname);
  }
  const title = lines.join('\n');
  const cls = `st-status ${state}`;

  if (!onClick)
    return (
      <span className={cls} role="status" aria-live="polite" title={title}>
        {label}
      </span>
    );
  return (
    <button type="button" className={cls} aria-live="polite" title={title} onClick={onClick}>
      {label}
    </button>
  );
}
