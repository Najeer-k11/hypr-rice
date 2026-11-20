//! Window look: rounding, borders, gaps, opacity, effects. Each change is applied live
//! (`hyprctl eval`) and saved to ~/.config/hypr/settings.lua.

use crate::{hypr, theme, ui};
use gtk::prelude::*;
use serde_json::json;
use std::{cell::Cell, cell::RefCell, rc::Rc};

type Refreshers = Rc<RefCell<Vec<Box<dyn Fn()>>>>;

fn mock(active: bool) -> gtk::Box {
    let w = gtk::Box::new(gtk::Orientation::Vertical, 7);
    w.add_css_class("mock-win");
    if active {
        w.add_css_class("active");
    }
    w.set_vexpand(true);
    w.set_hexpand(true);
    for (i, width) in [36, 120, 84, 140].into_iter().enumerate() {
        let l = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        l.add_css_class("mock-line");
        if i == 0 && active {
            l.add_css_class("accent");
        }
        l.set_size_request(crate::theme::px(width), crate::theme::px(6));
        l.set_halign(gtk::Align::Start);
        w.append(&l);
    }
    w
}

/// A slider bound to one Hyprland option.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    card: &gtk::Box, refs: &Refreshers, loading: &Rc<Cell<bool>>,
    icon: &str, title: &str, sub: &str, key: &'static str,
    min: f64, max: f64, step: f64, digits: i32,
    after: impl Fn(f64) + 'static,
) {
    let int = digits == 0;
    let read = move || if int { hypr::opt_i(key, 0) as f64 } else { hypr::opt_f(key, 1.0) };
    let s = ui::slider(min, max, step, digits, read());
    let r = ui::row(icon, title, sub);
    r.suffix(&s);
    ui::add(card, &r.row);
    let after = Rc::new(after);
    let (l, a) = (loading.clone(), after.clone());
    s.connect_value_changed(move |s| {
        let v = s.value();
        a(v);
        if l.get() {
            return;
        }
        if int {
            hypr::set(key, json!(v.round() as i64));
        } else {
            hypr::set(key, json!((v * 100.0).round() / 100.0));
        }
    });
    let s2 = s.clone();
    refs.borrow_mut().push(Box::new(move || s2.set_value(read())));
}

fn switch_row(
    card: &gtk::Box, refs: &Refreshers, loading: &Rc<Cell<bool>>,
    icon: &str, title: &str, sub: &str, key: &'static str,
) {
    let sw = ui::switch(hypr::opt_b(key, false));
    let r = ui::row(icon, title, sub);
    r.suffix(&sw);
    ui::add(card, &r.row);
    let l = loading.clone();
    sw.connect_active_notify(move |sw| {
        if !l.get() {
            hypr::set(key, json!(sw.is_active()));
        }
    });
    let s2 = sw.clone();
    refs.borrow_mut().push(Box::new(move || s2.set_active(hypr::opt_b(key, false))));
}

