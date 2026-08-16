#!/bin/sh
# VoiceGarden-SPD installer — downloads the latest release and installs
# the module user-locally (no root required).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/AACTools/VoiceGarden-SPD/main/scripts/install.sh | sh
#
# Or from an extracted release tarball:
#   ./install.sh
set -eu

REPO="AACTools/VoiceGarden-SPD"
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64) ARCH_DIR="x86_64-linux" ;;
    aarch64|arm64) ARCH_DIR="aarch64-linux" ;;
    *) echo "unsupported architecture: $ARCH (x86_64 and aarch64 builds coming; build from source for now)" >&2; exit 1 ;;
esac

# Locate the tarball: bundled (running from an extracted release) or latest.
if [ -f "$(dirname "$0")/sd_voicegarden" ]; then
    echo "Installing from local directory $(dirname "$0")"
    BIN_DIR="$(dirname "$0")"
else
    URL="https://github.com/${REPO}/releases/latest/download/voicegarden-spd-latest-${ARCH_DIR}.tar.gz"
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    echo "Downloading latest release for ${ARCH_DIR}…"
    if ! curl -fsSL -o "$TMP/vg.tar.gz" "$URL"; then
        # 'latest' alias may not exist; resolve the newest tagged asset.
        TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
        [ -n "$TAG" ] || { echo "could not resolve latest release" >&2; exit 1; }
        URL="https://github.com/${REPO}/releases/download/${TAG}/voicegarden-spd-${TAG}-${ARCH_DIR}.tar.gz"
        curl -fsSL -o "$TMP/vg.tar.gz" "$URL"
    fi
    tar -xzf "$TMP/vg.tar.gz" -C "$TMP"
    BIN_DIR="$(dirname "$(find "$TMP" -name sd_voicegarden | head -1)")"
fi

# The management CLI does the install (copies to
# ~/.local/libexec/speech-dispatcher-modules, writes config, registers the
# AddModule line).
if [ ! -x "$BIN_DIR/voicegarden-spd" ]; then
    echo "voicegarden-spd management tool not found next to sd_voicegarden" >&2
    exit 1
fi
"$BIN_DIR/voicegarden-spd" install

cat <<'EOF'

VoiceGarden-SPD installed (user-local).

Next:
  1. Optional: add cloud engine credentials to
       ~/.config/voicegarden-spd/engines.json        (mode 600)
     then run:  voicegarden-spd refresh
  2. Restart speech-dispatcher:
       systemctl --user restart speech-dispatcher.socket
     (or log out and back in)
  3. Test:
       spd-say -o voicegarden-spd "Hello from VoiceGarden"

Sherpa-onnx models: place them under ~/.rust-tts-wrapper/sherpaonnx/<model-id>/
EOF
