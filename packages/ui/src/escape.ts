/*
 * Échap, n'importe où, ferme le dialogue du dessus : il « appuie » sur le `.st-modal-back` le plus
 * haut, qui se ferme sur pointerdown (un clic sur le fond), sauf si la touche a déjà été traitée.
 * Repris de LiveCut.
 */

let installed = false;

/** Le fond le plus haut : plus grand z-index, puis le dernier de la page. */
function topBackdrop(): HTMLElement | null {
  let best: HTMLElement | null = null;
  let bestZ = -Infinity;
  for (const el of document.querySelectorAll<HTMLElement>('.st-modal-back')) {
    const z = Number(getComputedStyle(el).zIndex) || 0;
    if (z >= bestZ) {
      best = el;
      bestZ = z;
    }
  }
  return best;
}

export function installEscape() {
  if (installed) return;
  installed = true;
  addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || e.defaultPrevented) return;
    const back = topBackdrop();
    if (!back) return;
    e.preventDefault();
    back.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true }));
  });
}
