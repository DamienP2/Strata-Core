# Une appli Strata derrière le Core

Ce que le Core attend d'une appli (LiveCut, CG, Multiview, Network, Media, Call), et ce qu'il lui
donne. Les applis d'aujourd'hui sont autonomes ; chacune sera reprise pour suivre ce contrat.

## En bref

1. L'appli est **un service à part**, qui écoute sur **un port interne**, en `127.0.0.1` seulement.
2. Le Core est **la seule porte d'entrée** : `livecut.<serveur>` arrive au Core, qui relaie vers
   `127.0.0.1:<port>`, WebSocket compris.
3. L'utilisateur est **déjà connecté** : le Core l'a vérifié et dit qui il est par des en-têtes.
   L'appli n'a ni page de connexion ni comptes ; elle décide seulement des **droits** dans ses écrans.

## La fiche : `strata-app.toml`

Livrée avec chaque release de l'appli, posée dans `<données du Core>/apps/<id>/` avec ses fichiers.

```toml
id = "livecut"          # minuscules, chiffres, tirets : c'est aussi son sous-domaine
name = "LiveCut"
version = "0.3.0"
icon = "scissors"       # une icône lucide
port = 8101             # son port interne, unique sur le serveur, 1024 ou plus

[requires]
core = ">=0.3, <1"      # les versions du Core acceptées (semver)

[run]                   # comment le Core la lance
command = "bin/strata-livecut"   # relatif au dossier de l'appli, ou absolu
args = ["serve"]

[relay]
public = ["/output/"]   # débuts de chemin servis SANS connexion (sorties pour un mélangeur, OBS…)
```

- Sans `[run]`, le Core ne lance pas l'appli : elle est démarrée à part (pratique en développement,
  avec `cargo run`) et le Core ne fait que la relayer.
- Deux applis actives ne peuvent pas avoir le même port.

## Publier une release installable

Le Core installe et met à jour une appli depuis les **releases GitHub** de son dépôt (*Réglages ›
Bibliothèque › Installer une appli*), ou depuis la même archive envoyée à la main (serveur sans
internet). Une release d'appli contient :

| Fichier | Contenu |
| --- | --- |
| `<id>-<version>-linux-x86_64.tar.gz` | `strata-app.toml` et les fichiers de l'appli (binaire, interface…), à la racine ou dans un seul dossier |
| `release.json` | la fiche de la release : `name` = l'id, `version`, `requires.core`, et l'empreinte de chaque fichier |
| `SHA256SUMS` | les empreintes, pour un humain |

`release.json` et `SHA256SUMS` s'écrivent avec le script du Core, en lui donnant la fiche de l'appli :

```bash
node scripts/release-manifest.mjs dist 0.3.0 livecut strata-app.toml
```

Ce que fait le Core :

- il prend **la release la plus récente qui accepte sa version** (`requires.core`), hors brouillons et
  préversions (`v0.4.0-rc.1`) ;
- il télécharge l'archive et **refuse une empreinte différente** de celle de `release.json` ;
- il vérifie que l'archive contient bien l'appli et la version annoncées, qu'elle accepte le Core, et
  que son port n'est pas déjà pris par une autre appli ;
- il la met en place d'un coup, à la place de l'ancienne version, et **relance** l'appli si elle est
  active. Ses données (`STRATA_APP_DATA`) ne sont pas touchées.

Un dépôt privé demande **une clé** : un jeton GitHub en lecture seule sur ce dépôt, donné à
l'installation, gardé sur le serveur et jamais renvoyé au navigateur ; les mises à jour s'en servent.

## Lancement

Quand l'appli est **active**, le Core la lance : la commande `[run]`, dans le dossier de l'appli, avec
ces variables :

| Variable | Exemple | |
| --- | --- | --- |
| `STRATA_APP_ID` | `livecut` | |
| `STRATA_APP_PORT` | `8101` | le port de la fiche |
| `STRATA_APP_BIND` | `127.0.0.1:8101` | **l'adresse d'écoute à utiliser** |
| `STRATA_APP_URL` | `https://livecut.strata01.regie.fr/` | son adresse publique |
| `STRATA_APP_DATA` | `/var/lib/strata-core/app-data/livecut` | son dossier de données (créé par le Core) |
| `STRATA_CORE_URL` | `http://127.0.0.1:8080` | l'API du Core, sur la machine |
| `STRATA_CORE_VERSION` | `0.3.0` | |
| `STRATA_SERVER_NAME` | `Régie A` | le nom d'usage du serveur |

- **Arrêt** : `SIGTERM` à son groupe de processus, `SIGKILL` 10 s plus tard. L'appli doit finir
  proprement sur `SIGTERM` (fermer ses fichiers, ses entrées).
- **Plantage** : relancée après 1 s, 2 s, 4 s… 30 s au plus (de nouveau 1 s après une minute de bonne
  marche).
