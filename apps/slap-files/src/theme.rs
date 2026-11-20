//! The look: the matugen palette + window radius/border from Hyprland (same engine as
//! rice-settings). Palette source = whichever is newer of ~/.config/waybar/colors.css (the
//! rice's matugen template) and ~/.config/rice/colors.css (written by rice-settings).
//! Both are watched; the app recolours live when the wallpaper changes.

use crate::{hypr, ui, util};
use gtk::prelude::*;
use gtk::{gdk, gio};
use std::{cell::Cell, cell::RefCell, fs, path::PathBuf};

struct Vars {
    radius: f64,
    border: f64,
}

thread_local! {
    static DONE: Cell<bool> = const { Cell::new(false) };
    static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
    static VARS: RefCell<Vars> = const { RefCell::new(Vars { radius: 6.0, border: 1.0 }) };
    static PALETTE: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    static MONITORS: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
}

const FALLBACK: &[(&str, &str)] = &[
    ("primary", "#a6c8ff"), ("on_primary", "#00315f"), ("primary_container", "#004786"),
    ("on_primary_container", "#d4e3ff"), ("secondary", "#bdc7dc"), ("on_secondary", "#273141"),
    ("secondary_container", "#3e4758"), ("on_secondary_container", "#d9e3f8"),
    ("tertiary", "#dbbce1"), ("on_tertiary", "#3e2845"),
    ("tertiary_container", "#563e5c"), ("error", "#ffb4ab"), ("on_error", "#690005"),
    ("background", "#111318"), ("on_background", "#e2e2e9"), ("surface", "#111318"),
    ("on_surface", "#e2e2e9"), ("surface_variant", "#44474e"), ("on_surface_variant", "#c4c6d0"),
    ("surface_container_lowest", "#0c0e13"), ("surface_container_low", "#191c20"),
    ("surface_container", "#1d2024"), ("surface_container_high", "#282a2f"),
    ("surface_container_highest", "#33353a"), ("outline", "#8e9099"), ("outline_variant", "#44474e"),
];

fn candidates() -> [PathBuf; 2] {
    [util::cfg("waybar/colors.css"), util::cfg("rice/colors.css")]
}

fn colors_path() -> Option<PathBuf> {
    candidates()
        .into_iter()
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// Install the stylesheet once per process and start watching the palette files.
pub fn init() {
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let display = gdk::Display::default().expect("no display");
    PROVIDER.with(|p| {
        gtk::style_context_add_provider_for_display(&display, p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION)
    });
    VARS.with(|v| {
        let mut v = v.borrow_mut();
        v.radius = hypr::opt_i("decoration:rounding", 6) as f64;
        v.border = hypr::opt_i("general:border_size", 1) as f64;
    });
    load_palette();
    rebuild();

    // matugen rewrites colors.css when the wallpaper changes -> restyle live.
    let deb = ui::Debounce::default();
    for path in candidates() {
        let dir = path.parent().unwrap().to_path_buf();
        if !dir.exists() {
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
                });
            }
        });
        MONITORS.with(|s| s.borrow_mut().push(m));
    }
}

fn read_palette_file() -> Vec<(String, String)> {
    let txt = colors_path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
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
    let mut css = String::with_capacity(16_000);
    PALETTE.with(|p| {
        for (n, c) in p.borrow().iter() {
            css.push_str(&format!("@define-color {n} {c};\n"));
        }
    });
    let app = VARS.with(|v| {
        let v = v.borrow();
        APP_CSS
            .replace("{{R}}", &format!("{}", v.radius.clamp(0.0, 18.0)))
            .replace("{{RS}}", &format!("{}", (v.radius * 0.7).clamp(0.0, 12.0)))
            .replace("{{B}}", &format!("{}", v.border.clamp(0.0, 8.0)))
            .replace("{{FS}}", &format!("{:.2}", base_font_px()))
    });
    css.push_str(&app);
    PROVIDER.with(|p| p.load_from_string(&css));
}

/// UI text size in px: 14 at 1.0, scaled by the desktop's text-scaling-factor
/// (or SLAP_FONT_SCALE). Every other size in APP_CSS is in em, so it all follows.
fn base_font_px() -> f64 {
    static PX: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PX.get_or_init(|| {
    let scale = std::env::var("SLAP_FONT_SCALE").ok().and_then(|v| v.parse::<f64>().ok()).or_else(|| {
        let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "text-scaling-factor"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok()
    });
    14.0 * scale.unwrap_or(1.0).clamp(0.6, 3.0)
    })
}

