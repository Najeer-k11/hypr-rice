//! Drive UI: the "this pc" page (drive cards with free space), opening a volume (mount /
//! unlock), and the safe-eject dialog that walks through flush → unmount → lock → power off.

use crate::drives::{self, EjectError, Stage, Volume};
use crate::{ops, ui, util};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

/// Dialogs must outlive the button that started them: sidebars and drive cards are rebuilt
/// whenever drives change (which mounting/ejecting does), so anchor to the window.
fn stable(anchor: &gtk::Widget) -> gtk::Widget {
    anchor.root().map(|r| r.upcast::<gtk::Widget>()).unwrap_or_else(|| anchor.clone())
}

/// Mount (or unlock + mount) if needed, then hand over the mount point.
pub fn open_volume(anchor: &gtk::Widget, obj: &str, then: impl Fn(PathBuf) + 'static) {
    let anchor = &stable(anchor);
    let Some(v) = drives::find(obj) else { return };
    if let Some(m) = v.mount_point() {
        then(m.clone());
        return;
    }
    let then = Rc::new(then);
    if v.locked {
        unlock_prompt(anchor, v, then);
        return;
    }
    let (anchor, obj, title) = (anchor.clone(), obj.to_string(), v.title());
    glib::spawn_future_local(async move {
        match drives::mount(&obj).await {
            Ok(p) => then(p),
            Err(e) => ui::alert(&anchor, &format!("couldn't open {title}"), &e),
        }
    });
}

fn unlock_prompt(anchor: &gtk::Widget, v: Volume, then: Rc<dyn Fn(PathBuf)>) {
    let Some(d) = ui::Dialog::new(anchor, &format!("unlock {}", v.title()), "this drive is encrypted. enter its passphrase.") else { return };
    let pw = gtk::PasswordEntry::new();
    pw.set_show_peek_icon(true);
    let err = ui::label("", &["dialog-error"]);
    err.set_visible(false);
    d.content.append(&pw);
    d.content.append(&err);
    let submit: Rc<dyn Fn()> = {
        let (pw, err, obj, anchor, dw) = (pw.clone(), err.clone(), v.obj.clone(), anchor.clone(), Rc::downgrade(&d));
        Rc::new(move || {
            let pass = pw.text().to_string();
            pw.set_sensitive(false);
            let (pw, err, obj, anchor, dw, then) = (pw.clone(), err.clone(), obj.clone(), anchor.clone(), dw.clone(), then.clone());
            glib::spawn_future_local(async move {
                match drives::unlock_and_mount(&obj, &pass).await {
                    Ok(p) => {
                        if let Some(d) = dw.upgrade() {
                            d.close();
                        }
                        then(p);
                    }
                    Err(e) => {
                        pw.set_sensitive(true);
                        pw.set_text("");
                        pw.grab_focus();
                        err.set_text(if e.to_lowercase().contains("passphrase") || e.contains("key") { "wrong passphrase" } else { &e });
                        err.set_visible(true);
                        let _ = &anchor;
                    }
                }
            });
        })
    };
    d.button("cancel", &[], || true);
    let s = submit.clone();
    d.button("unlock", &["suggested"], move || {
        s();
        false
    });
    pw.connect_activate(move |_| submit());
    pw.grab_focus();
}

