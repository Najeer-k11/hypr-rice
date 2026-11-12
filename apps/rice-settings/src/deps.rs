//! Optional tools each page relies on. Missing ones get an inline "install" banner that
//! runs the right package-manager command in a terminal, then rebuilds the page.

use crate::{ui, util};
use gtk::prelude::*;
use std::path::Path;
use std::rc::Rc;

pub struct Dep {
    /// binaries that satisfy this (any one)
    pub bins: &'static [&'static str],
    pub name: &'static str,
    pub why: &'static str,
    pub arch: &'static str,
    pub aur: bool,
    pub apt: &'static str,
    pub dnf: &'static str,
    pub zypper: &'static str,
    /// systemd service that must be running, if any
    pub service: Option<&'static str>,
    /// shown when there is no package for this distro
    pub hint: &'static str,
}

macro_rules! dep {
    ($id:ident, [$($b:expr),+], $name:expr, $why:expr, arch=$a:expr, aur=$aur:expr, apt=$apt:expr, dnf=$dnf:expr, zypper=$z:expr, service=$s:expr, hint=$h:expr) => {
        pub const $id: Dep = Dep { bins: &[$($b),+], name: $name, why: $why, arch: $a, aur: $aur, apt: $apt, dnf: $dnf, zypper: $z, service: $s, hint: $h };
    };
}

dep!(MATUGEN, ["matugen"], "matugen", "generates the colour palette from your wallpaper",
     arch="matugen-bin", aur=true, apt="", dnf="", zypper="", service=None, hint="cargo install matugen");
dep!(WALLPAPER, ["awww", "swww", "hyprpaper"], "swww", "sets the wallpaper (awww, swww or hyprpaper)",
     arch="swww", aur=false, apt="swww", dnf="swww", zypper="swww", service=None, hint="");
dep!(NETWORKMANAGER, ["nmcli"], "NetworkManager", "wi-fi scanning and connecting",
     arch="networkmanager", aur=false, apt="network-manager", dnf="NetworkManager", zypper="NetworkManager", service=Some("NetworkManager"), hint="");
dep!(BLUEZ, ["bluetoothctl"], "BlueZ", "bluetooth devices",
     arch="bluez bluez-utils", aur=false, apt="bluez", dnf="bluez", zypper="bluez", service=Some("bluetooth"), hint="");
dep!(PPD, ["powerprofilesctl"], "power-profiles-daemon", "power saver / balanced / performance",
     arch="power-profiles-daemon", aur=false, apt="power-profiles-daemon", dnf="power-profiles-daemon", zypper="power-profiles-daemon", service=Some("power-profiles-daemon"), hint="");
dep!(BRIGHTNESSCTL, ["brightnessctl"], "brightnessctl", "keyboard backlight without root",
     arch="brightnessctl", aur=false, apt="brightnessctl", dnf="brightnessctl", zypper="brightnessctl", service=None, hint="");
dep!(ASUSCTL, ["asusctl"], "asusctl", "ASUS keyboard RGB, charge limit and profiles",
     arch="asusctl", aur=true, apt="", dnf="asusctl", zypper="", service=Some("asusd"), hint="see asus-linux.org");
dep!(OPENRGB, ["openrgb"], "OpenRGB", "RGB control for non-ASUS keyboards",
     arch="openrgb", aur=false, apt="openrgb", dnf="openrgb", zypper="OpenRGB", service=None, hint="");
dep!(WAYBAR, ["waybar"], "Waybar", "the panel",
     arch="waybar", aur=false, apt="waybar", dnf="waybar", zypper="waybar", service=None, hint="");

pub const ALL: &[&Dep] = &[&MATUGEN, &WALLPAPER, &WAYBAR, &NETWORKMANAGER, &BLUEZ, &PPD, &BRIGHTNESSCTL, &ASUSCTL, &OPENRGB];

pub fn have(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

pub fn installed(d: &Dep) -> bool {
    d.bins.iter().any(|b| have(b))
}

pub fn service_active(s: &str) -> bool {
    util::sh("systemctl", &["is-active", s]).is_some_and(|o| o.trim() == "active")
}

fn aur_helper() -> Option<&'static str> {
    ["paru", "yay"].into_iter().find(|h| have(h))
}

