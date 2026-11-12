//! The app's look: the matugen palette + window radius/border from Hyprland.
//! Palette source = whichever is newer of ~/.config/waybar/colors.css (a rice's matugen
//! template) and ~/.config/rice/colors.css (written by this app when it sets wallpapers itself).
//! Both are watched; the app restyles live when either changes.

use crate::{hypr, ui, util};
use gtk::prelude::*;
use gtk::{gdk, gio};
use std::{cell::RefCell, fs, path::PathBuf};

pub struct Vars {
    pub radius: f64,
    pub border: f64,
    pub gaps_out: f64,
    pub active: f64,
    pub inactive: f64,
}

thread_local! {
    static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
    static VARS: RefCell<Vars> = RefCell::new(Vars { radius: 6.0, border: 1.0, gaps_out: 2.0, active: 1.0, inactive: 1.0 });
    static PALETTE: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = RefCell::new(Vec::new());
    static MONITORS: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
}

const FALLBACK: &[(&str, &str)] = &[
    ("primary", "#a6c8ff"), ("on_primary", "#00315f"), ("primary_container", "#004786"),
    ("on_primary_container", "#d4e3ff"), ("secondary", "#bdc7dc"), ("on_secondary", "#273141"),
    ("secondary_container", "#3e4758"), ("tertiary", "#dbbce1"), ("on_tertiary", "#3e2845"),
    ("tertiary_container", "#563e5c"), ("error", "#ffb4ab"), ("on_error", "#690005"),
    ("background", "#111318"), ("on_background", "#e2e2e9"), ("surface", "#111318"),
    ("on_surface", "#e2e2e9"), ("surface_variant", "#44474e"), ("on_surface_variant", "#c4c6d0"),
    ("surface_container_lowest", "#0c0e13"), ("surface_container_low", "#191c20"),
    ("surface_container", "#1d2024"), ("surface_container_high", "#282a2f"),
    ("surface_container_highest", "#33353a"), ("outline", "#8e9099"), ("outline_variant", "#44474e"),
];

fn candidates() -> [PathBuf; 2] {
    [util::cfg("waybar/colors.css"), own_colors_path()]
}

/// The palette file this app writes itself.
pub fn own_colors_path() -> PathBuf {
    util::cfg("rice/colors.css")
}

pub fn colors_path() -> PathBuf {
    candidates()
        .into_iter()
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
        .unwrap_or_else(own_colors_path)
}

/// Write ~/.config/rice/colors.css from `matugen --json hex` output.
pub fn write_own_palette(json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return false };
    let Some(cols) = v["colors"].as_object() else { return false };
    let mut out = String::from("/* written by rice-settings from matugen */\n");
    for (name, c) in cols {
        if let Some(hex) = c["default"]["color"].as_str() {
            out.push_str(&format!("@define-color {name} {hex};\n"));
        }
    }
    let p = own_colors_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    fs::write(p, out).is_ok()
}

pub fn init() {
    let display = gdk::Display::default().expect("no display");
    PROVIDER.with(|p| {
        gtk::style_context_add_provider_for_display(&display, p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION)
    });
    VARS.with(|v| {
        let mut v = v.borrow_mut();
        v.radius = hypr::opt_i("decoration:rounding", 6) as f64;
        v.border = hypr::opt_i("general:border_size", 1) as f64;
        v.gaps_out = hypr::opt_i("general:gaps_out", 2) as f64;
        v.active = hypr::opt_f("decoration:active_opacity", 1.0);
        v.inactive = hypr::opt_f("decoration:inactive_opacity", 1.0);
    });
    load_palette();
    rebuild();

    // matugen rewrites colors.css when the wallpaper changes -> restyle live.
    let deb = ui::Debounce::default();
    for path in candidates() {
        let dir = path.parent().unwrap().to_path_buf();
        if path == own_colors_path() {
            let _ = fs::create_dir_all(&dir);
        } else if !dir.exists() {
            continue;
        }
        let Ok(m) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else { continue };
        let deb = deb.clone();
        m.connect_changed(move |_, f, other, _| {
            let hit = |x: Option<&gio::File>| {
                x.and_then(|f| f.basename()).is_some_and(|b| b.as_os_str() == "colors.css")
            };
            if hit(Some(f)) || hit(other) {
                deb.run(150, || {
                    load_palette();
                    rebuild();
                    LISTENERS.with(|l| l.borrow().iter().for_each(|f| f()));
                });
            }
        });
        MONITORS.with(|s| s.borrow_mut().push(m));
    }
}

