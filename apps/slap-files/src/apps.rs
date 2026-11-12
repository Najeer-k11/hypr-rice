//! Applications: "open with" (recommended apps + a searchable picker of all apps, optionally
//! made the default) and "open in terminal". App icons are not loaded (no icon theme in RAM).

use crate::ui;
use gtk::prelude::*;
use gtk::gio;
use std::path::{Path, PathBuf};

pub fn content_type(p: &Path) -> String {
    if p.is_dir() {
        return "inode/directory".into();
    }
    gio::content_type_guess(Some(p), None::<&[u8]>).0.to_string()
}

/// Default app first, then the other recommended ones (deduplicated).
pub fn recommended(p: &Path) -> Vec<gio::AppInfo> {
    let ct = content_type(p);
    let mut v: Vec<gio::AppInfo> = Vec::new();
    if let Some(d) = gio::AppInfo::default_for_type(&ct, false) {
        v.push(d);
    }
    for a in gio::AppInfo::recommended_for_type(&ct) {
        if !v.iter().any(|x| x.id() == a.id()) {
            v.push(a);
        }
    }
    v
}

pub fn by_id(id: &str) -> Option<gio::AppInfo> {
    gio::AppInfo::all().into_iter().find(|a| a.id().as_deref() == Some(id))
}

pub fn launch(app: &gio::AppInfo, paths: &[PathBuf], w: &impl IsA<gtk::Widget>) -> Result<(), String> {
    let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
    let ctx = w.as_ref().display().app_launch_context();
    let r = if app.supports_files() || files.len() <= 1 {
        app.launch(&files, Some(&ctx))
    } else {
        // app takes one file at a time
        files.iter().try_for_each(|f| app.launch(std::slice::from_ref(f), Some(&ctx)))
    };
    r.map_err(|e| e.message().to_string())
}

/// Full picker: all apps, filter box, "always use for this kind of file".
pub fn choose(anchor: &impl IsA<gtk::Widget>, paths: Vec<PathBuf>) {
    let Some(first) = paths.first().cloned() else { return };
    let ct = content_type(&first);
    let desc = gio::content_type_get_description(&ct).to_lowercase();
    let title = match paths.as_slice() {
        [p] => format!("open '{}' with", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        many => format!("open {} items with", many.len()),
    };
    let Some(d) = ui::Dialog::new(anchor, &title, "") else { return };

    let mut apps: Vec<gio::AppInfo> = recommended(&first);
    let mut rest: Vec<gio::AppInfo> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show() && !apps.iter().any(|x| x.id() == a.id()))
        .collect();
    rest.sort_by_key(|a| a.name().to_lowercase());
    let n_rec = apps.len();
    apps.extend(rest);

    let filter = gtk::SearchEntry::new();
    filter.set_placeholder_text(Some("filter apps"));
    d.content.append(&filter);
    let list = gtk::ListBox::new();
    list.add_css_class("app-list");
    for (i, a) in apps.iter().enumerate() {
        let row = gtk::ListBoxRow::new();
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        b.append(&ui::label(if i < n_rec { "\u{f04ce}" } else { "\u{f08c6}" }, &[if i < n_rec { "accent" } else { "dim" }]));
        let name = ui::label(&a.name(), &[]);
        name.set_hexpand(true);
        b.append(&name);
        if i == 0 && n_rec > 0 {
            b.append(&ui::label("default", &["dim"]));
        }
        row.set_child(Some(&b));
        row.set_widget_name(&a.name().to_lowercase());
        list.append(&row);
    }
    if let Some(r) = list.row_at_index(0) {
        list.select_row(Some(&r));
    }
    let sc = gtk::ScrolledWindow::new();
    sc.set_hscrollbar_policy(gtk::PolicyType::Never);
    sc.set_min_content_height(260);
    sc.set_max_content_height(260);
    sc.set_child(Some(&list));
    d.content.append(&sc);
    let always = gtk::CheckButton::with_label(&format!("always use this app for {desc}"));
    d.content.append(&always);

    let l2 = list.clone();
    filter.connect_search_changed(move |f| {
        let q = f.text().to_lowercase();
        let mut i = 0;
        let mut first_vis = None;
        while let Some(r) = l2.row_at_index(i) {
            let vis = q.is_empty() || r.widget_name().contains(q.as_str());
            r.set_visible(vis);
            if vis && first_vis.is_none() {
                first_vis = Some(r);
            }
            i += 1;
        }
        l2.select_row(first_vis.as_ref());
    });

    let go = {
        let (list, apps, always, ct, anchor) = (list.clone(), apps.clone(), always.clone(), ct.clone(), anchor.as_ref().clone());
        move || -> bool {
            let Some(r) = list.selected_row() else { return false };
            let Some(app) = apps.get(r.index() as usize) else { return false };
            if always.is_active() {
                let _ = app.set_as_default_for_type(&ct);
            }
            if let Err(e) = launch(app, &paths, &anchor) {
                ui::alert(&anchor, &format!("couldn't start {}", app.name()), &e);
            }
            true
        }
    };
    let go = std::rc::Rc::new(go);
    d.button("cancel", &[], || true);
    let g = go.clone();
    d.button("open", &["suggested"], move || g());
    let (g, dw) = (go.clone(), std::rc::Rc::downgrade(&d));
    list.connect_row_activated(move |_, _| {
        if g() {
            if let Some(d) = dw.upgrade() {
                d.close();
            }
        }
    });
    let (g, dw) = (go, std::rc::Rc::downgrade(&d));
    filter.connect_activate(move |_| {
        if g() {
            if let Some(d) = dw.upgrade() {
                d.close();
            }
        }
    });
    filter.grab_focus();
}

/// $TERMINAL, else the first common terminal found on PATH.
pub fn terminal() -> Option<String> {
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.trim().is_empty() {
            return Some(t);
        }
    }
    let path = std::env::var("PATH").unwrap_or_default();
    ["kitty", "foot", "alacritty", "wezterm", "ghostty", "konsole", "gnome-terminal", "kgx", "xfce4-terminal", "xterm"]
        .into_iter()
        .find(|t| path.split(':').any(|d| Path::new(d).join(t).is_file()))
        .map(String::from)
}

pub fn open_terminal(dir: &Path) -> Result<(), String> {
    let t = terminal().ok_or("no terminal found (set $TERMINAL)")?;
    let mut parts = t.split_whitespace();
    let prog = parts.next().ok_or("bad $TERMINAL")?;
    let mut cmd = std::process::Command::new(prog);
    cmd.args(parts).current_dir(dir);
    if prog.ends_with("wezterm") {
        cmd.args(["start", "--cwd"]).arg(dir);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

