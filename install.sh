#!/usr/bin/env bash
# hypr-rice installer for Arch Linux.
#
# Nothing here is hidden: every package is shown with what it does before it is installed,
# every command that changes your system is printed before it runs, every step asks first,
# and any file it would overwrite is copied to a backup folder first.
#
#   ./install.sh               interactive install (recommended)
#   ./install.sh --list        show every package, file and system change; change nothing
#   ./install.sh --dry-run     walk through the install, printing commands instead of running them
#   ./install.sh --only STEPS  run only these steps (comma separated), e.g. --only configs,scripts
#   ./install.sh --yes         answer yes to every question (pacman still asks for its own confirmation)
#
# Steps, in order:
#   packages   official-repo packages from packages/*.txt (sudo pacman)
#   aur        AUR packages from packages/aur.txt (needs yay or paru)
#   configs    config/  -> ~/.config        (existing files are backed up)
#   scripts    bin/     -> ~/.local/bin     (existing files are backed up)
#   wallpapers creates ~/Pictures/Wallpapers; draws a starter gradient if it is empty
#   apps       builds rice-settings and slap-files from apps/ with cargo
#   theme      GTK theme, icons, cursor, dark mode (gsettings)
#   palette    first matugen run, so everything has colours on first login
#   services   NetworkManager, bluetooth, power-profiles-daemon (sudo systemctl)
#   sddm       login-screen theme (sudo, writes /usr/share/sddm and /etc/sddm.conf.d)
#   shell      ~/.zshrc
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALL_STEPS=(packages aur configs scripts wallpapers apps theme palette services sddm shell)
DRY=0; YES=0; MODE=install; ONLY=""
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
BIN_HOME="$HOME/.local/bin"
WALL_HOME="${WALL_DIR:-$HOME/Pictures/Wallpapers}"
BACKUP="$CONFIG_HOME/hypr-rice-backup-$(date +%Y%m%d-%H%M%S)"
DEFAULT_WALLPAPER="hypr-rice-default.jpg"
SDDM_THEME=/usr/share/sddm/themes/rice
export PATH="$BIN_HOME:$PATH"   # so the scripts find each other during the install

while (( $# )); do
    case "$1" in
        --list)        MODE=list ;;
        --packages-md) MODE=markdown ;;
        --dry-run)     DRY=1 ;;
        --yes|-y)      YES=1 ;;
        --only)        ONLY="${2:?--only needs a list of steps}"; shift ;;
        --only=*)      ONLY="${1#--only=}" ;;
        -h|--help)     sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done

# ───────────────────────────────────────────────────────────── output ────
if [[ -t 1 ]]; then B=$'\e[1m'; D=$'\e[2m'; G=$'\e[32m'; Y=$'\e[33m'; R=$'\e[31m'; C=$'\e[36m'; N=$'\e[0m'
else B=""; D=""; G=""; Y=""; R=""; C=""; N=""; fi
hdr()  { printf '\n%s━━ %s%s\n' "$B$C" "$*" "$N"; }
say()  { printf '%s\n' "$*"; }
note() { printf '%s%s%s\n' "$D" "$*" "$N"; }
ok()   { printf '%s✓%s %s\n' "$G" "$N" "$*"; }
warn() { printf '%s!%s %s\n' "$Y" "$N" "$*"; }
die()  { printf '%s✗ %s%s\n' "$R" "$*" "$N" >&2; exit 1; }

# run CMD… — print the command, then run it (or only print it with --dry-run)
run() {
    printf '%s  $ %s%s\n' "$D" "$(printf '%q ' "$@")" "$N"
    (( DRY )) || "$@"
}

# ask "question" [default y|n] — yes/no prompt
ask() {
    local def="${2:-y}" hint="[Y/n]" reply
    [[ $def == n ]] && hint="[y/N]"
    if (( YES || DRY )); then printf '%s %s %s\n' "$1" "$hint" "$D(auto: yes)$N"; return 0; fi
    read -rp "$1 $hint " reply </dev/tty || reply=""
    reply="${reply:-$def}"
    [[ $reply =~ ^[Yy] ]]
}

