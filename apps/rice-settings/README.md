# rice-settings

One settings window for Hyprland. Rust + GTK4 (no libadwaita). It recolours itself live
from the palette matugen generates from your wallpaper, and uses ~40–60 MB of RAM.

| page | what it does | how |
|---|---|---|
| wallpaper | grid of `~/Pictures/Wallpapers` (or `$WALL_DIR`); click to apply | your `wall` script if present, else awww / swww / hyprpaper + matugen |
| colours | light mode switch, palette swatches (click to copy), matugen scheme / contrast | matugen + gsettings (GTK theme and colour scheme); options in `~/.config/rice/theme.env` |
| appearance | rounding, corner shape, border, gaps, opacity, blur, shadows, dim, animations, layout | live via hyprctl, saved to an override file |
| panel | waybar position, height, margin, font size, modules on/off (corners follow the appearance page's radius) | edits `~/.config/waybar/config.jsonc` + `style.css` |
| wi-fi | radio, networks, connect (inline password), disconnect, forget | nmcli |
| bluetooth | power, scan, pair, connect, remove | bluetoothctl |
| display | resolution / refresh, scale (only scales Hyprland accepts) | override file |
| power | battery %, status, time left, health, draw, cycles, NVIDIA GPU state, charge limit, power profiles, auto switch and history (with `rice-battery`), busiest apps | sysfs, procfs, asusctl, power-profiles-daemon |
| keyboard | backlight level, RGB effect + colour, *follow wallpaper* | sysfs / brightnessctl, asusctl (ASUS) or OpenRGB |
| about | device name (rename), model, CPU, GPUs, memory, disk, software, components | sysfs, hostnamectl |

Anything missing shows an **install** banner on its page: it runs the right command for
pacman (+ yay/paru for AUR), apt, dnf or zypper in a terminal, enables the needed service,
and rebuilds the page. *about → components* lists all of them.

## Install

**Any distro** (needs cargo + GTK 4.14 headers; the script tells you how to get them):

    ./install.sh              # ~/.local/bin
    ./install.sh --system     # /usr/local/bin
    ./install.sh --uninstall

**Arch** package:

    cd packaging/arch && makepkg -si

Then add a key and a floating rule (install.sh prints the right syntax for your config):

    # hyprland.conf
    bind = SUPER, I, exec, rice-settings
    windowrule = float, class:^(rice.settings)$
    windowrule = center, class:^(rice.settings)$

    -- hyprland.lua
    hl.bind("SUPER + I", hl.dsp.exec_cmd("rice-settings"))
    hl.window_rule({ name = "rice-settings", match = { class = "^(rice.settings)$" }, float = true, center = true })

## Usage

    rice-settings [page]          # wallpaper colours appearance panel wifi bluetooth display power keyboard about
    rice-settings --sync-keyboard # RGB keyboard -> wallpaper accent (if "follow wallpaper" is on)

Esc closes. Call `--sync-keyboard` from your wallpaper script so the keyboard follows wallpapers
changed outside the app (the app does it itself when it sets the wallpaper).

## Where things are saved

| what | file |
|---|---|
| Hyprland overrides | `~/.config/hypr/settings.lua` (Lua config) or `~/.config/hypr/rice-settings.conf` (hyprland.conf) |
| app state (bar layout, keyboard) | `~/.config/rice/settings.json` |
| palette options | `~/.config/rice/theme.env` |
| palette (when the app sets wallpapers) | `~/.config/rice/colors.css` |
| thumbnails | `~/.cache/rice/settings-thumbs/` |

The override file only takes effect after restarts if your main config loads it. The appearance
and display pages offer to add that one line (a backup of your config is kept as `*.pre-rice-settings`).
Delete the override file, or press *reset to config*, to undo everything.

Live changes: Lua configs have no `hyprctl keyword`, so the app uses `hyprctl eval 'hl.config{…}'`;
`.conf` setups use `hyprctl keyword`.

## Theme

Colours: the newer of `~/.config/waybar/colors.css` (a matugen template, as in this rice) and
`~/.config/rice/colors.css`, each as `@define-color name #hex;` using matugen's names.
Both are watched. Font: GeistMono Nerd Font (falls back to monospace). Icons are Nerd Font glyphs.
Corner radius follows Hyprland's `decoration:rounding`.

## RAM

The CPU renderer is the default (`GSK_RENDERER=cairo`): ~40 MB PSS on one page, ~58 MB with every
page and all thumbnails loaded. The GL renderer used ~200 MB. Pages build on first visit; GPU names
come from sysfs + pci.ids, so opening *about* never wakes a sleeping dGPU.
Set `GSK_RENDERER=ngl` to use GL anyway.

## Build

    cargo build --release && cargo test --release

## License

**Public domain ([Unlicense](LICENSE)).** Anyone can use it, fork it, change it, rename it,
redistribute it or sell it, for any purpose, with no permission, credit or conditions needed.
Do whatever you want with it.
