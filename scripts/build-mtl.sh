#!/bin/bash
# Construit DPDK, Intel Media Transport Library (MTL) et le codec JPEG XS (SVT-JPEG-XS, avec son
# greffon pour MTL) pour le Core, installés sous
# /opt/strata/lib/mtl (le chemin final : DPDK y cherche ses pilotes). Utilisé par la release.
#
#   scripts/build-mtl.sh <dossier de travail> [<dossier de sortie>]
#
# La sortie reçoit lib/mtl/lib/ (libmtl.so, les bibliothèques et les pilotes DPDK), prête à
# archiver : la même disposition que sous /opt/strata/lib/mtl.
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
# Les bibliothèques se trouvent entre elles ($ORIGIN, et les pilotes deux crans plus haut) ;
# DPDK cherche ses pilotes au chemin final (/opt/strata/lib/mtl/lib/dpdk/pmds-*).
export LDFLAGS='-Wl,-rpath,$ORIGIN:$ORIGIN/../..'
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
# JPEG XS (ST 2110-22) : SVT-JPEG-XS et son greffon pour MTL, posés à côté de libmtl (le Core
# l'inscrit auprès de MTL au démarrage). La version est celle que MTL donne.
JXS_REV=$(sed -n 's/^SVT_JPEG_XS_VER=//p' "$WORK/mtl/versions.env")
cd "$WORK"
if [ ! -d svt-jpeg-xs ]; then
  git clone -q https://github.com/OpenVisualCloud/SVT-JPEG-XS.git svt-jpeg-xs
fi
git -C svt-jpeg-xs fetch -q origin "$JXS_REV" 2>/dev/null || true
git -C svt-jpeg-xs checkout -q "$JXS_REV"
rm -rf svt-jpeg-xs/Build/strata svt-jpeg-xs/mtl-plugin/build-strata
cmake -S svt-jpeg-xs -B svt-jpeg-xs/Build/strata -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_SHARED_LIBS=ON -DBUILD_APPS=OFF -DBUILD_TESTING=OFF
cmake --build svt-jpeg-xs/Build/strata -j"$(nproc)"
DESTDIR="$WORK/stage" cmake --install svt-jpeg-xs/Build/strata
for pc in "$WORK/stage$PREFIX"/lib/pkgconfig/mtl.pc "$WORK/stage$PREFIX"/lib/pkgconfig/SvtJpegxs.pc; do
  [ -f "$pc" ] && sed -i "s|^prefix=.*|prefix=$WORK/stage$PREFIX|" "$pc"
done
export PKG_CONFIG_PATH="$WORK/stage$PREFIX/lib/pkgconfig:$PKG_CONFIG_PATH"
meson setup svt-jpeg-xs/mtl-plugin/build-strata svt-jpeg-xs/mtl-plugin --prefix="$PREFIX" --libdir=lib
ninja -C svt-jpeg-xs/mtl-plugin/build-strata
DESTDIR="$WORK/stage" meson install -C svt-jpeg-xs/mtl-plugin/build-strata
test -f "$WORK/stage$PREFIX/lib/libst_plugin_st22_svt_jpeg_xs.so"

# La sortie : les bibliothèques (sans en-têtes ni pkg-config), les pilotes DPDK, les licences.
rm -rf "$OUT/lib/mtl"
mkdir -p "$OUT/lib/mtl/lib"
cp -a "$WORK/stage$PREFIX/lib/." "$OUT/lib/mtl/lib/"
rm -rf "$OUT/lib/mtl/lib/pkgconfig"
find "$OUT/lib/mtl" -name '*.a' -delete
cp "$WORK/mtl/LICENSE" "$OUT/lib/mtl/LICENSE-MTL.txt"
cp "$WORK/svt-jpeg-xs/LICENSE.md" "$OUT/lib/mtl/LICENSE-SVT-JPEG-XS.md"
cp "$WORK/dpdk/license/bsd-3-clause.txt" "$OUT/lib/mtl/LICENSE-DPDK.txt" 2>/dev/null || true
echo "MTL ready in $OUT/lib/mtl"
