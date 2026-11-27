#!/bin/bash
# Construit DPDK et Intel Media Transport Library (MTL) pour le Core, installés sous
# /opt/strata/lib/mtl (le chemin final : DPDK y cherche ses pilotes). Utilisé par la release.
#
#   scripts/build-mtl.sh <dossier de travail> [<dossier de sortie>]
#
# La sortie reçoit lib/mtl/ (libmtl.so et les bibliothèques DPDK), prête à archiver.
set -euo pipefail
WORK=$(realpath "$1")
OUT=$(realpath "${2:-$WORK/out}")
PREFIX=/opt/strata/lib/mtl
DPDK_VERSION=v26.07
MTL_REV=1c5bb95a19000405c0eaccb36c209240d1af8a6d
mkdir -p "$WORK" "$OUT"
cd "$WORK"
if [ ! -d mtl ]; then
  git clone -q https://github.com/OpenVisualCloud/Media-Transport-Library.git mtl
fi
git -C mtl checkout -q "$MTL_REV"
if [ ! -d dpdk ]; then
  git clone -q --depth 1 --branch "$DPDK_VERSION" https://github.com/DPDK/dpdk.git dpdk
  for p in mtl/patches/dpdk/${DPDK_VERSION#v}/*.patch; do git -C dpdk apply "$(realpath "$p")"; done
fi
# Les bibliothèques se trouvent entre elles ($ORIGIN) ; les pilotes au chemin final.
export LDFLAGS='-Wl,-rpath,$ORIGIN'
cd "$WORK/dpdk"
rm -rf build
meson setup build --prefix="$PREFIX" --libdir=lib -Dplatform=generic -Dtests=false -Ddisable_apps='*' \
  -Denable_drivers='bus/pci,bus/vdev,bus/auxiliary,common/iavf,mempool/ring,mempool/stack,mempool/bucket,net/ice,net/iavf,net/i40e,net/af_xdp,net/af_packet,dma/idxd,dma/ioat'
ninja -C build
DESTDIR="$WORK/stage" ninja -C build install
export PKG_CONFIG_PATH="$WORK/stage$PREFIX/lib/pkgconfig"
# Le pkg-config de DPDK donne le préfixe final : on construit MTL contre la copie posée.
sed -i "s|^prefix=.*|prefix=$WORK/stage$PREFIX|" "$PKG_CONFIG_PATH"/libdpdk*.pc
cd "$WORK/mtl"
rm -rf build-strata
meson setup build-strata --prefix="$PREFIX" --libdir=lib
ninja -C build-strata
DESTDIR="$WORK/stage" ninja -C build-strata install
# La sortie : les bibliothèques (sans en-têtes ni pkg-config), les pilotes DPDK, les licences.
rm -rf "$OUT/lib/mtl"
mkdir -p "$OUT/lib/mtl"
cp -a "$WORK/stage$PREFIX/lib/." "$OUT/lib/mtl/"
rm -rf "$OUT/lib/mtl/pkgconfig"
find "$OUT/lib/mtl" -name '*.a' -delete
cp "$WORK/mtl/LICENSE" "$OUT/lib/mtl/LICENSE-MTL.txt"
cp "$WORK/dpdk/license/bsd-3-clause.txt" "$OUT/lib/mtl/LICENSE-DPDK.txt" 2>/dev/null || true
echo "MTL ready in $OUT/lib/mtl"
