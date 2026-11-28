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
access = "/strata/access"  # facultatif : où elle dit si quelqu'un y a accès (voir plus bas)

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
  (les 1000 dernières lignes dans *Réglages › Applis › <appli> › Journal*, 30 jours dans *Santé ›
  Journal*). Une ligne qui contient `ERROR` ou `WARN` dans ses premiers mots compte comme erreur ou
  avertissement.
- **Plantage** : un arrêt qu'elle n'a pas demandé laisse un rapport (90 jours) avec la fin de son
  journal.
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

- Sans session, l'appli **ne reçoit rien** (sauf ses chemins publics `[relay] public`, sur une
  frontière de segment : `/health` couvre `/health/…`, pas `/healthX` ; un chemin avec `..`, `//` ou
  `%2e` n'est jamais public) : une page demandée renvoie à la connexion du cockpit, qui
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
| `sdi` | les cartes DeckLink : nom, modèle, présente ou non, genlock ; pour chaque connecteur (`number` = SDI n, numéro logiciel), son nom, sa fonction (`input`, `output`, `unused`), l'appareil Desktop Video (`device`) et le fichier partagé (`share`) : à lire pour une entrée captée, à écrire pour une sortie |
| `network` | les interfaces et leur adresse : `management`, `ndi`, `media1`, `media2`, `ptp` ; `mediaMode` : `redundant` (SMPTE 2022-7 : `media1` rouge, `media2` bleu) ou `separate` (deux ports indépendants) |
| `flows` | les flux ST 2110 : identifiant (`id`), nom, sens, essence, `red` (et `blue` en 2022-7), `port` (1 ou 2, ports indépendants) ; pour un flux envoyé, le fichier où publier ses images (`share`) |
| `ndi` | le dossier des réglages NDI (`configDir`), le serveur de découverte, le nom de machine |
| `ptp` | le domaine PTP, le décalage TAI − UTC |
| `gpus` | les GPU : fabricant, nom, pilote, nœud de rendu (`renderNode`), interface (`nvenc` ou `vaapi`), codecs encodés et décodés, prêt ou non |
| `clock` | d'où le serveur prend l'heure (`ptp` ou `ntp`), le timecode maison (`house` : cadence, heure locale / UTC / décalage, drop frame, recalage quotidien) et le fuseau du serveur |

NDI : le Core donne `NDI_CONFIG_DIR` à l'appli qu'il lance ; la bibliothèque NDI y lit l'interface,
le serveur de découverte et le nom de machine, les mêmes pour toutes les applis.

## Le timecode maison

Le Core tient l'heure du serveur sur le PTP (ou le NTP), et toutes les machines tenues par le même
leader ont la même heure. Le timecode maison s'en **calcule** : une appli n'a besoin ni de câble LTC
ni de lire le timecode d'une entrée. Avec `clock` de `/api/hardware/shared` :

```rust
use strata_core::timecode::{house_timecode, HouseClock};

let house: HouseClock = serde_json::from_value(shared["clock"]["house"].clone())?;
let tz = shared["clock"]["timezone"].as_str().unwrap_or("UTC");
let tai = shared["ptp"]["taiOffset"].as_u64().unwrap_or(37) as u32;
let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as i128;
let tc = house_timecode(now, &house, tz, tai); // 13:53:20:08
```

- Aux cadences entières (25, 30, 24), c'est l'heure du jour comptée en images.
- En 29,97 et 23,98, le timecode compte les images depuis le **recalage quotidien** (SMPTE ST
  2059-1), en drop frame si réglé ; leurs instants sont alignés sur l'époque PTP.
- *Réglages › Matériel › Horloge* compare le timecode de chaque entrée SDI à celui de la maison.

## Les flux vidéo : MXL

Les images circulent entre le Core et les applis par **MXL** (Media eXchange Layer, EBU DMF /
Linux Foundation) : des flux en mémoire partagée, sur le serveur, sans passer par le réseau.

