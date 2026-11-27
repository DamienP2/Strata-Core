# Strata-Core — Brief de base

> Statut : brief, pas encore de code.
> Applis de référence : **CG** (Strata Graphics, v0.7.1) et **Strata-Cut** (Strata LiveCut, v0.2.0).

## 1. Le but

Toutes les applis Strata (Graphics, LiveCut, Multiview, Network, et bientôt Media et Call) ont **le même design** et **une partie de leurs fonctions en commun**. Aujourd'hui, ce socle est recopié d'une appli à l'autre, puis chaque copie évolue de son côté.

Strata-Core devient **le socle de chaque serveur physique Strata** :

- il porte **le design commun** de toutes les applis ;
- il gère **ce qui concerne le serveur et pas une appli en particulier** : les comptes, le matériel, la santé, les mises à jour ;
- il permet de **choisir les applis installées et actives sur chaque serveur**, ce qui personnalise chaque machine ;
- sa page web est **le cockpit** du serveur : on y ouvre les applis actives, et les administrateurs y règlent le serveur.

Une nouvelle appli (Media, Call) démarre directement sur le Core au lieu de recopier. Le Core ne contient **rien de métier** : pas de rendu graphique, pas d'enregistrement vidéo, pas de multiview.

## 2. Décisions prises

| Sujet | Décision |
| --- | --- |
| Langage du serveur | **Rust** |
| Plateforme | **Ubuntu Server 24.04 LTS**, x86-64, sans bureau. **26.04 LTS** ajoutée une fois validée (pilote Desktop Video de Blackmagic disponible pour son noyau). **Pas de 22.04** : les serveurs existants passent en 24.04 |
| Architecture | **option A** : le Core est un service à part (`strata-core`) ; chaque appli est son propre service, installé, activé et mis à jour par le Core |
| Adresses | **un nom par serveur pour le Core, un sous-domaine par appli** : `serveur.local` → cockpit, `livecut.serveur.local` → LiveCut (§ 4) |
| Forme du code partagé | un **crate Rust** (dépendance git sur une étiquette de version de Strata-Core) + un **paquet web** (React / TypeScript, fichier `strata-ui-<version>.tgz` joint à chaque release de Strata-Core) |
| Compilation | binaires construits sur **Ubuntu 24.04** par GitHub Actions, publiés en **releases GitHub** ; jamais compilés sur les serveurs. Les paquets des applis sont refaits pour 24.04 |
| Installateur | **programme Rust** plein terminal (§ 5.7) |
| Onglets | le cockpit ouvre **chaque appli dans son propre onglet** du navigateur (un onglet par appli, réutilisé) |
| Plusieurs serveurs | **un seul cockpit** pour tous les serveurs : un serveur **maître** affiche le cockpit et propose les applis des serveurs **membres** ; alertes et ressources restent sur chaque machine (§ 4 bis) |
| Organisation | **proposition C** des maquettes : cockpit en lanceur (tuiles par appli), bouton ⊞ dans toutes les barres pour changer d'appli, réglages derrière la roue dentée, rangés par objet (Ce serveur, Serveurs, Applis, Personnes, Matériel, Surveillance) |
| Cockpit | chaque utilisateur n'y voit **que les applis actives où il a des droits** ; chaque appli répond au Core « cet utilisateur a-t-il accès ? » |
| Comptes | le Core gère **qui sont les utilisateurs** (comptes, SSO, groupes) ; **chaque appli gère ce qu'ils peuvent y faire** (autorisations des utilisateurs et des groupes dans ses propres réglages) |
| Matériel | **partagé** : une ressource a **la même fonction pour toutes les applis** (une entrée SDI reste une entrée pour toutes) |
| Versions | Core en versions sémantiques ; chaque appli déclare la plage de versions du Core qu'elle accepte (§ 6) |
| Migration | **aucune** : on repart de zéro, les serveurs existants sont réinstallés |
| Premier lot | **le design**, à partir du code de **CG et de Cut** — inventaire : [`docs/design/INVENTAIRE.md`](docs/design/INVENTAIRE.md) |

## 3. Le cockpit — la page web du Core

La page qu'on ouvre en tapant le nom du serveur (`serveur.local`).

