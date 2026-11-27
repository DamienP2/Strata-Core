/*
 * Infobulles dessinées pour toute l'appli, au lieu de celles du navigateur : tout élément qui a un
 * `title` l'affiche dans une petite carte qui suit le pointeur. Le title passe dans `data-tip` au
 * premier survol (le navigateur n'affiche alors plus rien) ; un élément sans nom visible le garde
 * comme aria-label. Repris de CG.
 */

const DELAY = 380;

export function installTooltips() {
  if (typeof document === 'undefined' || document.getElementById('st-tip')) return;
  const tip = document.createElement('div');
  tip.id = 'st-tip';
  tip.className = 'st-tip';
  tip.setAttribute('role', 'tooltip');
  document.body.appendChild(tip);

  let target: HTMLElement | null = null;
  let timer = 0;
  let x = 0;
  let y = 0;
  let shown = false;

  const place = () => {
    const pad = 8;
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    let left = x + 14;
    let top = y + 18;
    if (left + w > window.innerWidth - pad) left = Math.max(pad, x - w - 10);
    if (top + h > window.innerHeight - pad) top = Math.max(pad, y - h - 12);
    tip.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
  };
  const hide = () => {
    window.clearTimeout(timer);
    target = null;
    if (shown) {
      shown = false;
      tip.classList.remove('on');
    }
  };
  const show = () => {
    if (!target || !target.isConnected) return hide();
    const text = target.dataset.tip;
    if (!text) return;
    tip.textContent = text;
    shown = true;
    tip.classList.add('on');
    place();
  };

  /** L'infobulle de l'élément, à la place de son title. */
  const tipOf = (el: HTMLElement): string | undefined => {
    const t = el.getAttribute('title');
    if (t) {
      el.dataset.tip = t;
      el.removeAttribute('title');
      if (!el.getAttribute('aria-label') && !el.textContent?.trim()) el.setAttribute('aria-label', t);
    }
    return el.dataset.tip;
  };

  document.addEventListener(
    'pointerover',
    (e) => {
      if (e.pointerType === 'touch') return;
      let el = e.target as HTMLElement | null;
      // L'élément le plus proche qui a une infobulle (les title des SVG restent ceux du navigateur).
      while (el && el !== document.body && !(el instanceof HTMLElement && (el.hasAttribute('title') || el.dataset.tip))) el = el.parentElement;
      if (!el || el === document.body) return hide();
      if (el === target) return;
      hide();
      if (!tipOf(el)) return;
      target = el;
      x = e.clientX;
      y = e.clientY;
      timer = window.setTimeout(show, DELAY);
    },
    true,
  );
  document.addEventListener(
    'pointermove',
    (e) => {
      x = e.clientX;
      y = e.clientY;
      if (shown) {
        // Suit le pointeur ; disparaît quand il quitte l'élément.
        if (target && !target.contains(e.target as Node)) return hide();
        place();
      }
    },
    { capture: true, passive: true },
  );
  for (const ev of ['pointerdown', 'wheel', 'keydown', 'blur'] as const) window.addEventListener(ev, hide, { capture: true, passive: true });
  document.addEventListener('pointerleave', hide);
}
