# Releases

Le Core et chaque appli Strata se publient de la même façon : une **release GitHub** par version, construite par GitHub Actions sur **Ubuntu 24.04**. Les serveurs ne compilent jamais rien : l'installateur et les mises à jour OTA prennent ces fichiers tels quels, après avoir vérifié leurs empreintes.

## Publier une version du Core

```bash
git tag v0.2.0
git push origin v0.2.0
```

Le workflow `Release` (`.github/workflows/release.yml`) :

1. reprend la version de l'étiquette (`scripts/stamp-version.sh` : `v0.2` → `0.2.0`) ;
2. construit le paquet web `@strata/ui` ;
3. écrit la fiche de la release et les empreintes (`scripts/release-manifest.mjs`) ;
4. publie la release avec ses fichiers.

Une version avec un tiret (`v0.3.0-rc.1`) est publiée en **préversion** : les serveurs ne la proposent pas d'eux-mêmes.

Lancé à la main (*Actions › Release › Run workflow*), le workflow construit les mêmes fichiers, sans rien publier : on les télécharge depuis la page de l'exécution.

## Ce que contient une release

| Fichier | Contenu |
| --- | --- |
| `strata-ui-<version>.tgz` | le paquet web `@strata/ui` |
| `strata-core-<version>-linux-x86_64.tar.gz` | le serveur `strata-core` *(à venir)* |
| `strata-install-<version>-linux-x86_64` | l'installateur *(à venir)* |
| `release.json` | la fiche de la release |
| `SHA256SUMS` | l'empreinte SHA-256 de chaque fichier |

`release.json` :

```json
{
  "name": "strata-core",
  "version": "0.2.0",
  "tag": "v0.2.0",
  "commit": "…",
  "date": "2026-11-27T08:00:00Z",
  "requires": {},
  "assets": [
    { "name": "strata-ui-0.2.0.tgz", "kind": "ui-package", "size": 293677, "sha256": "…" }
  ]
}
```

## Les versions

Le Core suit les versions sémantiques `MAJEUR.MINEUR.CORRECTIF` (brief, § 6). Le paquet web et le crate Rust portent le numéro du Core.

Une appli publie la même fiche, avec dans `requires` la plage de versions du Core qu'elle accepte, reprise de sa `strata-app.toml` :

```json
"requires": { "core": ">=0.2, <1" }
```

Le Core refuse d'installer, d'activer ou de mettre à jour une appli dont la plage n'inclut pas sa version.

## Utiliser `@strata/ui` dans une appli

Dans le `package.json` de l'appli, le paquet est référencé par l'adresse de son fichier dans la release :

```json
"dependencies": {
  "@strata/ui": "https://github.com/DamienP2/Strata-Core/releases/download/v0.2.0/strata-ui-0.2.0.tgz"
}
```

Changer de version du design, c'est changer cette adresse. Le paquet a besoin de `react`, `react-dom`, `zustand` et `lucide-react`, que l'appli a déjà.