- **Le domaine** : `mxl.domain` dans `/api/hardware/shared` (en général `/dev/shm/mxl`). `mxl`
  vaut `null` quand MXL n'est pas installé : le Core retombe alors sur `strata-share` (plus bas).
- **Les entrées** (SDI captées par le Core, ST 2110 reçues par le Core) : un flux par entrée, son
  identifiant dans `mxlFlow` du connecteur (`sdi`) ou du flux (`flows`). L'appli le **lit**. Le son
  embarqué d'une entrée SDI : `audioFlow` du connecteur (float32 48 kHz, autant de canaux que la
  carte en donne : 16, 8 ou 2), posé à l'instant de son image.
- **Les sorties** (SDI, flux ST 2110 envoyés) : `mxlFlow` du connecteur réglé en sortie, ou du flux
  ST 2110 sortant. L'appli **écrit** ce flux (son `label` est le nom montré dans les réglages) ; le
  Core l'envoie (SDI en 10 bits, calé sur la référence de la carte ; du noir quand rien n'arrive)
  et l'aperçu des réglages le montre. **Avec MXL, une appli n'ouvre jamais une sortie DeckLink
  elle-même.** Exemple : `crates/strata-mxl/examples/send.rs`.
  Le son de la sortie SDI : l'appli écrit `audioFlow` du connecteur (float32 48 kHz, autant de
  canaux qu'elle veut) ; le Core l'embarque avec l'image, rangé dans les canaux de la carte (16,
  8 ou 2 : en trop, ignorés ; en moins, silence), du silence quand rien n'arrive. Pour un son
  calé à l'image près, l'écrire à l'instant de son image, juste après elle :
  `audio.write_at(video.last_frame_audio_index().unwrap(), &pcm)` (1920 échantillons par image
  en 25 i/s ; 1601 ou 1602 en 29,97). Un son écrit au fil du temps (`write_interleaved`) passe
  aussi, une image et demie plus tard. `audio` de `playout` dans `/api/hardware/outputs` dit si
  le son arrive. Exemple : `crates/strata-mxl/examples/tone.rs`.
- **ST 2110** : quand *Réglages › Matériel › Réseau* confie les flux au Core (noyau, AF_XDP ou
  DPDK, avec Intel Media Transport Library), le Core reçoit les flux entrants (2022-7 compris) et
  envoie les sortants : l'appli ne touche jamais le réseau, elle lit et écrit `mxlFlow` du flux
  comme pour le SDI. Vidéo (2110-20) : v210, cadencée ST 2110-21, du noir quand l'appli n'écrit
  pas. Son (2110-30) : un flux audio MXL (float32 48 kHz, `channels` canaux), en L24 ou L16 sur le
  réseau (`bits`, paquets de `ptimeUs`), du silence quand l'appli n'écrit pas. Données annexes
  (2110-40 : timecode ATC, sous-titres, SCTE-104…) : un flux de données MXL `video/smpte291`, un
  grain par image (par trame en entrelacé, comme la vidéo), au rythme de `format` ; des blocs
  vides quand l'appli n'écrit pas. `io` de `/api/hardware/flows` dit ce qui passe (paquets reçus,
  perdus). Réglé sur « Non », les applis reçoivent et envoient elles-mêmes, comme avant.
- **Données annexes** (`strata_mxl::anc`) : un grain porte la charge RFC 8331 à partir du champ
  *Length* (le format de MXL) ; `anc::encode` / `anc::decode` la font et la lisent (paquets ST 291 :
  ligne, DID, SDID, mots sur 8 bits ; parité et somme de contrôle calculées), `atc_packet` et
  `atc_timecode` pour le timecode. Écrire : `mxl.data_writer(id, nom, cadence)` puis
  `write(&paquets, champ)` (champ F : 0 progressif, 2 / 3 première / seconde trame) ; calé sur
  une image : `write_at(video.last_index().unwrap(), …)`, le flux de données à la cadence des
  grains vidéo. Lire : `mxl.data_reader(id)` puis `next_grain()`, dans l'ordre. Essais :
  `strata-core test timecode 2110/<flux>`, `strata-core test data 2110/<flux>`.
