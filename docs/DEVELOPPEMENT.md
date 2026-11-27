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

## Vérifier avant d'envoyer

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd packages/ui && npm run build)
(cd web && npm run build)
```
