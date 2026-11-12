//! Properties (alt+enter): what Explorer's General tab shows, plus Unix permissions/owner
//! and an "allow executing" switch for files. Folder sizes are counted off-thread and the
//! count stops when the dialog closes.

use crate::item::Item;
use crate::{ui, util};
use gtk::prelude::*;
use gtk::{glib, pango};
use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

fn id_names(file: &str) -> HashMap<u32, String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?.to_string();
            let id = f.nth(1)?.parse().ok()?;
            Some((id, name))
        })
        .collect()
}

fn when(t: std::io::Result<SystemTime>) -> String {
    t.ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| util::fmt_time(d.as_secs() as i64))
        .unwrap_or_else(|| "—".into())
}

pub fn rwx(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    format!("{s}  ({:03o})", mode & 0o777)
}

/// Bytes, files, folders under `p` (no symlink following). Stops early on cancel.
pub fn tree_size(p: &Path, cancel: &AtomicBool) -> (u64, u64, u64) {
    let (mut bytes, mut files, mut dirs) = (0, 0, 0);
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                dirs += 1;
                stack.push(e.path());
            } else {
                files += 1;
                bytes += m.len();
            }
        }
    }
    (bytes, files, dirs)
}

fn grid_row(grid: &gtk::Grid, row: i32, key: &str, val: &str) -> gtk::Label {
    let k = ui::label(key, &["prop-key"]);
    k.set_valign(gtk::Align::Start);
    let v = ui::label(val, &["prop-val"]);
    v.set_wrap(true);
    v.set_wrap_mode(pango::WrapMode::Char);
    v.set_selectable(true);
    v.set_hexpand(true);
    grid.attach(&k, 0, row, 1, 1);
    grid.attach(&v, 1, row, 1, 1);
    v
}

pub fn show(anchor: &impl IsA<gtk::Widget>, items: &[Item], cwd: &Path) {
    let (title, icon, tint) = match items {
        [it] => (it.name.clone(), it.icon, it.tint),
        [] => (crate::browser::display_name(cwd), "\u{f024b}", "t-folder"),
        many => (format!("{} items", many.len()), "\u{f0222}", "t-file"),
    };
    let Some(d) = ui::Dialog::new(anchor, "properties", "") else { return };
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let ic = ui::label(icon, &["prop-head-icon", tint]);
    head.append(&ic);
    let t = ui::label(&title, &["dialog-title"]);
    t.set_wrap(true);
    t.set_wrap_mode(pango::WrapMode::WordChar);
    t.set_valign(gtk::Align::Center);
    t.set_hexpand(true);
    head.append(&t);
    d.content.append(&head);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(7);
    grid.set_margin_top(6);
    d.content.append(&grid);
    let mut r = 0;
    let mut add = |k: &str, v: &str| {
        let l = grid_row(&grid, r, k, v);
        r += 1;
        l
    };

    let paths: Vec<PathBuf> = if items.is_empty() { vec![cwd.to_path_buf()] } else { items.iter().map(Item::path).collect() };
    let single = paths.len() == 1;
    let p0 = paths[0].clone();
    let lm = std::fs::symlink_metadata(&p0).ok();
    let m = std::fs::metadata(&p0).ok().or(lm.clone());

    if single {
        let kind = items.first().map(|i| i.kind.to_string()).unwrap_or_else(|| "folder".into());
        add("type", &kind);
        add("location", &util::tilde(p0.parent().unwrap_or(Path::new("/"))));
        if lm.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
            if let Ok(t) = std::fs::read_link(&p0) {
                add("links to", &t.to_string_lossy());
            }
        }
    } else {
        add("location", &util::tilde(cwd));
    }

    // size (counted in the background for folders / multiple items)
    let any_dir = paths.iter().any(|p| p.is_dir());
    let size_l = add("size", "counting…");
    let contains_l = if any_dir { Some(add("contains", "counting…")) } else { None };
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let c = cancel.clone();
        d.connect_close(move || c.store(true, Ordering::Relaxed));
        let (sl, cl) = (size_l.downgrade(), contains_l.as_ref().map(|l| l.downgrade()));
        let ps = paths.clone();
        util::bg(
            move || {
                let (mut b, mut f, mut dd) = (0, 0, 0);
                for p in &ps {
                    match std::fs::symlink_metadata(p) {
                        Ok(m) if m.is_dir() => {
                            let (x, y, z) = tree_size(p, &cancel);
                            b += x;
                            f += y;
                            dd += z;
                        }
                        Ok(m) => {
                            b += m.len();
                            f += 1;
                        }
                        Err(_) => {}
                    }
                }
                (b, f, dd)
            },
            move |(b, f, dd)| {
                if let Some(l) = sl.upgrade() {
                    let exact = if b >= 1024 { format!("  ({} bytes)", group(b)) } else { String::new() };
                    l.set_text(&format!("{}{exact}", util::human_bytes(b)));
                }
                if let Some(l) = cl.and_then(|w| w.upgrade()) {
                    l.set_text(&format!("{} files, {} folders", group(f), group(dd)));
                }
            },
        );
    }

    if single {
        if let Some(m) = &m {
            add("created", &when(m.created()));
            add("modified", &when(m.modified()));
            add("accessed", &when(m.accessed()));
            let users = id_names("/etc/passwd");
            let groups = id_names("/etc/group");
            let u = users.get(&m.uid()).cloned().unwrap_or_else(|| m.uid().to_string());
            let g = groups.get(&m.gid()).cloned().unwrap_or_else(|| m.gid().to_string());
            add("owner", &format!("{u}  ·  group {g}"));
            let perm_l = add("permissions", &rwx(m.mode()));

            // "allow executing file as program" for regular files you own
            if m.is_file() && m.uid() == my_uid() {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.set_margin_top(8);
                let l = ui::label("allow executing as a program", &["prop-val"]);
                l.set_hexpand(true);
                let sw = gtk::Switch::new();
                sw.set_active(m.mode() & 0o111 != 0);
                sw.set_valign(gtk::Align::Center);
                let p = p0.clone();
                sw.connect_state_set(move |_, on| {
                    if let Ok(md) = std::fs::metadata(&p) {
                        let mode = md.mode() & 0o7777;
                        // add x wherever r is set (like chmod +x respecting who can read)
                        let new = if on { mode | ((mode & 0o444) >> 2) } else { mode & !0o111 };
                        if std::fs::set_permissions(&p, std::fs::Permissions::from_mode(new)).is_ok() {
                            perm_l.set_text(&rwx(new));
                        }
                    }
                    glib::Propagation::Proceed
                });
                row.append(&l);
                row.append(&sw);
                d.content.append(&row);
            }
        }
    }
    d.button("close", &["suggested"], || true).grab_focus();
}

fn my_uid() -> u32 {
    // current user's uid without libc: the owner of our own /proc entry
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX)
}

/// 1234567 -> "1,234,567"
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats() {
        assert_eq!(super::rwx(0o100755), "rwxr-xr-x  (755)");
        assert_eq!(super::group(1234567), "1,234,567");
        assert_eq!(super::group(12), "12");
    }
}