const APP_CSS: &str = r#"
window.slap, window.slap popover, window.slap tooltip {
  font-family: "Google Sans Flex", "GeistMono Nerd Font", "Symbols Nerd Font", sans-serif;
  font-size: {{FS}}px;
}
window.slap { background: @surface; color: @on_surface; }

/* ---------- sidebar ---------- */
.sidebar {
  background: @surface_container_low;
  border-right: 1px solid @outline_variant;
  padding: 14px 8px 12px 8px;
}
.brand { padding: 2px 10px 14px 10px; }
.brand-icon { color: @primary; font-size: 1.36em; }
.brand-title { font-weight: 700; font-size: 1.08em; }
.nav, .nav > row { background: transparent; }
.nav > row {
  padding: 6px 10px; margin: 1px 0;
  border-radius: {{R}}px;
  color: @on_surface_variant; outline: none;
}
.nav > row:hover { background: @surface_container; color: @on_surface; }
.nav > row:selected { background: @surface_container_highest; color: @on_surface; }
.nav > row:selected .nav-icon { color: @primary; }
.nav > row.nav-head { padding: 12px 10px 4px 10px; }
.nav > row.nav-head:hover { background: transparent; }
.nav-head label { color: @outline; font-size: 0.84em; }
.nav-icon { font-size: 1.2em; min-width: 24px; }
.sidebar-foot { color: @outline; font-size: 0.84em; padding: 0 10px; }

/* ---------- toolbar / address ---------- */
.fm-toolbar { background: @surface; padding: 10px 14px 10px 10px; border-bottom: 1px solid @outline_variant; }
button {
  background: @surface_container_high; color: @on_surface;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  padding: 4px 12px; min-height: 22px;
  box-shadow: none; text-shadow: none; outline: none;
}
button:hover { background: @surface_container_highest; }
button:active { background: @surface_variant; }
button.tool {
  background: transparent; border-color: transparent;
  padding: 3px 8px; font-size: 1.2em; min-width: 18px; color: @on_surface_variant;
}
button.tool:hover { background: @surface_container_high; color: @on_surface; }
button.tool:disabled { color: @outline_variant; background: transparent; }
button.tool:checked { background: @surface_container_highest; color: @primary; }

.address {
  background: @surface_container_low;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  min-height: 30px; padding: 0 4px;
}
.address:hover { border-color: @outline; }
.address.editing { border-color: @primary; }
.address entry, .address entry:focus-within {
  background: transparent; border: none; box-shadow: none; min-height: 28px; padding: 0 6px;
}
button.crumb {
  background: transparent; border-color: transparent;
  padding: 2px 6px; min-height: 20px; color: @on_surface_variant;
  border-radius: {{RS}}px;
}
button.crumb:hover { background: @surface_container_high; color: @on_surface; }
button.crumb.current { color: @on_surface; font-weight: 600; }
.crumb-sep { color: @outline; font-size: 0.8em; margin: 0 1px; }

entry {
  background: @surface_container_low; color: @on_surface;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  padding: 3px 8px; min-height: 24px; box-shadow: none; outline: none;
  caret-color: @primary;
}
entry:focus-within { border-color: @primary; }
entry > image { color: @outline; }
entry selection, label selection { background: alpha(@primary, 0.35); color: @on_surface; }

/* ---------- details view ---------- */
columnview, gridview { background: @surface; color: @on_surface; }
columnview > header { background: @surface; border-bottom: 1px solid @outline_variant; }
columnview > header > button {
  background: transparent; border: none; border-radius: 0;
  padding: 6px 10px; min-height: 18px;
  color: @outline; font-size: 0.88em; font-weight: 600;
}
columnview > header > button:hover { color: @on_surface; background: @surface_container_low; }
columnview > header > button sort-indicator { color: @primary; min-width: 12px; min-height: 12px; }
columnview > listview { background: transparent; padding: 4px 6px; }
columnview > listview > row {
  border-radius: 0; color: @on_surface; outline: none;
  margin: 0; padding: 0;
}
columnview > listview > row > cell { padding: 4px 10px; }
columnview > listview > row:hover { background: @surface_container_low; }
columnview > listview > row:selected { background: alpha(@primary, 0.18); }
columnview > listview > row:selected:hover { background: alpha(@primary, 0.25); }
columnview > listview > row:focus-visible { outline: 1px solid alpha(@primary, 0.6); outline-offset: -1px; }
.file-icon { font-size: 1.28em; min-width: 22px; }
.cell-dim { color: @outline; }

