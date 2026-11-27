# @strata/ui

Le design commun des applis Strata : jetons, thèmes clair et sombre, police Inter, composants React.

## Dans une appli

```ts
import '@strata/ui/styles.css';
import { Brand, DialogHost, Toasts, installEscape, installTooltips, addTranslations } from '@strata/ui';

installTooltips(); // infobulles dessinées pour tous les `title`
installEscape();   // Échap ferme le dialogue du dessus
addTranslations('fr', fr); // les textes français de l'appli
```

Une seule fois dans l'appli : `<DialogHost />` et `<Toasts />`.

Le paquet se publie avec chaque release de Strata-Core (`strata-ui-<version>.tgz`). L'appli le référence par son adresse dans son `package.json`.

## Classes

Toutes les classes du Core commencent par `st-` : `st-btn`, `st-card`, `st-field`, `st-badge`… Le CSS d'une appli n'utilise jamais ce préfixe.

## Développer

```bash
npm install
npm run dev     # banc d'essai : chaque composant, dans les deux thèmes
npm run build   # dist/ : strata-ui.js, strata-ui.css, fonts/, types
```