**Pour tout le monde**, après la connexion :
- les **applis actives où il a des droits**, en grandes tuiles : icône, nom, état (en marche, en erreur) ; un clic ouvre l'appli **dans un nouvel onglet**, à son adresse (`livecut.serveur.local`). Chaque appli a son onglet attitré : recliquer sur la tuile ramène à l'onglet déjà ouvert au lieu d'en créer un autre ; le cockpit reste ouvert dans le sien. Les droits étant gérés dans les applis, chaque appli répond au Core « cet utilisateur a-t-il accès ? » ;
- la barre du haut commune : marque, pastille de connexion, menu utilisateur (langue, thème, déconnexion).

**Pour les administrateurs**, en plus, l'accès aux **réglages du serveur** :

| Réglage | Contenu |
| --- | --- |
| Utilisateurs | comptes, SSO, groupes (§ 5.2) |
| Hardware | 2110, cartes réseau, DeckLink, NDI (§ 5.3) |
| Bibliothèque | installer, activer, désactiver les applis (§ 5.4) |
| Santé | journaux, ressources, alertes, SNMP (§ 5.5) |
| Serveur | nom, adresses, HTTPS, mises à jour (§ 5.6) |

**Dans chaque appli**, la barre du haut commune garde un accès au cockpit et aux autres applis actives où l'utilisateur a des droits : on passe de LiveCut à Graphics sans repasser par l'accueil.

Une appli **inactive** n'apparaît pas dans le cockpit ; seuls les administrateurs la voient, dans la Bibliothèque.

Avant d'écrire le code, il faut une **maquette HTML du cockpit et de la barre commune**, à valider (règle d'interface n° 7).

## 4. Les adresses

- **Le Core** répond au **nom du serveur** : `serveur.local` → le cockpit.
- **Chaque appli active** répond à **son sous-domaine** : `livecut.serveur.local`, `graphics.serveur.local`, `multiview.serveur.local`…
- **Une seule porte d'entrée** : le Core écoute sur les ports 80 / 443 et relaie chaque requête vers le service de l'appli selon le nom demandé. Les applis écoutent sur des ports internes, jamais exposés.
- **Une seule connexion** : la session est valable pour le nom du serveur et tous ses sous-domaines. On se connecte une fois au cockpit, on est connecté dans toutes les applis.
- **HTTPS** : un certificat pour `serveur.local` et `*.serveur.local` (fait par le Core, ou celui de l'entreprise).
- **Thème et langue** : chaque sous-domaine est une origine différente pour le navigateur. Le choix est donc **gardé sur le serveur, par utilisateur**, et relu à l'ouverture de chaque appli.

**Résolution des noms** — point d'attention :
- **`.local` passe par mDNS** (Avahi sur le serveur). `serveur.local` marche presque partout ; les noms à plusieurs niveaux comme `livecut.serveur.local` sont mal résolus par Windows et souvent bloqués sur les réseaux d'entreprise.
- **Recommandation** : un **nom DNS de l'entreprise** par serveur, avec **un enregistrement générique** (`*.serveur.domaine` → adresse du serveur). Une seule ligne dans le DNS par serveur, et toutes les applis, présentes et futures, sont joignables.
- mDNS (`.local`) reste proposé pour les tests et les petits réseaux.
- **Par adresse IP**, seul le cockpit répond ; ouvrir une appli demande le nom.

## 4 bis. Plusieurs serveurs : un seul cockpit

Quand une régie a plusieurs serveurs physiques, on veut **un seul cockpit** qui propose toutes les applis de tous les serveurs.

### Deux rôles

| | **Maître** | **Membre** |
| --- | --- | --- |
| Cockpit | affiché : les applis de **tous** les serveurs | éteint : son adresse renvoie vers le cockpit du maître |
| Connexion des utilisateurs | **c'est lui qui connecte** (comptes, groupes, SSO) | délègue au maître |
| Applis, matériel, santé, alertes, mises à jour | les siens | les siens, **sur la machine** |
| Réglages du serveur | les siens + la liste des serveurs | les siens (administrateurs) |

Un serveur seul est un maître sans membre : rien ne change pour lui. **Activer ou non le cockpit**, c'est donc choisir le rôle du serveur (Réglages › Serveur › Cockpit, et une question de l'installateur).

