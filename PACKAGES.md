# Packages

Every package this setup installs, and what it is for. This file is generated from
[`packages/*.txt`](packages/) by `./install.sh --packages-md > PACKAGES.md`; the installer
reads the same files, so what you see here is exactly what it offers to install.

Run `./install.sh --list` to see this list with a tick next to what you already have.

## Desktop (needed)

The compositor and the pieces you see on screen. Without these there is no desktop.

| package | what it does here |
|---|---|
| [`hyprland`](https://archlinux.org/packages/?name=hyprland) | The Wayland compositor / tiling window manager. This rice uses its Lua config (config/hypr/hyprland.lua), tested on 0.57. |
| [`uwsm`](https://archlinux.org/packages/?name=uwsm) | Starts Hyprland as a systemd-managed session (the "Hyprland (uwsm)" entry on the login screen), so user services start and stop cleanly. |
| [`hyprlock`](https://archlinux.org/packages/?name=hyprlock) | Lock screen (SUPER+L). Shows the wallpaper, clock, and music / notification / battery lines from rice-lockinfo. |
| [`hypridle`](https://archlinux.org/packages/?name=hypridle) | Idle daemon: dims at 4 min, locks at 5, screen off at 6, suspends at 15 (config/hypr/hypridle.conf). |
| [`hyprpolkitagent`](https://archlinux.org/packages/?name=hyprpolkitagent) | The password prompt that appears when an app asks for admin rights. |
| [`xdg-desktop-portal-hyprland`](https://archlinux.org/packages/?name=xdg-desktop-portal-hyprland) | Screen sharing and screenshots for sandboxed apps and browsers. |
| [`xdg-desktop-portal-gtk`](https://archlinux.org/packages/?name=xdg-desktop-portal-gtk) | The "Open file / Save as" dialogs used by browsers, Code and other apps. |
| [`waybar`](https://archlinux.org/packages/?name=waybar) | The top bar. Config and style in config/waybar/, colours from matugen. |
| [`fuzzel`](https://archlinux.org/packages/?name=fuzzel) | App launcher (SUPER+Space). Also draws every dropdown panel (wi-fi, bluetooth, memory, power, clipboard, window switcher). |
| [`mako`](https://archlinux.org/packages/?name=mako) | Notification daemon. Also draws the volume / brightness popup. |
| [`libnotify`](https://archlinux.org/packages/?name=libnotify) | Provides notify-send, which the scripts use to show notifications. |
| [`kitty`](https://archlinux.org/packages/?name=kitty) | Terminal (SUPER+Return). Recolours live when the wallpaper changes. |
| [`awww`](https://archlinux.org/packages/?name=awww) | Wallpaper daemon with animated transitions (formerly swww). Driven by the `wall` script. |
| [`qt5-wayland`](https://archlinux.org/packages/?name=qt5-wayland) | Lets Qt 5 apps run natively on Wayland instead of through XWayland. |
| [`qt6-wayland`](https://archlinux.org/packages/?name=qt6-wayland) | Same, for Qt 6 apps. |

## Theming (needed)

Turns the wallpaper into a colour palette and gives apps fonts, icons and a GTK theme to draw with.

| package | what it does here |
|---|---|
| [`matugen`](https://archlinux.org/packages/?name=matugen) | Generates a Material You palette from the wallpaper and fills the templates in config/matugen/templates/ (Hyprland, hyprlock, waybar, fuzzel, mako, kitty, GTK, VS Code). |
| [`adw-gtk-theme`](https://archlinux.org/packages/?name=adw-gtk-theme) | GTK 3 theme that looks like libadwaita, so GTK 3 and GTK 4 apps match. Recoloured through ~/.config/gtk-3.0/colors.css. |
| [`papirus-icon-theme`](https://archlinux.org/packages/?name=papirus-icon-theme) | Icon theme for the launcher, notifications and apps. |
| [`otf-geist-mono-nerd`](https://archlinux.org/packages/?name=otf-geist-mono-nerd) | GeistMono Nerd Font: the one font used everywhere (bar, launcher, terminal, lock screen, apps). Its Nerd Font glyphs are the icons. |
| [`noto-fonts`](https://archlinux.org/packages/?name=noto-fonts) | Fallback font for regular text in apps and web pages. |
| [`noto-fonts-emoji`](https://archlinux.org/packages/?name=noto-fonts-emoji) | Colour emoji. |
| [`noto-fonts-cjk`](https://archlinux.org/packages/?name=noto-fonts-cjk) | Chinese / Japanese / Korean glyphs, so those don't render as empty boxes. |
| [`qt6ct`](https://archlinux.org/packages/?name=qt6ct) | Qt 6 settings tool. hyprland.lua sets QT_QPA_PLATFORMTHEME=qt6ct so Qt apps take their fonts and style from it. |
| [`qt5ct`](https://archlinux.org/packages/?name=qt5ct) | Same, for Qt 5 apps. |
| [`nwg-look`](https://archlinux.org/packages/?name=nwg-look) | GTK settings editor (theme, icons, cursor, font) for when you want something other than the defaults the installer sets. |
| [`glib2`](https://archlinux.org/packages/?name=glib2) | Provides gsettings, used by the installer and rice-settings to set the GTK theme, icon theme, cursor and dark / light mode. |
| [`gsettings-desktop-schemas`](https://archlinux.org/packages/?name=gsettings-desktop-schemas) | The settings schema (org.gnome.desktop.interface) those gsettings calls write to. |
| [`dconf`](https://archlinux.org/packages/?name=dconf) | The storage backend behind gsettings. |

## Tools the scripts and apps call (needed)

Small programs used by the rice-* scripts in bin/ and by the two apps. Each line names who uses it.

| package | what it does here |
|---|---|
| [`python`](https://archlinux.org/packages/?name=python) | Runs the Python tools: lyra, rice-wallpicker, rice-calendar, rice-battery, rice-overview, rice-vscode-sync. Shell scripts also use it to read hyprctl's JSON (so jq is not needed). |
| [`python-gobject`](https://archlinux.org/packages/?name=python-gobject) | GTK bindings for Python, used by those tools to draw their windows. |
| [`gtk4`](https://archlinux.org/packages/?name=gtk4) | UI toolkit for the Python popups and for both Rust apps (rice-settings, slap-files). |
| [`playerctl`](https://archlinux.org/packages/?name=playerctl) | Media control: the media keys, the music module in the bar (lyra) and the lock-screen music line. |
| [`imagemagick`](https://archlinux.org/packages/?name=imagemagick) | Makes wallpaper thumbnails (rice-wallpicker) and the login-screen background (rice-sddm-sync). |
| [`grim`](https://archlinux.org/packages/?name=grim) | Takes screenshots (rice-shot) and the live window thumbnails in the overview (rice-overview). |
| [`slurp`](https://archlinux.org/packages/?name=slurp) | Lets you drag-select the screenshot region. |
| [`wl-clipboard`](https://archlinux.org/packages/?name=wl-clipboard) | wl-copy / wl-paste: copies screenshots, feeds the clipboard history. |
| [`cliphist`](https://archlinux.org/packages/?name=cliphist) | Clipboard history (SUPER+V). |
| [`brightnessctl`](https://archlinux.org/packages/?name=brightnessctl) | Screen and keyboard backlight (brightness keys, idle dimming, rice-settings). |
| [`wireplumber`](https://archlinux.org/packages/?name=wireplumber) | Provides wpctl for the volume keys; also the session manager PipeWire needs. |
| [`power-profiles-daemon`](https://archlinux.org/packages/?name=power-profiles-daemon) | power-saver / balanced / performance switching (bar module, rice-battery auto-switch, rice-settings). |
| [`udisks2`](https://archlinux.org/packages/?name=udisks2) | Lists, mounts, unlocks and safely ejects drives in slap-files. |
| [`libarchive`](https://archlinux.org/packages/?name=libarchive) | Provides bsdtar: "extract here" in slap-files (zip, tar.*, 7z, rar, iso). |
| [`zip`](https://archlinux.org/packages/?name=zip) | "Compress to zip" in slap-files. |
| [`unzip`](https://archlinux.org/packages/?name=unzip) | Unpacks zip files from the terminal. |
| [`zstd`](https://archlinux.org/packages/?name=zstd) | "Compress to tar.zst" in slap-files. |
| [`psmisc`](https://archlinux.org/packages/?name=psmisc) | Provides fuser: slap-files names the programs blocking an eject. |
| [`poppler`](https://archlinux.org/packages/?name=poppler) | Provides pdftoppm: PDF thumbnails in slap-files. |
| [`ffmpeg`](https://archlinux.org/packages/?name=ffmpeg) | Video thumbnails in slap-files. |

## Audio, network, bluetooth (needed)

System services the bar modules and settings pages talk to. Skip any you already have set up differently.

| package | what it does here |
|---|---|
| [`pipewire`](https://archlinux.org/packages/?name=pipewire) | Audio server. |
| [`pipewire-pulse`](https://archlinux.org/packages/?name=pipewire-pulse) | Lets PulseAudio apps (most apps) play through PipeWire. |
| [`pipewire-alsa`](https://archlinux.org/packages/?name=pipewire-alsa) | Lets ALSA apps play through PipeWire. |
| [`pipewire-jack`](https://archlinux.org/packages/?name=pipewire-jack) | Lets JACK (pro-audio) apps play through PipeWire. |
| [`pavucontrol`](https://archlinux.org/packages/?name=pavucontrol) | Volume mixer window (right-click the volume in the bar). |
| [`networkmanager`](https://archlinux.org/packages/?name=networkmanager) | Network service. rice-wifi and rice-settings use its nmcli tool. |
| [`network-manager-applet`](https://archlinux.org/packages/?name=network-manager-applet) | Tray icon, and the connection editor (nm-connection-editor) for VPNs / static IPs. |
| [`bluez`](https://archlinux.org/packages/?name=bluez) | Bluetooth service. |
| [`bluez-utils`](https://archlinux.org/packages/?name=bluez-utils) | Provides bluetoothctl, used by rice-bt and rice-settings. |

## Login screen (optional)

Graphical login with a theme that mirrors the lock screen and follows the wallpaper. Skip if you use another display manager or log in from a TTY.

| package | what it does here |
|---|---|
| [`sddm`](https://archlinux.org/packages/?name=sddm) | Display manager (login screen). The theme lives in sddm/ and is installed to /usr/share/sddm/themes/rice. |

## Shell (optional)

The terminal experience. kitty.conf starts zsh; change `shell zsh` there if you skip this.

| package | what it does here |
|---|---|
| [`zsh`](https://archlinux.org/packages/?name=zsh) | Shell. home/.zshrc is a plugin-free config whose prompt uses the terminal palette, so it follows the wallpaper. |
| [`zsh-completions`](https://archlinux.org/packages/?name=zsh-completions) | Extra tab-completions for zsh. |
| [`fastfetch`](https://archlinux.org/packages/?name=fastfetch) | System info summary (config/fastfetch/config.jsonc). |

## Build tools (needed)

Needed once, to compile rice-settings and slap-files from the source in apps/. Nothing is downloaded prebuilt.

| package | what it does here |
|---|---|
| [`rust`](https://archlinux.org/packages/?name=rust) | Rust compiler and cargo. Skipped automatically if you already have cargo (for example from rustup). |
| [`pkgconf`](https://archlinux.org/packages/?name=pkgconf) | Lets the build find the GTK 4 headers. |
| [`git`](https://archlinux.org/packages/?name=git) | Clones this repo and AUR packages. |
| [`base-devel`](https://archlinux.org/packages/?name=base-devel) | Compiler toolchain, needed to build AUR packages. |

## Hardware extras (optional)

Only useful on matching hardware. Each one is asked separately. The setup works without them.

| package | what it does here |
|---|---|
| [`asusctl`](https://archlinux.org/packages/?name=asusctl) | ASUS laptops only: keyboard RGB, battery charge limit, fan profiles (rice-settings power and keyboard pages, rice-battery). |
| [`openrgb`](https://archlinux.org/packages/?name=openrgb) | RGB keyboards on non-ASUS machines (rice-settings keyboard page, "follow wallpaper"). |

## AUR packages (optional)

Not in the official repos; built by an AUR helper (yay or paru). Read each PKGBUILD before you build it.

| package | what it does here |
|---|---|
| [`bibata-cursor-theme`](https://aur.archlinux.org/packages/bibata-cursor-theme) | Cursor theme (Bibata-Modern-Classic). Without it you get the default cursor. |
| [`zen-browser-bin`](https://aur.archlinux.org/packages/zen-browser-bin) | Web browser bound to SUPER+B. Skip it and change the bind in ~/.config/hypr/binds.lua if you use another browser. |
| [`pipes.sh`](https://aur.archlinux.org/packages/pipes.sh) | Animated pipes terminal toy (SUPER+ALT+P). Purely decorative. |

## Not installed by this setup

- **Graphics drivers.** Install the ones for your GPU yourself (`mesa` plus `vulkan-intel` /
  `vulkan-radeon`, or `nvidia-open` + `nvidia-utils`). See the Arch wiki for your hardware.
- **Your apps.** Browser aside, no editors, chat or media apps are installed. If VS Code /
  Code OSS / VSCodium is present, its colours follow the wallpaper automatically.
- **`jq`** is not needed: scripts read `hyprctl -j` output with Python.
