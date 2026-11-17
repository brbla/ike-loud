#!/bin/sh
# Installs ike loud for the current user, from this source tree:
#   binaries      ~/.local/bin/ikeloud-{gui,daemon,cli}
#   service       ~/.config/systemd/user/ikeloud.service (the daemon)
#   menu entry    ~/.local/share/applications/io.github.brbla.IkeLoud.desktop
#   autostart     ~/.config/autostart/io.github.brbla.IkeLoud.desktop
#                 (after login: the tray icon only)
# The app starts the service after the first setup; it starts it again
# after every sign-in.
#
#   scripts/install-user.sh              build (release) and install
#   scripts/install-user.sh --uninstall  stop and remove all of the above;
#                                        settings, the session, the synced
#                                        folder and its state stay
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
SERVICE="$CONF/systemd/user/ikeloud.service"
MENU="$DATA/applications/io.github.brbla.IkeLoud.desktop"
AUTOSTART="$CONF/autostart/io.github.brbla.IkeLoud.desktop"

if [ "${1:-}" = "--uninstall" ]; then
    systemctl --user disable --now ikeloud.service 2>/dev/null || true
    rm -f "$SERVICE" "$MENU" "$AUTOSTART" \
        "$DATA/icons/hicolor/scalable/apps/io.github.brbla.IkeLoud.svg" \
        "$BIN/ikeloud-gui" "$BIN/ikeloud-daemon" "$BIN/ikeloud-cli"
    systemctl --user daemon-reload
    echo "ike loud removed (settings, session and the synced folder stay)."
    exit 0
fi

cd "$ROOT"
cargo build --release --locked --offline -p ikeloud-gui -p ikeloud-daemon -p ikeloud-cli

# A running daemon keeps its old binary open; stop it for the update.
systemctl --user stop ikeloud.service 2>/dev/null || true
for name in ikeloud-gui ikeloud-daemon ikeloud-cli; do
    install -Dm755 "target/release/$name" "$BIN/$name"
done

# Absolute paths: ~/.local/bin need not be on the PATH of the session.
sed "s|^ExecStart=.*|ExecStart=$BIN/ikeloud-daemon|" packaging/systemd/ikeloud.service \
    | install -Dm644 /dev/stdin "$SERVICE"
# The icon by its full path: menus and the window overview then need no
# icon theme lookup, which caches (GTK's, Plasma's) can hide.
ICON="$DATA/icons/hicolor/scalable/apps/io.github.brbla.IkeLoud.svg"
for entry in "io.github.brbla.IkeLoud.desktop:$MENU" "io.github.brbla.IkeLoud-autostart.desktop:$AUTOSTART"; do
    sed -e "s|^Exec=ikeloud-gui|Exec=$BIN/ikeloud-gui|" -e "s|^Icon=.*|Icon=$ICON|" \
        "packaging/desktop/${entry%%:*}" | install -Dm644 /dev/stdin "${entry#*:}"
done

install -Dm644 packaging/icons/io.github.brbla.IkeLoud.svg \
    "$DATA/icons/hicolor/scalable/apps/io.github.brbla.IkeLoud.svg"

systemctl --user daemon-reload
systemctl --user enable ikeloud.service >/dev/null
command -v update-desktop-database >/dev/null && update-desktop-database "$DATA/applications" || true
# Menus look icons up through caches; a cache older than the new icon hides
# it. The user's hicolor folder has no index.theme, so gtk-update-icon-cache
# cannot rebuild its cache there: remove a stale one (lookups then scan).
rm -f "$DATA/icons/hicolor/icon-theme.cache"
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true

echo "ike loud installed: start it from the menu (or run $BIN/ikeloud-gui)."
