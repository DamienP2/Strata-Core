#!/usr/bin/env bash
# Les builds de release prennent leur version dans l'étiquette (v1.2.3 → 1.2.3) : ce qui est publié
# annonce toujours la version sous laquelle il est publié (les mises à jour OTA la comparent avec la
# dernière release). Une étiquette courte est complétée (v0.2 → 0.2.0, v1 → 1.0.0).
# Hors étiquette (CI, lancement à la main) : ne change rien.
# Usage : scripts/stamp-version.sh            (dans GitHub Actions, d'après GITHUB_REF_NAME)
#         scripts/stamp-version.sh 1.2.3      (à la main)
set -euo pipefail
cd "$(dirname "$0")/.."
if [ $# -ge 1 ]; then
  V=$1
else
  [ "${GITHUB_REF_TYPE:-}" = tag ] || { echo "pas d'étiquette : version inchangée"; exit 0; }
  V=${GITHUB_REF_NAME#v}
fi
case "$V" in
  *.*.*) ;;
  *.*) V=$V.0 ;;
  *) V=$V.0.0 ;;
esac
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "version invalide : $V" >&2; exit 1; }

# Paquet web (et son package-lock.json).
perl -pi -e 's/"version": "[^"]*"/"version": "'"$V"'"/ if $. < 6' packages/ui/package.json
perl -0pi -e 's/("name": "\@strata\/ui",\s*"version": ")[^"]+/${1}'"$V"'/g' packages/ui/package-lock.json
# Interface du Core.
perl -pi -e 's/"version": "[^"]*"/"version": "'"$V"'"/ if $. < 6' web/package.json
perl -0pi -e 's/("name": "strata-core-web",\s*"version": ")[^"]+/${1}'"$V"'/g' web/package-lock.json
# Crates Rust : la version du workspace, et celle de nos crates dans Cargo.lock (--locked).
perl -pi -e 's/^version = "[^"]*"/version = "'"$V"'"/ if $. < 15' Cargo.toml
perl -0pi -e 's/(name = "strata-core(?:-server)?"\r?\nversion = ")[^"]+/${1}'"$V"'/g' Cargo.lock
echo "version : $V"