/// Parse the current palette file (no GTK needed; safe on any thread).
pub fn read_palette_file() -> Vec<(String, String)> {
    let txt = fs::read_to_string(colors_path()).unwrap_or_default();
    let mut v: Vec<(String, String)> = txt
        .lines()
        .filter_map(|l| {
            let mut it = l.trim().strip_prefix("@define-color ")?.trim_end_matches(';').split_whitespace();
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect();
    // fill anything missing so the stylesheet never references an undefined colour
    for (n, c) in FALLBACK {
        if !v.iter().any(|(k, _)| k == n) {
            v.push((n.to_string(), c.to_string()));
        }
    }
    v
}

fn load_palette() {
    let v = read_palette_file();
    PALETTE.with(|p| *p.borrow_mut() = v);
}

fn rebuild() {
    let mut css = String::with_capacity(12_000);
    PALETTE.with(|p| {
        for (n, c) in p.borrow().iter() {
            css.push_str(&format!("@define-color {n} {c};\n.sw-{n} {{ background: @{n}; }}\n"));
            if matches!(n.as_str(), "primary" | "secondary" | "tertiary") {
                // keyboard LED swatches: the accent pushed to full saturation
                css.push_str(&format!(".kv-{n} {{ background: #{}; }}\n", crate::pages::keyboard::vivid(c)));
            }
        }
    });
    let app = VARS.with(|v| {
        let v = v.borrow();
        APP_CSS
            .replace("{{R}}", &format!("{}", v.radius.clamp(0.0, 18.0)))
            .replace("{{B}}", &format!("{}", v.border.clamp(0.0, 8.0)))
            .replace("{{GO}}", &format!("{}", v.gaps_out.clamp(0.0, 30.0) + 6.0))
            .replace("{{AO}}", &format!("{}", v.active))
            .replace("{{IO}}", &format!("{}", v.inactive))
    });
    css.push_str(&app);
    PROVIDER.with(|p| p.load_from_string(&css));
}

/// Update a style variable (used by the appearance page) and restyle.
pub fn set_var(f: impl FnOnce(&mut Vars)) {
    VARS.with(|v| f(&mut v.borrow_mut()));
    rebuild();
}

pub fn hex(name: &str) -> String {
    PALETTE.with(|p| {
        p.borrow().iter().find(|(n, _)| n == name).map(|(_, c)| c.clone()).unwrap_or_default()
    })
}

/// Called after the palette changes (i.e. after a wallpaper change).
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

const APP_CSS: &str = r#"
window.rice, window.rice popover, window.rice tooltip {
  font-family: "GeistMono Nerd Font", monospace;
  font-size: 12.5px;
}
window.rice { background: @surface; color: @on_surface; }

/* ---------- sidebar ---------- */
.sidebar {
  background: @surface_container_low;
  border-right: 1px solid @outline_variant;
  padding: 14px 8px 12px 8px;
}
.brand { padding: 2px 10px 14px 10px; }
.brand-icon { color: @primary; font-size: 17px; }
.brand-title { font-weight: 700; font-size: 13.5px; }
.nav, .nav > row { background: transparent; }
.nav > row {
  padding: 7px 10px; margin: 1px 0;
  border-radius: {{R}}px;
  color: @on_surface_variant; outline: none;
}
.nav > row:hover { background: @surface_container; color: @on_surface; }
.nav > row:selected { background: @surface_container_highest; color: @on_surface; }
.nav > row:selected .nav-icon { color: @primary; }
.nav > row.nav-head { padding: 12px 10px 4px 10px; }
.nav > row.nav-head:hover { background: transparent; }
.nav-head label { color: @outline; font-size: 10.5px; }
.nav-icon { font-size: 15px; min-width: 24px; }
.sidebar-foot { color: @outline; font-size: 10.5px; padding: 0 10px; }

/* ---------- page ---------- */
.page-icon { color: @primary; font-size: 26px; min-width: 38px; }
.page-title { font-size: 19px; font-weight: 700; }
.page-sub { color: @outline; }
.section { color: @outline; font-size: 10.5px; font-weight: 600; margin: 10px 4px 2px 4px; }
.card {
  background: @surface_container;
  border: 1px solid @outline_variant;
  border-radius: {{R}}px;
}
.card > separator { background: @outline_variant; min-height: 1px; margin: 0; }
.row { padding: 9px 14px; min-height: 30px; }
.row-icon { font-size: 16px; min-width: 24px; color: @on_surface_variant; }
.row-icon.on { color: @primary; }
.row-sub { color: @outline; font-size: 11.5px; }
.dim { color: @outline; }
.accent { color: @primary; }
.err { color: @error; }
.status { color: @outline; font-size: 11.5px; margin: 2px 4px; }
.empty { color: @outline; padding: 18px; }

/* ---------- controls ---------- */
button {
  background: @surface_container_high; color: @on_surface;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  padding: 4px 12px; min-height: 22px;
  box-shadow: none; text-shadow: none; outline: none;
}
button:hover { background: @surface_container_highest; }
button:active { background: @surface_variant; }
button:disabled { color: @outline; }
button.suggested { background: @primary; color: @on_primary; border-color: @primary; }
button.suggested:hover { background: @primary_container; color: @on_primary_container; }
button.flat { background: transparent; border-color: transparent; }
button.flat:hover { background: @surface_container_high; }
button.danger:hover { color: @error; border-color: @error; }
button.icon { padding: 4px 8px; font-size: 14px; }

switch {
  background: @surface_container_highest; border: 1px solid @outline_variant;
  border-radius: 999px; padding: 2px; min-width: 38px; min-height: 18px; outline: none;
}
switch:checked { background: @primary; border-color: @primary; }
switch > slider {
  background: @outline; border: none; box-shadow: none;
  border-radius: 999px; min-width: 18px; min-height: 18px; margin: 0;
}
switch:checked > slider { background: @on_primary; }
switch > image { color: transparent; }

scale { padding: 8px 0; }
scale > trough { background: @surface_container_highest; min-height: 4px; border-radius: 999px; border: none; outline: none; }
scale > trough > highlight { background: @primary; border-radius: 999px; min-height: 4px; border: none; }
scale > trough > slider {
  background: @primary; border: 3px solid @surface_container; box-shadow: none;
  min-width: 12px; min-height: 12px; margin: -6px; border-radius: 999px;
}
scale > value { color: @on_surface_variant; min-width: 38px; }

entry {
  background: @surface_container_low; color: @on_surface;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  padding: 3px 8px; min-height: 24px; box-shadow: none; outline: none;
  caret-color: @primary;
}
entry:focus-within { border-color: @primary; }
entry > image { color: @outline; }

dropdown > button { padding: 4px 10px; }
popover > contents {
  background: @surface_container_high; color: @on_surface;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  box-shadow: none; padding: 4px;
}
popover > arrow { background: @surface_container_high; border: 1px solid @outline_variant; }
popover listview { background: transparent; }
popover listview > row { border-radius: {{R}}px; padding: 5px 8px; color: @on_surface; }
popover listview > row:hover, popover listview > row:selected { background: @surface_container_highest; }
popover listview > row image { color: @primary; }

.meter { background: @surface_container_highest; border-radius: 999px; }
.meter-fill { background: @primary; border-radius: 999px; }

scrollbar { background: transparent; border: none; }
scrollbar slider { background: @surface_container_highest; border-radius: 999px; min-width: 6px; min-height: 24px; border: none; margin: 0; }
scrollbar.horizontal slider { min-width: 24px; min-height: 6px; }
tooltip { background: @surface_container; border: 1px solid @outline_variant; border-radius: {{R}}px; }
tooltip label { color: @on_surface; }

/* ---------- wallpaper ---------- */
flowbox > flowboxchild { padding: 0; background: transparent; outline: none; border-radius: {{R}}px; }
.wall-frame { border-radius: {{R}}px; border: 2px solid transparent; background: @surface_container; }
flowboxchild:hover .wall-frame { border-color: @outline; }
flowboxchild.current .wall-frame { border-color: @primary; }
.wall-name { color: @on_surface_variant; font-size: 11px; }
flowboxchild.current .wall-name { color: @primary; }

/* ---------- colours ---------- */
.swatch-card { padding: 6px; border-radius: {{R}}px; }
.swatch-card:hover { background: @surface_container_high; }
.swatch { min-height: 48px; border-radius: {{R}}px; border: 1px solid @outline_variant; }
.swatch-name { font-size: 11px; color: @on_surface_variant; }
.hex { color: @outline; font-size: 10.5px; }

/* ---------- appearance preview (live from hyprland values) ---------- */
.mock-desk {
  background: @surface_container_lowest;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  padding: {{GO}}px; min-height: 150px;
}
.mock-win {
  background: @surface_container;
  border: {{B}}px solid @surface_container_high;
  border-radius: {{R}}px; padding: 12px; opacity: {{IO}};
}
.mock-win.active { border-color: @primary; opacity: {{AO}}; }
.mock-line { background: @surface_container_highest; border-radius: 999px; min-height: 6px; }
.mock-line.accent { background: @primary; }

/* ---------- banners / segmented / power / keyboard ---------- */
.banner { border-color: @primary; background: alpha(@primary, 0.07); }
.segmented > button { border-radius: 0; margin-left: -1px; padding: 4px 14px; }
.segmented > button:first-child { border-radius: {{R}}px 0 0 {{R}}px; margin-left: 0; }
.segmented > button:last-child { border-radius: 0 {{R}}px {{R}}px 0; }
button:checked { background: @primary; color: @on_primary; border-color: @primary; }
button.profile {
  background: @surface_container; border: 1px solid @outline_variant;
  border-radius: {{R}}px; padding: 14px 16px;
}
button.profile:hover { background: @surface_container_high; }
button.profile.selected { background: alpha(@primary, 0.12); border-color: @primary; }
.profile-icon { font-size: 22px; color: @on_surface_variant; }
button.profile.selected .profile-icon { color: @primary; }
button.flat.selected { background: @surface_container_high; color: @primary; }
.app-row { padding: 6px 14px; font-size: 12px; }
.app-row progressbar trough { min-height: 4px; background: @surface_container_high; border: none; border-radius: 2px; }
.app-row progressbar progress { min-height: 4px; background: @primary; border: none; border-radius: 2px; }
.profile-name { font-weight: 700; }
button.led {
  min-width: 26px; min-height: 26px; padding: 0;
  border-radius: 999px; border: 2px solid @outline_variant;
}
button.led:hover { border-color: @on_surface; }
.kc-white { background: #ffffff; } .kc-red { background: #ff0000; } .kc-orange { background: #ff5a00; }
.kc-yellow { background: #ffc800; } .kc-green { background: #00ff3c; } .kc-cyan { background: #00e5ff; }
.kc-blue { background: #0040ff; } .kc-purple { background: #8a2be2; } .kc-pink { background: #ff1e8c; }

/* ---------- about ---------- */
.hero { padding: 18px 20px; }
.hero-logo { font-size: 64px; color: @primary; min-width: 90px; }
.hero-title { font-size: 21px; font-weight: 700; }
.badge {
  background: @primary_container; color: @on_primary_container;
  border-radius: 999px; padding: 2px 10px; font-size: 11px;
}
"#;
