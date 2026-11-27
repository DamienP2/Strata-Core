/* Petits composants : état vide, « i », interrupteur, contrôle segmenté, petit choix, badge, point. */
import type { ComponentType, ReactNode } from 'react';
import { Info } from 'lucide-react';

/** Une zone vide dans un panneau : icône, un texte court, un bouton facultatif. */
export function Empty({ icon: Icon, text, children }: { icon?: ComponentType<{ size?: number }>; text: string; children?: ReactNode }) {
  return (
    <div className="st-empty">
      {Icon && (
        <span className="st-empty-icon">
          <Icon size={22} />
        </span>
      )}
      <span className="st-empty-text">{text}</span>
      {children}
    </div>
  );
}

/** Le « i » après un libellé : son texte s'affiche en infobulle dessinée. À utiliser avec parcimonie. */
export function InfoTip({ text }: { text: string }) {
  return (
    <span className="st-info" title={text} aria-label={text} role="img" tabIndex={0}>
      <Info size={13} />
    </span>
  );
}

/** Un interrupteur marche / arrêt (jamais de case à cocher visible). */
export function Switch({ checked, onChange, label, disabled, title }: { checked: boolean; onChange: (v: boolean) => void; label?: ReactNode; disabled?: boolean; title?: string }) {
  return (
    <label className="st-switch" title={title}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  );
}

export interface SegOption<T extends string> {
  value: T;
  label: ReactNode;
  title?: string;
  disabled?: boolean;
}

/** Quelques choix côte à côte (jamais de boutons radio). */
export function Segmented<T extends string>({ value, options, onChange, className }: { value: T; options: SegOption<T>[]; onChange: (v: T) => void; className?: string }) {
  return (
    <div className={className ? `st-segmented ${className}` : 'st-segmented'} role="group">
      {options.map((o) => (
        <button key={o.value} type="button" title={o.title} disabled={o.disabled} aria-pressed={o.value === value} className={o.value === value ? 'on' : ''} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Le petit choix des menus (langue FR | EN, thème Sombre | Clair | Système). */
export function Choice<T extends string>({ value, options, onChange }: { value: T; options: SegOption<T>[]; onChange: (v: T) => void }) {
  return (
    <div className="st-choice" role="group">
      {options.map((o) => (
        <button key={o.value} type="button" title={o.title} aria-pressed={o.value === value} className={o.value === value ? 'on' : ''} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export type Tone = 'ok' | 'warn' | 'bad' | 'accent';

/** Une étiquette courte, colorée selon l'état. */
export function Badge({ tone, children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={tone ? `st-badge ${tone}` : 'st-badge'} title={title}>
      {children}
    </span>
  );
}

/** Un point d'état : vert ok, orange attention, rouge perdu ; gris sans état. */
export function Dot({ tone, title }: { tone?: Exclude<Tone, 'accent'>; title?: string }) {
  return <span className={tone ? `st-dot ${tone}` : 'st-dot'} title={title} />;
}
