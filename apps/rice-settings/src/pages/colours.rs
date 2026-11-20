//! Palette view + matugen options. Options are saved to ~/.config/rice/theme.env, which
//! `wall` reads, so they also apply to every future wallpaper change.

use crate::{deps, theme, ui, util};
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    fs,
    rc::{Rc, Weak},
};

const SCHEMES: &[&str] = &[
    "tonal-spot", "vibrant", "expressive", "fidelity", "content",
    "rainbow", "fruit-salad", "neutral", "monochrome",
];

const ROLES: &[&str] = &[
    "primary", "secondary", "tertiary", "error",
    "primary_container", "secondary_container", "tertiary_container", "outline",
    "surface", "surface_container", "surface_container_high", "on_surface",
];

pub struct Opts {
    pub mode: String,
    pub scheme: String,
    pub contrast: f64,
}

fn env_path() -> std::path::PathBuf {
    util::cfg("rice/theme.env")
}

pub fn read_opts() -> Opts {
    let mut o = Opts { mode: "dark".into(), scheme: "tonal-spot".into(), contrast: 0.0 };
    for l in fs::read_to_string(env_path()).unwrap_or_default().lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "MODE" => o.mode = v.into(),
            "SCHEME" => o.scheme = v.trim_start_matches("scheme-").into(),
            "CONTRAST" => o.contrast = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    o
}

fn write_opts(o: &Opts) {
    let p = env_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _ = fs::write(
        p,
        format!(
            "# written by rice-settings; read by `wall` when it runs matugen\nMODE={}\nSCHEME=scheme-{}\nCONTRAST={:.1}\n",
            o.mode, o.scheme, o.contrast
        ),
    );
}

