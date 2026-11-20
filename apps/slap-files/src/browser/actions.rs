//! What you can do to files from a browser pane: context menus, clipboard (interoperable with
//! other file managers), rename / new / trash / delete, copy & move with a progress panel,
//! drag & drop, and undo. The heavy lifting is in `crate::ops` on worker threads.

use super::{display_name, is_pc, is_trash, is_virtual, BRef, Browser};
use crate::item::{self, Item};
use crate::ops::{self, Conflict, Kind, Undo};
use crate::{apps, archive, props, sidebar, trash, ui, util};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

// ------------------------------------------------------------ per-item hooks --

/// Right-click menu, drag source and (for folders) drop target on a list cell / grid tile.
pub(super) fn attach(br: &BRef, li: &gtk::ListItem, w: &gtk::Widget, drop: bool) {
    let liw = li.downgrade();

    let g = gtk::GestureClick::new();
    g.set_button(3);
    let (l, b) = (liw.clone(), br.clone());
    g.connect_pressed(move |g, _, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        let (Some(li), Some(b)) = (l.upgrade(), b.get()) else { return };
        let pos = li.position();
        if !b.sel.is_selected(pos) {
            b.sel.select_item(pos, true);
        }
        if let Some(w) = g.widget() {
            b.context_menu(&w, x, y);
        }
    });
    w.add_controller(g);

    // middle-click a folder: open it in a new tab
    let mc = gtk::GestureClick::new();
    mc.set_button(2);
    let (l, b) = (liw.clone(), br.clone());
    mc.connect_pressed(move |g, _, _, _| {
        let (Some(li), Some(b)) = (l.upgrade(), b.get()) else { return };
        let Some(o) = li.item() else { return };
        let (dir, p) = {
            let it = item::of(&o);
            (it.is_dir, it.path())
        };
        if dir {
            g.set_state(gtk::EventSequenceState::Claimed);
            b.open_in_tab(p);
        }
    });
    w.add_controller(mc);

    let ds = gtk::DragSource::new();
    ds.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    let (l, b) = (liw.clone(), br.clone());
    ds.connect_prepare(move |_, _, _| {
        let (li, b) = (l.upgrade()?, b.get()?);
        let pos = li.position();
        if !b.sel.is_selected(pos) {
            b.sel.select_item(pos, true);
        }
        let files: Vec<gio::File> = b.selected().iter().map(|i| gio::File::for_path(i.path())).collect();
        if files.is_empty() {
            return None;
        }
        Some(gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value()))
    });
    // drag badge: "icon  name" or "N items", themed like the rest
    let b = br.clone();
    ds.connect_drag_begin(move |_, drag| {
        let Some(b) = b.get() else { return };
        let items = b.selected();
        let text = match items.as_slice() {
            [one] => format!("{}  {}", one.icon, one.name),
            many => format!("\u{f0222}  {} items", many.len()),
        };
        let badge = ui::label(&text, &["drag-badge"]);
        badge.set_ellipsize(pango::EllipsizeMode::Middle);
        badge.set_max_width_chars(36);
        gtk::DragIcon::for_drag(drag).set_child(Some(&badge));
        drag.set_hotspot(-14, -14);
    });
    w.add_controller(ds);

    if drop {
        let dir_of: Rc<dyn Fn() -> Option<PathBuf>> = Rc::new(move || {
            let o = liw.upgrade()?.item()?;
            let it = item::of(&o);
            it.is_dir.then(|| it.path())
        });
        let dt = drop_target();
        let d = dir_of.clone();
        dt.connect_accept(move |_, _| d().is_some());
        let d = dir_of.clone();
        dt.connect_enter(move |t, _, _| d().map(|dir| choose_action(t, &dir)).unwrap_or(gdk::DragAction::empty()));
        let d = dir_of.clone();
        dt.connect_motion(move |t, _, _| d().map(|dir| choose_action(t, &dir)).unwrap_or(gdk::DragAction::empty()));
        let b = br.clone();
        dt.connect_drop(move |t, v, _, _| match (dir_of(), b.get()) {
            (Some(dir), Some(b)) => b.drop_files(v, &dir, choose_action(t, &dir)),
            _ => false,
        });
        w.add_controller(dt);
    }
}

/// Drops on empty space go into the current folder.
pub(super) fn attach_drop_zone(b: &Rc<Browser>) {
    let dt = drop_target();
    let w = Rc::downgrade(b);
    dt.connect_enter(move |t, _, _| w.upgrade().map(|b| choose_action(t, &b.cwd())).unwrap_or(gdk::DragAction::empty()));
    let w = Rc::downgrade(b);
    dt.connect_motion(move |t, _, _| w.upgrade().map(|b| choose_action(t, &b.cwd())).unwrap_or(gdk::DragAction::empty()));
    let w = Rc::downgrade(b);
    dt.connect_drop(move |t, v, _, _| {
        let Some(b) = w.upgrade() else { return false };
        let cwd = b.cwd();
        b.drop_files(v, &cwd, choose_action(t, &cwd))
    });
    b.view_area.add_controller(dt);
}