pub fn build() -> gtk::Widget {
    let p = ui::page("\u{f1fc}", "Appearance", "Window corners, borders, gaps and effects · applied live");
    let refs: Refreshers = Rc::default();
    let loading = Rc::new(Cell::new(false));

    if let Some(b) = hypr::persist_banner() {
        p.body.append(&b);
    }

    // ---- live preview
    let gi = hypr::opt_i("general:gaps_in", 3) as i32;
    let desk = gtk::Box::new(gtk::Orientation::Horizontal, gi * 2);
    desk.add_css_class("mock-desk");
    let col = gtk::Box::new(gtk::Orientation::Vertical, gi * 2);
    col.append(&mock(false));
    col.append(&mock(false));
    desk.append(&mock(true));
    desk.append(&col);
    p.body.append(&desk);

    // ---- corners
    let c = ui::section(&p.body, "Corners");
    slider_row(&c, &refs, &loading, "󰢡", "Corner radius", "Window rounding in px · this app and the bar follow it",
        "decoration:rounding", 0.0, 24.0, 1.0, 0, |v| {
            theme::set_var(|t| t.radius = v);
            super::panel::sync_radius(v.round() as i64);
        });
    slider_row(&c, &refs, &loading, "󰘸", "Corner shape", "2 = circle · higher = squircle",
        "decoration:rounding_power", 1.0, 10.0, 0.5, 1, |_| {});

    // ---- borders & gaps
    let c = ui::section(&p.body, "Borders & gaps");
    slider_row(&c, &refs, &loading, "󰢤", "Border width", "Coloured outline around windows",
        "general:border_size", 0.0, 8.0, 1.0, 0, |v| theme::set_var(|t| t.border = v));
    let (d, cc) = (desk.clone(), col.clone());
    slider_row(&c, &refs, &loading, "󰕰", "Inner gaps", "Space between windows",
        "general:gaps_in", 0.0, 20.0, 1.0, 0, move |v| {
            d.set_spacing(v as i32 * 2);
            cc.set_spacing(v as i32 * 2);
        });
    slider_row(&c, &refs, &loading, "󰹑", "Outer gaps", "Space between windows and screen edges",
        "general:gaps_out", 0.0, 40.0, 1.0, 0, |v| theme::set_var(|t| t.gaps_out = v));

    // ---- effects
    let c = ui::section(&p.body, "Effects");
    slider_row(&c, &refs, &loading, "󰌁", "Focused opacity", "",
        "decoration:active_opacity", 0.5, 1.0, 0.01, 2, |v| theme::set_var(|t| t.active = v));
    slider_row(&c, &refs, &loading, "󰌀", "Unfocused opacity", "",
        "decoration:inactive_opacity", 0.5, 1.0, 0.01, 2, |v| theme::set_var(|t| t.inactive = v));
    switch_row(&c, &refs, &loading, "󰂵", "Blur", "Blur behind transparent windows · costs GPU",
        "decoration:blur:enabled");
    switch_row(&c, &refs, &loading, "󰘷", "Shadows", "Drop shadows under windows",
        "decoration:shadow:enabled");
    switch_row(&c, &refs, &loading, "󰃝", "Dim unfocused", "Darken windows you're not using",
        "decoration:dim_inactive");
    switch_row(&c, &refs, &loading, "󰶘", "Animations", "Open, close, workspace slide",
        "animations:enabled");

    // ---- layout
    let c = ui::section(&p.body, "Layout");
    let layouts = ["dwindle", "master"];
    let cur = hypr::opt_s("general:layout", "dwindle");
    let dd = ui::dropdown(&layouts, layouts.iter().position(|l| *l == cur).unwrap_or(0) as u32);
    let r = ui::row("󰙀", "Tiling layout", "Dwindle splits in a spiral · master keeps one big window");
    r.suffix(&dd);
    ui::add(&c, &r.row);
    let l = loading.clone();
    dd.connect_selected_notify(move |d| {
        if !l.get() {
            hypr::set("general:layout", json!(ui::dropdown_text(d)));
        }
    });
    let d2 = dd.clone();
    refs.borrow_mut().push(Box::new(move || {
        let cur = hypr::opt_s("general:layout", "dwindle");
        d2.set_selected(if cur == "master" { 1 } else { 0 });
    }));

    p.body.append(&ui::label(
        &format!("Saved to {} · loaded after {}", crate::util::tilde(&hypr::override_path()), crate::util::tilde(&hypr::main_config())),
        &["status"],
    ));

    // ---- reset: drop overrides, reload hyprland, read the values back
    let reset = ui::button("󰦛  reset to config", &["flat"]);
    reset.set_tooltip_text(Some("Forget changes made here and use hyprland.lua values"));
    let (refs2, l) = (refs.clone(), loading.clone());
    reset.connect_clicked(move |_| {
        hypr::reset_options();
        let (refs3, l2) = (refs2.clone(), l.clone());
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
            l2.set(true);
            refs3.borrow().iter().for_each(|f| f());
            l2.set(false);
        });
    });
    p.actions.append(&reset);

    p.root.upcast()
}
