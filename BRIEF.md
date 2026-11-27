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
| Plateforme | **Linux uniquement** : **Ubuntu Server 24.04 LTS**, x86-64, version Server (sans bureau). **26.04 LTS** ajoutée une fois validée (pilote Desktop Video de Blackmagic disponible pour son noyau). **Pas de 22.04** (support standard terminé en avril 2027) |
| Compilation | binaires construits sur **Ubuntu 24.04** (ils tournent aussi sur 26.04) par GitHub Actions, publiés en **releases GitHub** ; jamais compilés sur les serveurs |
| Installateur | **programme Rust** plein terminal (§ 3.7) |
| Forme | un **crate Rust** + un **paquet web** (React / TypeScript), consommés par les applis |
| Premier lot | **le design** — inventaire : [`docs/design/INVENTAIRE.md`](docs/design/INVENTAIRE.md) |
| Architecture | **option A** : le Core est un service à part (`strata-core`) ; chaque appli est son propre service, installé et mis à jour par le Core, qui lui délègue la connexion |

Une spec du design et des fonctions de base existe déjà, tirée de CG : `Strata-Network/docs/design.md`. Strata-Cut et Strata-Network ont été construits à partir d'elle. C'est la base de travail du Core.

## 3. Les briques du Core

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
- **Installée et active sont deux choses distinctes.** Une appli peut être installée sans être active : elle est sur le disque, à jour, mais son service ne tourne pas et elle ne consomme rien. On l'**active à la demande** (et on la désactive) depuis la Bibliothèque, sans réinstaller.

  | État | Sur le disque | Service | Mises à jour | Navigation, autorisations, santé |
  | --- | --- | --- | --- | --- |
  | Non installée | non | — | — | absente |
  | Installée, inactive | oui | arrêté, ne démarre pas avec le serveur | oui | grisée dans la Bibliothèque seulement |
  | Active | oui | en marche, démarre avec le serveur | oui | présente partout |

