# Strata-Core — Brief de base

> Statut : brief, pas encore de code.
> Applis de référence : **CG** (Strata Graphics, v0.7.1) et **Strata-Cut** (Strata LiveCut, v0.2.0).

## 1. Le but

Toutes les applis Strata (Graphics, LiveCut, Multiview, Network, et bientôt Media et Call) ont **le même design** et **une partie de leurs fonctions en commun**. Aujourd'hui, ce socle est recopié d'une appli à l'autre, puis chaque copie évolue de son côté.

Strata-Core devient **le socle de chaque serveur physique Strata** :

- il porte **le design commun** de toutes les applis ;
- il gère **ce qui concerne le serveur et pas une appli en particulier** : les utilisateurs, le matériel, la santé, les mises à jour ;
- il permet de **choisir les applis installées sur chaque serveur** (Graphics, LiveCut…), ce qui personnalise chaque machine.

Une nouvelle appli (Media, Call) démarre directement sur le Core au lieu de recopier. L'utilisateur retrouve la même interface, les mêmes comptes et les mêmes réglages d'une appli à l'autre.

Le Core ne contient **rien de métier** : pas de rendu graphique, pas d'enregistrement vidéo, pas de multiview.

## 2. Décisions prises

| Sujet | Décision |
| --- | --- |
| Langage du serveur | **Rust** |
| Plateforme | **Linux uniquement** |
| Forme | un **crate Rust** + un **paquet web** (React / TypeScript), consommés par les applis |
| Premier lot | **le design** — inventaire : [`docs/design/INVENTAIRE.md`](docs/design/INVENTAIRE.md) |
| Architecture | **option A** : le Core est un service à part (`strata-core`) ; chaque appli est son propre service, installé et mis à jour par le Core, qui lui délègue la connexion |

Une spec du design et des fonctions de base existe déjà, tirée de CG : `Strata-Network/docs/design.md`. Strata-Cut et Strata-Network ont été construits à partir d'elle. C'est la base de travail du Core.

## 3. Les six briques du Core

### 3.1 DESIGN — le design commun

L'identité visuelle de toutes les applis, au même endroit.

- **Jetons** : couleurs (`--bg`, `--panel*`, `--line*`, `--text*`, `--accent*`, `--live`, `--pvw`, `--warn`, `--key`, `--mask`), rayon, échelle typographique (`--fs-11` à `--fs-20`), hauteurs de contrôle, police mono.
- **Thèmes** : sombre par défaut, clair, ou système ; le choix suit l'utilisateur d'une appli à l'autre.
- **Police** : Inter (400 à 800), embarquée.
- **Signature** : Strata + Le Figaro AudioVisuel (`Brand`).
- **Composants** : `StatusPill`, `Empty`, `InfoTip`, `Dropdown`, `ContextMenu`, dialogues, champs de formulaire, menu utilisateur, infobulles, mise en page des réglages, panneau latéral, toasts, bandeau de plantage.
- **Coquille commune** : en-tête, navigation, menu utilisateur (thème, langue, déconnexion), page de connexion.
- **Langue** : français, avec l'outil de contrôle des traductions (`i18n-check`).

État actuel : déjà aligné entre CG et Cut (jetons identiques valeur pour valeur, `Brand` identique à 97 %). C'est le premier lot.

### 3.2 UTILISATEURS — comptes, SSO, rôles, autorisations

