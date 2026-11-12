//! Keyboard backlight brightness + RGB colour/effect.
//! RGB backends: asusctl (ASUS laptops) or OpenRGB (most other RGB keyboards).
//! "follow wallpaper" sets the keyboard to a vivid version of the matugen accent; the
//! `rice-settings --sync-keyboard` CLI does the same headlessly (call it from a wallpaper hook).

use crate::{deps, hypr, theme, ui, util};
use gtk::prelude::*;
use serde_json::json;
use std::{cell::RefCell, fs, path::PathBuf, process::Command, rc::Rc};

#[derive(Clone, Copy, PartialEq)]
enum Backend {
    Asus,
    OpenRgb,
    None,
}

fn backend() -> Backend {
    if deps::have("asusctl") {
        Backend::Asus
    } else if deps::have("openrgb") {
        Backend::OpenRgb
    } else {
        Backend::None
    }
}

fn is_asus() -> bool {
    fs::read_to_string("/sys/class/dmi/id/sys_vendor").is_ok_and(|v| v.to_uppercase().contains("ASUS"))
}

/// (effect id, label, colours used, has speed)
const ASUS_EFFECTS: &[(&str, &str, u8, bool)] = &[
    ("static", "static", 1, false),
    ("breathe", "breathe", 2, true),
    ("pulse", "pulse", 1, false),
    ("rainbow-cycle", "colour cycle", 0, true),
    ("rainbow-wave", "rainbow wave", 0, true),
];
const RGB_EFFECTS: &[(&str, &str, u8, bool)] = &[("static", "static", 1, false), ("off", "off", 0, false)];

const PRESETS: &[(&str, &str)] = &[
    ("white", "ffffff"), ("red", "ff0000"), ("orange", "ff5a00"), ("yellow", "ffc800"),
    ("green", "00ff3c"), ("cyan", "00e5ff"), ("blue", "0040ff"), ("purple", "8a2be2"), ("pink", "ff1e8c"),
];

// ------------------------------------------------------------------ state --
#[derive(Clone)]
struct Kb {
    effect: String,
    colour: String,
    colour2: String,
    speed: String,
    follow: bool,
}

/// Best effort: what asusd last applied (/etc/asusd/aura_*.ron), so the page opens on the
/// keyboard's real colour instead of a default.
fn asusd_current() -> Option<Kb> {
    let ron = fs::read_dir("/etc/asusd").ok()?.flatten().map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("aura_")))
        .and_then(|p| fs::read_to_string(p).ok())?;
    let mode = ron.split("current_mode:").nth(1)?.split(',').next()?.trim().to_string();
    let block = ron.split(&format!("{mode}: (")).nth(1)?;
    let rgb = |key: &str| -> Option<String> {
        let b = block.split(key).nth(1)?;
        let n = |c: &str| b.split(&format!("{c}:")).nth(1)?.split(',').next()?.trim().parse::<u8>().ok();
        Some(format!("{:02x}{:02x}{:02x}", n("r")?, n("g")?, n("b")?))
    };
    let effect = match mode.as_str() {
        "Static" => "static", "Breathe" => "breathe", "Pulse" => "pulse",
        "RainbowCycle" => "rainbow-cycle", "RainbowWave" => "rainbow-wave", _ => "static",
    };
    Some(Kb {
        effect: effect.into(),
        colour: rgb("colour1:").unwrap_or_else(|| "ffffff".into()),
        colour2: rgb("colour2:").unwrap_or_else(|| "000000".into()),
        speed: "med".into(),
        follow: false,
    })
}

fn load() -> Kb {
    let Some(v) = hypr::state_get("keyboard") else {
        return asusd_current().unwrap_or(Kb { effect: "static".into(), colour: "ffffff".into(), colour2: "000000".into(), speed: "med".into(), follow: false });
    };
    let s = |k: &str, d: &str| v[k].as_str().unwrap_or(d).to_string();
    Kb {
        effect: s("effect", "static"),
        colour: s("colour", "ffffff"),
        colour2: s("colour2", "000000"),
        speed: s("speed", "med"),
        follow: v["follow"].as_bool().unwrap_or(false),
    }
}

fn save(k: &Kb) {
    hypr::state_set(
        "keyboard",
        json!({ "effect": k.effect, "colour": k.colour, "colour2": k.colour2, "speed": k.speed, "follow": k.follow }),
    );
}

