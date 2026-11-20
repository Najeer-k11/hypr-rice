//! Widget helpers so every page is built from the same few pieces.

use gtk::prelude::*;
use gtk::{glib, pango};
use std::{cell::RefCell, rc::Rc, time::Duration};

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

pub struct Page {
    pub root: gtk::ScrolledWindow,
    pub body: gtk::Box,
    pub actions: gtk::Box,
    pub sub: gtk::Label,
}

pub fn page(icon: &str, title: &str, sub: &str) -> Page {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(crate::theme::px(22));
    body.set_margin_bottom(crate::theme::px(28));
    body.set_margin_start(crate::theme::px(28));
    body.set_margin_end(crate::theme::px(28));

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.set_margin_bottom(10);
    let ic = label(icon, &["page-icon"]);
    ic.set_xalign(0.5);
    let tb = gtk::Box::new(gtk::Orientation::Vertical, 2);
    tb.set_hexpand(true);
    tb.set_valign(gtk::Align::Center);
    tb.append(&label(title, &["page-title"]));
    let subl = label(sub, &["page-sub"]);
    subl.set_wrap(true);
    tb.append(&subl);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_valign(gtk::Align::Center);
    head.append(&ic);
    head.append(&tb);
    head.append(&actions);
    body.append(&head);

    let root = gtk::ScrolledWindow::new();
    root.set_hscrollbar_policy(gtk::PolicyType::Never);
    root.set_vexpand(true);
    root.set_hexpand(true);
    root.set_child(Some(&body));
    Page { root, body, actions, sub: subl }
}

pub fn card() -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
    c.add_css_class("card");
    c
}

/// Section title + an empty card appended to `body`.
pub fn section(body: &gtk::Box, title: &str) -> gtk::Box {
    body.append(&label(title, &["section"]));
    let c = card();
    body.append(&c);
    c
}

/// Append to a card, with a hairline between rows.
pub fn add(card: &gtk::Box, w: &impl IsA<gtk::Widget>) {
    if card.first_child().is_some() {
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }
    card.append(w);
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

pub struct Row {
    pub row: gtk::Box,
    pub icon: gtk::Label,
    pub sub: gtk::Label,
}

impl Row {
    pub fn set_sub(&self, s: &str) {
        self.sub.set_text(s);
        self.sub.set_visible(!s.is_empty());
    }
    pub fn suffix(&self, w: &impl IsA<gtk::Widget>) -> &Self {
        self.row.append(w);
        self
    }
}

pub fn row(icon: &str, title: &str, sub: &str) -> Row {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    r.add_css_class("row");
    let ic = label(icon, &["row-icon"]);
    ic.set_xalign(0.5);
    let tb = gtk::Box::new(gtk::Orientation::Vertical, 1);
    tb.set_hexpand(true);
    tb.set_valign(gtk::Align::Center);
    let t = label(title, &["row-title"]);
    t.set_ellipsize(pango::EllipsizeMode::End);
    let s = label(sub, &["row-sub"]);
    s.set_visible(!sub.is_empty());
    s.set_ellipsize(pango::EllipsizeMode::End);
    tb.append(&t);
    tb.append(&s);
    r.append(&ic);
    r.append(&tb);
    Row { row: r, icon: ic, sub: s }
}

/// A button. "<glyph>  label" gets a real icon + label pair (the nerd glyph on the mono font,
/// a proper gap, sentence-cased text) instead of one string in a proportional font.
pub fn button(text: &str, classes: &[&str]) -> gtk::Button {
    let split = text
        .split_once(' ')
        .filter(|(g, rest)| !g.is_empty() && !g.is_ascii() && !rest.trim().is_empty());
    let b = match split {
        Some((glyph, rest)) => {
            let b = gtk::Button::new();
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, crate::theme::px(6));
            bx.append(&label(glyph, &["btn-icon"]));
            bx.append(&label(&crate::util::sentence(rest.trim()), &[]));
            b.set_child(Some(&bx));
            b
        }
        None => gtk::Button::with_label(text),
    };
    for c in classes {
        b.add_css_class(c);
    }
    b.set_valign(gtk::Align::Center);
    b
}

pub fn slider(min: f64, max: f64, step: f64, digits: i32, value: f64) -> gtk::Scale {
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    s.set_digits(digits);
    s.set_round_digits(digits);
    s.set_value(value);
    s.set_draw_value(true);
    s.set_value_pos(gtk::PositionType::Right);
    s.set_size_request(crate::theme::px(240), -1);
    s.set_valign(gtk::Align::Center);
    // The mouse wheel scrolls the page, never the slider: scrolling past a slider must
    // not silently change a setting.
    let sc = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    sc.set_propagation_phase(gtk::PropagationPhase::Capture);
    sc.connect_scroll(|c, _dx, dy| {
        let sw = c
            .widget()
            .and_then(|w| w.ancestor(gtk::ScrolledWindow::static_type()))
            .and_downcast::<gtk::ScrolledWindow>();
        if let Some(sw) = sw {
            let adj = sw.vadjustment();
            adj.set_value(adj.value() + dy * 48.0);
        }
        glib::Propagation::Stop
    });
    s.add_controller(sc);
    s
}

pub fn switch(active: bool) -> gtk::Switch {
    let s = gtk::Switch::new();
    s.set_active(active);
    s.set_valign(gtk::Align::Center);
    s
}

pub fn dropdown(items: &[&str], selected: u32) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_selected(selected);
    d.set_valign(gtk::Align::Center);
    d
}

pub fn dropdown_text(d: &gtk::DropDown) -> String {
    d.selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|s| s.string().to_string())
        .unwrap_or_default()
}

/// Re-run `f` every `secs` while `w` is on screen; stops once `w` is gone.
pub fn every_while_mapped(w: &impl IsA<gtk::Widget>, secs: u32, f: impl Fn() + 'static) {
    let weak = w.as_ref().downgrade();
    glib::timeout_add_seconds_local(secs, move || match weak.upgrade() {
        Some(w) => {
            if w.is_mapped() {
                f();
            }
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

/// Collapse bursts (slider drags) into one call.
#[derive(Clone, Default)]
pub struct Debounce(Rc<RefCell<Option<glib::SourceId>>>);

impl Debounce {
    pub fn run(&self, ms: u64, f: impl FnOnce() + 'static) {
        if let Some(id) = self.0.borrow_mut().take() {
            id.remove();
        }
        let me = self.0.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            me.borrow_mut().take();
            f();
        });
        *self.0.borrow_mut() = Some(id);
    }
}
