//! Widget helpers so every part is built from the same few pieces (from rice-settings).

use gtk::glib;
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc, time::Duration};

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A flat icon button (Nerd Font glyph) with a tooltip.
pub fn tool(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(icon);
    b.add_css_class("tool");
    b.set_tooltip_text(Some(tip));
    b.set_valign(gtk::Align::Center);
    b.set_focus_on_click(false);
    b
}

pub fn tool_toggle(icon: &str, tip: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::with_label(icon);
    b.add_css_class("tool");
    b.set_tooltip_text(Some(tip));
    b.set_valign(gtk::Align::Center);
    b.set_focus_on_click(false);
    b
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// Collapse bursts (file-monitor storms, palette rewrites) into one call.
#[derive(Clone, Default)]
pub struct Debounce(Rc<RefCell<Option<glib::SourceId>>>);

impl Debounce {
    pub fn pending(&self) -> bool {
        self.0.borrow().is_some()
    }
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

// ------------------------------------------------------------------ dialogs --
// Drawn inside the window (an overlay over everything) instead of separate dialog windows:
// cheaper, and nothing for the compositor to place.

use std::rc::Weak;

thread_local! {
    /// Open dialogs own themselves through this list until closed (closures hold weak refs).
    static OPEN: RefCell<Vec<Rc<Dialog>>> = const { RefCell::new(Vec::new()) };
}

pub struct Dialog {
    scrim: gtk::Box,
    #[allow(dead_code)]
    pub card: gtk::Box,
    pub content: gtk::Box,
    pub buttons: gtk::Box,
    host: gtk::Overlay,
    prev_focus: Option<gtk::Widget>,
    on_close: RefCell<Vec<Box<dyn FnOnce()>>>,
    /// while true, esc / clicking outside don't close it (e.g. mid-eject)
    locked: std::cell::Cell<bool>,
}

fn host_of(anchor: &impl IsA<gtk::Widget>) -> Option<(gtk::Window, gtk::Overlay)> {
    let win = anchor.as_ref().root()?.downcast::<gtk::Window>().ok()?;
    let host = win.child()?.downcast::<gtk::Overlay>().ok()?;
    Some((win, host))
}

/// Is a dialog showing in this window? (window shortcuts stay quiet while one is)
pub fn modal_open(win: &impl IsA<gtk::Widget>) -> bool {
    let Some((_, host)) = host_of(win) else { return false };
    let mut c = host.first_child();
    while let Some(w) = c {
        if w.has_css_class("scrim") {
            return true;
        }
        c = w.next_sibling();
    }
    false
}

impl Dialog {
    pub fn new(anchor: &impl IsA<gtk::Widget>, title: &str, body: &str) -> Option<Rc<Dialog>> {
        let (win, host) = host_of(anchor)?;
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("scrim");
        scrim.set_hexpand(true);
        scrim.set_vexpand(true);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("dialog");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        card.set_vexpand(true);
        card.set_width_request(400);
        card.append(&label(title, &["dialog-title"]));
        if !body.is_empty() {
            let b = label(body, &["dialog-body"]);
            b.set_wrap(true);
            b.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            b.set_max_width_chars(52);
            card.append(&b);
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.append(&content);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_top(8);
        card.append(&buttons);
        scrim.append(&card);

        let d = Rc::new(Dialog { scrim, card, content, buttons, host, prev_focus: gtk::prelude::RootExt::focus(&win), on_close: RefCell::default(), locked: std::cell::Cell::new(false) });
        let w: Weak<Dialog> = Rc::downgrade(&d);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gtk::gdk::Key::Escape {
                if let Some(d) = w.upgrade() {
                    if !d.locked.get() {
                        d.close();
                    }
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        d.scrim.add_controller(keys);
        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(&d);
        click.connect_released(move |_, _, x, y| {
            if let Some(d) = w.upgrade() {
                if !d.locked.get() && d.scrim.pick(x, y, gtk::PickFlags::DEFAULT).as_ref() == Some(d.scrim.upcast_ref()) {
                    d.close();
                }
            }
        });
        d.scrim.add_controller(click);
        d.host.add_overlay(&d.scrim);
        OPEN.with(|o| o.borrow_mut().push(d.clone()));
        Some(d)
    }

    /// Add a button; `f` returns true to close the dialog.
    pub fn button(self: &Rc<Self>, text: &str, classes: &[&str], f: impl Fn() -> bool + 'static) -> gtk::Button {
        let b = gtk::Button::with_label(text);
        for c in classes {
            b.add_css_class(c);
        }
        let w = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if f() {
                if let Some(d) = w.upgrade() {
                    d.close();
                }
            }
        });
        self.buttons.append(&b);
        b
    }

    pub fn set_locked(&self, on: bool) {
        self.locked.set(on);
    }

    /// Run `f` when the dialog closes (any way).
    pub fn connect_close(&self, f: impl FnOnce() + 'static) {
        self.on_close.borrow_mut().push(Box::new(f));
    }

    pub fn close(&self) {
        for f in self.on_close.borrow_mut().drain(..) {
            f();
        }
        if self.scrim.parent().is_some() {
            self.host.remove_overlay(&self.scrim);
            if let Some(w) = &self.prev_focus {
                w.grab_focus();
            }
        }
        // drop our owner entry (deferred: we may be inside one of our own closures)
        let me = self as *const Dialog;
        glib::idle_add_local_once(move || OPEN.with(|o| o.borrow_mut().retain(|d| !std::ptr::eq(Rc::as_ptr(d), me))));
    }
}

/// Yes / no question. `f` runs on yes.
pub fn confirm(anchor: &impl IsA<gtk::Widget>, title: &str, body: &str, ok: &str, danger: bool, f: impl Fn() + 'static) {
    let Some(d) = Dialog::new(anchor, title, body) else { return };
    let c = d.button("cancel", &[], || true);
    let b = d.button(ok, &[if danger { "destructive" } else { "suggested" }], move || {
        f();
        true
    });
    // destructive: enter must not destroy anything, so focus starts on cancel
    let _ = if danger { c.grab_focus() } else { b.grab_focus() };
}

/// Just a message.
pub fn alert(anchor: &impl IsA<gtk::Widget>, title: &str, body: &str) {
    let Some(d) = Dialog::new(anchor, title, body) else { return };
    d.button("ok", &["suggested"], || true).grab_focus();
}

/// Several choices; `f(index)` for the picked one. `default` gets focus + accent.
pub fn choose(anchor: &impl IsA<gtk::Widget>, title: &str, body: &str, options: &[&str], default: usize, f: impl Fn(usize) + 'static) {
    let Some(d) = Dialog::new(anchor, title, body) else { return };
    let f = Rc::new(f);
    for (i, o) in options.iter().enumerate() {
        let f = f.clone();
        let b = d.button(o, if i == default { &["suggested"] } else { &[] }, move || {
            f(i);
            true
        });
        if i == default {
            b.grab_focus();
        }
    }
}

/// Ask for a name. `validate` returns an error to show (dialog stays open), `f` gets the text.
pub fn prompt(
    anchor: &impl IsA<gtk::Widget>,
    title: &str,
    initial: &str,
    select_chars: i32,
    ok: &str,
    validate: impl Fn(&str) -> Option<String> + 'static,
    f: impl Fn(String) + 'static,
) {
    let Some(d) = Dialog::new(anchor, title, "") else { return };
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    let err = label("", &["dialog-error"]);
    err.set_visible(false);
    d.content.append(&entry);
    d.content.append(&err);
    let submit: Rc<dyn Fn() -> bool> = {
        let (entry, err) = (entry.clone(), err.clone());
        Rc::new(move || {
            let t = entry.text().to_string();
            match validate(&t) {
                Some(e) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    false
                }
                None => {
                    f(t);
                    true
                }
            }
        })
    };
    d.button("cancel", &[], || true);
    let s = submit.clone();
    d.button(ok, &["suggested"], move || s());
    let w = Rc::downgrade(&d);
    entry.connect_activate(move |_| {
        if submit() {
            if let Some(d) = w.upgrade() {
                d.close();
            }
        }
    });
    let e2 = err.clone();
    entry.connect_changed(move |_| e2.set_visible(false));
    entry.grab_focus();
    let n = select_chars;
    let e = entry.clone();
    glib::idle_add_local_once(move || e.select_region(0, n));
}