/* ---------- grid view ---------- */
gridview { padding: 8px; }
gridview > child {
  border-radius: 0; padding: 10px 6px 8px 6px; margin: 2px;
  min-width: 96px; outline: none;
}
gridview > child:hover { background: @surface_container_low; }
gridview > child:selected { background: alpha(@primary, 0.18); }
gridview > child:selected:hover { background: alpha(@primary, 0.25); }
gridview > child:focus-visible { outline: 1px solid alpha(@primary, 0.6); outline-offset: -1px; }
.grid-icon { font-size: 3.36em; }
.grid-thumb { border-radius: {{RS}}px; }
.grid-name { font-size: 0.92em; }

rubberband { background: alpha(@primary, 0.14); border: 1px solid @primary; }

/* type tints: follow the wallpaper palette */
.t-folder  { color: @primary; }
.t-image   { color: @tertiary; }
.t-media   { color: @secondary; }
.t-archive { color: @tertiary; }
.t-code    { color: @primary; }
.t-doc     { color: @on_surface_variant; }
.t-exec    { color: @secondary; }
.t-link    { color: @on_surface_variant; }
.t-file    { color: @outline; }
.hidden-item { opacity: 0.6; }

/* ---------- empty / status ---------- */
.empty-view { color: @outline; font-size: 1.04em; }
.empty-view.err { color: @error; }
.empty-icon { font-size: 3.52em; color: @outline_variant; }
.statusbar {
  background: @surface_container_low; border-top: 1px solid @outline_variant;
  padding: 4px 14px; color: @outline; font-size: 0.88em;
}
.statusbar .err { color: @error; }

/* ---------- misc ---------- */
scrollbar { background: transparent; border: none; }
scrollbar slider { background: @surface_container_highest; border-radius: 999px; min-width: 6px; min-height: 24px; border: none; margin: 0; }
scrollbar.horizontal slider { min-width: 24px; min-height: 6px; }
tooltip { background: @surface_container; border: 1px solid @outline_variant; border-radius: {{R}}px; }
tooltip label { color: @on_surface; }
popover > contents {
  background: @surface_container_high; color: @on_surface;
  border: 1px solid @outline_variant; border-radius: {{R}}px;
  box-shadow: none; padding: 4px;
}
popover > arrow { background: @surface_container_high; border: 1px solid @outline_variant; }

/* ---------- context menu ---------- */
popover.menu > contents { padding: 4px; min-width: 210px; }
popover.menu modelbutton {
  padding: 5px 10px; min-height: 22px; border-radius: {{RS}}px;
  color: @on_surface; background: transparent;
}
popover.menu modelbutton:hover, popover.menu modelbutton:focus { background: @surface_container_highest; outline: none; }
popover.menu modelbutton:disabled { color: @outline; }
popover.menu modelbutton accelerator { color: @outline; margin-left: 28px; font-size: 0.88em; }
popover.menu separator { background: @outline_variant; min-height: 1px; margin: 4px 6px; }

/* ---------- dialogs (in-window) ---------- */
.scrim { background: alpha(@surface_container_lowest, 0.55); }
.dialog {
  background: @surface_container; color: @on_surface;
  border: {{B}}px solid @outline_variant; border-radius: {{R}}px;
  padding: 18px 18px 14px 18px;
}
.dialog-title { font-weight: 700; font-size: 1.12em; margin-bottom: 2px; }
.dialog-body { color: @on_surface_variant; }
.dialog-error { color: @error; font-size: 0.88em; }
button.suggested { background: @primary; color: @on_primary; border-color: @primary; }
button.suggested:hover { background: @primary_container; color: @on_primary_container; }
button.destructive { background: @error; color: @on_error; border-color: @error; }
button.destructive:hover { background: alpha(@error, 0.8); }
button:focus-visible { outline: 2px solid alpha(@primary, 0.6); outline-offset: 1px; }

/* ---------- operations panel ---------- */
.ops { background: @surface_container_low; border-top: 1px solid @outline_variant; padding: 8px 14px; }
.op-title { font-size: 0.96em; }
.op-sub { color: @outline; font-size: 0.88em; }
progressbar > trough { background: @surface_container_highest; border-radius: 999px; min-height: 4px; border: none; }
progressbar > trough > progress { background: @primary; border-radius: 999px; min-height: 4px; min-width: 0; margin: 0; border: none; }