/// Breadcrumb segments accept drops too (drag a file onto "home" to move it there).
pub(super) fn attach_crumb_drop(b: &Rc<Browser>, btn: &gtk::Button, target: PathBuf) {
    let w = Rc::downgrade(b);
    attach_path_drop(btn.upcast_ref(), target, move || w.upgrade());
}

/// Make any widget (crumb, sidebar place) a drop target for `target`; the transfer runs in
/// whichever browser `get` returns (for its progress panel).
pub fn attach_path_drop(widget: &gtk::Widget, target: PathBuf, get: impl Fn() -> Option<Rc<Browser>> + 'static) {
    let dt = drop_target();
    let t1 = target.clone();
    dt.connect_enter(move |t, _, _| choose_action(t, &t1));
    let t2 = target.clone();
    dt.connect_motion(move |t, _, _| choose_action(t, &t2));
    dt.connect_drop(move |t, v, _, _| match get() {
        Some(b) => b.drop_files(v, &target, choose_action(t, &target)),
        None => false,
    });
    widget.add_controller(dt);
}

fn drop_target() -> gtk::DropTarget {
    let dt = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
    dt.set_preload(true); // need the file list during motion to pick copy vs move
    dt
}

fn dropped_paths(v: &glib::Value) -> Vec<PathBuf> {
    v.get::<gdk::FileList>().map(|fl| fl.files().iter().filter_map(|f| f.path()).collect()).unwrap_or_default()
}

/// Explorer rules: ctrl = copy, shift = move, otherwise move within a drive, copy across.
fn choose_action(t: &gtk::DropTarget, dest: &Path) -> gdk::DragAction {
    let srcs = t.value().map(|v| dropped_paths(&v)).unwrap_or_default();
    // never into itself or its own subfolder
    if srcs.iter().any(|s| dest.starts_with(s)) {
        return gdk::DragAction::empty();
    }
    let st = t.current_event_state();
    let same_folder = !srcs.is_empty() && srcs.iter().all(|s| s.parent() == Some(dest));
    if st.contains(gdk::ModifierType::CONTROL_MASK) {
        gdk::DragAction::COPY
    } else if same_folder {
        gdk::DragAction::empty() // moving onto itself does nothing
    } else if st.contains(gdk::ModifierType::SHIFT_MASK) {
        gdk::DragAction::MOVE
    } else if srcs.first().is_some_and(|s| ops::is_same_device(s, dest)) {
        gdk::DragAction::MOVE
    } else {
        gdk::DragAction::COPY
    }
}

fn validate_name(dir: &Path, t: &str, current: Option<&str>) -> Option<String> {
    if t.trim().is_empty() {
        return Some("a name is needed".into());
    }
    if t.contains('/') {
        return Some("names can't contain /".into());
    }
    if t == "." || t == ".." {
        return Some("that name is reserved".into());
    }
    if Some(t) == current {
        return None;
    }
    if std::fs::symlink_metadata(dir.join(t)).is_ok() {
        return Some(format!("'{t}' already exists here"));
    }
    None
}

/// Characters to preselect when renaming: the stem, not the extension.
fn stem_chars(name: &str, is_dir: bool) -> i32 {
    match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => name[..i].chars().count() as i32,
        _ => -1,
    }
}

// ------------------------------------------------------------------ actions --

