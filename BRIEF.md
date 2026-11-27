# Strata-Core — Brief de base

> Statut : brief, pas encore de code.
> Applis de référence : **CG** (Strata Graphics, v0.7.1) et **Strata-Cut** (Strata LiveCut, v0.2.0).

## 1. Le but

Toutes les applis Strata (Graphics, LiveCut, Multiview, Network, et bientôt Media et Call) ont **le même design** et **une partie de leurs fonctions en commun**. Aujourd'hui, ce socle est recopié d'une appli à l'autre, puis chaque copie évolue de son côté.

Strata-Core doit devenir **la source unique de ce socle commun** :

- on le corrige ou on l'améliore une fois, toutes les applis en profitent ;
- une nouvelle appli (Media, Call) démarre directement dessus au lieu de recopier ;
- l'utilisateur retrouve la même interface, les mêmes réglages et les mêmes comportements d'une appli à l'autre.

Le Core ne contient **rien de métier** : pas de rendu graphique, pas d'enregistrement vidéo, pas de multiview. Seulement ce que toutes les applis ont en commun.

## 2. Ce que les deux applis de référence ont déjà en commun

### 2.1 La même pile technique

| | CG | Strata-Cut |
| --- | --- | --- |
| Serveur | Rust, axum 0.8, tokio | Rust, axum 0.8, tokio |
| Interface | React 19, TypeScript 5.9, Vite 8, zustand 5 | idem |
| Icônes | lucide-react | lucide-react |
| Interface embarquée dans le binaire | rust-embed + gzip à la compilation | idem |
| Comptes | Argon2, `users.json` | idem |
| Mises à jour | releases GitHub (ureq + rustls) | idem |
| Plateformes | Linux, macOS, Windows (+ app de barre des tâches) | Linux seulement |

### 2.2 Le design (quasi identique)

- **Les jetons de couleur et de taille sont les mêmes valeur pour valeur** dans `styles.css` : `--bg`, `--panel*`, `--line*`, `--text*`, `--accent*` (pervenche `#8fa3ff`), `--live` (rouge), `--pvw` (vert), `--warn` (orange), `--key`, `--mask`, `--radius: 8px`, l'échelle typographique `--fs-11` à `--fs-20`, les hauteurs de contrôle `--h-control` (32 px) et `--h-control-dense` (28 px), `--hit-min`, `--mono`.
- **Les thèmes** : sombre par défaut, clair (`styles-light.css`, `data-theme="light"`), ou système. Le choix est gardé côté serveur et dans le navigateur (`theme.ts`).
- **La police** : Inter (400 à 800), embarquée.
- **La signature** : `Brand.tsx` (Strata + Le Figaro AudioVisuel), identique à 97 %.
- **Les composants d'interface présents dans les deux** : `Brand`, `StatusPill`, `Empty`, `InfoTip`, `Dropdown`, `ContextMenu`, `dialogs`, `fields`, `UserMenu`, `tooltips`, `escape`, `serverError`, `displayPrefs`, la mise en page des réglages (`settings/ui.tsx`) et le panneau latéral (`SidePage`).
- **La langue** : interface en français, outil de contrôle des traductions (`scripts/i18n-check.mjs`).

### 2.3 Les fonctions (mêmes rôles, code divergé)

Côté serveur, ces modules existent dans les deux applis, avec la même intention et souvent les mêmes règles, mais un code réécrit (5 à 25 % de lignes identiques) :

