# Releases

Le Core et chaque appli Strata se publient de la même façon : une **release GitHub** par version, construite par GitHub Actions sur **Ubuntu 24.04**. Les serveurs ne compilent jamais rien : l'installateur et les mises à jour OTA prennent ces fichiers tels quels, après avoir vérifié leurs empreintes.

## Publier une version du Core

```bash
git tag v0.2.0
git push origin v0.2.0
```

Le workflow `Release` (`.github/workflows/release.yml`) :

1. reprend la version de l'étiquette (`scripts/stamp-version.sh` : `v0.2` → `0.2.0`) ;
2. construit le paquet web `@strata/ui`, l'interface du Core, puis le serveur `strata-core` qui l'embarque ;
3. écrit la fiche de la release et les empreintes (`scripts/release-manifest.mjs`) ;
4. publie la release avec ses fichiers.

Une version avec un tiret (`v0.3.0-rc.1`) est publiée en **préversion** : les serveurs ne la proposent pas d'eux-mêmes.

Autre façon de publier, sans étiquette à pousser : *Actions › Release › Run workflow*, choisir la branche, et donner la version (`0.1.0`). La release `v0.1.0` est publiée sur le dernier commit de cette branche, et son étiquette créée avec elle. Sans version, le workflow construit les mêmes fichiers sans rien publier : on les télécharge depuis la page de l'exécution.

## Ce que contient une release

| Fichier | Contenu |
| --- | --- |
| `strata-ui-<version>.tgz` | le paquet web `@strata/ui` |
| `strata-core-<version>-linux-x86_64.tar.gz` | le serveur `strata-core`, interface comprise |
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

## Comment un serveur se met à jour

*Réglages › Ce serveur › Mises à jour* : le Core cherche la release la plus récente de son dépôt
(hors préversions), et celles des applis installées depuis GitHub (seulement des versions qui
acceptent le Core en place). Il cherche chaque jour si on le veut, mais n'installe jamais rien seul.

Pour le Core : l'archive `strata-core-<version>-linux-x86_64.tar.gz` est téléchargée et son empreinte
vérifiée ; le nouveau programme est écrit à côté de l'actuel et lancé une fois (`strata-core
--version` doit dire la version attendue) ; il prend alors la place de l'ancien, gardé en
`strata-core.previous`, et le Core redémarre (les sessions restent ouvertes). Le dossier du programme
doit donc être modifiable par le service. Sans internet : la même archive, envoyée depuis la page.

Revenir en arrière : remettre `strata-core.previous` à la place de `strata-core`, puis relancer le
service.

## Les versions

Le Core suit les versions sémantiques `MAJEUR.MINEUR.CORRECTIF` (brief, § 6). Le paquet web et le crate Rust portent le numéro du Core.

Une appli publie la même fiche, avec dans `requires` la plage de versions du Core qu'elle accepte, reprise de sa `strata-app.toml` :

```json
"requires": { "core": ">=0.2, <1" }
```

Le Core refuse d'installer, d'activer ou de mettre à jour une appli dont la plage n'inclut pas sa version.

Pour une appli, le script prend son identifiant et sa fiche, et classe son archive
(`<id>-<version>-linux-x86_64.tar.gz`) en `app-linux-x86_64` :

```bash
node scripts/release-manifest.mjs dist 0.3.0 livecut strata-app.toml
```

Ce que le Core vérifie à l'installation : [APPLIS.md](APPLIS.md#publier-une-release-installable).

## Utiliser `@strata/ui` dans une appli

Dans le `package.json` de l'appli, le paquet est référencé par l'adresse de son fichier dans la release :

```json
"dependencies": {
  "@strata/ui": "https://github.com/DamienP2/Strata-Core/releases/download/v0.2.0/strata-ui-0.2.0.tgz"
}
```

Changer de version du design, c'est changer cette adresse. Le paquet a besoin de `react`, `react-dom`, `zustand` et `lucide-react`, que l'appli a déjà.
