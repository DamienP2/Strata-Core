// Les cartes DeckLink, sans three.js : les couleurs des fonctions, l'allure de chaque modèle et ses
// numéros imprimés. Les pages qui ne dessinent pas de carte s'en servent sans charger le rendu 3D.
import type { Look } from './render3d';

/** Les couleurs des fonctions des connecteurs SDI. */
export const SDI_COLOR = { input: '#22c55e', output: '#8fa3ff', unused: '', ref: '#f5c542' } as const;

export interface DeckLinkLook extends Look {
  /** Les numéros imprimés sur l'équerre, de haut en bas (sinon : les numéros logiciels). */
  printed?: number[];
}

const DECKLINK: Record<string, DeckLinkLook> = {
  // Imprimé 8 → 1 de haut en bas ; le logiciel (Desktop Video, vMix) : 8, 4, 7, 3, 6, 2, 5, 1.
  'decklink quad 2': { length: 168, fans: 2, lanes: 8, silk: 'DeckLink Quad 2', printed: [8, 7, 6, 5, 4, 3, 2, 1] },
  'decklink 8k pro': { length: 168, fans: 1, lanes: 8, silk: 'DeckLink 8K Pro' },
  'decklink duo 2': { length: 168, fans: 0, sink: true, lanes: 4, silk: 'DeckLink Duo 2' },
  'decklink sdi 4k': { length: 125, fans: 0, sink: true, lanes: 4, silk: 'DeckLink SDI 4K' },
  'decklink mini monitor': { length: 102, low: true, fans: 0, lanes: 1, hdmi: true, silk: 'DeckLink Mini Monitor' },
  'decklink mini monitor 4k': { length: 120, low: true, fans: 0, sink: true, lanes: 4, hdmi: true, silk: 'DeckLink Mini Monitor 4K' },
  'decklink mini recorder': { length: 102, low: true, fans: 0, lanes: 1, hdmi: true, silk: 'DeckLink Mini Recorder' },
  'decklink mini recorder 4k': { length: 120, low: true, fans: 0, sink: true, lanes: 4, hdmi: true, silk: 'DeckLink Mini Recorder 4K' },
};

export function decklinkLook(model: string, connectors: number): DeckLinkLook {
  return DECKLINK[model.trim().toLowerCase()] ?? { length: connectors > 2 ? 160 : 120, low: connectors <= 1, fans: connectors > 4 ? 1 : 0, sink: connectors > 1, lanes: 4, silk: model };
}

/** Le numéro imprimé d'un connecteur (SDI n), quand il diffère. */
export function printedOf(model: string, order: number[], n: number): number | null {
  const look = decklinkLook(model, order.length);
  const i = order.indexOf(n);
  const p = look.printed && look.printed.length === order.length && i >= 0 ? look.printed[i] : n;
  return p === n ? null : p;
}
