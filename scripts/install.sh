#!/usr/bin/env bash
# Installe un serveur Strata (Ubuntu 24.04, x86-64), d'une seule commande :
#   curl -fsSL https://github.com/DamienP2/Strata-Core/releases/latest/download/install.sh | sudo bash
# Télécharge l'installateur strata-install de la release, vérifie son empreinte, le lance (écran
# plein terminal). Les options passent : … | sudo bash -s -- --yes --dns-name strata01.regie.fr
set -euo pipefail
STRATA_REPO="${STRATA_REPO:-DamienP2/Strata-Core}"
STRATA_VERSION="${STRATA_VERSION:-latest}"
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "lancez-le avec sudo"
# /etc/os-release est lu à part : il définit VERSION, NAME… qui écraseraient les nôtres.
os_id="$(. /etc/os-release 2>/dev/null; echo "${ID:-}")"
os_name="$(. /etc/os-release 2>/dev/null; echo "${PRETTY_NAME:-inconnu}")"
[ "$os_id" = ubuntu ] || die "Strata s'installe sur Ubuntu 24.04 (ici : $os_name)"
[ "$(uname -m)" = x86_64 ] || die "Strata s'installe sur un processeur x86-64 (ici : $(uname -m))"
command -v curl >/dev/null || die "curl est nécessaire : apt-get install curl"

if [ "$STRATA_VERSION" = latest ]; then
  base="https://github.com/$STRATA_REPO/releases/latest/download"
else
  base="https://github.com/$STRATA_REPO/releases/download/v${STRATA_VERSION#v}"
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "impossible de joindre GitHub ($base)"
name="$(awk '$2 ~ /^strata-install-.*-linux-x86_64$/ { print $2; exit }' "$tmp/SHA256SUMS")"
[ -n "$name" ] || die "pas d'installateur dans cette release"
echo "Téléchargement de $name"
curl -fL --progress-bar "$base/$name" -o "$tmp/$name"
(cd "$tmp" && grep "  $name\$" SHA256SUMS | sha256sum -c --quiet -) || die "l'empreinte de $name ne correspond pas"
chmod +x "$tmp/$name"
# L'écran plein terminal lit le clavier, même lancé par « curl … | sudo bash ».
if [ -e /dev/tty ] && [ -t 1 ]; then
  "$tmp/$name" "$@" </dev/tty
else
  "$tmp/$name" "$@"
fi