have()      { command -v "$1" >/dev/null 2>&1; }
installed() { pacman -Qq "$1" >/dev/null 2>&1; }
wants()     { [[ -z $ONLY || ",$ONLY," == *",$1,"* ]]; }

# ───────────────────────────────────────────────────── package lists ────
# packages/*.txt:  "# title: …", "# required: yes|no", "# about: …", then "package | what it does"
group_meta() { sed -n "s/^# $2: //p" "$1" | head -n1; }
group_rows() { grep -v '^\s*#' "$1" | grep -v '^\s*$' || true; }

show_group() {
    local file="$1" pkg desc mark
    printf '\n%s%s%s  %s(%s)%s\n' "$B" "$(group_meta "$file" title)" "$N" "$D" \
        "$([[ $(group_meta "$file" required) == yes ]] && echo "needed" || echo "optional")" "$N"
    note "$(group_meta "$file" about)"
    while IFS='|' read -r pkg desc; do
        pkg="${pkg// /}"; desc="${desc# }"
        mark=" "
        have pacman && installed "$pkg" && mark="${G}✓${N}"
        printf '  %s %s%-28s%s ' "$mark" "$B" "$pkg" "$N"
        fold -s -w $(( $(tput cols 2>/dev/null || echo 100) - 34 )) <<<"$desc" | sed '2,$s/^/                                 /'
    done < <(group_rows "$file")
}

list_everything() {
    hdr "Packages (✓ = already installed)"
    local f
    for f in "$REPO"/packages/[0-9]*.txt "$REPO"/packages/aur.txt; do show_group "$f"; done

    hdr "Files this installer writes"
    say "  config/*      -> $CONFIG_HOME/   (only the files in this repo; others are left alone)"
    (cd "$REPO/config" && find . -type f | sort | sed "s|^\./|      $CONFIG_HOME/|")
    say "  bin/*         -> $BIN_HOME/"
    (cd "$REPO/bin" && find . -type f | sort | sed "s|^\./|      $BIN_HOME/|")
    say "  apps/*        -> $BIN_HOME/{rice-settings,slap-files} + ~/.local/share/applications/*.desktop (built with cargo)"
    say "  $WALL_HOME/$DEFAULT_WALLPAPER   (a generated gradient, only if the folder has no images)"
    say "  home/.zshrc   -> ~/.zshrc"
    say "  Anything that would be overwritten is first copied to ~/.config/hypr-rice-backup-<date>/"

    hdr "Commands that need sudo (each one is asked separately)"
    say "  sudo pacman -S --needed <packages above>"
    say "  sudo systemctl enable --now NetworkManager bluetooth power-profiles-daemon"
    say "  sudo install -d -o \$USER $SDDM_THEME          (login theme; owned by you so it can follow the wallpaper)"
    say "  sudo tee /etc/sddm.conf.d/10-rice.conf          (selects the theme)"
    say "  sudo tee /etc/sddm.conf.d/20-rice-greeter.conf  (lets the theme read the battery level)"
    say "  sudo systemctl enable sddm                      (only if no display manager is enabled yet)"

    hdr "Things generated later, on your machine"
    say "  matugen writes the palette files each time the wallpaper changes:"
    sed -n 's/^output_path *= *"\(.*\)"/      \1/p' "$REPO/config/matugen/config.toml"
    say "  rice-settings writes ~/.config/hypr/settings.lua, ~/.config/rice/*, and edits ~/.config/waybar/{config.jsonc,style.css}"
    say "  lyra (music popup) downloads album art from the URL your media player reports, into ~/.cache/lyra/"
    say "  Nothing else in this setup talks to the network, and nothing collects data."
}

