//! Wallpaper grid. Picking one sets it and regenerates the matugen palette, so this window
//! (and every app with a matugen template) recolours a moment later.
//! Uses the rice's `wall` script when present; otherwise does it itself with
//! awww / swww / hyprpaper + matugen.

use crate::{deps, theme, ui, util};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib, pango};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// small cached thumbnails: ~230 KB of texture each instead of tens of MB per full image
const TW: i32 = 320;
const TH: i32 = 180;

pub fn wall_dir() -> PathBuf {
    std::env::var_os("WALL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| util::home().join("Pictures/Wallpapers"))
}

pub fn current() -> String {
    fs::read_to_string(state_file())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn list(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 2 {
                    walk(&p, depth + 1, out);
                }
                continue;
            }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
                out.push(fs::canonicalize(&p).unwrap_or(p));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 1, &mut out);
    out.sort();
    out.dedup();
    out
}

fn state_file() -> PathBuf {
    util::home().join(".cache/rice/current_wallpaper")
}

fn running(name: &str) -> bool {
    util::sh("pgrep", &["-x", name]).is_some()
}

/// Set the wallpaper + palette without the `wall` script. Blocking: call from a worker.
fn apply_builtin(img: &str) -> Result<(), String> {
    use std::process::Command;
    let ok = |c: &mut Command| c.status().map(|s| s.success()).unwrap_or(false);
    let set = if deps::have("awww") || deps::have("swww") {
        let (cli, daemon) = if deps::have("awww") { ("awww", "awww-daemon") } else { ("swww", "swww-daemon") };
        if !running(daemon) {
            let _ = Command::new(daemon).spawn();
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        ok(Command::new(cli).args(["img", img, "--transition-type", "grow", "--transition-fps", "60"]))
    } else if deps::have("hyprpaper") {
        if !running("hyprpaper") {
            let _ = Command::new("hyprpaper").spawn();
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        ok(Command::new("hyprctl").args(["hyprpaper", "reload", &format!(",{img}")]))
    } else {
        return Err("no wallpaper tool (awww / swww / hyprpaper)".into());
    };
    if !set {
        return Err("couldn't set the wallpaper".into());
    }
    let _ = fs::create_dir_all(state_file().parent().unwrap());
    let _ = fs::write(state_file(), format!("{img}\n"));
    if deps::have("matugen") {
        let o = super::colours::read_opts();
        let c = format!("{:.1}", o.contrast);
        let scheme = format!("scheme-{}", o.scheme);
        // runs the user's own matugen templates too, and gives us the palette as JSON
        let out = Command::new("matugen")
            .args(["image", img, "-m", &o.mode, "-t", &scheme, "--contrast", &c, "--source-color-index", "0", "--json", "hex"])
            .output()
            .map_err(|e| e.to_string())?;
        if !theme::write_own_palette(&String::from_utf8_lossy(&out.stdout)) {
            return Err("matugen gave no palette".into());
        }
    }
    Ok(())
}

/// Apply a wallpaper by whichever route this system has; `done` gets an error message on failure.
pub fn apply(img: &str, done: impl FnOnce(Option<String>) + 'static) {
    if deps::have("wall") {
        util::run("wall", &["set", img], move |o| done((!o.ok).then(|| o.msg())));
    } else {
        let img = img.to_string();
        util::bg(move || apply_builtin(&img).err(), done);
    }
}

fn thumb(src: &Path) -> Option<PathBuf> {
    let md = fs::metadata(src).ok()?;
    let mut h = DefaultHasher::new();
    src.hash(&mut h);
    md.len().hash(&mut h);
    md.modified().ok()?.hash(&mut h);
    (TW, TH).hash(&mut h);
    let out = util::home().join(format!(".cache/rice/settings-thumbs/{:016x}.jpg", h.finish()));
    if out.exists() {
        return Some(out);
    }
    fs::create_dir_all(out.parent()?).ok()?;
    let (_, w, hh) = Pixbuf::file_info(src)?;
    let s = (TW as f64 / w as f64).max(TH as f64 / hh as f64);
    let (sw, sh) = (((w as f64) * s).ceil() as i32, ((hh as f64) * s).ceil() as i32);
    // decodes at reduced size (libjpeg scaling), so this stays cheap
    let pb = Pixbuf::from_file_at_scale(src, sw.max(TW), sh.max(TH), false).ok()?;
    let x = ((pb.width() - TW) / 2).max(0);
    let y = ((pb.height() - TH) / 2).max(0);
    let sub = pb.new_subpixbuf(x, y, TW.min(pb.width()), TH.min(pb.height()));
    sub.savev(&out, "jpeg", &[("quality", "86")]).ok()?;
    Some(out)
}

fn mark_current(flow: &gtk::FlowBox) {
    let cur = current();
    let mut c = flow.first_child();
    while let Some(w) = c {
        if w.widget_name() == cur.as_str() {
            w.add_css_class("current");
        } else {
            w.remove_css_class("current");
        }
        c = w.next_sibling();
    }
}

fn subtitle(n: usize, dir: &Path) -> String {
    let cur = current();
    let name = Path::new(&cur).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{n} in {} · current: {}", util::tilde(dir), if name.is_empty() { "none" } else { &name })
}

pub fn build() -> gtk::Widget {
    let dir = wall_dir();
    let files = list(&dir);
    let p = ui::page("󰸉", "wallpaper", &subtitle(files.len(), &dir));

    let random = ui::button("󰒝  random", &[]);
    let pool = files.clone();
    random.connect_clicked(move |_| {
        // tiny xorshift on the clock: no rand crate needed
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7) as usize;
        if let Some(f) = pool.get(n % pool.len().max(1)) {
            apply(&f.to_string_lossy(), |e| if let Some(e) = e { util::notify("wallpaper", &e) });
        }
    });
    let folder = ui::button("\u{f07b}  folder", &["flat"]);
    let d = dir.clone();
    folder.connect_clicked(move |_| util::spawn("xdg-open", &[&d.to_string_lossy()]));
    p.actions.append(&folder);
    p.actions.append(&random);

    if !deps::have("wall") {
        if let Some(b) = deps::banner(&[&deps::WALLPAPER, &deps::MATUGEN], || crate::reload_page("wallpaper")) {
            p.body.append(&b);
        }
    }
    p.body.append(&ui::label(
        "colours across the desktop are generated from the wallpaper you pick",
        &["status"],
    ));

    if files.is_empty() {
        let c = ui::card();
        c.append(&ui::label(&format!("no images in {}", util::tilde(&dir)), &["empty"]));
        p.body.append(&c);
        return p.root.upcast();
    }

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_activate_on_single_click(true);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(2);
    flow.set_max_children_per_line(8);
    flow.set_column_spacing(12);
    flow.set_row_spacing(14);
    flow.set_valign(gtk::Align::Start);

    let mut jobs = Vec::with_capacity(files.len());
    for f in &files {
        let child = gtk::FlowBoxChild::new();
        child.set_widget_name(&f.to_string_lossy());
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        // the picture sits in an overlay so its texture size never drives the grid layout
        let frame = gtk::Overlay::new();
        frame.add_css_class("wall-frame");
        frame.set_overflow(gtk::Overflow::Hidden);
        let sizer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sizer.set_size_request(168, 100);
        frame.set_child(Some(&sizer));
        let pic = gtk::Picture::new();
        pic.set_content_fit(gtk::ContentFit::Cover);
        pic.set_can_shrink(true);
        frame.add_overlay(&pic);
        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let l = ui::label(&name, &["wall-name"]);
        l.set_xalign(0.5);
        l.set_ellipsize(pango::EllipsizeMode::End);
        l.set_max_width_chars(14);
        v.append(&frame);
        v.append(&l);
        child.set_child(Some(&v));
        child.set_tooltip_text(Some(&util::tilde(f)));
        flow.append(&child);
        jobs.push((f.clone(), pic));
    }
    mark_current(&flow);

    flow.connect_child_activated(|flow, child| {
        let path = child.widget_name().to_string();
        let mut c = flow.first_child();
        while let Some(w) = c {
            w.remove_css_class("current");
            c = w.next_sibling();
        }
        child.add_css_class("current");
        apply(&path, |e| {
            if let Some(e) = e {
                util::notify("wallpaper failed", &e);
            }
        });
    });

    // palette changed => wallpaper changed (maybe from SUPER+W): refresh highlight + subtitle
    let fw = flow.downgrade();
    let sub = p.sub.clone();
    let n = files.len();
    theme::on_change(move || {
        if let Some(f) = fw.upgrade() {
            mark_current(&f);
            sub.set_text(&subtitle(n, &wall_dir()));
        }
    });

    // thumbnails one at a time in the background, filled in as they arrive
    glib::spawn_future_local(async move {
        for (src, pic) in jobs {
            if let Ok(Some(t)) = gio::spawn_blocking(move || thumb(&src)).await {
                pic.set_filename(Some(&t));
            }
        }
    });

    p.body.append(&flow);
    p.root.upcast()
}
