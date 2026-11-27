/* Cartes de réglages, champs, et la barre d'enregistrement. Repris de settings/ui.tsx (LiveCut). */
import { useEffect, type ReactNode } from 'react';
import { Save } from 'lucide-react';
import { t } from '../i18n';
import { confirmDialog } from './dialogs';

/** Un bloc de réglages titré (icône en 16). */
export function Card({ icon, title, children, actions, className }: { icon?: ReactNode; title: ReactNode; children?: ReactNode; actions?: ReactNode; className?: string }) {
  return (
    <section className={className ? `st-card ${className}` : 'st-card'}>
      <header>
        {icon}
        <h2>{title}</h2>
        <div className="st-spacer" />
        {actions}
      </header>
      {children ? <div className="st-card-body">{children}</div> : null}
    </section>
  );
}

/**
 * Un champ avec son libellé. `group` : un <div> au lieu d'un <label> (plusieurs boutons dedans, comme
 * un Segmented, ou un interrupteur). `hint` : une ligne dessous.
 */
export function Field({ label, children, hint, group, className }: { label: ReactNode; children: ReactNode; hint?: ReactNode; group?: boolean; className?: string }) {
  const body = (
    <>
      <span className="st-field-label">{label}</span>
      {children}
      {hint && <span className="st-field-hint">{hint}</span>}
    </>
  );
  const cls = className ? `st-field ${className}` : 'st-field';
  return group ? <div className={cls}>{body}</div> : <label className={cls}>{body}</label>;
}

/** Une grille de champs : 3 colonnes, ou 2 avec `two` ; une seule sur écran étroit. */
export function FieldGrid({ two, children }: { two?: boolean; children: ReactNode }) {
  return <div className={two ? 'st-grid two' : 'st-grid'}>{children}</div>;
}

// ---------------------------------------------------------------- quitter une page modifiée

type Guard = () => Promise<boolean>;
let guard: Guard | null = null;

/**
 * Le routeur de l'appli l'appelle avant de changer de page : vrai si on peut partir (rien de modifié,
 * ou l'utilisateur a choisi d'abandonner).
 */
export async function canLeave(): Promise<boolean> {
  return guard ? guard() : true;
}

/** Demande avant de quitter la page tant que `dirty` (changement de page, fermeture de l'onglet). */
export function useLeaveGuard(dirty: boolean) {
  useEffect(() => {
    if (!dirty) return;
    const ask: Guard = () => confirmDialog({ title: t('Discard the changes?'), confirm: t('Discard'), danger: true });
    guard = ask;
    const unload = (e: BeforeUnloadEvent) => e.preventDefault();
    addEventListener('beforeunload', unload);
    return () => {
      if (guard === ask) guard = null;
      removeEventListener('beforeunload', unload);
    };
  }, [dirty]);
}

/** Monte du bas de l'écran tant que la copie locale diffère des réglages enregistrés. */
export function SaveBar({ dirty, saving, onSave, onDiscard }: { dirty: boolean; saving: boolean; onSave: () => void; onDiscard: () => void }) {
  useLeaveGuard(dirty);
  return (
    <div className={`st-save-bar ${dirty ? 'show' : ''}`} aria-hidden={!dirty}>
      <span>{t('Some changes are not saved.')}</span>
      <div className="st-spacer" />
      <button type="button" className="st-btn ghost" disabled={saving} onClick={onDiscard} tabIndex={dirty ? 0 : -1}>
        {t('Discard')}
      </button>
      <button type="button" className="st-btn primary" disabled={saving} onClick={onSave} tabIndex={dirty ? 0 : -1}>
        <Save size={14} /> {saving ? t('Applying…') : t('Save and apply')}
      </button>
    </div>
  );
}