// ----------------------------------------------------------------- colour --
/// Pastel UI accents look washed-out on LEDs: keep the hue, push saturation and value up.
pub fn vivid(hex: &str) -> String {
    let h = hex.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255) as f64 / 255.0;
    let (r, g, b) = (p(0), p(2), p(4));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d < 0.04 {
        return "ffffff".into();
    }
    let hue = if max == r { ((g - b) / d).rem_euclid(6.0) } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 } * 60.0;
    let (s, v) = (1.0_f64, 1.0_f64);
    let c = v * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hue as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    format!("{:02x}{:02x}{:02x}", ((r1 + m) * 255.0) as u8, ((g1 + m) * 255.0) as u8, ((b1 + m) * 255.0) as u8)
}

/// Build the command for the current backend. Blocking-safe (no GTK).
fn command(k: &Kb) -> Option<Vec<String>> {
    let s = |x: &str| x.to_string();
    match backend() {
        Backend::Asus => {
            let mut a = vec![s("asusctl"), s("aura"), s("effect"), k.effect.clone()];
            match k.effect.as_str() {
                "static" | "pulse" => a.extend([s("-c"), k.colour.clone()]),
                "breathe" => a.extend([s("--colour"), k.colour.clone(), s("--colour2"), k.colour2.clone(), s("--speed"), k.speed.clone()]),
                "rainbow-cycle" | "rainbow-wave" => a.extend([s("--speed"), k.speed.clone()]),
                _ => {}
            }
            Some(a)
        }
        Backend::OpenRgb => Some(match k.effect.as_str() {
            "off" => vec![s("openrgb"), s("--noautoconnect"), s("-m"), s("static"), s("-c"), s("000000")],
            _ => vec![s("openrgb"), s("--noautoconnect"), s("-m"), s("static"), s("-c"), k.colour.clone()],
        }),
        Backend::None => None,
    }
}

/// Headless: if "follow wallpaper" is on, push the current accent to the keyboard.
/// Used by `rice-settings --sync-keyboard` and after the app sets a wallpaper itself.
pub fn sync_follow() -> bool {
    let mut k = load();
    if !k.follow {
        return false;
    }
    let primary = theme::read_palette_file()
        .into_iter()
        .find(|(n, _)| n == "primary")
        .map(|(_, c)| c)
        .unwrap_or_else(|| "#ffffff".into());
    k.colour = vivid(&primary);
    if !matches!(k.effect.as_str(), "static" | "breathe" | "pulse") {
        k.effect = "static".into();
    }
    save(&k);
    command(&k).is_some_and(|a| Command::new(&a[0]).args(&a[1..]).status().is_ok_and(|s| s.success()))
}

// --------------------------------------------------------------- backlight --
fn led_dir() -> Option<PathBuf> {
    fs::read_dir("/sys/class/leds")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains("kbd_backlight")))
}

