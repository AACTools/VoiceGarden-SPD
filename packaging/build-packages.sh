#!/bin/sh
# Build .deb and .rpm packages from built binaries using nfpm.
#
# Usage: build-packages.sh <bin-dir> <version> <deb-arch> <rpm-arch> <triplet> <out-dir>
#   bin-dir  — directory containing sd_voicegarden, voicegarden-spd-refresh,
#              voicegarden-spd, and voicegarden-spd.conf (sample config)
#   deb-arch — amd64 | arm64
#   rpm-arch — x86_64 | aarch64
#   triplet  — x86_64-linux-gnu | aarch64-linux-gnu
#
# Outputs <out-dir>/voicegarden-spd_<version>_<deb-arch>.deb and
#         <out-dir>/voicegarden-spd-<version>.<rpm-arch>.rpm
set -eu

BIN_DIR="$1"; VERSION="$2"; DEB_ARCH="$3"; RPM_ARCH="$4"; TRIPLET="$5"; OUT_DIR="$6"

[ -x "$BIN_DIR/sd_voicegarden" ] || { echo "missing $BIN_DIR/sd_voicegarden (build first)" >&2; exit 1; }
for f in voicegarden-spd-refresh voicegarden-spd; do
    [ -x "$BIN_DIR/$f" ] || { echo "missing $BIN_DIR/$f" >&2; exit 1; }
done
CONF="$BIN_DIR/voicegarden-spd.conf"
[ -f "$CONF" ] || CONF="$(dirname "$0")/../config/voicegarden-spd.conf"
[ -f "$CONF" ] || { echo "sample config not found" >&2; exit 1; }

mkdir -p "$OUT_DIR"

# ---- nfpm (single static binary; cache or download) -----------------------
NFPM_DIR="${NFPM_DIR:-$OUT_DIR/.nfpm}"
NFPM_VER="2.43.0"
case "$(uname -m)" in
    x86_64) NFPM_ARCH="x86_64" ;;
    aarch64|arm64) NFPM_ARCH="arm64" ;;
    *) echo "unsupported build host: $(uname -m)" >&2; exit 1 ;;
esac
NFPM="$NFPM_DIR/nfpm"
if [ ! -x "$NFPM" ]; then
    mkdir -p "$NFPM_DIR"
    url="https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VER}/nfpm_${NFPM_VER}_Linux_${NFPM_ARCH}.tar.gz"
    curl -fsSL "$url" | tar -xz -C "$NFPM_DIR" nfpm
    chmod +x "$NFPM"
fi

# ---- generate per-package configs ------------------------------------------
# Paths: deb uses /usr/lib/<triplet>/..., rpm uses /usr/lib64/... (Fedora/
# openSUSE convention). Both ship the CLI to /usr/bin.
make_yaml() {
    kind="$1"   # deb | rpm
    modules_dir="$2"
    file="$3"
    {
        echo "name: voicegarden-spd"
        echo "arch: $( [ "$kind" = deb ] && echo "$DEB_ARCH" || echo "$RPM_ARCH" )"
        echo "platform: linux"
        echo "version: $VERSION"
        echo "section: sound"
        echo "priority: optional"
        echo "maintainer: will wade <willwade@gmail.com>"
        echo "vendor: AACTools"
        echo "homepage: https://github.com/AACTools/VoiceGarden-SPD"
        echo "license: MIT"
        echo "description: |"
        echo "  Speech-dispatcher output module exposing 1300+ sherpa-onnx local voices"
        echo "  (Kokoro, Piper, Matcha, MMS) and 20 cloud engines (Azure, free Edge,"
        echo "  OpenAI, ElevenLabs, Google, ...) to every speechd client — Orca,"
        echo "  Firefox, Okular, Qt apps, spd-say."
        if [ "$kind" = deb ]; then
            echo "depends:"
            echo "  - speech-dispatcher"
            echo "recommends:"
            echo "  - sound-icons"
        else
            echo "depends:"
            echo "  - speech-dispatcher"
        fi
        echo "contents:"
        echo "  - src: $BIN_DIR/sd_voicegarden"
        echo "    dst: $modules_dir/sd_voicegarden"
        echo "  - src: $BIN_DIR/voicegarden-spd-refresh"
        echo "    dst: $modules_dir/voicegarden-spd-refresh"
        echo "  - src: $BIN_DIR/voicegarden-spd"
        echo "    dst: /usr/bin/voicegarden-spd"
        echo "  - src: $CONF"
        echo "    dst: /etc/speech-dispatcher/modules/voicegarden-spd.conf"
        echo "    type: config|noreplace"
        echo "scripts:"
        echo "  postinstall: $(dirname "$0")/scripts/postinst"
        echo "  preremove: $(dirname "$0")/scripts/prerm"
    } > "$file"
}

make_yaml deb "/usr/lib/$TRIPLET/speech-dispatcher-modules" "$OUT_DIR/nfpm-deb.yaml"
make_yaml rpm "/usr/lib64/speech-dispatcher-modules" "$OUT_DIR/nfpm-rpm.yaml"

"$NFPM" pkg -f "$OUT_DIR/nfpm-deb.yaml" -p deb -t "$OUT_DIR/voicegarden-spd_${VERSION}_${DEB_ARCH}.deb"
"$NFPM" pkg -f "$OUT_DIR/nfpm-rpm.yaml" -p rpm -t "$OUT_DIR/voicegarden-spd-${VERSION}.${RPM_ARCH}.rpm"

echo "built:"
ls -la "$OUT_DIR"/*.deb "$OUT_DIR"/*.rpm
