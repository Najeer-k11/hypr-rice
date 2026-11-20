//! Waybar: position, size, margins, font size, and which modules show.
//! Edits ~/.config/waybar/config.jsonc and style.css in place, then reloads the bar.
//! Its corner radius follows the window rounding (see `sync_radius`).

use crate::{deps, hypr, ui, util};
use gtk::prelude::*;
use serde_json::{json, Value};
use std::{cell::RefCell, fs, rc::Rc};

const SECTIONS: &[(&str, &str)] = &[("modules-left", "left"), ("modules-center", "centre"), ("modules-right", "right")];

fn cfg_path() -> std::path::PathBuf { util::cfg("waybar/config.jsonc") }
fn css_path() -> std::path::PathBuf { util::cfg("waybar/style.css") }

fn read_cfg() -> Option<Value> {
    let t = fs::read_to_string(cfg_path()).ok()?;
    serde_json::from_str(&t).ok().or_else(|| {
        // tolerate whole-line // comments (they are dropped on save)
        let s: String = t.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        serde_json::from_str(&s).ok()
    })
}

fn module_info(m: &str) -> (&'static str, String) {
    let icon = match m {
        "hyprland/workspaces" => "\u{f009}",
        "hyprland/window" => "\u{f2d0}",
        "clock" => "\u{f017}",
        "tray" => "\u{f0c9}",
        "mpris" => "\u{f001}",
        "memory" => "󰍛",
        "cpu" => "\u{f4bc}",
        "custom/profile" => "󰾅",
        "bluetooth" => "󰂯",
        "pulseaudio" | "wireplumber" => "󰕾",
        "network" => "󰤨",
        "battery" => "󰁹",
        "custom/power" => "⏻",
        "backlight" => "󰃠",
        _ => "\u{f12e}",
    };
    let name = m.rsplit('/').next().unwrap_or(m);
    let name = match name {
        "pulseaudio" | "wireplumber" => "Volume",
        "mpris" => "Media",
        "window" => "Window title",
        "profile" => "Power profile",
        "power" => "Power button",
        n => return (icon, crate::util::sentence(n)),
    };
    (icon, name.to_string())
}

// ---- tiny CSS editor: change one property inside one selector block -------
fn css_block(css: &str, selector: &str) -> Option<(usize, usize)> {
    let start = css.find(&format!("{selector} {{"))?;
    let open = start + selector.len() + 1;
    let end = open + css[open..].find('}')?;
    Some((open, end))
}

fn css_get(css: &str, selector: &str, prop: &str) -> Option<f64> {
    let (a, b) = css_block(css, selector)?;
    let block = &css[a..b];
    let i = block.find(&format!("{prop}:"))? + prop.len() + 1;
    let v = block[i..].split(';').next()?.trim().trim_end_matches("px");
    v.parse().ok()
}

fn css_set(css: &str, selector: &str, prop: &str, val: &str) -> String {
    let Some((a, b)) = css_block(css, selector) else { return css.to_string() };
    let block = &css[a..b];
    let Some(i) = block.find(&format!("{prop}:")) else { return css.to_string() };
    let vs = a + i + prop.len() + 1;
    let Some(ve) = css[vs..b].find(';').map(|e| vs + e) else { return css.to_string() };
    format!("{} {}{}", &css[..vs], val, &css[ve..])
}

/// Pills sit `PILL_INSET` px inside the bar, so their corners use a smaller radius to stay concentric.
const PILL_INSET: i64 = 3;

fn with_radius(css: &str, radius: i64) -> String {
    let outer = format!("{radius}px");
    let inner = format!("{}px", (radius - PILL_INSET).max(0));
    let mut out = css_set(css, "*", "border-radius", &inner);
    for sel in ["window#waybar > box", "tooltip", "menu"] {
        out = css_set(&out, sel, "border-radius", &outer);
    }
    out
}

/// Keep the bar's corners on the window rounding set in the appearance page.
pub fn sync_radius(radius: i64) {
    let Ok(css) = fs::read_to_string(css_path()) else { return };
    let next = with_radius(&css, radius);
    if next != css {
        let _ = fs::write(css_path(), next);
    }
}

struct Bar {
    cfg: RefCell<Value>,
    css: RefCell<String>,
    save_cfg: ui::Debounce,
    save_css: ui::Debounce,
}

