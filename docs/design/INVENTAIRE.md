# Inventaire du design commun

> Premier lot de Strata-Core : **DESIGN**. Ce document fait l'état des lieux avant d'écrire le paquet web.
> Applis analysées : **CG** (Graphics), **Strata-Cut** (LiveCut), **Strata-Multiview**, **Strata-Network**, au 27/11/2026.

## 1. En bref

- **Le design est déjà commun, mais il est copié quatre fois.** Les jetons de couleur, de taille et de thème sont identiques dans les quatre applis. **261 règles CSS sont identiques mot pour mot dans les quatre**, et 291 entre CG, Cut et Network.
- **CG est la référence.** Multiview est une copie de CG, presque à l'identique (1 326 règles CSS identiques, composants identiques à 93–100 %). Cut et Network ont été refaits à partir d'une spec tirée de CG, puis améliorés de leur côté.
- **Cette spec existe déjà** : `Strata-Network/docs/design.md` (1 362 lignes, « Reproduire le design et les fonctions de base de l'application de référence »). C'est elle que Cut cite dans son code (« spec § 5.1 »…). C'est en fait le cahier des charges du Core, rédigé avant le Core.
- **Ce qui reste à faire** : choisir, composant par composant, entre la version de CG et les améliorations de Cut et Network, puis tout réunir dans un seul paquet.
- **Décision** : le Core part **du code de CG et de Cut**. La spec de Network reste où elle est (§ 6).

## 2. Ce qui est déjà identique partout

### 2.1 Les jetons

**Thème sombre** (par défaut), identique dans les 4 applis :

| Groupe | Jetons |
| --- | --- |
| Fonds | `--bg #0b0c12` · `--panel #12141d` · `--panel-2 #181b26` · `--panel-3 #1f2331` |
| Traits | `--line #262a3a` · `--line-2 #323750` |
| Texte | `--text #e6e8f2` · `--text-2 #a3a8bf` · `--text-3 #8a90ad` |
| Accent (pervenche) | `--accent #8fa3ff` · `--accent-2 #bfcaff` · `--accent-soft` · `--on-accent #0b1236` |
| États | `--live #ff3b5c` (erreur, danger, perdu) · `--live-soft` · `--live-btn #e11d48` (bouton rouge plein) · `--pvw #22c55e` (ok) · `--warn #f5a524` (attention) · `--key #f5c542` · `--mask #2dd4bf` |
| Formes | `--radius 8px` |
| Typo | `--fs-11` · `--fs-12` · `--fs-13` · `--fs-15` · `--fs-20` (rien sous 11 px) · `--mono` |
| Contrôles | `--h-control 32px` · `--h-control-dense 28px` · `--hit-min 28px` |

**Thème clair** : 29 jetons identiques dans CG et Cut (dont `--pvw-fill`, `--warn-fill` et les textes colorés `--t-red`, `--t-green`, `--t-blue`, `--t-yellow`, `--t-pink`, `--t-violet`, `--t-orange`). Règle commune : **les images (aperçus, vignettes, consoles de journal) restent sombres dans les deux thèmes**.

Seuls écarts :
- Multiview redéfinit `--mask` et Network ajoute `--op-2`.
- La famille `--op-1` à `--op-4` (fonds de l'Opérateur de CG) sert à une règle commune (`.opx-void-ic`). Cut s'en sort avec une valeur de secours (`var(--op-2, #1c1f2c)`).
- **Décision** : `--op-1` à `--op-4` entrent dans les jetons du Core.

### 2.2 La police

Inter, en 5 graisses (400, 500, 600, 700, 800), sous forme de WOFF2 réduits aux écritures latines, à la ponctuation, aux flèches et aux symboles (environ 75 Ko par graisse). Les fichiers et les `unicode-range` sont les mêmes partout.
CG charge **en plus** les OTF complets (cyrillique, grec…) pour le texte des graphiques. C'est du métier : ça reste dans CG.

### 2.3 La base CSS

Identique partout (spec § 3.2) :
- `box-sizing` ;
- corps en 13 px / 1,4 Inter ;
- sélection de texte coupée sauf dans les champs et le code (« une application, pas un document ») ;
- champs sur `--panel-3` ;
- barres de défilement fines ;
- pas de cadre de focus, sauf au clavier (contour `--accent`) ;
- utilitaires `.spacer`, `.muted`, `.hint`, `.small`, `.empty`, `.loading`, `.dim`, `.row`, `.spin`.

### 2.4 Les mesures

Les mesures sont fixées dans la spec (§ 3.3) et respectées par les quatre applis :
- tailles de texte par usage : `h1` 22 px / 800, carte 14 px / 700, libellés 11 px MAJUSCULES… ;
- tailles d'icônes lucide par endroit : 13, 14, 15, 16, 17 et 22 ;
- échelle des rayons : 4, 6, 7, 8, 9, 10, 12, 14, 16 et 999 px ;
- ombres ;
- transitions de 0,12 s et 0,15 s, coupées par `prefers-reduced-motion` ;
- dégradés de fond (contenu, barre du haut, connexion).

### 2.5 Le socle CSS commun : 291 règles

Ces règles sont identiques mot pour mot dans CG, Cut et Network. Par famille :

| Famille | Règles | Ce que c'est |
| --- | --- | --- |
| `dd-*` | 30 | liste déroulante dessinée |
| `ux1-*` | 25 | menu utilisateur, pastilles d'état, avatar |
| base | 24 | § 2.3 |
| `user-*` | 20 | cartes et tableau des utilisateurs |
| `update-*` | 16 | page Mises à jour |
| `btn` | 15 | boutons (primaire, danger, petit, fantôme…) |
| `set-*` | 14 | cartes de réglages, champs, grille |
| `brand-*` | 13 | la marque |
| `login-*` | 10 | page de connexion |
| `lib-*`, `library` | 10 | liste de gauche et page de droite |
| `modal`, `dialog-*` | 11 | modales et dialogues |
| `topbar`, `tabs`, `nav-*` | 12 | barre du haut et navigation |
| `check` | 6 | interrupteur |
| `num-*`, `segmented` | 9 | champ nombre, contrôle segmenté |
| `badge`, `role-*` | 8 | badges et couleurs de rôle |
| `icon-btn` | 5 | bouton icône |
| `sys-*` | 4 | onglets de la page Système |
| `lang-*` | 4 | choix de la langue |
| `toast(s)`, `crash-*` | 5 | toasts, bandeau de plantage |
| autres | ~40 | `project-*`, `asr-*`, `opx-*`, `save-bar`, `search`, `empty`, `panel`, `menu`… |

### 2.6 Les règles d'interface

Elles sont écrites dans la spec (§ 1) et suivies par toutes les applis :
- interface en français, avec la typographie française ;
- une page = un titre, éventuellement un sous-titre factuel, et le contenu ;
- **aucun texte explicatif** ;
- aucune fonction superflue ;
- **contrôles dessinés uniquement** : jamais de `<select>`, `confirm()`, `alert()`, case à cocher ou infobulle natifs ;
- jetons uniquement, jamais de valeur en dur ;
- **maquette HTML validée avant tout nouvel écran** ;
- libellés courts.

Elles deviennent les règles du Core.

## 3. Ce qui a divergé

### 3.1 Les règles CSS modifiées

Onze règles ont le même sélecteur dans CG et Cut, mais un contenu différent :

| Règle | CG | Cut | Proposition |
| --- | --- | --- | --- |
| `.user-card.role-operator`, `.badge.role-operator` | vert (`--pvw`) | pervenche (`--accent-2`) | **pervenche** (décidé) |
| `.icon-btn.ghost` | taille `--hit-min` | sans taille | CG (cible de 28 px garantie) |
| `.sys-tabs` | largeur ajustée, retour à la ligne | marge de 18 px | fusionner |
| `a.btn` | `text-decoration: none` | `color: inherit` | fusionner (les deux) |
| `button.ux1-pill`, `.project-card` | — | + `font-family`, `position` | reprendre Cut |
| `.opx-void-ic` | `var(--op-2)` | `var(--op-2, #1c1f2c)` | CG (`--op-*` dans le Core) |
| `.hl-bar`, `.hl-inc`, `.gm-row` | une chose | une autre chose | **même nom, sens différent** : renommer dans le Core |

### 3.2 Le thème clair

Les jetons sont identiques, mais les surcharges par composant ne le sont pas : CG en a 70, Cut 23, Network 46. Le Core doit porter les surcharges claires de **ses** composants. Celles des écrans métier restent dans chaque appli.

### 3.3 Les composants React

Similarité du code avec CG (1,00 = identique) :

| Composant | CG | Cut | Multiview | Network | Version de départ proposée |
| --- | --- | --- | --- | --- | --- |
| `Brand` | ✓ | 1,00 | 1,00 | 1,00 | identique : seul le nom du module change → paramètre |
| `InfoTip` | ✓ | 0,73 | 1,00 | 0,60 | CG |
| `tooltips` (infobulles dessinées) | ✓ | 0,10 | 1,00 | 0,47 | CG (la plus complète, 96 lignes contre 44) |
| `escape` (Échap ferme le dessus) | ✓ | 0,36 | 1,00 | 0,50 | Cut (gère la pile des couches) — à vérifier |
| `Dropdown` | ✓ | 0,39 | 1,00 | 0,40 | **Cut** : s'ouvre au-dessus faute de place, mode riche (icône, sous-ligne) |
| `ContextMenu` | ✓ | 0,30 | — | 0,37 | **Cut** : `openContextMenu()` + un seul hôte, notes, séparateurs |
| `dialogs` (confirmer, demander, informer) | ✓ | 0,48 | 0,99 | 0,50 | même API partout ; Cut exporte les types |
| `fields` | ✓ | 0,21 | 0,99 | 0,05 | Core : `Segmented`, `NumberField`, `TextInput`, molette ; les champs couleur de CG restent dans CG |
| `StatusPill` (pastille de connexion) | ✓ | 0,25 | 0,99 | 0,29 | **CG** : montre la charge CPU, mène à la Santé |
| `UserMenu` | ✓ | 0,22 | 0,98 | 0,27 | mêmes fonctions ; à fusionner |
| `Empty` | ✓ | 0,63 | — | 0,59 | Cut (icône facultative) |
| `serverError` | ✓ | 0,09 | 0,82 | 0,03 | Network (138 lignes, la plus complète) — à vérifier |
| `NavMenu` | ✓ | — | 1,00 | 0,31 | CG |
| `Splitter` | ✓ | — | 0,93 | 0,36 | CG |
| `Boundary` (erreur d'affichage) | ✓ | — | ✓ | — | CG |
| `Toasts` | — | ✓ | — | 0,70 | Cut / Network |
| `CrashBanner` | — | ✓ | — | 0,52 | Cut / Network |
| `Shortcuts` (aide des raccourcis) | — | ✓ | — | 0,34 | Cut (145 lignes, la plus complète) |
| `format` (octets, durées, dates) | — | ✓ | — | 0,29 | à fusionner |
| `settings/ui` (`Card`, `Field`, `SaveBar`) | ✓ | 0,46 | 0,93 | 0,65 | **Cut** : + `Switch`, `useLeaveGuard`, aide sous le champ |
| `SidePage` (liste à gauche, page à droite) | ✓ | 0,53 | 0,69 | 0,35 | fusionner : `EmbeddedNav` (CG) + `NavItem` avec état et compteur (Cut) |
| `Login` | ✓ | 0,49 | 0,77 | 0,41 | Cut (erreurs SSO affichées) |
| `theme` | ✓ | 0,52 | 0,99 | 0,53 | CG (pages toujours sombres : sorties, intégrations) |
| `displayPrefs` | ✓ | 0,27 | 1,00 | 0,35 | à fusionner |
| `i18n` (`t`, `tc`, `tn`) | ✓ | 0,67 | 1,00 | 0,68 | même mécanisme (texte source anglais, dictionnaire `fr`) |
| `i18n-check` (script) | ✓ | 0,92 | ✓ | ✓ | CG |

Restent **dans leur appli** :
- CG et Multiview : `ColorPicker`, `FontPick`, `RichText`, `Monitor`, `canvasView`, `autoLayout`, `CanvasHeader`, `Logo` ;
- Cut : `Transport`, `AudioUi`, `RecLoop`, `ClipLevel`, `Channels`, `SendsPanel`…

### 3.4 Les noms propres à chaque appli

Ce qui doit devenir un **paramètre fourni par l'appli** :

| | CG | Cut |
| --- | --- | --- |
| Nom affiché (`MODULE` de `Brand`) | Graphics | LiveCut |
| Clé du thème dans le navigateur | `strata.theme` | `stratacut.theme` |
| Clé de la langue | `strata.lang` | `stratacut.lang` |

Avec un sous-domaine par appli, chaque appli est une origine différente pour le navigateur : ces clés ne servent plus qu'à éviter un éclair au premier affichage. **Le choix de référence est celui gardé sur le serveur pour l'utilisateur**, relu à l'ouverture de chaque appli.
| Titre de la page | Strata Graphics | Strata LiveCut |
| Pages toujours sombres | `/output`, `/embed` | — |

## 4. La coquille commune

Toutes les applis ont la même barre du haut, haute de 46 px :
- la **marque** ;
- la **navigation** (groupe segmenté, icônes seules sur écran étroit) ;
- un emplacement libre pour l'appli ;
- la **pastille de connexion** ;
- l'**aide** ;
- le **menu utilisateur** (langue, thème, raccourcis, déconnexion).

Sous la barre se trouve le contenu. Une seule fois dans l'app sont montés les **hôtes** : dialogues, menu contextuel, toasts, bandeau de plantage.

Avec l'option A (un service par appli), la coquille doit gagner **deux éléments qui n'existent nulle part aujourd'hui** :
- **le cockpit**, la page d'accueil du Core : les applis actives en tuiles, et pour les administrateurs, les réglages du serveur (brief, § 3) ;
- **le passage d'une appli active à l'autre** depuis la barre du haut, et le retour au cockpit.

D'après la règle 7, il faut une maquette avant de les coder.

## 5. Ce que le paquet web contiendra (proposition)

```
@strata/ui
  styles/
    fonts/inter-{400..800}.woff2
    tokens.css          jetons sombres + clairs, --op-* compris (§ 2.1)
    base.css            § 2.3
    components.css      les 291 règles du socle, divergences réglées, classes renommées
    light.css           surcharges claires des composants du Core
  theme.ts              sombre / clair / système, script d'avant le premier affichage
  i18n/                 t, tc, tn ; textes communs en français ; i18n-check
  components/           Brand, StatusPill, UserMenu, Dropdown, ContextMenu, dialogs,
                        fields, Empty, InfoTip, tooltips, escape, Toasts, CrashBanner,
                        Shortcuts, NavMenu, Splitter, Boundary, SidePage, settings/ui, Login
  shell/                barre du haut, navigation, hôtes, passage d'une appli à l'autre, cockpit
  format.ts, serverError.ts, displayPrefs.ts
```

Chaque appli fournit :
- son nom de module ;
- sa navigation ;
- ses dictionnaires de traduction ;
- son CSS métier.

## 6. Décisions

| Sujet | Décision |
| --- | --- |
| Point de départ | **le code de CG et de Cut**, composant par composant (§ 3.3). La spec `Strata-Network/docs/design.md` reste dans Network |
| Noms de classes | **renommés proprement** dans le Core (fin de `ux1-*`, `ux3-*`, `opx-*`, `asr-*`, `project-card` réutilisé…) ; chaque appli adapte son code en passant au paquet. La règle de nommage se fixe avec le paquet : un préfixe lisible par composant (`dropdown-*`, `user-menu-*`…) |
| Couleur des opérateurs | **pervenche** (`--accent-2`, comme Cut) |
| Jetons `--op-*` | **dans le Core** |
| Thème et langue | **gardés sur le serveur, par utilisateur**, relus à l'ouverture de chaque appli (indispensable avec un sous-domaine par appli) |
| Diffusion | crate Rust : dépendance git sur une étiquette de version de Strata-Core. Paquet web : **fichier `strata-ui-<version>.tgz` joint à chaque release de Strata-Core**, référencé par son adresse dans le `package.json` des applis |

## 7. Étapes suivantes

1. Maquette interactive du cockpit (proposition C retenue, plusieurs serveurs) : [`docs/maquettes/cockpit.html`](../maquettes/cockpit.html), à valider.
2. Comparaison fine, composant par composant, pour confirmer les versions de départ marquées « à vérifier ».
3. Écriture du paquet `@strata/ui` ([`packages/ui`](../../packages/ui)) : jetons, base, police, thème, langues, formats ; marque, boutons, badges, interrupteur, contrôle segmenté, liste déroulante, menu contextuel, champs nombre et texte, recherche ; cartes et champs de réglages, barre d’enregistrement, état vide ; page à liste de gauche, sous-onglets, barre du haut, pastille de connexion, menu utilisateur, page de connexion ; infobulles, Échap, dialogues, toasts. Reste : la coquille de la proposition C (lanceur, cockpit).