- Actions : **Installer**, **Activer**, **Désactiver**, **Désinstaller** (les données de l'appli sont gardées, sauf demande expresse).
- Pour chaque appli installée : sa version, son état (active, inactive, en erreur), le lien pour l'ouvrir.
- Une appli **active** apparaît dans la navigation commune, dans les autorisations (§ 3.2), dans la santé (§ 3.5). Toute appli **installée** apparaît dans les mises à jour (§ 3.6).

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

### 3.7 INSTALLATEUR — l'installation sur Linux

Un **installateur en ligne de commande, mais graphique** : un écran plein terminal, au design Strata (mêmes couleurs : fond sombre, accent pervenche, vert ok, orange attention, rouge erreur ; mêmes règles : français, libellés courts, rien d'inutile). On le lance en SSH, d'une seule commande, et on se déplace au clavier.

**Les étapes :**

1. **Accueil** : la marque Strata, la version qui va être installée.
2. **Vérification du système** : distribution, processeur, mémoire, disque, accès internet, chacun ✓ ou ✗. Un ✗ bloquant arrête là, avec la raison.
3. **Applis** : la liste des applis Strata ; pour chacune, *installer* et *activer maintenant* (une appli peut être installée sans être activée, § 3.4).
4. **Matériel** :
   - **DeckLink** : les cartes Blackmagic présentes sont détectées. Le pilote **Desktop Video** (qui contient la bibliothèque du SDK DeckLink) n'est pas redistribuable : Blackmagic le donne après un formulaire. L'installateur montre les étapes (télécharger sur son ordinateur, copier sur le serveur par `scp`, avec la commande exacte pour ce serveur), **attend l'archive**, la vérifie, l'installe et construit le module du noyau. Possible de passer.
   - **NDI** : le **SDK NDI** pour Linux est téléchargé, sa licence est affichée et doit être acceptée, puis la bibliothèque `libndi` est installée. Possible de passer.
5. **Réglages** : port de l'interface web (8080), dossier des données, disque dédié éventuel, ports à ouvrir dans le pare-feu.
6. **Compte administrateur** : nom et mot de passe.
7. **Récapitulatif**, puis **Installer** : une ligne par étape, avec sa progression et son résultat.
8. **Fin** : l'adresse de l'interface web, et ce qui reste à faire s'il y en a (redémarrer pour le pilote Blackmagic, par exemple).

**Relancé sur un serveur déjà installé**, il propose : *Mettre à jour*, *Ajouter ou retirer des applis*, *Installer DeckLink / NDI*, *Réparer*, *Désinstaller* (données gardées, sauf demande expresse).

**Sans écran** (automatisation) : les mêmes choix passent en options (`--apps`, `--admin`, `--ndi`, `--desktop-video <archive>`, `--yes`…), sans aucune question.

**Comment il est fait :**

1. **`install.sh`**, une trentaine de lignes, celui de la commande `curl … | sudo bash` : vérifie Ubuntu et x86-64, télécharge `strata-install` depuis les releases GitHub de Strata-Core, vérifie son empreinte SHA-256, le lance (en lui rendant le terminal).
2. **`strata-install`**, un binaire Rust **statique** (rien à installer avant), affichage plein terminal avec **ratatui**, aux couleurs Strata.
3. Il installe des **binaires déjà compilés**, pris dans les releases GitHub : pas de Rust ni de Node.js sur les serveurs, installation en quelques minutes, la version installée est exactement celle qui a été testée. Serveur sans internet : `--package <archive>`.
4. Il utilise **le même code que le Core** (téléchargement et vérification des releases, compte administrateur, service systemd, dossiers) : l'installation et les mises à jour OTA (§ 3.6) ne font qu'une seule et même chose.
5. **Pilote DeckLink et noyau** : le module Desktop Video est enregistré dans **DKMS**, pour être reconstruit à chaque mise à jour du noyau. La Santé (§ 3.5) alerte si une carte n'est plus vue.
6. **Journal complet** de chaque installation : `/var/log/strata-install.log`.

**Préalable : la chaîne de releases.** Le Core et chaque appli doivent être compilés par GitHub Actions (sur Ubuntu 24.04) et publiés en releases, avec leurs empreintes. Sans elle, ni l'installateur ni les mises à jour OTA ne fonctionnent. Modèle : `CG/.github/workflows/release.yml`.

Sources : `Strata-Cut/scripts/install-ubuntu.sh` (1 100 lignes : dépendances, pilote Desktop Video avec attente de l'archive, SDK NDI, service systemd, disque des données, pare-feu, désinstallation) ; `CG/packaging/ndi/fetch-runtime.sh` (licence et mention NDI).

## 4. Ce qui reste dans chaque appli

Tout le métier : le moteur de rendu, le designer et l'opérateur de CG ; l'enregistrement, les extraits et le streamer de LiveCut ; et pour les autres applis, leur propre domaine. Une appli s'appuie sur le Core, jamais l'inverse.

## 5. Questions ouvertes

- **Une seule adresse ou une par appli** : tout derrière le port du Core (`/graphics`, `/livecut`…) ou chaque appli sur son port ? (Cela joue aussi sur le thème et la langue d'une appli à l'autre : voir l'inventaire du design, § 6.)
- **Serveurs existants en 22.04** : en reste-t-il (LiveCut s'installe aujourd'hui sur 22.04) ? Ils devront passer en 24.04 avant de recevoir le Core.
- **Configuration minimale** du serveur (processeurs, mémoire, disque) : à fixer pour le README.
- **Attribution du matériel** : une carte DeckLink ou un flux 2110 peut-il être partagé entre deux applis, ou est-il réservé à une seule ?
- **Migration** : comment reprendre les comptes, groupes et réglages des installations CG et LiveCut existantes.
- **Versions** : comment une appli déclare la version du Core dont elle a besoin.

## 6. Prochaines étapes

1. ~~Renommer le dépôt en `Strata-Core`.~~ Fait.
2. ~~Trancher l'architecture.~~ Option A.
3. **Premier lot : DESIGN.** ~~Inventaire.~~ Fait ([`docs/design/INVENTAIRE.md`](docs/design/INVENTAIRE.md)). Puis : décisions de l'inventaire, maquette de la coquille commune, paquet `@strata/ui`.
4. **Chaîne de releases** (GitHub Actions sur Ubuntu 24.04), juste après le design : préalable de l'installateur et des mises à jour OTA.
5. Puis, dans l'ordre à définir : UTILISATEURS, SERVEUR, SANTÉ, HARDWARE, BIBLIOTHÈQUE, INSTALLATEUR.