packages_markdown() {
    cat <<'MD'
# Packages

Every package this setup installs, and what it is for. This file is generated from
[`packages/*.txt`](packages/) by `./install.sh --packages-md > PACKAGES.md`; the installer
reads the same files, so what you see here is exactly what it offers to install.

Run `./install.sh --list` to see this list with a tick next to what you already have.
MD
    local f pkg desc
    for f in "$REPO"/packages/[0-9]*.txt "$REPO"/packages/aur.txt; do
        printf '\n## %s (%s)\n\n%s\n\n' "$(group_meta "$f" title)" \
            "$([[ $(group_meta "$f" required) == yes ]] && echo needed || echo optional)" "$(group_meta "$f" about)"
        printf '| package | what it does here |\n|---|---|\n'
        while IFS='|' read -r pkg desc; do
            pkg="${pkg// /}"
            if [[ $f == */aur.txt ]]; then printf '| [`%s`](https://aur.archlinux.org/packages/%s) | %s |\n' "$pkg" "$pkg" "${desc# }"
            else printf '| [`%s`](https://archlinux.org/packages/?name=%s) | %s |\n' "$pkg" "$pkg" "${desc# }"; fi
        done < <(group_rows "$f")
    done
    cat <<'MD'

## Not installed by this setup

- **Graphics drivers.** Install the ones for your GPU yourself (`mesa` plus `vulkan-intel` /
  `vulkan-radeon`, or `nvidia-open` + `nvidia-utils`). See the Arch wiki for your hardware.
- **Your apps.** Browser aside, no editors, chat or media apps are installed. If VS Code /
  Code OSS / VSCodium is present, its colours follow the wallpaper automatically.
- **`jq`** is not needed: scripts read `hyprctl -j` output with Python.
MD
}

