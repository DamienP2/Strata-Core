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

## Vérifier avant d'envoyer

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd packages/ui && npm run build)
(cd web && npm run build)
```
