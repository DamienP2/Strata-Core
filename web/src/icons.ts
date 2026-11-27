// Les icônes des applis Strata, d'après le nom donné par leur fiche ; une appli inconnue : une fenêtre.
import type { ComponentType } from 'react';
import { AppWindow, Film, Layers, LayoutGrid, Network, Phone, Scissors } from 'lucide-react';

const ICONS: Record<string, ComponentType<{ size?: number }>> = { scissors: Scissors, layers: Layers, 'layout-grid': LayoutGrid, network: Network, film: Film, phone: Phone };

export const appIcon = (name: string): ComponentType<{ size?: number }> => ICONS[name] ?? AppWindow;