- Une appli peut aussi lire le flux qu'une autre écrit (Multiview qui montre le programme de
  LiveCut), sans rien demander au Core.
- **Format** : vidéo v210 (YCbCr 4:2:2 10 bits, BT.709), un grain par image ; en entrelacé, un grain
  par trame (index pair : trame du haut). Les grains sont rangés par leur instant (index TAI, la
  même horloge que PTP) : on lit « l'image de maintenant ».
- Les identifiants sont stables (UUID v5 d'après `sdi/<carte>/<n>`, `sdi-out/<carte>/<n>`,
  `2110/<flux>`) : ils ne changent pas d'un démarrage à l'autre.

En Rust, avec le crate officiel `mxl` (bibliothèque `libmxl.so` chargée au lancement ; le Core
l'installe dans `/opt/strata/lib`) :

```rust
let api = mxl::load_api("/opt/strata/lib/libmxl.so.1")?;
let mxl = mxl::MxlInstance::new(api, &shared["mxl"]["domain"].as_str().unwrap(), "")?;
// Lire une entrée.
let reader = mxl.create_flow_reader(flow_id)?.to_grain_reader()?;
let rate = reader.get_config_info()?.common().grain_rate()?;
let index = mxl.get_current_index(&rate);
let grain = reader.get_complete_grain(index, Duration::from_millis(100))?; // grain.payload : v210
// Écrire une sortie : la description du flux (NMOS) avec l'identifiant donné par le Core.
let (writer, _, _) = mxl.create_flow_writer(&flow_def_json, None)?;
```

En C ou C++ : `mxlCreateInstance`, `mxlCreateFlowReader` / `mxlCreateFlowWriter`,
`mxlFlowReaderGetGrain`, `mxlFlowWriterOpenGrain` / `mxlFlowWriterCommitGrain` (dépôt
`dmf-mxl/mxl`). Les applis tournent sous le compte du Core : elles ont accès au domaine.

## Les flux réseau (SRT, RTMP, HLS…)

Le Core reçoit et envoie les flux réseau lui-même (*Réglages › Matériel › Flux réseau*, avec
ffmpeg) : une appli ne parle jamais SRT ou RTMP, elle lit et écrit des flux MXL.

- **Les entrées** : `streams.inputs` dans `/api/hardware/shared`. Chacune a son image
  (`mxlFlow`, v210 en 1080, à la cadence et au balayage de la source par défaut : une source progressive reste progressive ; ou au format réglé) et son son (`audioFlow`, float32
  48 kHz stéréo ; du silence si la source n'en a pas). Le son est calé sur l'image : l'échantillon
  d'une image est à l'instant de son grain. `relay` (si réglé) : le flux d'origine, tel quel, en
  MPEG-TS sur `udp://127.0.0.1:<port>`, pour enregistrer ou relayer sans décoder.
- **Les sorties** : une appli déclare la sienne, puis écrit son image et son son en MXL ; le Core
  l'encode une fois (NVENC ou VA-API si le serveur a un GPU, sinon x264) et l'envoie à chaque
  destination. Une destination qui tombe est relancée seule. Sans image depuis une seconde : du
  noir et du silence, le flux continue.

```
POST $STRATA_CORE_URL/api/streams/app/outputs/programme
{"app": "livecut", "name": "Programme", "format": "1080p50", "videoKbps": 8000,
 "destinations": [
   {"id": "yt", "name": "YouTube", "url": "rtmp://a.rtmp.youtube.com/live2", "key": "xxxx-xxxx", "enabled": true},
   {"id": "hls", "url": "hls", "enabled": true}
 ]}
→ {"id": "livecut-programme", "mxlFlow": "…", "audioFlow": "…", "status": {…}}
```

- Depuis la machine, sans session (comme les alertes). L'identifiant devient `<appli>-<id>`.
- Une clé non redonnée (vide) garde la précédente ; les clés ne sont jamais renvoyées.
- `GET …/api/streams/app/outputs?app=livecut` : l'état de ses sorties (par destination : envoie,
  erreur, débit) ; `DELETE …/api/streams/app/outputs/programme?app=livecut` la retire.
- `"enabled": false` arrête l'envoi sans oublier la sortie.
- Destinations : `rtmp://` et `rtmps://` (la clé s'ajoute à l'adresse), `srt://` (la clé part en
  `streamid`), `udp://`, `rtp://`, `rist://` (MPEG-TS), et `hls` : servi par le Core sur
  `/hls/<sortie>/index.m3u8`.
- Les administrateurs voient et changent ces sorties dans les réglages, comme les autres.

## Lire une entrée SDI sans MXL

Sans MXL (bibliothèque absente), le Core partage les entrées avec `strata-share`.

Le pilote DeckLink ne laisse ouvrir une entrée qu'à **un seul programme**. Le Core capte donc
chaque connecteur réglé sur « Entrée » et publie ses images dans une mémoire partagée ; toutes les
applis la lisent en même temps, sans la rouvrir. **Une appli n'ouvre jamais une entrée DeckLink
elle-même.** Les sorties, elles, restent ouvertes par l'appli qui sort (une sortie, un programme).

Le fichier d'une entrée est dans `share` (`/api/hardware/shared`) :
`/dev/shm/strata-sdi-<carte>-<n>`. On le lit avec le crate `strata-share` du Core :

```rust
use strata_share::{Next, Reader};

let mut r = Reader::open(share.as_ref(), &std::env::var("STRATA_APP_ID")?)?;
let mut buf = Vec::new();
loop {
    match r.next(&mut buf, Duration::from_secs(1)) {
        Next::Frame(f) => { /* buf : f.info.width × f.info.height, UYVY, f.info.stride octets par ligne */ }
        Next::Timeout => { /* pas d'image : r.signal() dit si le signal manque */ }
        Next::Gone => { /* le Core a fermé ou recréé l'entrée : rouvrir */ }
    }
}
```

- **Les images** : UYVY 8 bits (4:2:2, BT.709), avec leur numéro (un saut = des images manquées),
  leur cadence (`rate`, 25/1, 30000/1001…), entrelacées ou non, et le **timecode** du signal
  (RP188/VITC) quand il en porte un. Un exemple complet : `crates/strata-share/examples/read.rs`.
- **Toujours la plus récente** : le Core garde un anneau de 3 images ; un lecteur en retard saute
  aux plus récentes plutôt que de prendre du retard. Une image lue est toujours entière.
- **Sans attente active** : `next` dort jusqu'à l'image suivante (réveil par futex).
- **L'appli s'inscrit** avec son nom (`STRATA_APP_ID`) : *Réglages › Matériel › Cartes SDI* montre
  qui lit chaque entrée. `latest_with` lit l'image sur place, sans copie, pour l'envoyer au GPU.
- **Quand le Core redémarre** ou que l'entrée est retirée des réglages, `next` rend `Gone` : rouvrir
  le fichier, toutes les secondes, jusqu'à ce qu'il revienne.
- Le format : une carte qui détecte le format suit le signal d'elle-même ; les autres sont ouvertes
  en 1080i50.
- Pas de son par `strata-share` : le son des entrées passe par MXL (`audioFlow`).

## Montrer ce qu'on envoie sans MXL (sorties SDI, flux ST 2110)

Une appli qui envoie une sortie SDI ou un flux ST 2110 vidéo publie aussi ce qu'elle envoie, dans
le fichier `share` de la sortie ou du flux (`/api/hardware/shared`) :
`/dev/shm/strata-sdi-out-<carte>-<n>` ou `/dev/shm/strata-2110-<flux>`. *Réglages › Matériel*
en montre l'image et le nom de l'appli ; une autre appli peut la lire comme une entrée.

**Seulement quand quelqu'un regarde** : l'aperçu des réglages le demande pendant qu'il est à
l'écran ; sinon rien n'est copié.

```rust
use strata_share::{FrameInfo, Writer};

let mut w = Writer::create(share.as_ref(), 3, 1920 * 1080 * 2)?;
loop {
    // … l'image part sur la sortie …
    if w.watched() {
        w.publish(&info, &frame_uyvy)?; // une image par seconde suffit à l'aperçu
    } else {
        w.beat();
    }
}
```

- Même format que les entrées : UYVY 8 bits, avec cadence, entrelacement et timecode. Une image
  sur 25 suffit ; toutes, si une autre appli la lit.
- Le fichier disparaît avec le `Writer` : l'appli qui s'arrête n'envoie plus rien, les réglages
  le montrent.
- Un exemple complet : `crates/strata-share/examples/send.rs`.

Les flux ST 2110 **reçus** n'ont rien à publier : le Core les rejoint lui-même pour l'aperçu,
seulement pendant qu'on le regarde (une image par seconde), et les quitte ensuite.

## Déclarer une alerte

Une appli signale un problème au Core, qui l'envoie avec les autres (*Santé › Alertes* : webhook,
e-mail) quand il a duré le délai réglé, puis dit quand il est fini. Depuis la machine, sans session :

