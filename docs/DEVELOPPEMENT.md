# Développer Strata Core

| Dossier | Contenu |
| --- | --- |
| `packages/ui` | `@strata/ui`, le design commun (banc d'essai : `npm run dev`) |
| `web` | l'interface du Core (cockpit, connexion), faite avec `@strata/ui` |
| `crates/strata-core` | ce que le Core et les applis partagent côté serveur |
| `crates/strata-core-server` | le service `strata-core` |

## Lancer le Core en local

```bash
(cd packages/ui && npm install && npm run build)
(cd web && npm install && npm run build)
cargo run -p strata-core-server -- --data-dir ./data
```

Puis `http://localhost:8080/` : la première visite, depuis la machine elle-même, crée le compte administrateur.

Pour travailler sur l'interface : `cargo run` d'un côté, `cd web && npm run dev` de l'autre (le serveur de Vite relaie `/api` vers le port 8080).

## Une appli de test dans le cockpit

```bash
mkdir -p data/apps/livecut
cat > data/apps/livecut/strata-app.toml <<'TOML'
id = "livecut"
name = "LiveCut"
version = "0.3.0"
icon = "scissors"
port = 8101

[requires]
core = ">=0.1, <1"
TOML
cargo run -p strata-core-server -- app activate livecut --data-dir ./data
```

Sans section `[run]`, le Core ne lance pas l'appli : démarrez-la à part sur son port (`127.0.0.1:8101`),
le Core la relaie. Avec `[run]`, il la lance lui-même (voir [APPLIS.md](APPLIS.md)).

## Ouvrir une appli par son sous-domaine, en local

Le relais choisit l'appli d'après le nom demandé (`livecut.<nom du serveur>`). En local, réglez le nom
du serveur dans `data/config.json` :

```json
{ "name": "Régie A", "dnsName": "strata.test", "scheme": "http" }
```

puis faites pointer `strata.test` et `*.strata.test` vers `127.0.0.1` : une ligne par nom dans
`/etc/hosts` (`127.0.0.1 strata.test livecut.strata.test`), ou dnsmasq
(`address=/strata.test/127.0.0.1`). Le cockpit est alors `http://strata.test:8080/` et LiveCut
`http://livecut.strata.test:8080/`, avec la même connexion.

## Le matériel sans le matériel

Des cartes simulées, pour travailler sur *Réglages › Matériel* :

```bash
STRATA_FAKE_DECKLINK=quad2,8kpro,duo2,minimonitor STRATA_FAKE_NICS=connectx,e810,i350 cargo run -p strata-core-server -- --data-dir ./data
```

Chaque connecteur simulé réglé en entrée donne une mire animée (720p25, avec timecode), partagée
dans `/dev/shm` comme une vraie entrée (`STRATA_SHARE_DIR` pour un autre dossier). Pour la lire
comme une appli :

```bash
STRATA_APP_ID=livecut cargo run -p strata-share --example read -- /dev/shm/strata-sdi-<carte>-1
```

Les GPU : `STRATA_FAKE_GPU=nvidia,intel,nouveau,amd`.

L'horloge : `STRATA_FAKE_CLOCK=ptp` (ou `ntp`, `none`) simule l'état de PTP et de chrony ;
`STRATA_SYSTEM_ROOT=./data/root` fait écrire les fichiers de chrony et ptp4l dans ce dossier au lieu
de `/etc`, sans rien relancer.

Le pont DeckLink (`crates/strata-decklink`) se compile avec g++ ; sans compilateur C++, le Core se
construit sans lui (avertissement). Le pilote Desktop Video n'est nécessaire qu'à l'exécution.

## Plusieurs serveurs en local

Deux Core sur la même machine, chacun son dossier de données et son port, avec deux noms qui
mènent à `127.0.0.1` (`/etc/hosts` : `127.0.0.1 strata.test strata2.test`) réglés dans *Nom et
adresses*. Sur le premier : *Serveurs › Ajouter un serveur* ; sur le second : *Rejoindre un cockpit*
avec `http://strata.test:8090` et le code. Les appels entre serveurs suivent les variables de proxy
(`HTTPS_PROXY`, `NO_PROXY`).

## Le HTTPS en local

*Réglages › Ce serveur › HTTPS › Fait par Strata*, avec un port libre (8443) : le Core fait son autorité
et le certificat de `strata.test` et `*.strata.test`, puis écoute en HTTPS sans redémarrer ; le HTTP
renvoie vers le HTTPS (sauf `localhost`). Pour que le navigateur ne prévienne plus, installez
l'autorité (`/api/https/ca.crt`) dans ses autorités de confiance. Pour revenir au HTTP sans
l'interface : `cargo run -p strata-core-server -- https off --data-dir ./data`, puis relancer.

## Vérifier avant d'envoyer

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd packages/ui && npm run build)
(cd web && npm run build)
```
