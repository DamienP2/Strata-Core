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

export function HwSaveBar() {
  const dirty = useHwDirty();
  const saving = useHw((s) => s.saving);
  return <SaveBar dirty={dirty} saving={saving} onSave={() => void saveHw()} onDiscard={discardHw} />;
}