```
POST $STRATA_CORE_URL/api/health/alerts/app
{"app": "livecut", "key": "rec-full", "text": "plus de place pour la boucle de SDI 2",
 "over": "de nouveau de la place pour la boucle"}
```

- `key` : une par problème (64 caractères au plus) ; renvoyer la même clé ne fait rien de plus.
- Problème fini : la même requête avec `"active": false`.
- Les alertes d'une appli tombent quand elle est désactivée.

## Qui voit l'appli dans le cockpit

Avec `access = "/strata/access"` dans la fiche, le Core demande à l'appli, pour chaque personne qui
ouvre le cockpit : `GET http://127.0.0.1:<port>/strata/access`, avec les mêmes en-têtes
`X-Strata-*` qu'une requête relayée. L'appli répond :

- `200` et `{"access": true}` (ou un `200` sans corps lisible) : montrée ;
- `200` et `{"access": false}`, ou `403` / `401` : cachée à cette personne.

La réponse est gardée 30 s par personne (et par ses groupes). Sur un maître, il pose la question à
chaque membre pour ses applis (appel signé), qui la pose à ses applis.

Rien ne bloque : sans `access`, ou si l'appli ne répond pas en 1,5 s (arrêtée, lente, autre statut),
elle est **montrée** ; de même pour les applis d'un membre qui ne répond pas. Le cockpit ne fait que
cacher : l'appli refuse elle-même ce qui n'est pas permis.

## Ce que l'appli garde pour elle

- **Les droits** : qui peut faire quoi dans ses écrans, par groupe ou par personne, en s'appuyant sur
  `X-Strata-Groups` et `X-Strata-User-Id`. Un administrateur des serveurs n'a pas tous les droits dans
  les applis pour autant ; c'est à l'appli d'en décider.
- Son interface, construite avec `@strata/ui` (le paquet `strata-ui-<version>.tgz` des releases du
  Core).

## Le catalogue de la Bibliothèque

La Bibliothèque montre les applis Strata connues du Core (`crates/strata-core-server/catalog.json` :
dépôt, nom, icône) avec, pour chacune, la description du dépôt sur GitHub, sa dernière release qui
accepte ce Core, et si elle est déjà installée ; « Installer » passe par l'installation habituelle.
Une appli apparaît comme installable dès que son dépôt publie une release au format décrit plus haut.
GitHub est relu toutes les heures (bouton « Actualiser » pour le faire tout de suite). Une nouvelle
appli : l'ajouter à `catalog.json` (elle arrive avec la version suivante du Core) ; en attendant,
« Installer une appli » accepte n'importe quel dépôt.