/// Shell command that installs `d`, or None if this distro has no package for it.
pub fn install_cmd(d: &Dep) -> Option<String> {
    let base = if have("pacman") {
        if d.aur {
            aur_helper().map(|h| format!("{h} -S --needed {}", d.arch))?
        } else {
            format!("sudo pacman -S --needed {}", d.arch)
        }
    } else if have("apt") && !d.apt.is_empty() {
        format!("sudo apt install {}", d.apt)
    } else if have("dnf") && !d.dnf.is_empty() {
        format!("sudo dnf install {}", d.dnf)
    } else if have("zypper") && !d.zypper.is_empty() {
        format!("sudo zypper install {}", d.zypper)
    } else {
        return None;
    };
    Some(match d.service {
        Some(s) => format!("{base} && sudo systemctl enable --now {s}"),
        None => base,
    })
}

fn no_cmd_hint(d: &Dep) -> String {
    if have("pacman") && d.aur {
        format!("AUR package {} · needs an AUR helper (yay / paru)", d.arch)
    } else if !d.hint.is_empty() {
        d.hint.to_string()
    } else {
        format!("install {} with your package manager", d.name)
    }
}

/// A terminal that can run a command, as argv prefix.
fn terminal() -> Option<Vec<String>> {
    let mut cands: Vec<String> = std::env::var("TERMINAL").ok().into_iter().collect();
    cands.extend(["kitty", "foot", "alacritty", "ghostty", "wezterm", "konsole", "gnome-terminal", "xterm"].map(String::from));
    for t in cands {
        let bin = t.split_whitespace().next().unwrap_or("").to_string();
        if bin.is_empty() || !(have(&bin) || Path::new(&bin).is_file()) {
            continue;
        }
        let name = Path::new(&bin).file_name().unwrap_or_default().to_string_lossy().into_owned();
        return Some(match name.as_str() {
            "wezterm" => vec![bin, "start".into(), "--".into()],
            "gnome-terminal" => vec![bin, "--wait".into(), "--".into()],
            "kitty" => vec![bin, "--class".into(), "kitty-float".into(), "-e".into()],
            _ => vec![bin, "-e".into()],
        });
    }
    None
}

/// Run `cmd` visibly (sudo needs a password prompt); `done` runs when the terminal closes.
pub fn run_in_terminal(cmd: &str, done: impl FnOnce() + 'static) -> bool {
    let Some(mut argv) = terminal() else { return false };
    let script = format!("echo '$ {0}'; echo; {0}; echo; read -rp 'done · press enter to close ' _", cmd.replace('\'', "'\\''"));
    argv.extend(["sh".into(), "-c".into(), script]);
    let prog = argv.remove(0);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    util::run(&prog, &args, move |_| done());
    true
}

/// Banner listing what's missing for this page (None when all is well).
/// `on_done` runs after an install / service start so the page can rebuild.
pub fn banner(deps: &[&'static Dep], on_done: impl Fn() + 'static) -> Option<gtk::Box> {
    let on_done = Rc::new(on_done);
    let card = ui::card();
    card.add_css_class("banner");
    for d in deps {
        let r = if !installed(d) {
            let r = ui::row("󰏗", &format!("{} is not installed", d.name), d.why);
            match install_cmd(d) {
                Some(cmd) => {
                    let b = ui::button("󰇚  install", &["suggested"]);
                    b.set_tooltip_text(Some(&cmd));
                    let od = on_done.clone();
                    let sub = r.sub.clone();
                    b.connect_clicked(move |b| {
                        b.set_sensitive(false);
                        let (od, b2) = (od.clone(), b.clone());
                        if !run_in_terminal(&cmd, move || {
                            b2.set_sensitive(true);
                            od();
                        }) {
                            sub.set_text(&format!("no terminal found · run: {cmd}"));
                            b.set_sensitive(true);
                        }
                    });
                    r.suffix(&b);
                }
                None => {
                    r.sub.set_text(&no_cmd_hint(d));
                }
            }
            r
        } else if let Some(s) = d.service.filter(|s| !service_active(s)) {
            let r = ui::row("󰒲", &format!("{s} service is not running"), d.why);
            let b = ui::button("start", &["suggested"]);
            let od = on_done.clone();
            let s = s.to_string();
            b.connect_clicked(move |b| {
                b.set_sensitive(false);
                let (od, b2) = (od.clone(), b.clone());
                util::run("pkexec", &["systemctl", "enable", "--now", &s], move |o| {
                    b2.set_sensitive(true);
                    if !o.ok {
                        util::notify("couldn't start service", &o.msg());
                    }
                    od();
                });
            });
            r.suffix(&b);
            r
        } else {
            continue;
        };
        r.icon.add_css_class("accent");
        ui::add(&card, &r.row);
    }
    card.first_child().map(|_| card)
}