| Fonction | Module(s) | Ce qu'il fait |
| --- | --- | --- |
| Comptes et sessions | `auth.rs`, `users.rs` | utilisateurs, mots de passe Argon2, sessions (cookie HttpOnly ou Bearer), commande `user` en ligne de commande |
| SSO | `sso.rs` | OpenID Connect avec PKCE (Entra, Okta, Keycloak, Google…), rôle déduit des groupes |
| Rôles et droits | `auth.rs` (CG), `roles.rs` + `groups.rs` (Cut) | qui peut faire quoi |
| Sécurité des requêtes | `guard.rs` | CSRF, actions réservées à la machine locale, limitation des tentatives de connexion, en-têtes |
| Mises à jour | `update.rs` | vérification et installation depuis les releases GitHub, ou par archive envoyée |
| Santé | `health.rs` | journal sur disque (30 j), rapports de plantage (90 j), export pour le support |
| Journal | `logs.rs` | les dernières lignes du log en mémoire, lues par la page Système |
| Préférences utilisateur | `prefs.rs` | langue, thème, préférences d'affichage, pareilles sur tous les navigateurs |
| Chemins et données | `paths.rs`, `config.rs` | dossier de données, écriture JSON atomique, configuration du serveur |
| Service système | `service.rs` | installation en service (systemd ; launchd et Windows pour CG) |
| Serveur web | `web.rs`, `main.rs` | service de l'interface embarquée, CLI (clap) |
| Matériel vidéo | `decklink` (shim C++ + SDK), `ndi.rs` | accès aux cartes Blackmagic et au NDI |
| Performances | `perf.rs`, `metrics` | charge machine (sysinfo) |

Côté interface, les écrans qui en découlent sont aussi communs : **Connexion**, **Utilisateurs**, **Sécurité**, **Mises à jour**, **Santé**, **Système**, le **menu utilisateur** (thème, langue, déconnexion), les **raccourcis clavier**.

Il existe aussi des fonctions présentes dans une seule appli mais qui ont vocation à être partagées : **HTTPS / certificats** (`tls.rs`, CG), **alertes** webhook et e-mail (`alerts.rs`, Cut), **toasts** et **bandeau de plantage** (Cut).

## 3. Ce qui reste dans chaque appli

Tout le métier : le moteur de rendu, le designer et l'opérateur de CG ; l'enregistrement, les extraits et le streamer de LiveCut ; et pour les autres applis, leur propre domaine. Le Core ne doit jamais dépendre d'une appli.

## 4. Ce que ça implique

1. **Choisir une version de référence pour chaque brique**, car CG et Cut ont divergé. Par exemple : le SSO et les mises à jour de CG sont plus complets (multiplateforme, archive hors ligne) ; les droits par groupes et les alertes de Cut sont plus fins.
2. **Rendre le Core générique** : nom de l'appli, dépôt GitHub des mises à jour, dossier de données, clé de thème (`strata.theme` dans CG, `stratacut.theme` dans Cut), liste des droits… tout cela devient un paramètre fourni par l'appli.
3. **Migrer les applis une par une**, sans rien casser pour les installations existantes (comptes, réglages, données).

## 5. Questions ouvertes (à trancher avant de coder)

- **Forme du Core** : un dépôt qui fournit un crate Rust (`strata-core`) et un paquet web (`@strata/ui`), consommés en dépendance par chaque appli ? Ou autre chose (sous-module git, modèle à copier) ?
- **Périmètre du premier lot** : commencer par le design seul (jetons CSS, thème, police, `Brand`, composants de base), puis les fonctions serveur ? Ou les deux d'un coup ?
- **Rôles et droits** : modèle unique pour toutes les applis (groupes + droits déclarés par chaque appli, comme Cut) ?
- **Plateformes** : le Core doit-il couvrir Linux, macOS et Windows comme CG, même si certaines applis sont Linux seulement ?
- **Langues** : français seulement, ou prévoir l'anglais dans le Core ?
- **Versions** : comment une appli suit-elle les versions du Core (tags, numéros de version) ?

## 6. Prochaines étapes

1. Renommer le dépôt `StrataCore` en `Strata-Core` (fait par Damien dans les réglages GitHub).
2. Valider ce brief et répondre aux questions ouvertes.
3. Faire l'inventaire détaillé, brique par brique, de la meilleure version entre CG et Cut.
4. Étendre l'inventaire à Multiview et Network pour vérifier qu'ils rentrent dans le même moule.