### Rattacher un serveur

1. Sur le maître : *Serveurs › Ajouter un serveur* donne un **code d'invitation** à usage unique (valable quelques minutes).
2. Sur le nouveau serveur (réglages ou installateur) : *Rejoindre un cockpit*, adresse du maître + code.
3. Les deux serveurs échangent leurs clés ; ensuite, ils se parlent en HTTPS authentifié dans les deux sens. Le code ne sert plus.

Détacher un membre le rend à nouveau autonome (son cockpit se rallume).

### Le cockpit avec plusieurs serveurs

- **Une tuile par appli**, pas par serveur. Une appli présente sur plusieurs serveurs porte un bouton par serveur, au **nom d'usage** du serveur (« Régie A », « Régie B », réglé dans *Nom et adresses*). Les utilisateurs ne voient jamais le nom de machine (`strata01`), réservé aux administrateurs.
- **Utilisateurs** : seulement *disponible* ou *indisponible* (serveur injoignable).
- **Administrateurs** : en plus, le nom du serveur sous chaque tuile, un point de couleur par serveur (ok, alerte, injoignable), et la liste des serveurs (nom, nom de machine, nombre d'alertes, injoignable) ; un clic ouvre les réglages du serveur dans son propre onglet.
- Validé avec la maquette interactive : [`docs/maquettes/cockpit.html`](docs/maquettes/cockpit.html) (proposition C).

### Ce que le maître sait des membres

- **Seulement ce qu'il faut au cockpit** : nom du serveur, applis actives, leurs adresses, leur état (en marche, injoignable).
- Le **membre ouvre la connexion vers le maître** (pas l'inverse) et la garde ouverte : un pare-feu entre les deux ne gêne que dans un sens, et le maître voit tout de suite un membre qui disparaît.
- Pour savoir quelles applis montrer à un utilisateur, le maître demande au membre, qui demande à ses applis « cet utilisateur a-t-il accès ? ».
- **Alertes, ressources, journaux, matériel, mises à jour restent sur chaque machine.** Dans le cockpit, la liste des serveurs (administrateurs) donne pour chacun : joignable ou non, et un lien vers **ses propres réglages**, qui s'ouvrent dans un nouvel onglet.

### Une seule connexion pour tous les serveurs

Les applis d'un membre ont leur adresse sur ce membre (`livecut.strata02.regie.exemple.fr`). Le cookie de session du maître n'y est pas valable (autre nom de domaine). Le maître joue donc le rôle d'**un fournisseur d'identité, comme Okta** :

1. l'utilisateur ouvre LiveCut sur `strata02` depuis le cockpit ;
2. LiveCut n'a pas encore de session : il renvoie vers le maître ;
3. le maître, où l'utilisateur est déjà connecté, renvoie aussitôt vers LiveCut avec un jeton signé (identité + groupes) ;
4. LiveCut ouvre sa session. L'utilisateur ne voit qu'un aller-retour de quelques millisecondes.

Les comptes, les groupes et la configuration SSO ne sont gérés **que sur le maître**.

### Si le maître tombe

**L'antenne ne dépend jamais du maître.** Les applis des membres continuent de tourner ; les sessions déjà ouvertes continuent. Seuls le cockpit et les nouvelles connexions sont touchés. Pour ne pas bloquer une régie :
- chaque membre garde un **compte administrateur local de secours** ;
- si le SSO de l'entreprise est utilisé, le maître transmet sa configuration aux membres, qui peuvent **connecter directement par le SSO** quand le maître ne répond pas.

### Ce qui reste propre à chaque machine

- Le matériel : une entrée SDI de `strata01` n'est utilisable que par les applis de `strata01` (entre serveurs, les images passent par NDI ou ST 2110).
- Les mises à jour : chaque serveur met à jour son Core et ses applis.

## 5. Les briques du Core

### 5.1 DESIGN — le design commun

L'identité visuelle de toutes les applis, au même endroit. Détail et décisions : [`docs/design/INVENTAIRE.md`](docs/design/INVENTAIRE.md).

- **Jetons** : couleurs, rayon, échelle typographique, hauteurs de contrôle, police mono, y compris les jetons `--op-*`.
- **Thèmes** : sombre par défaut, clair, ou système.
- **Police** : Inter (400 à 800), embarquée.
- **Signature** : Strata + Le Figaro AudioVisuel (`Brand`).
- **Composants** : `StatusPill`, `Empty`, `InfoTip`, `Dropdown`, `ContextMenu`, dialogues, champs de formulaire, menu utilisateur, infobulles, mise en page des réglages, panneau latéral, toasts, bandeau de plantage, raccourcis.
- **Coquille commune** : barre du haut, passage d'une appli à l'autre, menu utilisateur, page de connexion.
- **Langue** : français, avec l'outil de contrôle des traductions (`i18n-check`).

### 5.2 UTILISATEURS — qui sont les utilisateurs

Le Core gère **l'identité**. Les **autorisations** sont dans chaque appli.

**Dans le Core :**
- **Comptes locaux** : mots de passe Argon2, sessions (cookie HttpOnly valable sur tous les sous-domaines, `Bearer` pour l'automation), commande `user` en ligne de commande.
- **SSO** : OpenID Connect avec PKCE (Okta, Microsoft Entra, Keycloak, Google, ADFS…) ; les groupes de l'annuaire arrivent avec l'utilisateur.
- **Groupes** : créés dans le Core ou venus de l'annuaire.
- **Administrateur du serveur** : le seul droit géré par le Core ; il donne accès aux réglages du cockpit.
- **Préférences** de chaque utilisateur (langue, thème, affichage), relues par toutes les applis.
- **Sécurité des requêtes** : CSRF, limitation des tentatives de connexion, en-têtes.
- Le Core **fournit aux applis** la liste des utilisateurs et des groupes, et dit qui est connecté.

**Dans chaque appli :** les **autorisations des utilisateurs et des groupes**, dans ses propres réglages (par exemple : qui peut lancer un direct dans LiveCut, qui peut publier un modèle dans Graphics).

Sources : `auth.rs`, `sso.rs`, `guard.rs`, `prefs.rs` (CG et Cut), `groups.rs` + `users.rs` (Cut). Le modèle de droits de Cut (`roles.rs`) sert de modèle aux autorisations **des applis**.

### 5.3 HARDWARE — le matériel du serveur

Le matériel appartient au serveur : le Core le découvre, le règle une fois, et **toutes les applis le voient avec la même fonction**. Une entrée SDI réglée en entrée est une entrée pour toutes les applis ; on ne la règle pas appli par appli.

- **ST 2110** : flux, adresses multicast, synchronisation PTP (ptp4l / phc2sys) et NTP.
- **Cartes réseau** : interfaces, adresses, rôle de chaque carte (gestion, flux, 2110…).
- **DeckLink** : cartes Blackmagic, connecteurs SDI / HDMI, fonction de chaque connecteur (entrée ou sortie), nom, format ; pilote Desktop Video.
- **NDI** : bibliothèque, sources visibles sur le réseau.
- **Représentation** : chaque carte (DeckLink, réseau) est rendue en 3D réaliste (three.js : matières, éclairage, ombre), en vue isométrique, selon son modèle, avec ses connecteurs numérotés comme sur l'équerre et comme le logiciel les voit, à la couleur de leur fonction.
- **ST 2110** : les deux ports média servent soit en **SMPTE 2022-7** (rouge et bleu, chaque flux sur les deux), soit comme **deux ports indépendants** (chaque flux sur l'un ou l'autre).

**Les entrées DeckLink passent par le Core.** Le pilote Blackmagic ne laisse **qu'un seul programme à la fois** capter une entrée : le second se voit refuser l'entrée. Le SDK le prévoit (état `bmdDeviceCaptureBusy`), et CG comme LiveCut le rencontrent déjà (« cannot enable the video input (in use by another program?) » dans leur code DeckLink). Avec un service par appli, deux applis ne peuvent donc pas ouvrir la même entrée SDI chacune de leur côté. Le Core **capte chaque entrée DeckLink une seule fois** et **la redistribue** aux applis sur la machine (mémoire partagée, sans copie réseau ni réencodage). Pour l'utilisateur, rien ne change : une entrée est une entrée pour toutes les applis.

Fait : le crate `strata-share` (un fichier `/dev/shm/strata-sdi-<carte>-<n>` par entrée, anneau de 3 images UYVY gardées par un verrou de séquence, réveil par futex, lecteurs inscrits avec le nom de leur appli) ; le Core ouvre et ferme les captures selon les réglages et les cartes présentes ; *Cartes SDI* montre pour chaque entrée son image, son format, son timecode et les applis qui la lisent. Contrat des applis : docs/APPLIS.md. Reste : l'audio (le pont DeckLink ne le capte pas encore) et la mesure sur une vraie carte (latence, charge).

- **Entrées NDI et 2110** : plusieurs applis peuvent les recevoir directement (réseau, multicast) ; le Core ne fait que les décrire.
- **Sorties SDI** : une seule appli à la fois les pilote ; le Core montre laquelle.

**L'horloge du site** (*Matériel › Horloge*). Un leader unique fournit BB, PTP, TC et WC.

- **BB** (ou tri-level) vers la REF de chaque DeckLink, genlock activé : les DeckLink SDI ne se calent pas sur le PTP.
- **PTP** vers **tous** les serveurs. ptp4l (profil ST 2059-2, esclave) tient l'horloge de la carte réseau PTP ; **chrony** règle l'heure du serveur sur cette horloge, les serveurs NTP en secours (ou le NTP seul). Un seul service règle l'heure : un phc2sys qui la règle aussi est signalé. Le Core écrit `/etc/chrony/conf.d/strata.conf`, `/etc/linuxptp/strata-ptp4l.conf` et le service `strata-ptp4l`, et relance ce qui change ; sans droits, il le dit (l'installateur les donnera).
- **TC** : pas de câble LTC. Le **timecode maison** se calcule depuis l'heure du serveur (`strata_core::timecode`) : heure locale, UTC ou décalage fixe, comme l'LTC du leader ; en 29,97 / 23,98, recalage quotidien (ST 2059-1) et drop frame.
- **WC** : pour les appareils audio, pas pour les serveurs (l'audio SDI suit la carte, l'audio 2110 le PTP).
- **Cadence maison** (25, 29,97, 30, 24, 23,98) : celle de toutes les images du site.
- **Bilan** : l'heure suit-elle le PTP, les REF sont-elles verrouillées et à la cadence maison, le timecode de chaque entrée SDI est-il celui de la maison.

Sources : `hardware.rs`, `sync.rs`, `decklink.rs`, `ndi.rs` (CG), `settings/Hardware.tsx` et le shim DeckLink (CG et Cut, même SDK).

### 5.4 BIBLIOTHÈQUE — les applis du serveur

L'endroit où l'on choisit **ce que fait ce serveur physique** (administrateurs).

- Liste des applis Strata disponibles, avec leur description et leur version.
- **Installée et active sont deux choses distinctes.** Une appli installée mais inactive est sur le disque et à jour, mais son service ne tourne pas et ne consomme rien. On l'**active à la demande**, sans réinstaller.

  | État | Sur le disque | Service | Mises à jour | Cockpit et santé |
  | --- | --- | --- | --- | --- |
  | Non installée | non | — | — | absente |
  | Installée, inactive | oui | arrêté, ne démarre pas avec le serveur | oui | absente (visible dans la Bibliothèque) |
  | Active | oui | en marche, démarre avec le serveur | oui | présente |

- Actions : **Installer**, **Activer**, **Désactiver**, **Désinstaller** (les données de l'appli sont gardées, sauf demande expresse).
- Le Core refuse d'installer ou d'activer une appli qui ne correspond pas à sa version (§ 6), en disant pourquoi.
- Ce que le Core attend d'une appli (fiche, lancement, en-têtes d'identité) : [docs/APPLIS.md](docs/APPLIS.md).

### 5.5 SANTÉ — surveiller le serveur

- **Journaux** : le log en mémoire, le journal sur disque (30 jours), les rapports de plantage (90 jours), l'export pour le support, pour le Core et pour chaque appli.
- **Ressources** : processeurs, mémoire, disques, réseau ; ce que consomme chaque appli ; ce que la machine peut encore prendre.
- **Alertes** : webhook (Slack, Teams, Discord, Mattermost, JSON) et e-mail (SMTP), envoyées quand un problème dure et quand il est fini. Les applis peuvent y déclarer leurs propres alertes. Une carte DeckLink qui n'est plus vue (après une mise à jour du noyau, par exemple) déclenche une alerte.
- **SNMP** : exposer l'état du serveur et des applis aux outils de supervision de la régie (nouveau).

Sources : `health.rs`, `logs.rs` (CG et Cut), `perf.rs` (CG), `resources.rs`, `alerts.rs` (Cut).

### 5.6 SERVEUR — réglages et mises à jour OTA

- **Mises à jour OTA depuis GitHub** : pour **le Core et pour chaque appli installée**, sans désinstaller ni perdre les données.
- **Une clé par dépôt** : le Core et chaque appli ont leur clé GitHub (jeton en lecture seule pour les dépôts privés), gardée sur le serveur et jamais envoyée au navigateur.
- **Mise à jour hors ligne** : envoi d'une archive de release par un admin.
- **Réglages du serveur** (*Ce serveur*) : nom (et donc les adresses des applis, § 4), HTTPS / certificats, redémarrage, service systemd.
  - **HTTPS** : *fait par Strata* (une autorité par serveur, à installer une fois sur chaque poste ; le certificat de `<nom>` et `*.<nom>` est refait tout seul), *entreprise* (certificat importé, qui doit couvrir les mêmes noms), ou *relais* devant le Core. Un certificat qui bloque l'accès se retire par `sudo strata-core https off`.

Sources : `update.rs` (CG, le plus complet), `service.rs`, `tls.rs`, `paths.rs`, `config.rs`, `system.rs`.

### 5.7 INSTALLATEUR — l'installation sur Linux

Un **installateur en ligne de commande, mais graphique** : un écran plein terminal au design Strata, lancé en SSH d'une seule commande, piloté au clavier.

**Les étapes :**

1. **Accueil** : la marque Strata, la version qui va être installée.
2. **Vérification du système** : Ubuntu 24.04, processeur, mémoire, disque, accès internet, chacun ✓ ou ✗. Un ✗ bloquant arrête là, avec la raison.
3. **Nom du serveur** : le nom qui donne les adresses (§ 4) ; l'installateur vérifie s'il est résolu et, sinon, dit quel enregistrement DNS créer.
4. **Applis** : pour chacune, *installer* et *activer maintenant*.
5. **Matériel** :
   - **DeckLink** : les cartes Blackmagic présentes sont détectées. Le pilote **Desktop Video** (qui contient la bibliothèque du SDK DeckLink) n'est pas redistribuable : Blackmagic le donne après un formulaire. L'installateur montre les étapes (télécharger sur son ordinateur, copier sur le serveur par `scp`, avec la commande exacte), **attend l'archive**, la vérifie, l'installe et l'enregistre dans **DKMS** (reconstruit à chaque mise à jour du noyau). Possible de passer.
   - **NDI** : le **SDK NDI** pour Linux est téléchargé, sa licence est affichée et doit être acceptée, puis la bibliothèque `libndi` est installée. Possible de passer.
6. **Réglages** : dossier des données, disque dédié éventuel, ports à ouvrir dans le pare-feu.
7. **Compte administrateur** : nom et mot de passe.
8. **Récapitulatif**, puis **Installer** : une ligne par étape, avec sa progression et son résultat.
9. **Fin** : l'adresse du cockpit, et ce qui reste à faire (redémarrer pour le pilote Blackmagic, créer l'enregistrement DNS…).

**Relancé sur un serveur déjà installé**, il propose : *Mettre à jour*, *Ajouter ou retirer des applis*, *Installer DeckLink / NDI*, *Réparer*, *Désinstaller* (données gardées, sauf demande expresse).

**Sans écran** (automatisation) : les mêmes choix passent en options (`--name`, `--apps`, `--admin`, `--ndi`, `--desktop-video <archive>`, `--yes`…).

**Comment il est fait :**

1. **`install.sh`**, une trentaine de lignes (la commande `curl … | sudo bash`) : vérifie Ubuntu et x86-64, télécharge `strata-install` depuis les releases GitHub de Strata-Core, vérifie son empreinte SHA-256, le lance.
2. **`strata-install`**, un binaire Rust **statique**, affichage plein terminal avec **ratatui**, aux couleurs Strata.
3. Il installe des **binaires déjà compilés**, pris dans les releases GitHub. Serveur sans internet : `--package <archive>`.
4. Il utilise **le même code que le Core** (releases, compte administrateur, service systemd, dossiers) : installation et mises à jour OTA ne font qu'une seule et même chose.
5. **Journal complet** : `/var/log/strata-install.log`.

**Préalable : la chaîne de releases.** Le Core et chaque appli sont compilés par GitHub Actions sur Ubuntu 24.04 et publiés en releases, avec leurs empreintes. Modèle : `CG/.github/workflows/release.yml`.

Sources : `Strata-Cut/scripts/install-ubuntu.sh` (dépendances, pilote Desktop Video avec attente de l'archive, SDK NDI, service systemd, disque des données, pare-feu, désinstallation) ; `CG/packaging/ndi/fetch-runtime.sh` (licence et mention NDI).

## 6. Les versions du Core et des applis

- **Le Core suit les versions sémantiques** `MAJEUR.MINEUR.CORRECTIF` :
  - *correctif* : corrections, rien ne change pour les applis ;
  - *mineur* : ajouts, les applis existantes continuent de marcher ;
  - *majeur* : le contrat avec les applis change (API, crate, paquet web) ; les applis doivent suivre.
- **Le crate Rust et le paquet web portent le même numéro que le Core.**
- **Chaque release d'appli contient une fiche** (`strata-app.toml`) : nom, version, et la **plage de versions du Core acceptée**, par exemple `core = ">=1.4, <2"`.
- **Le Core s'en sert partout** :
  - il refuse d'installer ou d'activer une appli incompatible, en disant pourquoi (« demande le Core 1.5 ou plus ») ;
  - il ne propose la mise à jour d'une appli que si la nouvelle version accepte le Core en place ;
  - avant de se mettre à jour lui-même, il prévient si une appli installée ne l'accepterait pas.

## 7. Ce qui reste dans chaque appli

- Tout le métier : le moteur de rendu, le designer et l'opérateur de CG ; l'enregistrement, les extraits et le streamer de LiveCut ; et pour les autres applis, leur propre domaine.
- **Les autorisations** de ses utilisateurs et de ses groupes.
- Ses réglages propres.

Une appli s'appuie sur le Core, jamais l'inverse.

## 8. Questions ouvertes


- **Entrées DeckLink redistribuées par le Core** (§ 5.3) : en place (mémoire partagée) ; à mesurer sur une machine avec une carte (latence, charge processeur, plusieurs applis sur la même entrée).
- **Référence de synchronisation** : un leader unique (BB, PTP, TC, WC). BB vers la REF des DeckLink ; PTP vers tous les serveurs ; timecode maison calculé depuis le PTP ; WC pour les appareils audio seulement (§ 5.3, *Horloge*). Reste à vérifier sur un vrai réseau PTP : l'option `offset -37` de chrony sur l'horloge de la carte, et ptp4l avec chaque famille de carte (ConnectX, E810, I350).
- **Configuration minimale** : à **calculer** en mesurant, sur un serveur de référence, le Core seul puis chaque appli à vide et en charge typique. Le README donnera le minimum du Core et ce qu'ajoute chaque appli.

## 9. Prochaines étapes

1. ~~Renommer le dépôt en `Strata-Core`.~~ Fait.
2. ~~Trancher l'architecture.~~ Option A, cockpit, sous-domaines.
3. **Premier lot : DESIGN.** ~~Inventaire, décisions, maquette du cockpit.~~ Faits. Paquet `@strata/ui` ([`packages/ui`](packages/ui)) : design, composants et coquille du cockpit écrits ; reste à le brancher sur le serveur du Core (données réelles du cockpit) et à y passer une première appli.
4. **Chaîne de releases** : en place ([`docs/RELEASES.md`](docs/RELEASES.md)) pour le paquet web et le serveur `strata-core` ; l'installateur s'y ajoutera. Une étiquette `v*` publie la release, avec sa fiche `release.json` et ses empreintes.
5. **Serveur `strata-core`** ([`crates/`](crates), interface dans [`web/`](web)) : commencé. Fait : comptes locaux et sessions (cookie valable pour les sous-domaines), premier administrateur, garde des requêtes, préférences, applis installées et actives, cockpit réel ; réglages **Bibliothèque** (applis du serveur, une page par appli, activer et désactiver) et **Utilisateurs** (créer, modifier, administrateur ou non, mot de passe, désactiver, supprimer) ; **Groupes** (reliés aux groupes de l'annuaire) et **Annuaire** (SSO OpenID Connect : Entra, Okta, Keycloak, Google ; groupes de l'annuaire → groupes du Core et administrateurs ; comptes locaux autorisés ; domaines). **Lancement des applis et relais** : le Core lance chaque appli active (commande `[run]` de sa fiche, variables `STRATA_*`), la relance si elle s'arrête, garde son journal ; il relaie `<appli>.<serveur>` vers son port interne (HTTP et WebSocket), exige la connexion (sauf chemins publics), transmet l'identité par en-têtes `X-Strata-*` ; état du service, relance et journal dans la page de chaque appli. Contrat des applis : [docs/APPLIS.md](docs/APPLIS.md) ; les applis actuelles, autonomes, y seront amenées une par une. **Installer depuis la Bibliothèque** : depuis le dépôt GitHub d'une appli (release la plus récente qui accepte ce Core, empreinte vérifiée, clé par dépôt privé gardée sur le serveur) ou depuis une archive envoyée ; mise à jour (rechercher, mettre à jour, relance si active) ; désinstallation (données gardées sauf demande). **Ce serveur** : vue d'ensemble ; nom, nom DNS (vérification de la résolution, enregistrements à demander) et fuseau horaire, adresses du cockpit et des applis ; HTTPS au choix (aucun, fait par Strata avec l'autorité du serveur à installer sur les postes et un certificat renouvelé tout seul, certificat de l'entreprise importé, ou relais devant le Core), appliqué sans redémarrer, le HTTP renvoyant vers le HTTPS ; redémarrage du Core sans perdre les sessions ; secours `strata-core https off`. **Mises à jour** : recherche des nouvelles versions du Core et des applis (à la demande, et chaque jour si voulu, sans rien installer seul) ; mise à jour du Core depuis GitHub ou une archive envoyée (empreinte vérifiée, nouveau programme essayé, ancien gardé en `strata-core.previous`, redémarrage, sessions gardées), avec l'avertissement des applis qui n'accepteraient pas la nouvelle version ; clé par dépôt. **Matériel** (repris de CG) : cartes DeckLink détectées par le SDK (pont C++ `crates/strata-decklink`, pilote chargé au lancement), dessinées en 3D isométrique selon leur modèle avec la numérotation imprimée et logicielle, fonction et nom de chaque connecteur, genlock ; cartes réseau (en 3D, regroupées par carte PCI) et leurs rôles, deux ports média ST 2110 en SMPTE 2022-7 ou indépendants ; NDI (bibliothèque, réglages partagés, sources) ; flux ST 2110 ; horloge (PTP, NTP) ; les applis lisent ces réglages (`/api/hardware/shared`). Mode simulé : `STRATA_FAKE_DECKLINK`, `STRATA_FAKE_NICS`. À suivre : capter les entrées DeckLink dans le Core et les partager entre les applis, la liste des applis Strata dans la Bibliothèque (quand elles publieront au format de [docs/APPLIS.md](docs/APPLIS.md)), matériel, santé, mises à jour, plusieurs serveurs, puis l'installateur (qui fera des applis des services systemd).
6. Puis, dans l'ordre à définir : UTILISATEURS, SERVEUR, SANTÉ, HARDWARE, BIBLIOTHÈQUE, INSTALLATEUR.
7. Mesures pour la configuration minimale.