# pick_packages FILE — prints the packages from FILE the user agreed to and that are missing
pick_packages() {
    local file="$1" pkg desc choice="a" todo=() missing=()
    while IFS='|' read -r pkg desc; do
        pkg="${pkg// /}"
        if [[ $pkg == rust ]] && have cargo; then continue; fi
        installed "$pkg" || missing+=("$pkg|${desc# }")
    done < <(group_rows "$file")
    if (( ${#missing[@]} == 0 )); then ok "all installed already" >&2; return 0; fi
    if ! (( YES || DRY )); then
        read -rp "Install the ${#missing[@]} missing? [A]ll / [c]hoose one by one / [s]kip " choice </dev/tty || choice=s
        choice="${choice:-a}"
    fi
    case "${choice,,}" in
        s*|n*) return 0 ;;
        c*) for pkg in "${missing[@]}"; do
                printf '\n  %s%s%s — %s\n' "$B" "${pkg%%|*}" "$N" "${pkg#*|}" >&2
                ask "  install ${pkg%%|*}?" >&2 && todo+=("${pkg%%|*}")
            done ;;
        *)  for pkg in "${missing[@]}"; do todo+=("${pkg%%|*}"); done ;;
    esac
    (( ${#todo[@]} )) && printf '%s\n' "${todo[@]}"
    return 0
}

# ─────────────────────────────────────────────────────────── steps ────
step_packages() {
    hdr "Packages from the official Arch repos"
    local f todo
    for f in "$REPO"/packages/[0-9]*.txt; do
        show_group "$f"
        mapfile -t todo < <(pick_packages "$f")
        (( ${#todo[@]} )) || continue
        run sudo pacman -S --needed "${todo[@]}"
    done
}

step_aur() {
    hdr "AUR packages"
    show_group "$REPO/packages/aur.txt"
    local helper="" todo
    for h in paru yay; do have "$h" && { helper="$h"; break; }; done
    if [[ -z $helper ]]; then
        warn "no AUR helper (yay / paru) found, skipping. To get one:"
        say  "    git clone https://aur.archlinux.org/yay.git && cd yay && makepkg -si"
        say  "  then run:  ./install.sh --only aur"
        return 0
    fi
    mapfile -t todo < <(pick_packages "$REPO/packages/aur.txt")
    (( ${#todo[@]} )) || return 0
    run "$helper" -S --needed "${todo[@]}"
}

COPIED=0; SAME=0; BACKED=0
# put SRC DST [MODE] — copy one file, backing up a different existing one first
put() {
    local src="$1" dst="$2" mode="${3:-644}" b
    if [[ -e $dst || -L $dst ]]; then
        if cmp -s "$src" "$dst"; then SAME=$((SAME + 1)); return 0; fi
        b="$BACKUP/${dst#"$HOME"/}"
        note "  backup  $dst"
        if ! (( DRY )); then mkdir -p "$(dirname "$b")"; cp -a "$dst" "$b"; fi
        BACKED=$((BACKED + 1))
    fi
    note "  write   $dst"
    (( DRY )) || install -Dm"$mode" "$src" "$dst"
    COPIED=$((COPIED + 1))
}
put_summary() {
    ok "$COPIED written, $SAME already identical, $BACKED backed up"
    (( BACKED )) && say "  backups: $BACKUP"
    COPIED=0; SAME=0; BACKED=0
}

step_configs() {
    hdr "Config files -> $CONFIG_HOME"
    say "Copies the files under config/. Your other files in those folders are not touched."
    ask "Copy configs?" || return 0
    local f
    while IFS= read -r f; do put "$REPO/config/$f" "$CONFIG_HOME/$f"; done < <(cd "$REPO/config" && find . -type f | sed 's|^\./||' | sort)
    put_summary
    note "Machine-specific settings (monitors, extra binds) go in $CONFIG_HOME/hypr/local.lua; see local.lua.example there."
}

step_scripts() {
    hdr "Scripts -> $BIN_HOME"
    say "The wall script plus the rice-* helpers and lyra. They are plain bash / Python; read them in bin/."
    ask "Copy scripts?" || return 0
    local f
    for f in "$REPO"/bin/*; do put "$f" "$BIN_HOME/$(basename "$f")" 755; done
    put_summary
    note "hyprland.lua and .zshrc both put $BIN_HOME on your PATH."
}

first_wallpaper() {
    find -L "$WALL_HOME" -maxdepth 2 -type f \( -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.png' -o -iname '*.webp' \) 2>/dev/null | sort | head -n1
}

step_wallpapers() {
    hdr "Wallpaper folder -> $WALL_HOME"
    say "This repo ships no wallpapers: use your own. Any image you put in $WALL_HOME"
    say "shows up in the picker (SUPER+W) and in Settings, and the desktop takes its colours from it."
    ask "Create the folder (and ~/Pictures/Screenshots for the screenshot key)?" || return 0
    run mkdir -p "$WALL_HOME" "$HOME/Pictures/Screenshots"
    if [[ -n $(first_wallpaper) ]]; then ok "you already have wallpapers there"; return 0; fi
    have magick || { note "The folder is empty. Add an image before you log in, or the desktop starts plain grey."; return 0; }
    say "The folder is empty. A plain starter wallpaper can be drawn locally with ImageMagick"
    say "(a soft blue-grey gradient), so the first login has something to show and to take colours from."
    ask "Generate $WALL_HOME/$DEFAULT_WALLPAPER?" || return 0
    run magick -size 2560x1440 -define gradient:angle=145 gradient:'#1b2433-#3d5a80' \
        \( -size 2560x1440 radial-gradient:'#98c1d9-#00000000' -evaluate multiply 0.35 \) -compose screen -composite \
        -attenuate 0.12 +noise Gaussian -quality 92 "$WALL_HOME/$DEFAULT_WALLPAPER"
}

step_apps() {
    hdr "Apps: rice-settings and slap-files"
    say "Both are built here from the Rust source in apps/ (a few minutes the first time), then"
    say "installed to $BIN_HOME with a launcher entry. cargo downloads the crates listed in each Cargo.lock."
    ask "Build and install both apps?" || return 0
    if ! have cargo; then warn "cargo not found: install the 'rust' package (step: packages) and rerun with --only apps"; return 0; fi
    if ! pkg-config --atleast-version=4.14 gtk4 2>/dev/null; then warn "GTK 4.14+ headers not found: install gtk4 and pkgconf, then rerun with --only apps"; return 0; fi
    run "$REPO/apps/rice-settings/install.sh"
    if ask "Make slap-files the default file manager (opens folders, handles \"show in folder\")?"; then
        run "$REPO/apps/slap-files/install.sh" --default
    else
        run "$REPO/apps/slap-files/install.sh"
    fi
}

step_theme() {
    hdr "GTK theme, icons, cursor"
    have gsettings || { warn "gsettings not found (package glib2), skipping"; return 0; }
    say "Sets these desktop-wide preferences with gsettings (stored in dconf, per user):"
    say "  GTK theme adw-gtk3-dark · icons Papirus-Dark · cursor Bibata-Modern-Classic 24 · dark mode"
    ask "Apply?" || return 0
    local s=org.gnome.desktop.interface
    run gsettings set $s gtk-theme adw-gtk3-dark
    run gsettings set $s icon-theme Papirus-Dark
    run gsettings set $s color-scheme prefer-dark
    if [[ -d /usr/share/icons/Bibata-Modern-Classic || -d $HOME/.icons/Bibata-Modern-Classic ]]; then
        run gsettings set $s cursor-theme Bibata-Modern-Classic
        run gsettings set $s cursor-size 24
    else
        note "Bibata cursor not installed (AUR: bibata-cursor-theme), leaving the cursor as it is"
    fi
}

step_palette() {
    hdr "First colour palette"
    say "Runs matugen once on a wallpaper so the bar, launcher, terminal and lock screen have colours"
    say "at first login. After that, changing the wallpaper (SUPER+W) regenerates everything."
    have matugen || { warn "matugen not installed, skipping (the desktop starts with fallback greys; press SUPER+W to theme it)"; return 0; }
    [[ -f $CONFIG_HOME/matugen/config.toml ]] || (( DRY )) || { warn "matugen config missing (run the configs step first), skipping"; return 0; }
    local state="$HOME/.cache/rice/current_wallpaper" img=""
    [[ -f $state ]] && img="$(<"$state")"
    [[ -f $img ]] || img="$(first_wallpaper)"
    if [[ ! -f $img ]]; then
        if (( DRY )); then img="$WALL_HOME/$DEFAULT_WALLPAPER"
        else warn "no wallpaper in $WALL_HOME yet: add an image there, log in, and press SUPER+W"; return 0; fi
    fi
    ask "Generate the palette from $(basename "$img")?" || return 0
    if [[ -n ${HYPRLAND_INSTANCE_SIGNATURE:-} ]] && have awww && [[ -x $BIN_HOME/wall ]]; then
        run "$BIN_HOME/wall" set "$img"     # already inside Hyprland: set it for real
    else
        run mkdir -p "$(dirname "$state")" "$CONFIG_HOME/mako"
        # the templates' reload hooks have nothing to reload outside a session; their errors are harmless
        if (( DRY )); then run matugen image "$img" -m dark -t scheme-tonal-spot --source-color-index 0
        else
            printf '%s  $ matugen image %q -m dark -t scheme-tonal-spot --source-color-index 0%s\n' "$D" "$img" "$N"
            matugen image "$img" -m dark -t scheme-tonal-spot --source-color-index 0 >/dev/null 2>&1 || true
            echo "$img" > "$state"
        fi
    fi
    if (( DRY )) || [[ -f $CONFIG_HOME/waybar/colors.css ]]; then ok "palette generated"
    else warn "matugen did not write the palette; after logging in, press SUPER+W and pick a wallpaper"; fi
}

step_services() {
    hdr "System services"
    say "The bar's wi-fi, bluetooth and power-profile modules need these running."
    local unit why
    while IFS='|' read -r unit why; do
        systemctl cat "$unit" >/dev/null 2>&1 || { note "  $unit: not installed, skipping"; continue; }
        if systemctl is-enabled --quiet "$unit" 2>/dev/null; then ok "$unit already enabled"; continue; fi
        if [[ $unit == NetworkManager.service ]] && { systemctl is-active --quiet iwd 2>/dev/null || systemctl is-active --quiet systemd-networkd 2>/dev/null; }; then
            warn "iwd or systemd-networkd is managing your network. Enabling NetworkManager next to them can drop your connection; switch over deliberately."
            ask "  Enable $unit anyway? ($why)" n || continue
        else
            ask "  Enable and start $unit? ($why)" || continue
        fi
        run sudo systemctl enable --now "$unit"
    done <<'EOF'
NetworkManager.service|wi-fi and wired networking
bluetooth.service|bluetooth devices
power-profiles-daemon.service|power-saver / balanced / performance
EOF
}

step_sddm() {
    hdr "Login screen (SDDM theme)"
    have sddm || { note "sddm not installed, skipping"; return 0; }
    say "Installs the theme in sddm/ to $SDDM_THEME and selects it. The folder is owned by"
    say "your user, so changing the wallpaper can update the login background without sudo."
    ask "Install the login theme?" || return 0
    run sudo install -d -o "$(id -un)" -g "$(id -gn)" "$SDDM_THEME"
    run install -m644 "$REPO/sddm/Main.qml" "$REPO/sddm/metadata.desktop" "$SDDM_THEME/"
    local conf="[Theme]"$'\n'"Current=rice"
    [[ -d /usr/share/icons/Bibata-Modern-Classic ]] && conf+=$'\n'"CursorTheme=Bibata-Modern-Classic"$'\n'"CursorSize=24"
    say "Writing /etc/sddm.conf.d/10-rice.conf:"; note "$(sed 's/^/    /' <<<"$conf")"
    run sudo install -d /etc/sddm.conf.d
    (( DRY )) || printf '%s\n' "$conf" | sudo tee /etc/sddm.conf.d/10-rice.conf >/dev/null
    conf="[General]"$'\n'"# lets the rice theme read battery state from /sys"$'\n'"GreeterEnvironment=QML_XHR_ALLOW_FILE_READ=1"
    say "Writing /etc/sddm.conf.d/20-rice-greeter.conf:"; note "$(sed 's/^/    /' <<<"$conf")"
    (( DRY )) || printf '%s\n' "$conf" | sudo tee /etc/sddm.conf.d/20-rice-greeter.conf >/dev/null
    # wallpaper + colours into the theme (needs the palette step)
    if [[ -x $BIN_HOME/rice-sddm-sync && -f $CONFIG_HOME/hypr/hyprlock-colors.conf ]]; then
        run "$BIN_HOME/rice-sddm-sync" || warn "could not sync wallpaper into the theme yet; it happens on the next wallpaper change"
    fi
    if systemctl is-enabled --quiet display-manager.service 2>/dev/null; then
        ok "a display manager is already enabled ($(basename "$(readlink -f /etc/systemd/system/display-manager.service)")); not changing it"
    elif ask "No display manager is enabled. Enable sddm so it starts at boot?"; then
        run sudo systemctl enable sddm.service
    fi
}

step_shell() {
    hdr "Shell config"
    say "home/.zshrc: plugin-free zsh config (history, completion, git branch prompt in terminal colours)."
    say "Your login shell is not changed; kitty starts zsh by itself (shell zsh in kitty.conf)."
    ask "Copy it to ~/.zshrc?" || return 0
    put "$REPO/home/.zshrc" "$HOME/.zshrc"
    put_summary
}

# ──────────────────────────────────────────────────────────── main ────
case "$MODE" in
    list)     list_everything; exit 0 ;;
    markdown) packages_markdown; exit 0 ;;
esac

(( EUID != 0 )) || die "run this as your normal user, not root. It calls sudo itself, and tells you each time."
have pacman || die "this installer is for Arch Linux (pacman not found). ./install.sh --list shows what it would set up, to do it by hand."
for s in ${ONLY//,/ }; do [[ " ${ALL_STEPS[*]} " == *" $s "* ]] || die "unknown step '$s' (steps: ${ALL_STEPS[*]})"; done

printf '%shypr-rice%s — Hyprland desktop themed from your wallpaper\n' "$B" "$N"
say "Each step explains itself and asks before doing anything. Ctrl+C stops at any point;"
say "rerunning is safe (installed packages and identical files are skipped)."
(( DRY )) && warn "dry run: commands are printed, nothing is changed"

for s in "${ALL_STEPS[@]}"; do
    wants "$s" && "step_$s"
done

hdr "Done"
[[ -d $BACKUP ]] && say "Files that were replaced are in: $BACKUP"
cat <<EOF
Next:
  1. Log out, and at the login screen pick the "Hyprland (uwsm-managed)" session.
     (From a TTY:  uwsm start hyprland.desktop)
  2. SUPER+/ shows every keybind.  SUPER+W picks a wallpaper and re-themes everything.
  3. SUPER+I opens Settings: set your monitor's resolution and scale under "display".
See README.md for how the pieces fit together, and PACKAGES.md for what each package does.
EOF
