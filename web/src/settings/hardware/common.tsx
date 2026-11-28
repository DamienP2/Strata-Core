// Ce que partagent les pages du matériel : le chargement, la barre d'enregistrement.
import { useEffect } from 'react';
import { SaveBar } from '@strata/ui';
import { discardHw, loadHw, saveHw, useHw, useHwDirty, type HwConfig, type Snapshot } from './store';

/** Charge le matériel à la première page qui en a besoin ; rend l'instantané et les réglages. */
export function useHardware(): [Snapshot, HwConfig] | null {
  const snap = useHw((s) => s.snap);
  const draft = useHw((s) => s.draft);
  useEffect(() => {
    if (!useHw.getState().snap) void loadHw();
  }, []);
  return snap && draft ? [snap, draft] : null;
}

/** Les pages qui partagent ces réglages : on passe de l'une à l'autre sans perdre les changements. */
const HW_PAGES = ['/reglages/sdi', '/reglages/ndi', '/reglages/st2110', '/reglages/reseau', '/reglages/horloge'];
const leave = { keeps: (to: string) => HW_PAGES.includes(to.split(/[?#]/)[0]), onDiscard: discardHw };

export function HwSaveBar() {
  const dirty = useHwDirty();
  const saving = useHw((s) => s.saving);
  return <SaveBar dirty={dirty} saving={saving} onSave={() => void saveHw()} onDiscard={discardHw} leave={leave} />;
}

