#!/usr/bin/env bash
# Slap File Manager installer — builds from source and installs for the current user.
#   ./install.sh                 install to ~/.local
#   ./install.sh --system        install to /usr/local (uses sudo)
#   ./install.sh --default       also make it the default app for folders
#   ./install.sh --uninstall [--system]
set -euo pipefail
cd "$(dirname "$0")"

prefix="$HOME/.local"; sudo=""
[[ " $* " == *" --system "* ]] && { prefix=/usr/local; sudo=sudo; }

if [[ " $* " == *" --uninstall "* ]]; then
    $sudo rm -f "$prefix/bin/slap-files" "$prefix/share/applications/slap-files.desktop"
    rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services/org.freedesktop.FileManager1.service"
    echo "removed. your settings and pins stay in ~/.config/slap/files.json"
    exit 0
fi

if ! command -v cargo >/dev/null || ! pkg-config --atleast-version=4.14 gtk4 2>/dev/null; then
    echo "missing build tools (cargo and GTK 4.14+ headers). on arch: sudo pacman -S --needed rust gtk4 pkgconf"
    exit 1
fi

echo "building (release)…"
cargo build --release --locked
$sudo install -Dm755 target/release/slap-files "$prefix/bin/slap-files"
$sudo install -Dm644 slap-files.desktop "$prefix/share/applications/slap-files.desktop"
echo "installed: $prefix/bin/slap-files"
[[ ":$PATH:" == *":$prefix/bin:"* ]] || echo "note: $prefix/bin is not in your PATH"

svc="${XDG_DATA_HOME:-$HOME/.local/share}/dbus-1/services/org.freedesktop.FileManager1.service"
if [[ " $* " == *" --default "* ]]; then
    xdg-mime default slap-files.desktop inode/directory && echo "now the default app for folders"
    # "show in folder" from browsers etc.; the per-user file wins over system ones (dolphin's)
    mkdir -p "$(dirname "$svc")"
    printf '[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=%s --service\n' "$prefix/bin/slap-files" > "$svc"
    echo "now handles 'show in folder' requests"
fi

echo
if [[ -f "$HOME/.config/hypr/hyprland.lua" ]]; then
cat <<'LUA'
add to hyprland.lua for a key:
    hl.bind("SUPER + E", hl.dsp.exec_cmd("slap-files"))
LUA
else
cat <<'CONF'
add to hyprland.conf for a key:
    bind = SUPER, E, exec, slap-files
CONF
fi
echo "needs at runtime: udisks2 (drives/eject); optional: zip, bsdtar (libarchive), a terminal"
