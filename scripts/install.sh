#!/bin/sh
# VoiceGarden-SPD installer
#
#   curl -fsSL https://raw.githubusercontent.com/AACTools/VoiceGarden-SPD/main/scripts/install.sh | sh
#
# Strategy:
#   1. Prefer a native package (.deb / .rpm) from the latest release —
#      integrates with the package manager, handles upgrades + removal.
#   2. Fall back to the user-local tarball install (no root) with --user
#      or automatically when sudo is unavailable.
#
# Flags:
#   --user     force the user-local (tarball) install even with root
#   --version vX.Y.Z   install a specific release instead of latest
set -eu

REPO="AACTools/VoiceGarden-SPD"
ARCH="$(uname -m)"
WANT_USER=0
VERSION="latest"

while [ $# -gt 0 ]; do
    case "$1" in
        --user) WANT_USER=1; shift ;;
        --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
        --version=*) VERSION="${1#--version=}"; shift ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

case "$ARCH" in
    x86_64)  DEB_ARCH="amd64"; RPM_ARCH="x86_64"; TARBALL_ARCH="x86_64-linux" ;;
    aarch64|arm64) DEB_ARCH="arm64"; RPM_ARCH="aarch64"; TARBALL_ARCH="aarch64-linux" ;;
    *) echo "unsupported architecture: $ARCH — build from source: https://github.com/${REPO}#development" >&2; exit 1 ;;
esac

have() { command -v "$1" >/dev/null 2>&1; }

need() {
    for t in "$@"; do
        have "$t" || { echo "this install path needs '$t' which was not found" >&2; return 1; }
    done
}

resolve_tag() {
    if [ "$VERSION" = "latest" ]; then
        VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
        [ -n "$VERSION" ] || { echo "could not resolve latest release" >&2; exit 1; }
    fi
}

download_release() { # $1 = filename
    resolve_tag
    curl -fsSL -o "$TMP/$1" "https://github.com/${REPO}/releases/download/${VERSION}/$1"
}

# ---- pick the install path -------------------------------------------------

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

SUDO=""
if [ "$(id -u)" != "0" ]; then
    if have sudo; then SUDO="sudo"; fi
fi

can_root() { [ "$(id -u)" = "0" ] || have sudo; }

install_deb() {
    resolve_tag
    file="voicegarden-spd_${VERSION#v}_${DEB_ARCH}.deb"
    echo "Downloading ${file}…"
    download_release "$file"
    ($SUDO apt-get update -qq || true)
    $SUDO apt-get install -y "$TMP/$file"
}

install_rpm() {
    resolve_tag
    file="voicegarden-spd-${VERSION#v}.${RPM_ARCH}.rpm"
    echo "Downloading ${file}…"
    download_release "$file"
    if have dnf; then
        ($SUDO dnf install -y "$TMP/$file")
    elif have zypper; then
        ($SUDO zypper --non-interactive install "$TMP/$file")
    elif have rpm; then
        $SUDO rpm -Uvh "$TMP/$file" || $SUDO rpm -ivh "$TMP/$file"
    else
        echo "no rpm installer available" >&2
        return 1
    fi
}

install_user_tarball() {
    resolve_tag
    file="voicegarden-spd-${VERSION}-${TARBALL_ARCH}.tar.gz"
    echo "Downloading ${file}…"
    if ! download_release "$file"; then
        # older releases may lack the tag-named file; try the latest alias
        curl -fsSL -o "$TMP/vg.tar.gz" \
            "https://github.com/${REPO}/releases/latest/download/voicegarden-spd-latest-${TARBALL_ARCH}.tar.gz" \
            || { echo "download failed" >&2; exit 1; }
    fi
    tar -xzf "$TMP/$file" -C "$TMP"
    BIN_DIR="$(dirname "$(find "$TMP" -name sd_voicegarden-spd -type f | head -1)")"
    [ -x "$BIN_DIR/voicegarden-spd" ] || { echo "release tarball incomplete" >&2; exit 1; }
    "$BIN_DIR/voicegarden-spd" install
}

PKG_OK=0
if [ "$WANT_USER" = "0" ] && can_root; then
    # Native package path (best integration: upgrades, clean removal)
    if have apt-get; then
        install_deb && PKG_OK=1
    elif have dnf || have zypper || have rpm; then
        install_rpm && PKG_OK=1
    fi
fi

if [ "$PKG_OK" != "1" ]; then
    if [ "$WANT_USER" = "0" ] && can_root; then
        echo "falling back to user-local install (package path unavailable)"
    fi
    need curl || { echo "install requires curl" >&2; exit 1; }
    install_user_tarball
fi

RESTART_CMD="systemctl --user restart speech-dispatcher"
if systemctl is-active --quiet speech-dispatcher 2>/dev/null; then
    RESTART_CMD="sudo systemctl restart speech-dispatcher"
elif command -v speech-dispatcher >/dev/null 2>&1; then
    RESTART_CMD="sudo killall -HUP speech-dispatcher || sudo speech-dispatcher"
fi

cat <<EOF

Done. To finish setup:
  1. Restart speech-dispatcher:
       $RESTART_CMD
  2. Find + install a voice:
       voicegarden-spd model find en
       voicegarden-spd model install kokoro-en-int8-v0_19
  3. Test:
       spd-say -o voicegarden-spd -y "kokoro" -e "Hello from VoiceGarden"
  4. Optional cloud voices:
       ~/.config/voicegarden-spd/engines.json  (mode 600)
       voicegarden-spd refresh
  5. Troubleshooting: voicegarden-spd doctor
EOF
