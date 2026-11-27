/*
 * Une fenêtre au-dessus de la page : titre, contenu, boutons en bas. Se ferme par la croix, un clic sur
 * le fond ou Échap (installEscape). Avec `onSubmit`, c'est un formulaire : Entrée l'envoie.
 */
import { useEffect, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';
import { t } from '../i18n';

export function Modal({ title, icon, children, footer, onClose, onSubmit, wide }: {
  title: ReactNode;
  /** Une icône en 16 avant le titre. */
  icon?: ReactNode;
  children: ReactNode;
  /** Les boutons, à droite. */
  footer?: ReactNode;
  onClose: () => void;
  onSubmit?: () => void;
  wide?: boolean;
}) {
  const form = useRef<HTMLFormElement>(null);
  useEffect(() => {
    // Le premier champ prend la main.
    form.current?.querySelector<HTMLElement>('input:not([type="checkbox"]), textarea, button.st-dd-btn')?.focus();
  }, []);
  return (
    <div className="st-modal-back" onPointerDown={onClose}>
      <form
        ref={form}
        className={`st-modal ${wide ? 'wide' : ''}`}
        role="dialog"
        aria-modal="true"
        onPointerDown={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit?.();
        }}
      >
        <header>
          {icon}
          <h2>{title}</h2>
          <button type="button" className="st-icon-btn ghost" title={t('Close')} onClick={onClose}>
            <X size={16} />
          </button>
        </header>
        {children}
        {footer && <footer>{footer}</footer>}
      </form>
    </div>
  );
}
