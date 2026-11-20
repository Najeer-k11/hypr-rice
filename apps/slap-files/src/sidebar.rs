//! Navigation pane: quick access (home, XDG folders, your pins) and this pc.
//! Pins are shared by every window and saved in files.json. Rows accept dropped files.
//! "this pc" lists drives (mount on click, eject button for external ones) and the trash.

use crate::{browser, drives, state, trash, ui, util};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

struct Place {
    icon: &'static str,
    title: String,
    path: PathBuf,
    pinned: bool,
    /// (drive object, name) when this place can be ejected
    eject: Option<(String, String)>,
    dim: bool,
}

impl Place {
    fn new(icon: &'static str, title: impl Into<String>, path: PathBuf) -> Place {
        Place { icon, title: title.into(), path, pinned: false, eject: None, dim: false }
    }
}

/// Sidebar address of a volume that isn't mounted yet (the window mounts it on click).
pub const VOL_PREFIX: &str = "slap:vol:";

thread_local! {
    static ALL: RefCell<Vec<Weak<Sidebar>>> = const { RefCell::new(Vec::new()) };
}

pub fn pins() -> Vec<PathBuf> {
    state::get_list("pins").into_iter().map(PathBuf::from).collect()
}

pub fn is_pinned(p: &Path) -> bool {
    pins().iter().any(|x| x == p)
}

/// Pin or unpin a folder in quick access (all windows update).
pub fn toggle_pin(p: &Path) {
    let mut v = pins();
    if let Some(i) = v.iter().position(|x| x == p) {
        v.remove(i);
    } else {
        v.push(p.to_path_buf());
    }
    state::set("pins", v.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>());
    ALL.with(|a| {
        a.borrow_mut().retain(|w| w.strong_count() > 0);
        for s in a.borrow().iter().filter_map(Weak::upgrade) {
            s.rebuild();
        }
    });
}

fn quick_access() -> Vec<Place> {
    use glib::UserDirectory as U;
    let home = util::home();
    let mut v = vec![Place::new("\u{f02dc}", "home", home.clone())];
    let dirs = [
        (U::Desktop, "\u{f0379}", "Desktop"),
        (U::Documents, "\u{f0219}", "Documents"),
        (U::Downloads, "\u{f01da}", "Downloads"),
        (U::Pictures, "\u{f02e9}", "Pictures"),
        (U::Music, "\u{f075a}", "Music"),
        (U::Videos, "\u{f0567}", "Videos"),
    ];
    for (d, icon, default) in dirs {
        // no user-dirs.dirs -> glib returns nothing (or $HOME); fall back to the usual names
        let p = glib::user_special_dir(d).filter(|p| *p != home).unwrap_or_else(|| home.join(default));
        if p != home && p.is_dir() && !v.iter().any(|x| x.path == p) {
            // the folder's real name, case and all (it may be localised or renamed)
            let title = browser::display_name(&p);
            v.push(Place::new(icon, title, p));
        }
    }
    for p in pins() {
        if let Some(x) = v.iter_mut().find(|x| x.path == p) {
            x.pinned = true;
        } else {
            let title = browser::display_name(&p);
            let mut pl = Place::new("\u{f0403}", title, p);
            pl.pinned = true;
            v.push(pl);
        }
    }
    let full = trash::count() > 0;
    v.push(Place::new(if full { "\u{f01b4}" } else { "\u{f0a7a}" }, "trash", PathBuf::from(browser::TRASH)));
    v
}

fn this_pc() -> Vec<Place> {
    let mut v = vec![Place::new("\u{f0322}", "this pc", PathBuf::from(browser::PC)), Place::new("\u{f02ca}", "file system", PathBuf::from("/"))];
    for vol in drives::volumes() {
        let path = vol.mount_point().cloned().unwrap_or_else(|| PathBuf::from(format!("{VOL_PREFIX}{}", vol.obj)));
        let mut p = Place::new(vol.icon(), vol.title(), path);
        p.dim = vol.mount_point().is_none();
        if vol.is_external() {
            p.eject = Some((vol.drive.clone(), vol.title()));
        }
        v.push(p);
    }
    v
}

/// What the sidebar asks of its window.
pub struct Hooks {
    /// open a folder (bool = in a new tab)
    pub go: Box<dyn Fn(PathBuf, bool)>,
    /// files dropped on a place: (widget, target folder) -> attach a drop target
    pub attach_drop: Box<dyn Fn(&gtk::Widget, PathBuf)>,
    /// safely eject (anchor, drive object, name)
    pub eject: Box<dyn Fn(&gtk::Widget, String, String)>,
}

pub struct Sidebar {
    pub root: gtk::Box,
    nav: gtk::ListBox,
    hooks: Hooks,
    current: RefCell<PathBuf>,
    trash_mon: RefCell<Option<gio::FileMonitor>>,
}

