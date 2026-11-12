#!/usr/bin/env bash
# rice-settings installer — builds from source and installs for the current user.
#   ./install.sh            install to ~/.local
#   ./install.sh --system   install to /usr/local (uses sudo)
#   ./install.sh --uninstall [--system]
set -euo pipefail
cd "$(dirname "$0")"

prefix="$HOME/.local"; sudo=""
[[ " $* " == *" --system "* ]] && { prefix=/usr/local; sudo=sudo; }

if [[ " $* " == *" --uninstall "* ]]; then
    $sudo rm -f "$prefix/bin/rice-settings" "$prefix/share/applications/rice-settings.desktop"
    echo "removed. your settings stay in ~/.config/rice and ~/.config/hypr/settings.lua|rice-settings.conf"
    exit 0
fi

need_build_deps() {
    if command -v pacman >/dev/null;  then echo "sudo pacman -S --needed rust gtk4 pkgconf"
    elif command -v apt >/dev/null;   then echo "sudo apt install cargo libgtk-4-dev pkg-config"
    elif command -v dnf >/dev/null;   then echo "sudo dnf install cargo gtk4-devel pkgconf-pkg-config"
    elif command -v zypper >/dev/null; then echo "sudo zypper install cargo gtk4-devel pkg-config"
    else echo "install: Rust (cargo), GTK 4.14+ development files, pkg-config"; fi
}
if ! command -v cargo >/dev/null || ! pkg-config --atleast-version=4.14 gtk4 2>/dev/null; then
    echo "missing build tools (cargo and GTK 4.14+ headers). run:"
    echo "    $(need_build_deps)"
    exit 1
fi

echo "building (release)…"
cargo build --release --locked
$sudo install -Dm755 target/release/rice-settings "$prefix/bin/rice-settings"
$sudo install -Dm644 rice-settings.desktop "$prefix/share/applications/rice-settings.desktop"
echo "installed: $prefix/bin/rice-settings"
[[ ":$PATH:" == *":$prefix/bin:"* ]] || echo "note: $prefix/bin is not in your PATH"

echo
if [[ -f "$HOME/.config/hypr/hyprland.lua" ]]; then
cat <<'LUA'
add to your Hyprland config (hyprland.lua) for a key and a floating window:
    hl.bind("SUPER + I", hl.dsp.exec_cmd("rice-settings"))
    hl.window_rule({ name = "rice-settings", match = { class = "^(rice.settings)$" }, float = true, center = true })
LUA
else
cat <<'CONF'
add to your Hyprland config (hyprland.conf) for a key and a floating window:
    bind = SUPER, I, exec, rice-settings
    windowrule = float, class:^(rice.settings)$
    windowrule = center, class:^(rice.settings)$
CONF
fi
echo
echo "optional tools (the app offers to install any that are missing):"
echo "  matugen · swww/awww/hyprpaper · networkmanager · bluez · power-profiles-daemon · brightnessctl · asusctl (ASUS) · openrgb · waybar"