/// Safely eject the drive `drive` (all its volumes). `leave` moves tabs off its mounts.
pub fn eject(anchor: &gtk::Widget, drive: &str, name: &str, leave: Rc<dyn Fn(&[PathBuf])>) {
    let anchor = &stable(anchor);
    let on_drive: Vec<Volume> = drives::volumes().into_iter().filter(|v| v.drive == drive).collect();
    let is_image = on_drive.iter().any(|v| v.backing.is_some());
    let mounts: Vec<PathBuf> = on_drive.into_iter().flat_map(|v| v.mounts).collect();
    if mounts.iter().any(|m| ops::busy_under(m)) {
        ui::alert(
            anchor,
            &format!("{name} is still being written to"),
            "a copy or move involving this drive is still running. wait for it to finish (or cancel it), then eject again — pulling it now could leave files half-written.",
        );
        return;
    }
    leave(&mounts);

    let Some(d) = ui::Dialog::new(anchor, &format!("ejecting {name}"), "don't unplug the drive yet.") else { return };
    d.set_locked(true);
    let stage = ui::label("preparing…", &["prop-val"]);
    stage.set_ellipsize(pango::EllipsizeMode::End);
    let bar = gtk::ProgressBar::new();
    let sub = ui::label("", &["op-sub"]);
    d.content.append(&stage);
    d.content.append(&bar);
    d.content.append(&sub);

    // keep the process alive until the drive is done, even if the window closes
    let hold = gio::Application::default().map(|a| a.hold());
    let first_pending = Rc::new(Cell::new(0u64));
    let progress: Rc<dyn Fn(Stage)> = {
        let (stage, bar, sub, first) = (stage.clone(), bar.clone(), sub.clone(), first_pending.clone());
        Rc::new(move |s| match s {
            Stage::Flushing(left) => {
                stage.set_text("writing cached data to the drive…");
                if first.get() == 0 {
                    first.set(left.max(1));
                }
                if left > 4 << 20 {
                    bar.set_fraction(1.0 - (left as f64 / first.get().max(left) as f64));
                    sub.set_text(&format!("{} still to write", util::human_bytes(left)));
                } else {
                    bar.pulse();
                    sub.set_text("almost done");
                }
            }
            Stage::Unmounting(n) => {
                stage.set_text(&format!("unmounting {n}…"));
                bar.pulse();
                sub.set_text("finishing writes and closing the filesystem");
            }
            Stage::Locking => {
                stage.set_text("locking encryption…");
                bar.pulse();
            }
            Stage::PoweringOff => {
                stage.set_text(if is_image { "detaching the image…" } else { "powering the drive off…" });
                bar.set_fraction(1.0);
                sub.set_text("");
            }
        })
    };
    let (anchor, drive, name) = (anchor.clone(), drive.to_string(), name.to_string());
    glib::spawn_future_local(async move {
        let r = drives::safe_eject(&drive, progress).await;
        d.set_locked(false);
        d.close();
        drop(hold);
        match r {
            Ok(()) => {
                if is_image {
                    ui::alert(&anchor, &format!("{name} was ejected"), "everything was written to the image file and it was unmounted and detached.");
                } else {
                    ui::alert(&anchor, &format!("{name} can be unplugged"), "everything was written to it and it was unmounted and powered off.");
                    util::spawn("notify-send", &["-a", "files", "-i", "drive-removable-media", &format!("{name} can be unplugged"), "safe to remove"]);
                }
            }
            Err(EjectError::Busy(m, who)) => {
                let body = if who.is_empty() {
                    format!("something is still using {} — close files or terminals open on it, then try again.", util::tilde(&m))
                } else {
                    format!("still in use by:\n  {}\n\nclose those (or leave the folder in them), then try again.", who.join("\n  "))
                };
                let (a2, d2, n2, l2) = (anchor.clone(), drive.clone(), name.clone(), Rc::new(|_: &[PathBuf]| {}) as Rc<dyn Fn(&[PathBuf])>);
                ui::choose(&anchor, &format!("can't eject {name} yet"), &body, &["cancel", "try again"], 1, move |i| {
                    if i == 1 {
                        eject(&a2, &d2, &n2, l2.clone());
                    }
                });
            }
            Err(EjectError::Other(e)) => ui::alert(&anchor, &format!("couldn't eject {name}"), &e),
        }
    });
}

// --------------------------------------------------------------- this pc --

pub struct PcHooks {
    /// open a folder (bool = new tab)
    pub open: Box<dyn Fn(PathBuf, bool)>,
    pub eject: Box<dyn Fn(&gtk::Widget, String, String)>,
}

pub struct Page {
    pub root: gtk::ScrolledWindow,
    flow: gtk::FlowBox,
    hooks: PcHooks,
}

impl Page {
    pub fn new(hooks: PcHooks) -> Rc<Page> {
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_homogeneous(true);
        flow.set_min_children_per_line(1);
        flow.set_max_children_per_line(4);
        flow.set_row_spacing(10);
        flow.set_column_spacing(10);
        flow.set_valign(gtk::Align::Start);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.add_css_class("pc-page");
        body.append(&ui::label("drives", &["section"]));
        body.append(&flow);
        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_vexpand(true);
        root.set_child(Some(&body));
        let me = Rc::new(Page { root, flow, hooks });
        let w = Rc::downgrade(&me);
        drives::on_change(move || {
            if let Some(p) = w.upgrade() {
                p.rebuild();
            }
        });
        me.rebuild();
        me
    }

    pub fn rebuild(self: &Rc<Self>) {
        while let Some(c) = self.flow.first_child() {
            self.flow.remove(&c);
        }
        let root_usage = drives::fs_usage(std::path::Path::new("/"));
        self.flow.append(&self.card("\u{f0322}", "file system", "/", root_usage, None, None));
        for v in drives::volumes() {
            let sub = match (&v.locked, v.mount_point()) {
                (true, _) => "locked  ·  click to unlock".to_string(),
                (_, Some(m)) => util::tilde(m),
                (_, None) => format!("{}  ·  not mounted", v.fstype),
            };
            let eject = v.is_external().then(|| (v.drive.clone(), v.title()));
            self.flow.append(&self.card(v.icon(), &v.title(), &sub, v.usage, Some(v.obj.clone()), eject));
        }
    }

