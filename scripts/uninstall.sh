#!/bin/sh
# VoiceGarden-SPD uninstall script — removes everything and leaves no trace.
#
# Usage: curl -fsSL https://raw.githubusercontent.com/AACTools/VoiceGarden-SPD/main/scripts/uninstall.sh | sudo sh
# Or:    sudo sh scripts/uninstall.sh

set -eu

echo "=== VoiceGarden-SPD uninstall ==="

# 1. Remove the system package
if dpkg -l voicegarden-spd >/dev/null 2>&1; then
    echo "  Removing .deb package..."
    sudo apt-get remove -y voicegarden-spd
    sudo apt-get autoremove -y
elif rpm -q voicegarden-spd >/dev/null 2>&1; then
    echo "  Removing .rpm package..."
    sudo rpm -e voicegarden-spd
fi

# 2. Kill any running module instance
echo "  Stopping module..."
pkill -f sd_voicegarden-spd 2>/dev/null || true

# 3. Remove system config
echo "  Removing system config..."
sudo rm -f /etc/speech-dispatcher/modules/voicegarden-spd.conf 2>/dev/null || true

# 4. Remove user config
echo "  Removing user config..."
rm -rf ~/.config/voicegarden-spd 2>/dev/null || true

# 5. Remove speechd.conf AddModule line if present
SPEECHD_CONF="$HOME/.config/speech-dispatcher/speechd.conf"
if [ -f "$SPEECHD_CONF" ]; then
    sed -i '/AddModule.*voicegarden-spd/d' "$SPEECHD_CONF" 2>/dev/null || true
    # Remove the file if empty
    [ -s "$SPEECHD_CONF" ] || rm -f "$SPEECHD_CONF"
fi

# 6. Remove model directories
echo "  Removing model directories..."
rm -rf ~/.local/share/voicegarden 2>/dev/null || true
rm -rf ~/.rust-tts-wrapper 2>/dev/null || true

# 7. Remove log
rm -f /run/user/"$(id -u)"/speech-dispatcher/log/voicegarden-spd.log 2>/dev/null || true

# 8. Restart speech-dispatcher to release the module
if systemctl is-active --quiet speech-dispatcher 2>/dev/null; then
    sudo systemctl restart speech-dispatcher 2>/dev/null || true
fi

echo ""
echo "Done. VoiceGarden-SPD is fully removed."
echo "Install fresh: curl -fsSL https://raw.githubusercontent.com/AACTools/VoiceGarden-SPD/main/scripts/install.sh | sh"