impl Browser {
    pub(super) fn install_actions(self: &Rc<Self>) {
        let acts: &[(&str, fn(&Rc<Browser>))] = &[
            ("open", |b| b.open_selection()),
            ("cut", |b| b.cut()),
            ("copy", |b| b.copy()),
            ("paste", |b| b.paste()),
            ("rename", |b| b.rename()),
            ("trash", |b| b.trash()),
            ("delete", |b| b.delete()),
            ("copy-path", |b| b.copy_path()),
            ("new-folder", |b| b.new_item(true)),
            ("new-file", |b| b.new_item(false)),
            ("select-all", |b| b.select_all()),
            ("refresh", |b| b.refresh()),
            ("hidden", |b| b.toggle_hidden()),
            ("view-list", |b| b.set_view("list")),
            ("view-grid", |b| b.set_view("grid")),
            ("undo", |b| b.undo()),
            ("open-tab", |b| {
                for it in b.selected().into_iter().filter(|i| i.is_dir) {
                    b.open_in_tab(it.path());
                }
            }),
            ("open-location", |b| {
                if let Some(it) = b.selected().into_iter().next() {
                    b.reveal(&it.path());
                }
            }),
            ("pin", |b| {
                let p = b.selected().into_iter().find(|i| i.is_dir).map(|i| i.path()).unwrap_or_else(|| b.cwd());
                sidebar::toggle_pin(&p);
                b.flash(if sidebar::is_pinned(&p) { "pinned to quick access" } else { "unpinned" });
            }),
            ("terminal", |b| {
                let p = b.selected().into_iter().find(|i| i.is_dir).map(|i| i.path()).unwrap_or_else(|| b.cwd());
                if let Err(e) = apps::open_terminal(&p) {
                    ui::alert(&b.root, "couldn't open a terminal", &e);
                }
            }),
            ("properties", |b| b.properties()),
            ("restore", |b| b.restore_selected(false)),
            ("restore-all", |b| b.restore_selected(true)),
            ("empty-trash", |b| b.empty_trash()),
            ("extract", |b| b.extract()),
            ("mount-image", |b| {
                let Some(it) = b.selected().into_iter().next() else { return };
                let (p, w) = (it.path(), Rc::downgrade(b));
                glib::spawn_future_local(async move {
                    let r = crate::drives::mount_image(&p).await;
                    let Some(b) = w.upgrade() else { return };
                    match r {
                        Ok(m) => b.go(m),
                        Err(e) => ui::alert(&b.root, "couldn't mount the image", &e),
                    }
                });
            }),
            ("choose-app", |b| {
                let paths: Vec<PathBuf> = b.selected().iter().map(Item::path).collect();
                apps::choose(&b.root, paths);
            }),
        ];
        for &(name, f) in acts {
            let a = gio::SimpleAction::new(name, None);
            let w = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(b) = w.upgrade() {
                    f(&b);
                }
            });
            self.actions.add_action(&a);
        }
        // open with <app id>
        let a = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            let (Some(b), Some(id)) = (w.upgrade(), v.and_then(|v| v.str().map(String::from))) else { return };
            let paths: Vec<PathBuf> = b.selected().iter().map(Item::path).collect();
            if let Some(app) = apps::by_id(&id) {
                if let Err(e) = apps::launch(&app, &paths, &b.root) {
                    ui::alert(&b.root, &format!("couldn't start {}", app.name()), &e);
                }
            }
        });
        self.actions.add_action(&a);
        // compress to <extension>
        let a = gio::SimpleAction::new("compress", Some(glib::VariantTy::STRING));
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            if let (Some(b), Some(ext)) = (w.upgrade(), v.and_then(|v| v.str().map(String::from))) {
                b.compress(&ext);
            }
        });
        self.actions.add_action(&a);
        self.root.insert_action_group("fm", Some(&self.actions));
    }

    fn enable(&self, name: &str, on: bool) {
        if let Some(a) = self.actions.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            a.set_enabled(on);
        }
    }

    fn clipboard_has_files(&self) -> bool {
        let f = self.root.clipboard().formats();
        f.contain_mime_type(ops::MIME_URIS) || f.contain_mime_type(ops::MIME_GNOME)
    }

    /// Item menu when something is selected, folder menu otherwise.
    pub(super) fn context_menu(self: &Rc<Self>, w: &gtk::Widget, x: f64, y: f64) {
        let sel = self.selected();
        self.enable("paste", self.clipboard_has_files());
        self.enable("undo", ops::can_undo());
        let menu = gio::Menu::new();
        let section = |items: &[(&str, &str, &str)]| {
            let s = gio::Menu::new();
            for &(label, action, accel) in items {
                let i = gio::MenuItem::new(Some(label), Some(&format!("fm.{action}")));
                if !accel.is_empty() {
                    i.set_attribute_value("accel", Some(&accel.to_variant()));
                }
                s.append_item(&i);
            }
            menu.append_section(None, &s);
        };
        let pin_label = |p: &Path| if sidebar::is_pinned(p) { "unpin from quick access" } else { "pin to quick access" };
        let cwd = self.cwd();
        if is_pc(&cwd) {
            return;
        }
        if is_trash(&cwd) {
            if sel.is_empty() {
                section(&[("empty trash", "empty-trash", ""), ("restore all", "restore-all", "")]);
                section(&[("select all", "select-all", "<Control>a"), ("refresh", "refresh", "F5")]);
                section(&[("details", "view-list", "<Control>1"), ("large icons", "view-grid", "<Control>2")]);
            } else {
                section(&[("restore", "restore", "")]);
                section(&[("delete permanently", "delete", "Delete")]);
            }
            self.popup_menu(&menu, w, x, y);
            return;
        }
        if sel.is_empty() {
            section(&[("new folder", "new-folder", "<Control><Shift>n"), ("new file", "new-file", "")]);
            section(&[("paste", "paste", "<Control>v"), ("undo", "undo", "<Control>z")]);
            section(&[("open in terminal", "terminal", ""), (pin_label(&self.cwd()), "pin", ""), ("properties", "properties", "<Alt>Return")]);
            let hid = if self.show_hidden.get() { "hide hidden files" } else { "show hidden files" };
            section(&[("select all", "select-all", "<Control>a"), (hid, "hidden", "<Control>h"), ("refresh", "refresh", "F5")]);
            section(&[("details", "view-list", "<Control>1"), ("large icons", "view-grid", "<Control>2")]);
        } else {
            let one_dir = sel.len() == 1 && sel[0].is_dir;
            let all_files = sel.iter().all(|i| !i.is_dir);
            let mut s1 = vec![("open", "open", "Return")];
            if sel.iter().any(|i| i.is_dir) {
                s1.push(("open in new tab", "open-tab", ""));
            }
            if self.searching.get() {
                s1.push(("open file location", "open-location", ""));
            }
            section(&s1);
            if all_files {
                // open with ▸ recommended apps, then the full picker
                let sub = gio::Menu::new();
                for app in apps::recommended(&sel[0].path()).into_iter().take(8) {
                    if let Some(id) = app.id() {
                        let i = gio::MenuItem::new(Some(&app.name()), None);
                        i.set_action_and_target_value(Some("fm.open-with"), Some(&id.to_variant()));
                        sub.append_item(&i);
                    }
                }
                let more = gio::Menu::new();
                more.append(Some("choose another app…"), Some("fm.choose-app"));
                sub.append_section(None, &more);
                let s = gio::Menu::new();
                s.append_submenu(Some("open with"), &sub);
                menu.append_section(None, &s);
            }
            section(&[("cut", "cut", "<Control>x"), ("copy", "copy", "<Control>c"), ("paste", "paste", "<Control>v")]);
            let mut s3 = Vec::new();
            if sel.len() == 1 {
                s3.push(("rename", "rename", "F2"));
            }
            s3.push(("copy path", "copy-path", "<Control><Shift>c"));
            section(&s3);
            if one_dir {
                let p = sel[0].path();
                section(&[(pin_label(&p), "pin", ""), ("open in terminal", "terminal", "")]);
            }
            // archives
            let s = gio::Menu::new();
            if sel.len() == 1 && !sel[0].is_dir && [".iso", ".img"].iter().any(|e| sel[0].name.to_ascii_lowercase().ends_with(e)) {
                s.append_item(&gio::MenuItem::new(Some("mount image"), Some("fm.mount-image")));
            }
            if sel.iter().all(|i| !i.is_dir && archive::is_archive(&i.name)) {
                let i = gio::MenuItem::new(Some("extract here"), Some("fm.extract"));
                s.append_item(&i);
            }
            let sub = gio::Menu::new();
            for (label, ext) in archive::FORMATS {
                let i = gio::MenuItem::new(Some(label), None);
                i.set_action_and_target_value(Some("fm.compress"), Some(&ext.to_variant()));
                sub.append_item(&i);
            }
            s.append_submenu(Some("compress to"), &sub);
            menu.append_section(None, &s);
            section(&[("move to trash", "trash", "Delete"), ("delete permanently", "delete", "<Shift>Delete")]);
            section(&[("properties", "properties", "<Alt>Return")]);
        }
        self.popup_menu(&menu, w, x, y);
    }

    fn popup_menu(&self, menu: &gio::Menu, w: &gtk::Widget, x: f64, y: f64) {
        let pop = gtk::PopoverMenu::from_model(Some(menu));
        pop.add_css_class("context");
        pop.set_parent(&self.root);
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        pop.set_halign(gtk::Align::Start);
        let pt = w
            .compute_point(&self.root, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
        pop.set_pointing_to(Some(&gdk::Rectangle::new(pt.x() as i32, pt.y() as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Menu key / shift+f10.
    pub fn context_menu_kbd(self: &Rc<Self>) {
        let w: gtk::Widget = self.view_area.clone().upcast();
        let (x, y) = (w.width() as f64 / 2.0, w.height() as f64 / 3.0);
        self.context_menu(&w, x, y);
    }

    // ---------------------------------------------------------- clipboard --

    pub fn cut(self: &Rc<Self>) {
        self.clip(true);
    }

    pub fn copy(self: &Rc<Self>) {
        self.clip(false);
    }

    fn clip(self: &Rc<Self>, cut: bool) {
        let paths: Vec<PathBuf> = self.selected().iter().map(Item::path).collect();
        if paths.is_empty() {
            return;
        }
        let uris: Vec<String> = paths.iter().map(|p| ops::uri_of(p)).collect();
        let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
        let list = uris.join("\r\n") + "\r\n";
        let plain = paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
        let bytes = |s: String| glib::Bytes::from_owned(s.into_bytes());
        let mut provs = vec![
            gdk::ContentProvider::for_bytes(ops::MIME_GNOME, &bytes(gnome)),
            gdk::ContentProvider::for_bytes(ops::MIME_URIS, &bytes(list)),
            gdk::ContentProvider::for_bytes("text/plain;charset=utf-8", &bytes(plain)),
        ];
        if cut {
            provs.push(gdk::ContentProvider::for_bytes("application/x-kde-cutselection", &glib::Bytes::from_static(b"1")));
        }
        let _ = self.root.clipboard().set_content(Some(&gdk::ContentProvider::new_union(&provs)));
        self.flash(&format!("{} {}  ·  ctrl+v to paste", if cut { "cut" } else { "copied" }, plural(paths.len(), "item")));
    }

    pub fn copy_path(self: &Rc<Self>) {
        let items = self.selected();
        let text = if items.is_empty() {
            self.cwd().to_string_lossy().into_owned()
        } else {
            items.iter().map(|i| i.path().to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n")
        };
        self.root.clipboard().set_text(&text);
        self.flash("copied path");
    }

    pub fn paste(self: &Rc<Self>) {
        if is_virtual(&self.cwd()) {
            return;
        }
        let cb = self.root.clipboard();
        let me = self.clone();
        let dest = self.cwd();
        glib::spawn_future_local(async move {
            let Ok((stream, mime)) = cb.read_future(&[ops::MIME_GNOME, ops::MIME_URIS], glib::Priority::DEFAULT).await else {
                me.flash("nothing to paste");
                return;
            };
            let out = gio::MemoryOutputStream::new_resizable();
            let flags = gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET;
            if out.splice_future(&stream, flags, glib::Priority::DEFAULT).await.is_err() {
                return;
            }
            let bytes = out.steal_as_bytes();
            let (cut, paths) = ops::parse_clip(&String::from_utf8_lossy(&bytes), mime.as_str());
            let paths: Vec<PathBuf> = paths.into_iter().filter(|p| std::fs::symlink_metadata(p).is_ok()).collect();
            if paths.is_empty() {
                me.flash("nothing to paste");
                return;
            }
            // a cut is used up once pasted (like Explorer)
            let after: Option<Box<dyn FnOnce()>> = if cut {
                Some(Box::new(move || {
                    let _ = cb.set_content(None::<&gdk::ContentProvider>);
                }))
            } else {
                None
            };
            me.start_transfer(if cut { Kind::Move } else { Kind::Copy }, paths, dest, after);
        });
    }

    fn drop_files(self: &Rc<Self>, v: &glib::Value, dest: &Path, action: gdk::DragAction) -> bool {
        let srcs = dropped_paths(v);
        if srcs.is_empty() || action.is_empty() || is_pc(dest) {
            return false;
        }
        if is_trash(dest) {
            // dropping onto the trash = move to trash
            self.trash_paths(srcs);
            return true;
        }
        let kind = if action.contains(gdk::DragAction::MOVE) { Kind::Move } else { Kind::Copy };
        self.start_transfer(kind, srcs, dest.to_path_buf(), None);
        true
    }

    // --------------------------------------------------------- copy / move --

    /// Ask about name clashes (once, for the whole batch), then run.
    fn start_transfer(self: &Rc<Self>, kind: Kind, srcs: Vec<PathBuf>, dest: PathBuf, after: Option<Box<dyn FnOnce()>>) {
        let clashes: Vec<String> = srcs
            .iter()
            .filter(|s| s.parent() != Some(dest.as_path()))
            .filter_map(|s| {
                let n = s.file_name()?;
                std::fs::symlink_metadata(dest.join(n)).ok().map(|_| n.to_string_lossy().into_owned())
            })
            .collect();
        if clashes.is_empty() {
            self.run_transfer(kind, srcs, dest, Conflict::KeepBoth, after);
            return;
        }
        let body = if clashes.len() == 1 {
            format!("'{}' already exists in {}.", clashes[0], display_name(&dest))
        } else {
            format!("{} items with the same names already exist in {}.", clashes.len(), display_name(&dest))
        } + " replacing a folder merges it.";
        let me = self.clone();
        let pending = RefCell::new(Some((srcs, after)));
        ui::choose(&self.root, "replace or skip files", &body, &["cancel", "skip", "keep both", "replace"], 3, move |i| {
            let c = match i {
                1 => Conflict::Skip,
                2 => Conflict::KeepBoth,
                3 => Conflict::Replace,
                _ => return,
            };
            if let Some((s, a)) = pending.borrow_mut().take() {
                me.run_transfer(kind, s, dest.clone(), c, a);
            }
        });
    }

    fn sync_ops_visibility(&self) {
        let mut c = self.ops_box.first_child();
        let mut any = false;
        while let Some(w) = c {
            any |= w.get_visible(); // own flag (is_visible() also checks parents)
            c = w.next_sibling();
        }
        self.ops_box.set_visible(any);
    }

    fn run_transfer(self: &Rc<Self>, kind: Kind, srcs: Vec<PathBuf>, dest: PathBuf, conflict: Conflict, after: Option<Box<dyn FnOnce()>>) {
        let h = ops::Handle::default();
        let n = srcs.len();
        let verb = if kind == Kind::Copy { "copying" } else { "moving" };
        let what = if n == 1 { format!("'{}'", ops::name_of(&srcs[0])) } else { plural(n, "item") };

        // progress row: only shown if the job outlives 400 ms, so quick ones never flicker
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("op");
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.set_hexpand(true);
        let title = ui::label(&format!("{verb} {what} to {}", display_name(&dest)), &["op-title"]);
        title.set_ellipsize(pango::EllipsizeMode::Middle);
        let bar = gtk::ProgressBar::new();
        let sub = ui::label("counting…", &["op-sub"]);
        sub.set_ellipsize(pango::EllipsizeMode::Middle);
        col.append(&title);
        col.append(&bar);
        col.append(&sub);
        let cancel = ui::tool("\u{f0156}", "cancel");
        row.append(&col);
        row.append(&cancel);
        row.set_visible(false);
        self.ops_box.append(&row);
        let c = h.cancel.clone();
        cancel.connect_clicked(move |b| {
            c.store(true, Ordering::Relaxed);
            b.set_sensitive(false);
        });

        let done = Rc::new(Cell::new(false));
        let start = Instant::now();
        {
            let (done, h, row, bar, sub, me) = (done.clone(), h.clone(), row.clone(), bar.clone(), sub.clone(), self.clone());
            glib::timeout_add_local(Duration::from_millis(150), move || {
                if done.get() {
                    return glib::ControlFlow::Break;
                }
                if start.elapsed() > Duration::from_millis(400) && !row.get_visible() {
                    row.set_visible(true);
                    me.sync_ops_visibility();
                }
                let p = h.progress.lock().map(|p| p.clone()).unwrap_or_default();
                if p.scanning {
                    bar.pulse();
                    sub.set_text("counting…");
                } else if p.bytes_total > 0 {
                    let frac = (p.bytes_done as f64 / p.bytes_total as f64).min(1.0);
                    bar.set_fraction(frac);
                    let secs = start.elapsed().as_secs_f64().max(0.1);
                    let rate = p.bytes_done as f64 / secs;
                    let left = if rate > 0.0 { (p.bytes_total - p.bytes_done.min(p.bytes_total)) as f64 / rate } else { 0.0 };
                    let eta = if left >= 60.0 { format!("{:.0} min left", left / 60.0) } else { format!("{left:.0} s left") };
                    sub.set_text(&format!(
                        "{} of {}  ·  {}/s  ·  {eta}  ·  {}",
                        util::human_bytes(p.bytes_done),
                        util::human_bytes(p.bytes_total),
                        util::human_bytes(rate as u64),
                        p.current
                    ));
                } else {
                    let frac = if p.files_total > 0 { p.files_done as f64 / p.files_total as f64 } else { 0.0 };
                    bar.set_fraction(frac.min(1.0));
                    sub.set_text(&format!("{} of {}  ·  {}", p.files_done, p.files_total, p.current));
                }
                glib::ControlFlow::Continue
            });
        }

        // eject checks this registry; the hold keeps the process alive to finish/clean up
        let mut involved = srcs.clone();
        involved.push(dest.clone());
        let guard = ops::register(involved, h.cancel.clone());
        let hold = gio::Application::default().map(|a| a.hold());
        let me = self.clone();
        let h2 = h.clone();
        let d2 = dest.clone();
        util::bg(move || ops::transfer(kind, srcs, d2, conflict, &h2), move |out| {
            drop(guard);
            drop(hold);
            done.set(true);
            me.ops_box.remove(&row);
            me.sync_ops_visibility();
            let ok = out.errors.is_empty() && !out.cancelled;
            me.finish_transfer(kind, &dest, out);
            if ok {
                if let Some(f) = after {
                    f();
                }
            }
        });
    }

    fn finish_transfer(self: &Rc<Self>, kind: Kind, dest: &Path, out: ops::Outcome) {
        let fresh: Vec<&(PathBuf, PathBuf, bool)> = out.done.iter().filter(|d| d.2).collect();
        if !fresh.is_empty() {
            ops::push_undo(match kind {
                Kind::Copy => Undo::Copy(fresh.iter().map(|d| d.1.clone()).collect()),
                Kind::Move => Undo::Move(fresh.iter().map(|d| (d.0.clone(), d.1.clone())).collect()),
            });
        }
        if dest == self.cwd() {
            *self.select_after.borrow_mut() = out.done.iter().map(|d| ops::name_of(&d.1)).collect();
            self.refresh();
        }
        let verb = if kind == Kind::Copy { "copied" } else { "moved" };
        let n = out.done.len();
        if out.cancelled {
            self.flash(&format!("cancelled  ·  {} {verb}", plural(n, "item")));
        } else if n > 0 {
            self.flash(&format!("{verb} {}  ·  ctrl+z to undo", plural(n, "item")));
        }
        if !out.errors.is_empty() {
            let mut body = out.errors.iter().take(8).cloned().collect::<Vec<_>>().join("\n");
            if out.errors.len() > 8 {
                body.push_str(&format!("\n… and {} more", out.errors.len() - 8));
            }
            ui::alert(&self.root, &format!("some items couldn't be {verb}"), &body);
        }
    }

    // ------------------------------------------------- rename / new / delete --

    pub fn rename(self: &Rc<Self>) {
        if is_virtual(&self.cwd()) {
            return;
        }
        let Some(it) = self.selected().into_iter().next() else { return };
        let (name, dir) = (it.name.clone(), it.dir.to_path_buf());
        let (d2, n2) = (dir.clone(), name.clone());
        let me = self.clone();
        let (initial, sel) = (name.clone(), stem_chars(&name, it.is_dir));
        ui::prompt(&self.root, "rename", &initial, sel, "rename", move |t| validate_name(&d2, t, Some(&n2)), move |new| {
            if new == name {
                return;
            }
            let (from, to) = (dir.join(&name), dir.join(&new));
            match std::fs::rename(&from, &to) {
                Ok(()) => {
                    ops::push_undo(Undo::Rename { from, to });
                    *me.select_after.borrow_mut() = vec![new];
                    me.refresh();
                }
                Err(e) => ui::alert(&me.root, "couldn't rename", &e.to_string()),
            }
        });
    }

    /// New folder (ctrl+shift+n) or empty file, named in a prompt first.
    pub fn new_item(self: &Rc<Self>, folder: bool) {
        let dir = self.cwd();
        if is_virtual(&dir) {
            return;
        }
        let base = if folder { "new folder" } else { "new file.txt" };
        let initial = if std::fs::symlink_metadata(dir.join(base)).is_ok() {
            ops::name_of(&ops::unique_name(&dir, base, false))
        } else {
            base.to_string()
        };
        let sel = stem_chars(&initial, folder);
        let d2 = dir.clone();
        let me = self.clone();
        let title = if folder { "new folder" } else { "new file" };
        ui::prompt(&self.root, title, &initial, sel, "create", move |t| validate_name(&d2, t, None), move |name| {
            let p = dir.join(&name);
            let r = if folder {
                std::fs::create_dir(&p)
            } else {
                std::fs::OpenOptions::new().write(true).create_new(true).open(&p).map(|_| ())
            };
            match r {
                Ok(()) => {
                    ops::push_undo(Undo::Create(p));
                    *me.select_after.borrow_mut() = vec![name];
                    me.refresh();
                }
                Err(e) => ui::alert(&me.root, &format!("couldn't create {name}"), &e.to_string()),
            }
        });
    }

    pub fn trash(self: &Rc<Self>) {
        if is_trash(&self.cwd()) {
            self.delete();
            return;
        }
        let paths: Vec<PathBuf> = self.selected().iter().map(Item::path).collect();
        self.trash_paths(paths);
    }

    fn trash_paths(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let me = self.clone();
        util::bg(
            move || {
                let r = ops::trash(&paths);
                (paths, r)
            },
            move |(paths, (ok, errs))| {
                if !ok.is_empty() {
                    me.flash(&format!("moved {} to trash  ·  ctrl+z to undo", plural(ok.len(), "item")));
                }
                if !errs.is_empty() {
                    // e.g. a drive without a trash folder: offer the real thing
                    let failed: Vec<PathBuf> = paths.into_iter().filter(|p| !ok.contains(p)).collect();
                    let me2 = me.clone();
                    ui::confirm(
                        &me.root,
                        "couldn't move to trash",
                        &format!("{}\n\ndelete permanently instead? this can't be undone.", errs.join("\n")),
                        "delete",
                        true,
                        move || me2.delete_paths(failed.clone()),
                    );
                }
                if !ok.is_empty() {
                    ops::push_undo(Undo::Trash(ok));
                }
            },
        );
    }

    pub fn delete(self: &Rc<Self>) {
        let items = self.selected();
        if items.is_empty() {
            return;
        }
        let title = if items.len() == 1 {
            format!("delete '{}' permanently?", items[0].name)
        } else {
            format!("delete {} items permanently?", items.len())
        };
        let me = self.clone();
        if is_trash(&self.cwd()) {
            let entries: Vec<trash::Entry> = items.into_iter().filter_map(|i| i.trash.map(|t| *t)).collect();
            ui::confirm(&self.root, &title, "this can't be undone.", "delete", true, move || me.delete_trash(entries.clone()));
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(Item::path).collect();
        ui::confirm(&self.root, &title, "this can't be undone.", "delete", true, move || me.delete_paths(paths.clone()));
    }

    // ----------------------------------------------------------------- trash --

    fn delete_trash(self: &Rc<Self>, entries: Vec<trash::Entry>) {
        let n = entries.len();
        let me = self.clone();
        util::bg(
            move || entries.iter().filter_map(|e| trash::delete(e).err()).collect::<Vec<_>>(),
            move |errs| {
                me.refresh();
                if errs.is_empty() {
                    me.flash(&format!("deleted {} for good", plural(n, "item")));
                } else {
                    ui::alert(&me.root, "some items couldn't be deleted", &errs.join("\n"));
                }
            },
        );
    }

    fn restore_selected(self: &Rc<Self>, all: bool) {
        let me = self.clone();
        let picked: Option<Vec<trash::Entry>> = (!all).then(|| self.selected().into_iter().filter_map(|i| i.trash.map(|t| *t)).collect());
        util::bg(
            move || {
                let entries = picked.unwrap_or_else(trash::list);
                let n = entries.len();
                let errs: Vec<String> = entries.iter().filter_map(|e| trash::restore(e).err()).collect();
                (n, errs)
            },
            move |(n, errs)| {
                me.refresh();
                if errs.is_empty() {
                    me.flash(&format!("restored {}", plural(n, "item")));
                } else {
                    ui::alert(&me.root, "some items couldn't be restored", &errs.join("\n"));
                }
            },
        );
    }

    fn empty_trash(self: &Rc<Self>) {
        let n = trash::count();
        if n == 0 {
            self.flash("the trash is already empty");
            return;
        }
        let me = self.clone();
        ui::confirm(
            &self.root,
            "empty the trash?",
            &format!("{} will be deleted for good (including trash on mounted drives). this can't be undone.", plural(n, "item")),
            "empty trash",
            true,
            move || me.delete_trash(trash::list()),
        );
    }

    // -------------------------------------------------------------- archives --

    fn compress(self: &Rc<Self>, ext: &str) {
        let items = self.selected();
        let Some(first) = items.first() else { return };
        let dir = first.dir.to_path_buf();
        if items.iter().any(|i| *i.dir != *dir) {
            self.flash("select items from one folder to compress");
            return;
        }
        let base = if items.len() == 1 { archive::stem(&first.name) } else { display_name(&dir) };
        let initial = format!("{base}{ext}");
        let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
        let (d2, me) = (dir.clone(), self.clone());
        let sel = base.chars().count() as i32;
        ui::prompt(&self.root, "compress to", &initial, sel, "compress", move |t| validate_name(&d2, t, None), move |out_name| {
            let out = dir.join(&out_name);
            let (d, n, o) = (dir.clone(), names.clone(), out.clone());
            let mut involved: Vec<PathBuf> = names.iter().map(|x| dir.join(x)).collect();
            involved.push(out.clone());
            me.run_job(format!("compressing to '{out_name}'"), involved, move |c| archive::compress(&d, &n, &o, c).map(|_| vec![o.clone()]));
        });
    }

    fn extract(self: &Rc<Self>) {
        for it in self.selected().into_iter().filter(|i| !i.is_dir && archive::is_archive(&i.name)) {
            self.extract_file(it.path(), it.name.to_string());
        }
    }

    pub(crate) fn extract_file(self: &Rc<Self>, p: PathBuf, name: String) {
        let mut involved = vec![p.clone()];
        involved.extend(p.parent().map(Path::to_path_buf));
        self.run_job(format!("extracting '{name}'"), involved, move |c| archive::extract(&p, c).map(|x| vec![x]));
    }

    /// A job with an indeterminate progress row + cancel. `work` returns what it created
    /// (selected afterwards; ctrl+z moves it to the trash).
    fn run_job(
        self: &Rc<Self>,
        title: String,
        involved: Vec<PathBuf>,
        work: impl FnOnce(&AtomicBool) -> Result<Vec<PathBuf>, String> + Send + 'static,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = ops::register(involved, cancel.clone());
        let hold = gio::Application::default().map(|a| a.hold());
        let (row, bar, sub, btn) = self.job_row(&title);
        sub.set_text("working…");
        let c = cancel.clone();
        btn.connect_clicked(move |b| {
            c.store(true, Ordering::Relaxed);
            b.set_sensitive(false);
        });
        let done = Rc::new(Cell::new(false));
        let start = Instant::now();
        {
            let (done, row, me) = (done.clone(), row.clone(), self.clone());
            glib::timeout_add_local(Duration::from_millis(150), move || {
                if done.get() {
                    return glib::ControlFlow::Break;
                }
                if start.elapsed() > Duration::from_millis(400) && !row.get_visible() {
                    row.set_visible(true);
                    me.sync_ops_visibility();
                }
                bar.pulse();
                glib::ControlFlow::Continue
            });
        }
        let me = self.clone();
        let c2 = cancel.clone();
        util::bg(move || work(&c2), move |r| {
            done.set(true);
            me.ops_box.remove(&row);
            me.sync_ops_visibility();
            drop(guard);
            drop(hold);
            match r {
                Ok(created) => {
                    if let Some(first) = created.first() {
                        if first.parent() == Some(me.cwd().as_path()) {
                            *me.select_after.borrow_mut() = created.iter().map(|p| ops::name_of(p)).collect();
                            me.refresh();
                        }
                    }
                    ops::push_undo(Undo::Copy(created));
                    me.flash("done  ·  ctrl+z to undo");
                }
                Err(e) if cancel.load(Ordering::Relaxed) => {
                    let _ = e;
                    me.flash("cancelled");
                }
                Err(e) => ui::alert(&me.root, &format!("{title} failed"), &e),
            }
        });
    }

    fn job_row(&self, title: &str) -> (gtk::Box, gtk::ProgressBar, gtk::Label, gtk::Button) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("op");
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.set_hexpand(true);
        let t = ui::label(title, &["op-title"]);
        t.set_ellipsize(pango::EllipsizeMode::Middle);
        let bar = gtk::ProgressBar::new();
        let sub = ui::label("", &["op-sub"]);
        sub.set_ellipsize(pango::EllipsizeMode::Middle);
        col.append(&t);
        col.append(&bar);
        col.append(&sub);
        let cancel = ui::tool("\u{f0156}", "cancel");
        row.append(&col);
        row.append(&cancel);
        row.set_visible(false);
        self.ops_box.append(&row);
        (row, bar, sub, cancel)
    }

    fn delete_paths(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let n = paths.len();
        let me = self.clone();
        util::bg(move || ops::delete(&paths), move |errs| {
            if errs.is_empty() {
                me.flash(&format!("deleted {}", plural(n, "item")));
            } else {
                ui::alert(&me.root, "some items couldn't be deleted", &errs.join("\n"));
            }
        });
    }

    pub fn properties(self: &Rc<Self>) {
        props::show(&self.root, &self.selected(), &self.cwd());
    }

    pub fn undo(self: &Rc<Self>) {
        let Some(u) = ops::pop_undo() else {
            self.flash("nothing to undo");
            return;
        };
        let me = self.clone();
        util::bg(move || ops::run_undo(u), move |r| match r {
            Ok(m) => {
                me.flash(&m);
                me.refresh();
            }
            Err(e) => ui::alert(&me.root, "couldn't undo", &e),
        });
    }
}