    fn card(self: &Rc<Self>, icon: &str, title: &str, sub: &str, usage: Option<(u64, u64)>, obj: Option<String>, eject: Option<(String, String)>) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        card.add_css_class("drive-card");
        let ic = ui::label(icon, &["drive-icon"]);
        card.append(&ic);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.set_hexpand(true);
        col.set_valign(gtk::Align::Center);
        let t = ui::label(title, &["drive-title"]);
        t.set_ellipsize(pango::EllipsizeMode::End);
        col.append(&t);
        if let Some((free, total)) = usage.filter(|(_, t)| *t > 0) {
            let bar = gtk::ProgressBar::new();
            let used = 1.0 - free as f64 / total as f64;
            bar.set_fraction(used);
            if used > 0.9 {
                bar.add_css_class("full");
            }
            col.append(&bar);
            col.append(&ui::label(&format!("{} free of {}", util::human_bytes(free), util::human_bytes(total)), &["op-sub"]));
        } else {
            let s = ui::label(sub, &["op-sub"]);
            s.set_ellipsize(pango::EllipsizeMode::Middle);
            col.append(&s);
        }
        card.append(&col);
        if let Some((drive, name)) = eject.clone() {
            let b = ui::tool("\u{f01ea}", "safely eject");
            b.set_valign(gtk::Align::Center);
            let w = Rc::downgrade(self);
            b.connect_clicked(move |b| {
                if let Some(p) = w.upgrade() {
                    (p.hooks.eject)(b.upcast_ref(), drive.clone(), name.clone());
                }
            });
            card.append(&b);
        }
        card.set_tooltip_text(Some(sub));

        // click: open (mount first if needed); middle: new tab; right: menu
        let g = gtk::GestureClick::new();
        g.set_button(0);
        let w = Rc::downgrade(self);
        let (obj2, card2) = (obj.clone(), card.clone());
        g.connect_released(move |g, _, x, y| {
            let Some(p) = w.upgrade() else { return };
            let btn = g.current_button();
            let open = {
                let (p, obj2, card2) = (p.clone(), obj2.clone(), card2.clone());
                move |new_tab: bool| {
                    let p2 = p.clone();
                    match &obj2 {
                        None => (p.hooks.open)(PathBuf::from("/"), new_tab),
                        Some(o) => open_volume(card2.upcast_ref(), o, move |m| (p2.hooks.open)(m, new_tab)),
                    }
                }
            };
            match btn {
                1 => open(false),
                2 => open(true),
                3 => p.menu(&card2, x, y, obj2.clone(), eject.clone()),
                _ => {}
            }
        });
        card.add_controller(g);
        card
    }

    fn menu(self: &Rc<Self>, card: &gtk::Box, x: f64, y: f64, obj: Option<String>, eject: Option<(String, String)>) {
        let group = gio::SimpleActionGroup::new();
        let w = Rc::downgrade(self);
        let add = |name: &str, f: Rc<dyn Fn(&Rc<Page>)>| {
            let a = gio::SimpleAction::new(name, None);
            let w = w.clone();
            a.connect_activate(move |_, _| {
                if let Some(p) = w.upgrade() {
                    f(&p);
                }
            });
            group.add_action(&a);
        };
        let c = card.clone();
        let o = obj.clone();
        let open_with = move |new_tab: bool| -> Rc<dyn Fn(&Rc<Page>)> {
            let (c, o) = (c.clone(), o.clone());
            Rc::new(move |p: &Rc<Page>| {
                let p2 = p.clone();
                match &o {
                    None => (p.hooks.open)(PathBuf::from("/"), new_tab),
                    Some(o) => open_volume(c.upcast_ref(), o, move |m| (p2.hooks.open)(m, new_tab)),
                }
            })
        };
        add("open", open_with(false));
        add("open-tab", open_with(true));
        let m = gio::Menu::new();
        m.append(Some("open"), Some("pc.open"));
        m.append(Some("open in new tab"), Some("pc.open-tab"));
        if let Some((drive, name)) = eject {
            let c = card.clone();
            add("eject", Rc::new(move |p: &Rc<Page>| (p.hooks.eject)(c.upcast_ref(), drive.clone(), name.clone())));
            m.append(Some("safely eject"), Some("pc.eject"));
        }
        card.insert_action_group("pc", Some(&group));
        let pop = gtk::PopoverMenu::from_model(Some(&m));
        pop.set_parent(card);
        pop.set_has_arrow(false);
        pop.set_halign(gtk::Align::Start);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    pub fn count(&self) -> usize {
        drives::volumes().len() + 1
    }
}