- **Journal** : ce qu'elle écrit sur la sortie standard et la sortie d'erreur est gardé par le Core
  (les 1000 dernières lignes), visible dans *Réglages › Applis › <appli> › Journal*.
- « En marche » veut dire que son port répond.

Aujourd'hui, ce sont des processus enfants du Core. L'installateur en fera des services systemd
(`strata-app@<id>`), qui survivront à un redémarrage du Core.

## Les requêtes relayées

L'appli reçoit les requêtes **telles que le navigateur les a envoyées** (chemin, méthode, corps,
`Host: livecut.<serveur>`), avec en plus :

| En-tête | Valeur |
| --- | --- |
| `X-Strata-User-Id` | l'identifiant stable du compte |
| `X-Strata-User` | son nom d'utilisateur, encodé en pourcentage |
| `X-Strata-User-Name` | son nom affiché, encodé en pourcentage (`Ann%20L%C3%A9vy`) |
| `X-Strata-Admin` | `1` s'il est administrateur des serveurs, sinon `0` |
| `X-Strata-Groups` | les identifiants de ses groupes, séparés par des virgules |
| `X-Strata-Group-Names` | leurs noms, chacun encodé en pourcentage, séparés par des virgules |
| `X-Strata-Lang` / `X-Strata-Theme` | ses choix (`fr`, `dark`…), vides s'il n'en a pas fait |
| `X-Forwarded-For` / `-Proto` / `-Host` | l'adresse du navigateur, `http`/`https`, le nom demandé |

Garanties du Core :

- Sans session, l'appli **ne reçoit rien** : une page demandée renvoie à la connexion du cockpit, qui
  ramène ensuite à la même adresse ; une requête de son code (API, WebSocket) reçoit `401`.
- Les en-têtes `X-Strata-*` envoyés par le navigateur sont **retirés** : ceux que l'appli reçoit
  viennent du Core.
- Le cookie de session du Core et son jeton `Bearer` **ne sont pas transmis**. Les cookies propres à
  l'appli passent.
- Une modification (POST, PUT, DELETE…) ou un WebSocket envoyé par un autre site avec le cookie est
  refusé (`403`) avant d'arriver à l'appli.
- Un chemin `[relay] public` passe **sans connexion et sans identité**, même si quelqu'un est connecté.
- Les réponses de l'appli sont rendues **telles quelles** : ses en-têtes de sécurité (CSP…) sont les
  siens.
- Appli éteinte ou qui démarre : le Core répond `502` avec une page qui se recharge toute seule.

## Le matériel du serveur

Le matériel se règle une fois, dans le Core (*Réglages › Matériel*), pour toutes les applis : une
entrée SDI est une entrée pour toutes. L'appli lit ces réglages sur la machine, sans session :

```
GET $STRATA_CORE_URL/api/hardware/shared
```

| Partie | Contenu |
| --- | --- |
| `sdi` | les cartes DeckLink : nom, modèle, présente ou non, genlock ; pour chaque connecteur (`number` = SDI n, numéro logiciel), son nom, sa fonction (`input`, `output`, `unused`) et l'appareil Desktop Video à ouvrir (`device`) |
| `network` | les interfaces et leur adresse : `management`, `ndi`, `media1`, `media2`, `ptp` ; `mediaMode` : `redundant` (SMPTE 2022-7 : `media1` rouge, `media2` bleu) ou `separate` (deux ports indépendants) |
| `flows` | les flux ST 2110 : nom, sens, essence, `red` (et `blue` en 2022-7), `port` (1 ou 2, ports indépendants) |
| `ndi` | le dossier des réglages NDI (`configDir`), le serveur de découverte, le nom de machine |
| `ptp` | le domaine PTP, le décalage TAI − UTC |

NDI : le Core donne `NDI_CONFIG_DIR` à l'appli qu'il lance ; la bibliothèque NDI y lit l'interface,
le serveur de découverte et le nom de machine, les mêmes pour toutes les applis.

À venir : les entrées DeckLink captées une seule fois par le Core et partagées entre les applis
(le pilote n'en laisse qu'un programme à la fois).

## Ce que l'appli garde pour elle

- **Les droits** : qui peut faire quoi dans ses écrans, par groupe ou par personne, en s'appuyant sur
  `X-Strata-Groups` et `X-Strata-User-Id`. Un administrateur des serveurs n'a pas tous les droits dans
  les applis pour autant ; c'est à l'appli d'en décider.
- Son interface, construite avec `@strata/ui` (le paquet `strata-ui-<version>.tgz` des releases du
  Core).

## Ce qui viendra

- L'appli dira au Core si un utilisateur y a accès, pour que le cockpit ne montre que les applis
  permises.
- Les entrées DeckLink et le reste du matériel partagé passeront par le Core (brief § 5.3).
- Une liste des applis Strata dans la Bibliothèque, quand chacune publiera des releases à ce format.