impl Sidebar {
    pub fn new(hooks: Hooks) -> Rc<Sidebar> {
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.add_css_class("sidebar");
        side.set_size_request(180, -1);

        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        brand.add_css_class("brand");
        brand.append(&ui::label("\u{f024b}", &["brand-icon"]));
        brand.append(&ui::label("slap files", &["brand-title"]));
        side.append(&brand);

        let nav = gtk::ListBox::new();
        nav.add_css_class("nav");
        nav.set_selection_mode(gtk::SelectionMode::Single);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&nav));
        side.append(&scroll);
        side.append(&ui::label("ctrl+t  new tab", &["sidebar-foot"]));

        let me = Rc::new(Sidebar { root: side, nav, hooks, current: RefCell::default(), trash_mon: RefCell::default() });
        let w = Rc::downgrade(&me);
        me.nav.connect_row_activated(move |_, row| {
            if let Some(s) = w.upgrade() {
                (s.hooks.go)(PathBuf::from(row.widget_name().as_str()), false);
            }
        });
        me.rebuild();
        ALL.with(|a| a.borrow_mut().push(Rc::downgrade(&me)));
        // drives come and go; the trash fills and empties
        let w = Rc::downgrade(&me);
        drives::on_change(move || {
            if let Some(s) = w.upgrade() {
                s.rebuild();
            }
        });
        let mon = gio::File::for_path(trash::home_trash().join("info"))
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            .ok();
        if let Some(m) = &mon {
            let w = Rc::downgrade(&me);
            let deb = ui::Debounce::default();
            m.connect_changed(move |_, _, _, _| {
                let w = w.clone();
                deb.run(400, move || {
                    if let Some(s) = w.upgrade() {
                        s.rebuild();
                    }
                });
            });
        }
        *me.trash_mon.borrow_mut() = mon;
        me
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(c) = self.nav.first_child() {
            self.nav.remove(&c);
        }
        for (group, places) in [("quick access", quick_access()), ("this pc", this_pc())] {
            let head = gtk::ListBoxRow::new();
            head.add_css_class("nav-head");
            head.set_selectable(false);
            head.set_activatable(false);
            head.set_focusable(false);
            head.set_child(Some(&ui::label(group, &[])));
            self.nav.append(&head);
            for p in places {
                self.nav.append(&self.row(p));
            }
        }
        let cur = self.current.borrow().clone();
        self.highlight(&cur);
    }

    fn row(self: &Rc<Self>, p: Place) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&p.path.to_string_lossy());
        row.set_tooltip_text(Some(&util::tilde(&p.path)));
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let ic = ui::label(p.icon, &["nav-icon"]);
        ic.set_xalign(0.5);
        b.append(&ic);
        let t = ui::label(&p.title, &[]);
        t.set_ellipsize(gtk::pango::EllipsizeMode::End);
        t.set_hexpand(true);
        b.append(&t);
        if p.dim {
            row.add_css_class("unmounted");
        }
        if let Some((drive, name)) = p.eject.clone() {
            let e = ui::tool("\u{f01ea}", "safely eject");
            e.add_css_class("eject");
            let w = Rc::downgrade(self);
            e.connect_clicked(move |e| {
                if let Some(s) = w.upgrade() {
                    (s.hooks.eject)(e.upcast_ref(), drive.clone(), name.clone());
                }
            });
            b.append(&e);
        }
        row.set_child(Some(&b));

        // middle-click: new tab; right-click: menu
        let g = gtk::GestureClick::new();
        g.set_button(0);
        let w = Rc::downgrade(self);
        let (path, pinned, r) = (p.path.clone(), p.pinned, row.clone());
        g.connect_pressed(move |g, _, x, y| {
            let Some(s) = w.upgrade() else { return };
            match g.current_button() {
                2 => {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    (s.hooks.go)(path.clone(), true);
                }
                3 => {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    s.menu(&r, &path, pinned, x, y);
                }
                _ => {}
            }
        });
        row.add_controller(g);
        if !p.path.to_string_lossy().starts_with(VOL_PREFIX) && !browser::is_pc(&p.path) {
            (self.hooks.attach_drop)(row.upcast_ref(), p.path);
        }
        row
    }

    fn menu(self: &Rc<Self>, row: &gtk::ListBoxRow, path: &Path, pinned: bool, x: f64, y: f64) {
        let group = gio::SimpleActionGroup::new();
        let add = |name: &str, f: Box<dyn Fn()>| {
            let a = gio::SimpleAction::new(name, None);
            a.connect_activate(move |_, _| f());
            group.add_action(&a);
        };
        let w = Rc::downgrade(self);
        let p = path.to_path_buf();
        add("open-tab", Box::new(move || {
            if let Some(s) = w.upgrade() {
                (s.hooks.go)(p.clone(), true);
            }
        }));
        let p = path.to_path_buf();
        add("pin", Box::new(move || toggle_pin(&p)));
        let p = path.to_path_buf();
        add("terminal", Box::new(move || {
            let _ = crate::apps::open_terminal(&p);
        }));
        row.insert_action_group("side", Some(&group));

        let m = gio::Menu::new();
        m.append(Some("open in new tab"), Some("side.open-tab"));
        if !browser::is_virtual(path) && !path.to_string_lossy().starts_with(VOL_PREFIX) {
            m.append(Some("open in terminal"), Some("side.terminal"));
        }
        let virt = browser::is_virtual(path) || path.to_string_lossy().starts_with(VOL_PREFIX);
        if pinned {
            m.append(Some("unpin from quick access"), Some("side.pin"));
        } else if path != util::home() && !virt {
            m.append(Some("pin to quick access"), Some("side.pin"));
        }
        let pop = gtk::PopoverMenu::from_model(Some(&m));
        pop.set_parent(row);
        pop.set_has_arrow(false);
        pop.set_halign(gtk::Align::Start);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Highlight the place we're in (or nothing).
    pub fn highlight(&self, p: &Path) {
        *self.current.borrow_mut() = p.to_path_buf();
        let mut i = 0;
        while let Some(row) = self.nav.row_at_index(i) {
            if row.is_selectable() && Path::new(row.widget_name().as_str()) == p {
                self.nav.select_row(Some(&row));
                return;
            }
            i += 1;
        }
        self.nav.unselect_all();
    }
}