/* ---------- tabs ---------- */
.tabbar { background: @surface_container_low; padding: 6px 8px 0 8px; border-bottom: 1px solid @outline_variant; }
.tab {
  background: transparent; border: 1px solid transparent; border-bottom: none;
  border-radius: {{R}}px {{R}}px 0 0; padding: 0 4px 0 12px; min-height: 30px;
  color: @on_surface_variant;
}
.tab:hover { background: @surface_container; color: @on_surface; }
.tab.current { background: @surface; border-color: @outline_variant; color: @on_surface; margin-bottom: -1px; }
.tab-icon { color: @outline; font-size: 1.12em; }
.tab.current .tab-icon { color: @primary; }
.tab button.tool { font-size: 0.96em; padding: 0 6px; min-height: 20px; opacity: 0.6; }
.tab button.tool:hover { opacity: 1; }
button.tool.tab-add { margin: 0 0 4px 4px; }

/* ---------- search results ---------- */
.searching { color: @primary; }

/* ---------- preview pane ---------- */
.preview-wrap { background: @surface_container_low; border-left: 1px solid @outline_variant; }
.preview { padding: 18px 16px; }
.preview-icon { font-size: 5.76em; }
.preview-name { font-weight: 700; font-size: 1.08em; }
.preview-meta { color: @outline; font-size: 0.92em; }
.preview-text {
  background: @surface_container_lowest; border: 1px solid @outline_variant;
  border-radius: {{RS}}px; padding: 8px 10px; font-size: 0.88em; color: @on_surface_variant;
  font-family: "GeistMono Nerd Font", monospace;
}
.preview-pic { border-radius: {{RS}}px; }
paned > separator { background: @outline_variant; min-width: 1px; }

/* ---------- properties / pickers ---------- */
.prop-key { color: @outline; font-size: 0.92em; }
.prop-val { color: @on_surface; }
.prop-head-icon { font-size: 3.2em; }
.app-list, .app-list > row { background: transparent; }
.app-list > row { padding: 6px 10px; border-radius: {{RS}}px; }
.app-list > row:hover { background: @surface_container_high; }
.app-list > row:selected { background: alpha(@primary, 0.18); }
checkbutton { color: @on_surface_variant; }
checkbutton check { background: @surface_container_highest; border: 1px solid @outline_variant; border-radius: 4px; min-width: 14px; min-height: 14px; }
checkbutton check:checked { background: @primary; border-color: @primary; color: @on_primary; }
switch {
  background: @surface_container_highest; border: 1px solid @outline_variant;
  border-radius: 999px; padding: 2px; min-width: 34px; min-height: 16px; outline: none;
}
switch:checked { background: @primary; border-color: @primary; }
switch > slider { background: @outline; border: none; box-shadow: none; border-radius: 999px; min-width: 16px; min-height: 16px; margin: 0; }
switch:checked > slider { background: @on_primary; }
switch > image { color: transparent; }

/* ---------- drives / this pc ---------- */
.pc-page { padding: 18px 20px; }
.drive-card {
  background: @surface_container_low; border: 1px solid @outline_variant;
  border-radius: {{R}}px; padding: 12px 14px; min-width: 250px;
}
.drive-card:hover { background: @surface_container; border-color: @outline; }
.drive-icon { font-size: 2.72em; color: @primary; min-width: 40px; }
.drive-title { font-weight: 600; }
progressbar.full > trough > progress { background: @error; }
.nav > row.unmounted { color: @outline; }
.nav button.tool.eject { font-size: 1.04em; padding: 0 6px; min-height: 18px; opacity: 0.7; }
.nav button.tool.eject:hover { opacity: 1; color: @primary; }

/* ---------- drag & drop ---------- */
.drop-cell:drop(active) { background: alpha(@primary, 0.22); border-radius: 0; box-shadow: none; }
.drop-zone:drop(active) { box-shadow: inset 0 0 0 2px alpha(@primary, 0.55); }
.drag-badge {
  background: @surface_container_high; color: @on_surface;
  border: 1px solid @primary; border-radius: {{R}}px; padding: 6px 12px;
  font-family: "Google Sans Flex", "GeistMono Nerd Font", sans-serif; font-size: {{FS}}px;
}
button.crumb:drop(active) { background: alpha(@primary, 0.25); color: @on_surface; }
*:drop(active):focus, *:drop(active) { outline: none; }
"#;