- **Comptes locaux** : mots de passe Argon2, sessions (cookie HttpOnly pour le navigateur, `Bearer` pour l'automation), commande `user` en ligne de commande.
- **SSO** : OpenID Connect avec PKCE (Okta, Microsoft Entra, Keycloak, Google, ADFS…), rôle déduit des groupes de l'annuaire.
- **Rôles et groupes** : un admin peut tout ; les autres reçoivent ce que leurs groupes leur donnent.
- **Autorisations, avec un onglet par appli installée** : chaque appli déclare au Core la liste de ses droits ; l'écran Autorisations montre un onglet **Core** plus un onglet par appli présente sur le serveur. Une appli désinstallée, son onglet disparaît.
- **Préférences** de chaque utilisateur (langue, thème, affichage), pareilles sur tous les navigateurs et toutes les applis.
- **Sécurité des requêtes** : CSRF, limitation des tentatives de connexion, actions réservées à la machine locale, en-têtes.

Sources : `auth.rs`, `sso.rs`, `guard.rs`, `prefs.rs` (CG et Cut), `roles.rs` + `groups.rs` + `users.rs` (Cut, le modèle par groupes et droits le plus fin).

### 3.3 HARDWARE — le matériel du serveur

Le matériel appartient au serveur, pas à une appli : le Core le découvre, le décrit et dit à quelle appli chaque ressource est attribuée.

- **ST 2110** : flux, adresses multicast, synchronisation PTP (ptp4l / phc2sys) et NTP.
- **Cartes réseau** : interfaces, adresses, rôle de chaque carte (gestion, flux, 2110…).
- **DeckLink** : cartes Blackmagic, connecteurs SDI / HDMI, ce que fait chaque connecteur, pilote Desktop Video.
- **NDI** : runtime, sources visibles sur le réseau.

Sources : `hardware.rs`, `sync.rs`, `decklink.rs`, `ndi.rs` (CG), `settings/Hardware.tsx` et le shim DeckLink (CG et Cut, même SDK).

### 3.4 BIBLIOTHÈQUE — les applis installées sur ce serveur

L'endroit où l'on choisit **ce que fait ce serveur physique**.

- Liste des applis Strata disponibles (Graphics, LiveCut, Multiview, Network, Media, Call…), avec leur description et leur version.
- **Installer / désinstaller / activer / désactiver** une appli sur ce serveur.
- Pour chaque appli installée : sa version, son état (en marche, arrêtée, en erreur), le lien pour l'ouvrir.
- Une appli installée apparaît dans la navigation commune, dans les autorisations (§ 3.2), dans la santé (§ 3.5) et dans les mises à jour (§ 3.6).

### 3.5 SANTÉ — surveiller le serveur

- **Journaux** : le log en mémoire (dernières lignes), le journal sur disque (30 jours), les rapports de plantage (90 jours), l'export pour le support — pour le Core et pour chaque appli.
- **Ressources** : processeurs, mémoire, disques, réseau ; ce que consomme chaque appli ; ce que la machine peut encore prendre.
- **Alertes** : webhook (Slack, Teams, Discord, Mattermost, JSON) et e-mail (SMTP), envoyées quand un problème dure et quand il est fini. Les applis peuvent y déclarer leurs propres alertes.
- **SNMP** : exposer l'état du serveur et des applis aux outils de supervision de la régie (nouveau, n'existe dans aucune appli aujourd'hui).

Sources : `health.rs`, `logs.rs` (CG et Cut), `perf.rs` (CG), `resources.rs`, `alerts.rs` (Cut).

### 3.6 SERVEUR — réglages et mises à jour OTA

- **Mises à jour OTA depuis GitHub** : vérification et installation depuis les releases, pour **le Core et pour chaque appli installée**, sans désinstaller ni perdre les données.
- **Une clé par dépôt** : chaque appli et le Core ont leur clé GitHub (jeton en lecture seule pour les dépôts privés), gardée sur le serveur et jamais envoyée au navigateur.
- **Mise à jour hors ligne** : envoi d'une archive de release par un admin (serveurs sans internet).
- **Réglages du serveur** : nom, ports, HTTPS / certificats, redémarrage à chaud, service systemd.

Sources : `update.rs` (CG le plus complet : archive hors ligne, jeton), `service.rs`, `tls.rs`, `paths.rs`, `config.rs`, `system.rs`.

## 4. Ce qui reste dans chaque appli

Tout le métier : le moteur de rendu, le designer et l'opérateur de CG ; l'enregistrement, les extraits et le streamer de LiveCut ; et pour les autres applis, leur propre domaine. Une appli s'appuie sur le Core, jamais l'inverse.

## 5. Questions ouvertes

- **Une seule adresse ou une par appli** : tout derrière le port du Core (`/graphics`, `/livecut`…) ou chaque appli sur son port ? (Cela joue aussi sur le thème et la langue d'une appli à l'autre : voir l'inventaire du design, § 6.)
- **Attribution du matériel** : une carte DeckLink ou un flux 2110 peut-il être partagé entre deux applis, ou est-il réservé à une seule ?
- **Migration** : comment reprendre les comptes, groupes et réglages des installations CG et LiveCut existantes.
- **Versions** : comment une appli déclare la version du Core dont elle a besoin.

## 6. Prochaines étapes

1. ~~Renommer le dépôt en `Strata-Core`.~~ Fait.
2. ~~Trancher l'architecture.~~ Option A.
3. **Premier lot : DESIGN.** ~~Inventaire.~~ Fait ([`docs/design/INVENTAIRE.md`](docs/design/INVENTAIRE.md)). Puis : décisions de l'inventaire, maquette de la coquille commune, paquet `@strata/ui`.
4. Puis, dans l'ordre à définir : UTILISATEURS, SERVEUR, SANTÉ, HARDWARE, BIBLIOTHÈQUE.