fn read_num(p: PathBuf) -> u32 {
    fs::read_to_string(p).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn set_brightness(dir: &std::path::Path, v: u32, done: impl FnOnce(bool) + 'static) {
    if fs::write(dir.join("brightness"), v.to_string()).is_ok() {
        return done(true);
    }
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    if deps::have("brightnessctl") {
        util::run("brightnessctl", &["-d", &name, "set", &v.to_string()], move |o| done(o.ok));
    } else if deps::have("asusctl") {
        let lvl = ["off", "low", "med", "high"][v.min(3) as usize];
        util::run("asusctl", &["leds", "set", lvl], move |o| done(o.ok));
    } else {
        done(false);
    }
}

// --------------------------------------------------------------------- ui --
pub fn build() -> gtk::Widget {
    let p = ui::page("󰌌", "keyboard", "backlight and colours");

    // ---- brightness
    p.body.append(&ui::label("backlight", &["section"]));
    match led_dir() {
        None => {
            let c = ui::card();
            c.append(&ui::label("no keyboard backlight found", &["empty"]));
            p.body.append(&c);
        }
        Some(dir) => {
            let c = ui::card();
            let max = read_num(dir.join("max_brightness")).max(1);
            let cur = read_num(dir.join("brightness"));
            let r = ui::row("󰌌", "brightness", "");
            r.icon.add_css_class("on");
            let status = r.sub.clone();
            if max <= 4 {
                // a few fixed steps: segmented buttons read better than a slider
                let names: Vec<String> = (0..=max)
                    .map(|i| match (max, i) {
                        (_, 0) => "off".into(),
                        (3, 1) => "low".into(),
                        (3, 2) => "medium".into(),
                        (3, 3) => "high".into(),
                        (_, i) => i.to_string(),
                    })
                    .collect();
                let seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                seg.add_css_class("segmented");
                seg.set_valign(gtk::Align::Center);
                let mut first: Option<gtk::ToggleButton> = None;
                for (i, n) in names.iter().enumerate() {
                    let b = gtk::ToggleButton::with_label(n);
                    if let Some(f) = &first {
                        b.set_group(Some(f));
                    } else {
                        first = Some(b.clone());
                    }
                    b.set_active(i as u32 == cur);
                    let (d, st) = (dir.clone(), status.clone());
                    b.connect_toggled(move |b| {
                        if b.is_active() {
                            let st = st.clone();
                            set_brightness(&d, i as u32, move |ok| {
                                st.set_visible(!ok);
                                st.set_text("couldn't change it · install brightnessctl");
                            });
                        }
                    });
                    seg.append(&b);
                }
                r.suffix(&seg);
            } else {
                let s = ui::slider(0.0, max as f64, 1.0, 0, cur as f64);
                let d = dir.clone();
                s.connect_value_changed(move |s| set_brightness(&d, s.value() as u32, |_| {}));
                r.suffix(&s);
            }
            ui::add(&c, &r.row);
            p.body.append(&c);
            if fs::metadata(dir.join("brightness")).is_ok_and(|m| m.permissions().readonly()) || !deps::have("brightnessctl") {
                // only nag when writing sysfs directly won't work
                if fs::OpenOptions::new().write(true).open(dir.join("brightness")).is_err() {
                    if let Some(b) = deps::banner(&[&deps::BRIGHTNESSCTL], || crate::reload_page("keyboard")) {
                        p.body.append(&b);
                    }
                }
            }
        }
    }

    // ---- colour
    p.body.append(&ui::label("colour", &["section"]));
    let be = backend();
    let need: Vec<&'static deps::Dep> = match (be, is_asus()) {
        (Backend::None, true) => vec![&deps::ASUSCTL],
        (Backend::None, false) => vec![&deps::OPENRGB],
        (Backend::Asus, _) => vec![&deps::ASUSCTL], // also nudges if asusd isn't running
        _ => vec![],
    };
    if let Some(b) = deps::banner(&need, || crate::reload_page("keyboard")) {
        p.body.append(&b);
    }
    if be == Backend::None {
        return p.root.upcast();
    }
    let effects = if be == Backend::Asus { ASUS_EFFECTS } else { RGB_EFFECTS };
    let state = Rc::new(RefCell::new(load()));
    let c = ui::card();

    let follow = ui::switch(state.borrow().follow);
    let r = ui::row("󰸉", "follow wallpaper", "use the wallpaper's accent colour, updated on every change");
    r.icon.add_css_class("on");
    r.suffix(&follow);
    ui::add(&c, &r.row);

    let labels: Vec<&str> = effects.iter().map(|e| e.1).collect();
    let cur_i = effects.iter().position(|e| e.0 == state.borrow().effect).unwrap_or(0);
    let effect = ui::dropdown(&labels, cur_i as u32);
    let r = ui::row("󰌁", "effect", "");
    r.suffix(&effect);
    ui::add(&c, &r.row);

    // colour pickers: wallpaper colours + presets + hex
    let mk_picker = |title: &str, sub: &str| {
        let wrap = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrap.add_css_class("row");
        let head = ui::row("󰏘", title, sub);
        head.row.remove_css_class("row");
        let entry = gtk::Entry::new();
        entry.set_max_length(7);
        entry.set_width_chars(8);
        entry.set_placeholder_text(Some("#rrggbb"));
        entry.set_valign(gtk::Align::Center);
        head.suffix(&entry);
        wrap.append(&head.row);
        let sw = gtk::FlowBox::new();
        sw.set_selection_mode(gtk::SelectionMode::None);
        sw.set_max_children_per_line(16);
        sw.set_min_children_per_line(6);
        sw.set_column_spacing(6);
        sw.set_row_spacing(6);
        sw.set_margin_start(36);
        sw.set_halign(gtk::Align::Start);
        let mut items: Vec<(String, String)> =
            ["primary", "secondary", "tertiary"].iter().map(|r| (format!("kv-{r}"), format!("wallpaper {r}"))).collect();
        items.extend(PRESETS.iter().map(|(n, _)| (format!("kc-{n}"), n.to_string())));
        for (class, tip) in items {
            let b = gtk::Button::new();
            b.add_css_class("led");
            b.set_size_request(26, 26);
            b.set_halign(gtk::Align::Center);
            b.set_valign(gtk::Align::Center);
            b.add_css_class(&class);
            b.set_tooltip_text(Some(&tip));
            b.set_widget_name(&class);
            sw.append(&b);
        }
        wrap.append(&sw);
        (wrap, entry, sw)
    };
    let (pick1, entry1, sw1) = mk_picker("colour", "");
    let (pick2, entry2, sw2) = mk_picker("second colour", "breathe fades between the two");
    ui::add(&c, &pick1);
    ui::add(&c, &pick2);

    let speed = ui::dropdown(&["low", "med", "high"], ["low", "med", "high"].iter().position(|s| *s == state.borrow().speed).unwrap_or(1) as u32);
    let speed_row = ui::row("󰾆", "speed", "");
    speed_row.suffix(&speed);
    ui::add(&c, &speed_row.row);
    p.body.append(&c);
    let status = ui::label("", &["status"]);
    p.body.append(&status);

    // resolve a swatch class to a hex colour (wallpaper ones are made vivid)
    let class_hex = |class: &str| -> String {
        if let Some(role) = class.strip_prefix("kv-") {
            vivid(&theme::hex(role))
        } else {
            let n = class.trim_start_matches("kc-");
            PRESETS.iter().find(|(k, _)| *k == n).map(|(_, h)| h.to_string()).unwrap_or_default()
        }
    };

    // show/hide rows for the chosen effect, refresh entries
    let sync_ui = {
        let (state, pick1, pick2, sr, entry1, entry2, follow) =
            (state.clone(), pick1.clone(), pick2.clone(), speed_row.row.clone(), entry1.clone(), entry2.clone(), follow.clone());
        move || {
            let k = state.borrow();
            let e = effects.iter().find(|e| e.0 == k.effect).copied().unwrap_or(effects[0]);
            pick1.set_visible(e.2 >= 1);
            pick1.set_sensitive(!k.follow);
            pick2.set_visible(e.2 >= 2);
            sr.set_visible(e.3);
            follow.set_sensitive(e.2 >= 1);
            entry1.set_text(&format!("#{}", k.colour));
            entry2.set_text(&format!("#{}", k.colour2));
        }
    };
    let sync_ui = Rc::new(sync_ui);

    let deb = ui::Debounce::default();
    let apply = {
        let (state, status, sync_ui) = (state.clone(), status.clone(), sync_ui.clone());
        Rc::new(move || {
            {
                let mut k = state.borrow_mut();
                if k.follow {
                    k.colour = vivid(&theme::hex("primary"));
                }
                save(&k);
            }
            sync_ui();
            let k = state.borrow().clone();
            let st = status.clone();
            deb.run(200, move || {
                let Some(a) = command(&k) else { return };
                st.set_text("applying…");
                let args: Vec<&str> = a[1..].iter().map(String::as_str).collect();
                let st2 = st.clone();
                util::run(&a[0], &args, move |o| {
                    st2.set_text(&if o.ok { String::new() } else { format!("failed: {}", o.msg()) });
                });
            });
        })
    };

    let (s, a) = (state.clone(), apply.clone());
    effect.connect_selected_notify(move |d| {
        s.borrow_mut().effect = effects[d.selected() as usize].0.to_string();
        a();
    });
    let (s, a) = (state.clone(), apply.clone());
    speed.connect_selected_notify(move |d| {
        s.borrow_mut().speed = ui::dropdown_text(d);
        a();
    });
    let (s, a) = (state.clone(), apply.clone());
    follow.connect_active_notify(move |f| {
        s.borrow_mut().follow = f.is_active();
        a();
    });
    for (second, sw, entry) in [(false, sw1, entry1), (true, sw2, entry2)] {
        let mut ch = sw.first_child();
        while let Some(child) = ch {
            if let Some(btn) = child.first_child().and_downcast::<gtk::Button>() {
                let (s, a) = (state.clone(), apply.clone());
                let hex = class_hex(&btn.widget_name());
                let cls = btn.widget_name().to_string();
                btn.connect_clicked(move |_| {
                    // wallpaper swatches re-read the palette at click time
                    let h = if cls.starts_with("kv-") { vivid(&theme::hex(cls.trim_start_matches("kv-"))) } else { hex.clone() };
                    if second { s.borrow_mut().colour2 = h } else { s.borrow_mut().colour = h }
                    a();
                });
            }
            ch = child.next_sibling();
        }
        let (s, a) = (state.clone(), apply.clone());
        entry.connect_activate(move |e| {
            let t = e.text().trim().trim_start_matches('#').to_lowercase();
            if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                if second { s.borrow_mut().colour2 = t } else { s.borrow_mut().colour = t }
                a();
            }
        });
    }
    sync_ui();

    // wallpaper changed while the app is open
    let (s, a) = (state.clone(), apply.clone());
    theme::on_change(move || {
        if s.borrow().follow {
            a();
        }
    });
    p.root.upcast()
}