impl Bar {
    fn write_cfg(self: &Rc<Self>) {
        let me = self.clone();
        self.save_cfg.run(350, move || {
            let s = serde_json::to_string_pretty(&*me.cfg.borrow()).unwrap_or_default();
            if fs::write(cfg_path(), s + "\n").is_ok() {
                util::spawn("pkill", &["-SIGUSR2", "-x", "waybar"]);
            }
        });
    }
    fn write_css(self: &Rc<Self>) {
        let me = self.clone();
        // waybar has reload_style_on_change; it picks this up itself
        self.save_css.run(250, move || {
            let _ = fs::write(css_path(), &*me.css.borrow());
        });
    }
    fn num(&self, k: &str, def: f64) -> f64 {
        self.cfg.borrow()[k].as_f64().unwrap_or(def)
    }
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰕮", "Panel", &format!("Waybar · {}", util::tilde(&cfg_path())));

    if let Some(b) = deps::banner(&[&deps::WAYBAR], || crate::reload_page("panel")) {
        p.body.append(&b);
    }
    let Some(cfg) = read_cfg() else {
        let c = ui::card();
        c.append(&ui::label("Couldn't read waybar config.jsonc", &["empty"]));
        p.body.append(&c);
        return p.root.upcast();
    };
    let bar = Rc::new(Bar {
        cfg: RefCell::new(cfg),
        css: RefCell::new(fs::read_to_string(css_path()).unwrap_or_default()),
        save_cfg: ui::Debounce::default(),
        save_css: ui::Debounce::default(),
    });

    let toggle = ui::button("󰘊  show / hide", &["flat"]);
    toggle.connect_clicked(|_| util::spawn("pkill", &["-SIGUSR1", "-x", "waybar"]));
    let restart = ui::button("󰑐  reload", &[]);
    restart.connect_clicked(|_| util::spawn("pkill", &["-SIGUSR2", "-x", "waybar"]));
    p.actions.append(&toggle);
    p.actions.append(&restart);

    // ---- layout
    let c = ui::section(&p.body, "Bar");
    let pos = bar.cfg.borrow()["position"].as_str().unwrap_or("top").to_string();
    let dd = ui::dropdown(&["top", "bottom"], if pos == "bottom" { 1 } else { 0 });
    let r = ui::row("󰞒", "Position", "Screen edge the bar sits on");
    r.suffix(&dd);
    ui::add(&c, &r.row);

    let height = ui::slider(20.0, 48.0, 1.0, 0, bar.num("height", 30.0));
    let r = ui::row("󰡏", "Height", "");
    r.suffix(&height);
    ui::add(&c, &r.row);

    let m0 = if pos == "bottom" { bar.num("margin-bottom", 0.0) } else { bar.num("margin-top", 0.0) };
    let margin = ui::slider(0.0, 20.0, 1.0, 0, m0);
    let r = ui::row("󰹑", "Margin", "Gap between the bar and screen edges");
    r.suffix(&margin);
    ui::add(&c, &r.row);

    let font = ui::slider(9.0, 16.0, 0.5, 1, css_get(&bar.css.borrow(), "*", "font-size").unwrap_or(12.5));
    let r = ui::row("󰛖", "Font size", "");
    r.suffix(&font);
    ui::add(&c, &r.row);

    let apply_margin = {
        let bar = bar.clone();
        move |m: f64, pos: &str| {
            let mut cfg = bar.cfg.borrow_mut();
            let o = cfg.as_object_mut().unwrap();
            let m = json!(m as i64);
            o.remove("margin-top");
            o.remove("margin-bottom");
            o.insert(if pos == "bottom" { "margin-bottom" } else { "margin-top" }.into(), m.clone());
            o.insert("margin-left".into(), m.clone());
            o.insert("margin-right".into(), m);
        }
    };
    let apply_margin = Rc::new(apply_margin);

