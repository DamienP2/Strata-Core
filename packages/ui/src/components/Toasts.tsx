/* Messages courts en bas à droite, partis après 4 s. Repris de LiveCut (la file vit ici, plus dans le store de l'appli). */
import { create } from 'zustand';

export type ToastKind = 'info' | 'error';
interface Toast {
  id: number;
  message: string;
  kind: ToastKind;
}

const useToasts = create<{ toasts: Toast[] }>(() => ({ toasts: [] }));
let nextId = 0;

/** Affiche un message court ; `error` le borde de rouge. */
export function toast(message: string, kind: ToastKind = 'info') {
  const id = ++nextId;
  useToasts.setState((s) => ({ toasts: [...s.toasts, { id, message, kind }] }));
  setTimeout(() => useToasts.setState((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) })), 4000);
}

/** Monté une seule fois dans l'appli. */
export function Toasts() {
  const toasts = useToasts((s) => s.toasts);
  return (
    <div className="st-toasts" role="status" aria-live="polite">
      {toasts.map((x) => (
        <div key={x.id} className={`st-toast ${x.kind}`}>
          {x.message}
        </div>
      ))}
    </div>
  );
}