/// Matugen only recolours its templates; GTK apps and the portal follow these two settings.
fn apply_system_mode(mode: &str) {
    const KEYS: &str = "org.gnome.desktop.interface";
    let light = mode == "light";
    util::spawn("gsettings", &["set", KEYS, "color-scheme", if light { "prefer-light" } else { "prefer-dark" }]);

    let Some(cur) = util::sh("gsettings", &["get", KEYS, "gtk-theme"]) else { return };
    let cur = cur.trim().trim_matches('\'');
    let base = cur.strip_suffix("-dark").unwrap_or(cur);
    let target = if light { base.to_string() } else { format!("{base}-dark") };
    // not every theme ships both variants: leave the current one alone if this one is missing
    let dirs = [util::home().join(".themes"), util::home().join(".local/share/themes"), "/usr/share/themes".into()];
    if target != cur && dirs.iter().any(|d| d.join(&target).is_dir()) {
        util::spawn("gsettings", &["set", KEYS, "gtk-theme", &target]);
    }
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰏘", "Colours", "Generated from your wallpaper by matugen · click a swatch to copy it");

    if let Some(b) = deps::banner(&[&deps::MATUGEN], || crate::reload_page("colours")) {
        p.body.append(&b);
    }

    let o = read_opts();

    // ---- dark / light
    let appearance = ui::section(&p.body, "Mode");
    let light = ui::switch(o.mode == "light");
    let r = ui::row("󰔎", "Light mode", "Light or dark surfaces across the whole desktop");
    r.suffix(&light);
    ui::add(&appearance, &r.row);

    // ---- swatches
    p.body.append(&ui::label("Palette", &["section"]));
    let card = ui::card();
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_homogeneous(true);
    grid.set_min_children_per_line(2);
    grid.set_max_children_per_line(6);
    grid.set_column_spacing(4);
    grid.set_row_spacing(4);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_margin_start(8);
    grid.set_margin_end(8);
    let mut hexes = Vec::new();
    for role in ROLES {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 5);
        v.add_css_class("swatch-card");
        let sw = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sw.add_css_class("swatch");
        sw.add_css_class(&format!("sw-{role}"));
        sw.set_size_request(crate::theme::px(96), -1);
        let name = ui::label(&util::sentence(&role.replace('_', " ")), &["swatch-name"]);
        let hex = ui::label(&theme::hex(role), &["hex"]);
        v.append(&sw);
        v.append(&name);
        v.append(&hex);
        v.set_tooltip_text(Some("Click to copy"));
        let click = gtk::GestureClick::new();
        let r = role.to_string();
        let hx = hex.clone();
        click.connect_released(move |g, _, _, _| {
            let c = theme::hex(&r);
            if let Some(w) = g.widget() {
                w.clipboard().set_text(&c);
            }
            hx.set_text("Copied");
            let hx2 = hx.clone();
            let r2 = r.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
                hx2.set_text(&theme::hex(&r2));
            });
        });
        v.add_controller(click);
        grid.append(&v);
        hexes.push((role.to_string(), hex));
    }
    card.append(&grid);
    p.body.append(&card);
    theme::on_change(move || {
        for (r, l) in &hexes {
            l.set_text(&theme::hex(r));
        }
    });

    // ---- generation options
    let opts = ui::section(&p.body, "Generation");

    let scheme_i = SCHEMES.iter().position(|s| *s == o.scheme).unwrap_or(0) as u32;
    let scheme = ui::dropdown(SCHEMES, scheme_i);
    let r = ui::row("󰸌", "Scheme", "How colours are picked from the image");
    r.suffix(&scheme);
    ui::add(&opts, &r.row);

    let contrast = ui::slider(-1.0, 1.0, 0.1, 1, o.contrast);
    let r = ui::row("󰆕", "Contrast", "−1 soft · 0 standard · 1 maximum");
    r.suffix(&contrast);
    ui::add(&opts, &r.row);

    let status = ui::label("Changes apply to the current wallpaper now and every future one", &["status"]);
    p.body.append(&status);

    let regen = ui::button("󰑐  regenerate", &[]);
    p.actions.append(&regen);

    // apply = save options + rerun matugen on the current wallpaper
    let busy = Rc::new(Cell::new(false));
    // a change made while matugen is running reruns it afterwards, so the last one always wins
    let pending = Rc::new(Cell::new(false));
    let rerun: Rc<RefCell<Weak<dyn Fn()>>> = Rc::new(RefCell::new(Weak::<fn()>::new()));
    let apply: Rc<dyn Fn()> = {
        let (scheme, light, contrast, status, busy, pending, rerun) =
            (scheme.clone(), light.clone(), contrast.clone(), status.clone(), busy.clone(), pending.clone(), rerun.clone());
        Rc::new(move || {
            let o = Opts {
                mode: if light.is_active() { "light" } else { "dark" }.into(),
                scheme: ui::dropdown_text(&scheme),
                contrast: (contrast.value() * 10.0).round() / 10.0,
            };
            write_opts(&o);
            let wall = super::wallpaper::current();
            if wall.is_empty() {
                apply_system_mode(&o.mode);
                return;
            }
            if busy.get() {
                pending.set(true);
                return;
            }
            busy.set(true);
            status.set_text("Generating palette…");
            let c = format!("{:.1}", o.contrast);
            let s = format!("scheme-{}", o.scheme);
            let (st, b, again, re, mode) = (status.clone(), busy.clone(), pending.clone(), rerun.clone(), o.mode.clone());
            util::run(
                "matugen",
                &["image", &wall, "-m", &o.mode, "-t", &s, "--contrast", &c, "--source-color-index", "0", "--json", "hex"],
                move |out| {
                    b.set(false);
                    if again.replace(false) {
                        if let Some(f) = re.borrow().upgrade() {
                            f();
                            return;
                        }
                    }
                    if out.ok {
                        if !deps::have("wall") {
                            theme::write_own_palette(&out.stdout);
                        }
                        apply_system_mode(&mode);
                        st.set_text("Palette applied · saved for future wallpapers");
                        util::spawn("rice-sddm-sync", &[]);
                    } else {
                        st.set_text(&format!("Matugen failed: {}", out.msg()));
                    }
                },
            );
        })
    };
    *rerun.borrow_mut() = Rc::downgrade(&apply);
    let a = apply.clone();
    scheme.connect_selected_notify(move |_| a());
    let a = apply.clone();
    light.connect_active_notify(move |_| a());
    let deb = ui::Debounce::default();
    let a = apply.clone();
    contrast.connect_value_changed(move |_| {
        let a = a.clone();
        deb.run(450, move || a());
    });
    let a = apply.clone();
    regen.connect_clicked(move |_| a());

    p.root.upcast()
}
