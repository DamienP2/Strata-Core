/*
 * La mise en page des pages : une liste à gauche, la page à droite, avec son en-tête (titre, sous-titre
 * factuel, action principale) et d'éventuels sous-onglets. Repris de LiveCut (classes renommées st-) ;
 * NavSection accepte des actions comme dans CG.
 */
import type { ComponentType, ReactNode } from 'react';

export function SidePage({ nav, children, className, wide = true }: { nav: ReactNode; children: ReactNode; className?: string; wide?: boolean }) {
  return (
    <div className={`st-side-page st-page ${className ?? ''}`}>
      <aside className="st-side-nav">{nav}</aside>
      <main className="st-page-main">
        <div className={`st-page-inner ${wide ? 'wide' : ''}`}>{children}</div>
      </main>
    </div>
  );
}

/** Une page sans liste à gauche (le cockpit, une appli). */
export function Page({ children, className, wide = true }: { children: ReactNode; className?: string; wide?: boolean }) {
  return (
    <main className={`st-page-main st-page ${className ?? ''}`}>
      <div className={`st-page-inner ${wide ? 'wide' : ''}`}>{children}</div>
    </main>
  );
}

/** Un titre de section de la liste de gauche, avec des actions à droite si besoin. */
export function NavSection({ title, actions }: { title: string; actions?: ReactNode }) {
  return (
    <div className="st-side-section">
      <span className="st-grow">{title}</span>
      {actions}
    </div>
  );
}

export interface NavItemProps {
  icon: ComponentType<{ size?: number }>;
  label: string;
  on?: boolean;
  onClick: () => void;
  /** Un point : `ok` (vert), `warn` (orange), `bad` (rouge). */
  status?: 'ok' | 'warn' | 'bad' | null;
  count?: number;
}

export function NavItem({ icon: Icon, label, on, onClick, status, count }: NavItemProps) {
  return (
    <div
      className={`st-side-item ${on ? 'on' : ''}`}
      role="link"
      tabIndex={0}
      aria-current={on ? 'page' : undefined}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onClick();
        }
      }}
    >
      <Icon size={15} />
      <span className="st-grow">{label}</span>
      {count !== undefined && <span className="st-count">{count}</span>}
      {status && <span className={`st-dot ${status}`} />}
    </div>
  );
}

/** Titre de la page, sous-titre factuel (un compte, une version, un état), action principale à droite. */
export function PageHead({ title, sub, children }: { title: ReactNode; sub?: ReactNode; children?: ReactNode }) {
  return (
    <div className="st-page-head">
      <div>
        <h1>{title}</h1>
        {sub && <p className="st-muted">{sub}</p>}
      </div>
      <div className="st-spacer" />
      {children}
    </div>
  );
}

export interface SubTab<T extends string> {
  id: T;
  label: string;
  icon?: ReactNode;
  count?: number;
}

/** Les pages d'une section, sous son titre ; `children` : les actions de la page ouverte. */
export function SubTabs<T extends string>({ items, value, onChange, children }: { items: SubTab<T>[]; value: T; onChange: (v: T) => void; children?: ReactNode }) {
  return (
    <div className="st-subtabrow">
      <div className="st-tabs st-subtabs" role="tablist">
        {items.map((i) => (
          <button key={i.id} type="button" role="tab" aria-selected={i.id === value} className={i.id === value ? 'on' : ''} onClick={() => onChange(i.id)}>
            {i.icon}
            <span>{i.label}</span>
            {i.count !== undefined && <span className="st-subtab-n">{i.count}</span>}
          </button>
        ))}
      </div>
      {children}
    </div>
  );
}
