//! A file manager window: sidebar + tabs (each tab a `Browser`), and the Explorer keyboard
//! shortcuts. Opening `slap-files <path>` while a window exists adds a tab to it.

use crate::browser::{self, Browser};
use crate::sidebar::{self, Sidebar};
use crate::{driveui, drives, ops, state, theme, ui, util};
use gtk::prelude::*;
use gtk::glib::translate::IntoGlib;
use gtk::{gdk, glib, pango};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

pub struct Win {
    pub win: gtk::ApplicationWindow,
    tabs: RefCell<Vec<Rc<Browser>>>,
    cur: Cell<usize>,
    stack: gtk::Stack,
    tabbar: gtk::Box,
    tabstrip: gtk::Box,
    side: RefCell<Option<Rc<Sidebar>>>,
    closed: RefCell<Vec<PathBuf>>,
}

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<Win>>> = const { RefCell::new(Vec::new()) };
}

/// Safely eject a drive, first moving every tab (in every window) off it.
pub fn eject(anchor: &gtk::Widget, drive: &str, name: &str) {
    let leave: Rc<dyn Fn(&[PathBuf])> = Rc::new(|mounts: &[PathBuf]| {
        WINDOWS.with(|ws| {
            for w in ws.borrow().iter() {
                for b in w.tabs.borrow().iter() {
                    if mounts.iter().any(|m| b.cwd().starts_with(m)) {
                        b.go(PathBuf::from(browser::PC));
                    }
                }
            }
        });
    });
    driveui::eject(anchor, drive, name, leave);
}

/// Open `target` in a new tab of the existing window, or in a new window.
pub fn open(app: &gtk::Application, target: Option<PathBuf>) {
    let existing = WINDOWS.with(|w| w.borrow().last().cloned());
    let w = match existing {
        Some(w) => w,
        None => Win::new(app),
    };
    let (dir, file) = match target {
        Some(p) if p.is_dir() => (p, None),
        Some(p) if p.exists() => (p.parent().map(PathBuf::from).unwrap_or_else(util::home), Some(p)),
        _ => (util::home(), None),
    };
    let tab = w.new_tab(dir, true);
    if let Some(f) = file {
        tab.reveal(&f);
    }
    w.win.present();
    tab.focus_view();
}

/// Open `p`'s folder with `p` selected (optionally showing its properties).
pub fn reveal(app: &gtk::Application, p: &std::path::Path, props: bool) {
    let Some(parent) = p.parent() else { return open(app, Some(p.to_path_buf())) };
    open(app, Some(parent.to_path_buf()));
    let Some(w) = WINDOWS.with(|w| w.borrow().last().cloned()) else { return };
    if let Some(b) = w.current() {
        b.reveal(p);
        if props {
            let b2 = b.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || b2.properties());
        }
    }
}