    let (b, am, mg) = (bar.clone(), apply_margin.clone(), margin.clone());
    dd.connect_selected_notify(move |d| {
        let pos = ui::dropdown_text(d);
        b.cfg.borrow_mut()["position"] = json!(pos);
        am(mg.value(), &pos);
        b.write_cfg();
    });
    let b = bar.clone();
    height.connect_value_changed(move |s| {
        b.cfg.borrow_mut()["height"] = json!(s.value() as i64);
        b.write_cfg();
    });
    let (b, am, d) = (bar.clone(), apply_margin.clone(), dd.clone());
    margin.connect_value_changed(move |s| {
        am(s.value(), &ui::dropdown_text(&d));
        b.write_cfg();
    });
    let b = bar.clone();
    font.connect_value_changed(move |s| {
        let v = format!("{}px", (s.value() * 2.0).round() / 2.0);
        // reread: the appearance page may have changed the radius since this page was built
        let cur = fs::read_to_string(css_path()).unwrap_or_else(|_| b.css.borrow().clone());
        let css = css_set(&cur, "*", "font-size", &v);
        *b.css.borrow_mut() = css;
        b.write_css();
    });

    // ---- modules: remember the full layout so hidden modules return to their spot
    let current: Vec<(String, Vec<String>)> = SECTIONS
        .iter()
        .map(|(k, _)| {
            let v = bar.cfg.borrow()[*k].as_array().cloned().unwrap_or_default();
            (k.to_string(), v.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        })
        .collect();
    let is_subseq = |small: &[String], big: &[String]| {
        let mut it = big.iter();
        small.iter().all(|s| it.any(|b| b == s))
    };
    let snap: Vec<(String, Vec<String>)> = match hypr::state_get("bar_modules") {
        Some(Value::Object(o))
            if current.iter().all(|(k, cur)| {
                let full: Vec<String> = o.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                is_subseq(cur, &full)
            }) =>
        {
            current
                .iter()
                .map(|(k, _)| {
                    (k.clone(), o[k].as_array().unwrap().iter().filter_map(|x| x.as_str().map(String::from)).collect())
                })
                .collect()
        }
        _ => {
            let v: serde_json::Map<String, Value> = current.iter().map(|(k, m)| (k.clone(), json!(m))).collect();
            hypr::state_set("bar_modules", Value::Object(v));
            current.clone()
        }
    };

    let c = ui::section(&p.body, "Modules");
    let snap = Rc::new(snap);
    for (sec, mods) in snap.iter() {
        let side = SECTIONS.iter().find(|(k, _)| k == sec).map(|(_, n)| *n).unwrap_or("");
        for m in mods {
            let (icon, name) = module_info(m);
            let on = bar.cfg.borrow()[sec.as_str()].as_array().is_some_and(|a| a.iter().any(|x| x == m));
            let sw = ui::switch(on);
            let r = ui::row(icon, &name, &format!("{side} · {m}"));
            r.suffix(&sw);
            ui::add(&c, &r.row);
            let (b, sn, sec, m) = (bar.clone(), snap.clone(), sec.clone(), m.clone());
            sw.connect_active_notify(move |sw| {
                let on = sw.is_active();
                let full = &sn.iter().find(|(k, _)| *k == sec).unwrap().1;
                let mut cfg = b.cfg.borrow_mut();
                let cur: Vec<String> = cfg[sec.as_str()].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
                let next: Vec<&String> = full.iter().filter(|x| if **x == m { on } else { cur.contains(x) }).collect();
                cfg[sec.as_str()] = json!(next);
                drop(cfg);
                b.write_cfg();
            });
        }
    }
    p.body.append(&ui::label("Changes reload the bar automatically", &["status"]));
    p.root.upcast()
}

#[cfg(test)]
mod tests {
    use super::with_radius;

    const CSS: &str = "/* c */\n* {\n  border-radius: 16px;\n  font-size: 12px;\n}\nwindow#waybar > box { border-radius: 16px; }\ntooltip { border-radius: 8px; }\nmenu { border-radius: 8px; }\nmenu menuitem:hover { color: red; }\n";

    #[test]
    fn bar_uses_radius_and_pills_stay_inside() {
        let out = with_radius(CSS, 6);
        assert!(out.contains("* {\n  border-radius: 3px;"));
        assert!(out.contains("window#waybar > box { border-radius: 6px; }"));
        assert!(out.contains("tooltip { border-radius: 6px; }"));
        assert!(out.contains("menu { border-radius: 6px; }"));
    }

    #[test]
    fn small_radius_never_goes_negative() {
        assert!(with_radius(CSS, 2).contains("* {\n  border-radius: 0px;"));
    }
}