impl Win {
    fn new(app: &gtk::Application) -> Rc<Win> {
        theme::init();
        drives::init();
        let win = gtk::ApplicationWindow::builder()
            .application(app)
            .title("files")
            .default_width(1120)
            .default_height(700)
            .decorated(false) // Hyprland draws the border; no CSD title bar
            .build();
        win.add_css_class("slap");

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        let tabstrip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let tabbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabbar.add_css_class("tabbar");
        let strip_scroll = gtk::ScrolledWindow::new();
        strip_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        strip_scroll.set_child(Some(&tabstrip));
        strip_scroll.set_propagate_natural_width(true);
        tabbar.append(&strip_scroll);
        tabbar.set_visible(false);

        let me = Rc::new(Win {
            win: win.clone(),
            tabs: RefCell::default(),
            cur: Cell::new(0),
            stack: stack.clone(),
            tabbar: tabbar.clone(),
            tabstrip,
            side: RefCell::default(),
            closed: RefCell::default(),
        });

        let add = ui::tool("\u{f0415}", "new tab (ctrl+t)");
        add.add_css_class("tab-add");
        let w = Rc::downgrade(&me);
        add.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                let p = w.current().map(|b| b.cwd()).unwrap_or_else(util::home);
                w.new_tab(p, true);
            }
        });
        tabbar.append(&add);

        // sidebar talks to whichever tab is current
        let (w1, w2) = (Rc::downgrade(&me), Rc::downgrade(&me));
        let side = Sidebar::new(sidebar::Hooks {
            go: Box::new(move |p, new_tab| {
                let Some(w) = w1.upgrade() else { return };
                // an unmounted volume: mount (or unlock) first
                if let Some(obj) = p.to_str().and_then(|s| s.strip_prefix(sidebar::VOL_PREFIX)) {
                    let w2 = Rc::downgrade(&w);
                    driveui::open_volume(w.win.upcast_ref(), obj, move |m| {
                        let Some(w) = w2.upgrade() else { return };
                        if new_tab {
                            w.new_tab(m, false);
                        } else if let Some(b) = w.current() {
                            b.go(m);
                        }
                    });
                    return;
                }
                if new_tab {
                    w.new_tab(p, false);
                } else if let Some(b) = w.current() {
                    b.go(p);
                }
            }),
            eject: Box::new(|anchor, drive, name| eject(anchor, &drive, &name)),
            attach_drop: Box::new(move |widget, target| {
                let w = w2.clone();
                browser::attach_path_drop(widget, target, move || w.upgrade().and_then(|w| w.current()));
            }),
        });

        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.append(&tabbar);
        right.append(&stack);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&side.root);
        root.append(&right);
        // the overlay hosts in-window dialogs (ui::Dialog)
        let host = gtk::Overlay::new();
        host.set_child(Some(&root));
        win.set_child(Some(&host));
        *me.side.borrow_mut() = Some(side);

        me.keys();
        // mouse back / forward buttons
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let w = Rc::downgrade(&me);
        click.connect_pressed(move |g, _, _, _| {
            let Some(b) = w.upgrade().and_then(|w| w.current()) else { return };
            match g.current_button() {
                8 => b.go_back(),
                9 => b.go_forward(),
                _ => {}
            }
        });
        win.add_controller(click);

        let w = Rc::downgrade(&me);
        win.connect_close_request(move |win| {
            // never silently cut a copy short: ask first
            let n = ops::active_count();
            if n > 0 {
                let win2 = win.clone();
                ui::choose(
                    win,
                    &format!("{} still running", if n == 1 { "a file operation is".to_string() } else { format!("{n} file operations are") }),
                    "closing now cancels them (partly written files are removed). keep the window open to let them finish.",
                    &["cancel them and close", "keep open"],
                    1,
                    move |i| {
                        if i == 0 {
                            ops::cancel_all();
                            win2.destroy();
                        }
                    },
                );
                return glib::Propagation::Stop;
            }
            if let Some(w) = w.upgrade() {
                w.tabs.borrow_mut().clear();
                WINDOWS.with(|all| all.borrow_mut().retain(|x| !Rc::ptr_eq(x, &w)));
            }
            glib::Propagation::Proceed
        });
        WINDOWS.with(|all| all.borrow_mut().push(me.clone()));
        me
    }

    pub fn current(&self) -> Option<Rc<Browser>> {
        self.tabs.borrow().get(self.cur.get()).cloned()
    }

    /// Add a tab at `p` (right after the current one, like browsers do).
    pub fn new_tab(self: &Rc<Self>, p: PathBuf, switch: bool) -> Rc<Browser> {
        let b = Browser::new();
        let wb: Weak<Browser> = Rc::downgrade(&b);
        let w = Rc::downgrade(self);
        b.connect_navigated(move |p| {
            let (Some(w), Some(b)) = (w.upgrade(), wb.upgrade()) else { return };
            w.render_tabs();
            if w.current().is_some_and(|c| Rc::ptr_eq(&c, &b)) {
                w.sync_chrome(p);
            }
        });
        let w = Rc::downgrade(self);
        b.connect_open_tab(move |p| {
            if let Some(w) = w.upgrade() {
                w.new_tab(p, false);
            }
        });
        self.stack.add_child(&b.root);
        let at = if self.tabs.borrow().is_empty() { 0 } else { self.cur.get() + 1 };
        self.tabs.borrow_mut().insert(at, b.clone());
        b.go(p);
        if switch || self.tabs.borrow().len() == 1 {
            self.switch_to(at);
        }
        self.render_tabs();
        b
    }

    pub fn switch_to(self: &Rc<Self>, i: usize) {
        let Some(b) = self.tabs.borrow().get(i).cloned() else { return };
        self.cur.set(i);
        self.stack.set_visible_child(&b.root);
        // keep the per-window preview setting in step across tabs
        b.set_preview(state::get_bool("preview", false));
        self.sync_chrome(&b.cwd());
        self.render_tabs();
        b.focus_view();
    }

    pub fn close_tab(self: &Rc<Self>, i: usize) {
        let n = self.tabs.borrow().len();
        if n <= 1 {
            self.win.close();
            return;
        }
        let b = self.tabs.borrow_mut().remove(i);
        self.closed.borrow_mut().push(b.cwd());
        self.stack.remove(&b.root);
        drop(b); // the browser and all its widgets go away here
        // hand the freed heap back to the OS once GTK has finished disposing
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), util::trim_heap);
        let cur = self.cur.get();
        let next = if i < cur { cur - 1 } else if i == cur { i.min(n - 2) } else { cur };
        self.switch_to(next);
    }

    fn reopen_closed(self: &Rc<Self>) {
        if let Some(p) = self.closed.borrow_mut().pop() {
            self.new_tab(p, true);
        }
    }

    /// Window title + sidebar highlight follow the current tab.
    fn sync_chrome(&self, p: &std::path::Path) {
        self.win.set_title(Some(&format!("{} — files", browser::display_name(p))));
        if let Some(s) = self.side.borrow().as_ref() {
            s.highlight(p);
        }
    }

    /// Redraw the tab strip (only shown with two or more tabs).
    fn render_tabs(self: &Rc<Self>) {
        ui::clear(&self.tabstrip);
        let tabs = self.tabs.borrow().clone();
        self.tabbar.set_visible(tabs.len() > 1);
        for (i, b) in tabs.iter().enumerate() {
            let tab = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            tab.add_css_class("tab");
            if i == self.cur.get() {
                tab.add_css_class("current");
            }
            let cwd = b.cwd();
            tab.set_tooltip_text(Some(&util::tilde(&cwd)));
            tab.append(&ui::label(if b.is_searching() { "\u{f0968}" } else { "\u{f024b}" }, &["tab-icon"]));
            let l = ui::label(&browser::display_name(&cwd), &[]);
            l.set_ellipsize(pango::EllipsizeMode::End);
            l.set_max_width_chars(18);
            tab.append(&l);
            let x = ui::tool("\u{f0156}", "close tab (ctrl+w)");
            let w = Rc::downgrade(self);
            x.connect_clicked(move |_| {
                if let Some(w) = w.upgrade() {
                    w.close_tab(i);
                }
            });
            tab.append(&x);
            // left: switch, middle: close
            let g = gtk::GestureClick::new();
            g.set_button(0);
            let w = Rc::downgrade(self);
            g.connect_released(move |g, _, _, _| {
                let Some(w) = w.upgrade() else { return };
                match g.current_button() {
                    1 => w.switch_to(i),
                    2 => w.close_tab(i),
                    _ => {}
                }
            });
            tab.add_controller(g);
            self.tabstrip.append(&tab);
        }
    }

    fn keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            use gdk::Key as K;
            let Some(win) = w.upgrade() else { return glib::Propagation::Proceed };
            if ui::modal_open(&win.win) {
                return glib::Propagation::Proceed;
            }
            let Some(b) = win.current() else { return glib::Propagation::Proceed };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = mods.contains(gdk::ModifierType::ALT_MASK);
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
            let n = win.tabs.borrow().len();
            let stop = glib::Propagation::Stop;
            match (ctrl, alt, key) {
                // tabs
                (true, false, K::t) => {
                    win.new_tab(b.cwd(), true);
                }
                (true, false, K::T) => win.reopen_closed(),
                (true, false, K::w) => win.close_tab(win.cur.get()),
                (true, false, K::q) => win.win.close(),
                (true, false, K::Tab) | (true, false, K::Page_Down) => win.switch_to((win.cur.get() + 1) % n),
                (true, false, K::ISO_Left_Tab) | (true, false, K::Page_Up) => win.switch_to((win.cur.get() + n - 1) % n),
                (false, true, k) if (K::_1.into_glib()..=K::_9.into_glib()).contains(&k.into_glib()) => {
                    let i = (k.into_glib() - K::_1.into_glib()) as usize;
                    win.switch_to(if k == K::_9 { n - 1 } else { i });
                }
                // navigation
                (false, true, K::Left) => b.go_back(),
                (false, true, K::Right) => b.go_forward(),
                (false, true, K::Up) | (false, false, K::BackSpace) => b.go_up(),
                (false, true, K::Home) => b.go(util::home()),
                (true, false, K::l) | (false, false, K::F4) => b.edit_address(),
                (true, false, K::f) | (false, false, K::F3) => b.focus_search(None),
                (true, false, K::h) => b.toggle_hidden(),
                (true, false, K::r) | (false, false, K::F5) => b.refresh(),
                (true, false, K::_1) => b.set_view("list"),
                (true, false, K::_2) => b.set_view("grid"),
                (false, true, K::p) => b.toggle_preview(),
                (false, true, K::Return) | (false, true, K::KP_Enter) => b.properties(),
                // files
                (true, false, K::c) if !shift => b.copy(),
                (true, false, K::C) | (true, false, K::c) => b.copy_path(),
                (true, false, K::x) => b.cut(),
                (true, false, K::v) => b.paste(),
                (true, false, K::z) => b.undo(),
                (true, false, K::a) => b.select_all(),
                (true, false, K::N) | (true, false, K::n) if shift => b.new_item(true),
                (false, false, K::F2) => b.rename(),
                (false, false, K::Delete) | (false, false, K::KP_Delete) if shift => b.delete(),
                (false, false, K::Delete) | (false, false, K::KP_Delete) => b.trash(),
                (false, false, K::Menu) | (false, false, K::F10) if key == K::Menu || shift => b.context_menu_kbd(),
                (false, false, K::Return) | (false, false, K::KP_Enter) if !b.selected().is_empty() => b.open_selection(),
                (false, false, K::Escape) => {
                    if !b.escape() {
                        return glib::Propagation::Proceed;
                    }
                }
                _ => {
                    // type to filter, like Explorer's type-ahead
                    if !ctrl && !alt {
                        if let Some(c) = key.to_unicode().filter(|c| !c.is_control() && (!c.is_whitespace() || shift)) {
                            b.focus_search(Some(c));
                            return stop;
                        }
                    }
                    return glib::Propagation::Proceed;
                }
            }
            stop
        });
        self.win.add_controller(keys);
    }
}
