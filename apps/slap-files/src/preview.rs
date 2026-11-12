//! Preview pane (alt+p): image (decoded downscaled, off the UI thread), start of text files,
//! folder item count, or details. Holds at most one small texture, freed on every change.

use crate::item::Item;
use crate::{ui, util};
use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use std::cell::Cell;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;

const IMG_MAX: i32 = 560; // decoded size cap (pane is ~300 px; covers 2x scaling)
const TEXT_MAX: usize = 12 * 1024;

pub struct Pane {
    pub root: gtk::ScrolledWindow,
    body: gtk::Box,
    /// bumped on every change; async results compare against it and drop themselves if stale
    gen: Rc<Cell<u64>>,
}

fn is_text(it: &Item) -> bool {
    matches!(it.tint, "t-code" | "t-doc" | "t-exec")
        && !matches!(
            it.path().extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("pdf" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" | "ttf" | "otf" | "woff" | "woff2" | "torrent")
        )
}

impl Pane {
    pub fn new() -> Rc<Pane> {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        body.add_css_class("preview");
        body.set_vexpand(true);
        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&body));
        root.set_size_request(240, -1);
        // background on the scroller, not the content box: with the cairo renderer a
        // shrinking content box left stale pixels below it
        root.add_css_class("preview-wrap");
        Rc::new(Pane { root, body, gen: Rc::default() })
    }

    fn head(&self, icon: &str, tint: &str, name: &str, meta: &[String]) {
        let ic = ui::label(icon, &["preview-icon", tint]);
        ic.set_xalign(0.5);
        ic.set_margin_top(18);
        ic.set_margin_bottom(6);
        self.body.append(&ic);
        self.name_meta(name, meta);
    }

    fn name_meta(&self, name: &str, meta: &[String]) {
        let n = ui::label(name, &["preview-name"]);
        n.set_wrap(true);
        n.set_wrap_mode(pango::WrapMode::WordChar);
        n.set_selectable(true);
        self.body.append(&n);
        for m in meta {
            let l = ui::label(m, &["preview-meta"]);
            l.set_wrap(true);
            self.body.append(&l);
        }
    }

    /// Show the selection. Cheap to call often: stale async results are dropped.
    pub fn show(&self, items: &[Item]) {
        let gen = self.gen.get() + 1;
        self.gen.set(gen);
        ui::clear(&self.body); // drops any previous texture
        match items {
            [] => {
                let l = ui::label("select a file to preview it", &["preview-meta"]);
                l.set_xalign(0.5);
                l.set_vexpand(true);
                l.set_valign(gtk::Align::Center);
                self.body.append(&l);
            }
            [it] => self.one(it, gen),
            many => {
                let bytes: u64 = many.iter().filter(|i| !i.is_dir).map(|i| i.size).sum();
                let dirs = many.iter().filter(|i| i.is_dir).count();
                let mut meta = vec![format!("{} files  ·  {} folders", many.len() - dirs, dirs)];
                if bytes > 0 {
                    meta.push(util::human_bytes(bytes));
                }
                self.head("\u{f0222}", "t-file", &format!("{} items selected", many.len()), &meta);
            }
        }
    }

    fn one(&self, it: &Item, gen: u64) {
        let mut meta = vec![it.kind.to_string()];
        if !it.is_dir {
            meta.push(it.size_s());
        }
        meta.push(format!("modified {}", it.date_s()));
        let path = it.path();

        if it.tint == "t-image" && it.size < 80 << 20 {
            let pic = gtk::Picture::new();
            pic.add_css_class("preview-pic");
            pic.set_can_shrink(true);
            pic.set_content_fit(gtk::ContentFit::Contain);
            pic.set_size_request(-1, 200);
            self.body.append(&pic);
            self.name_meta(&it.name, &meta);
            let weak_pic = pic.downgrade();
            let current = self.gen.clone();
            util::bg(move || decode(&path), move |res| {
                if current.get() != gen {
                    return;
                }
                let (Some(pic), Some((w, h, alpha, stride, bytes))) = (weak_pic.upgrade(), res) else { return };
                let fmt = if alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
                let tex = gdk::MemoryTexture::new(w, h, fmt, &bytes, stride);
                pic.set_paintable(Some(&tex));
            });
            return;
        }

        if it.is_dir {
            self.head(it.icon, it.tint, &it.name, &meta);
            let l = ui::label("counting…", &["preview-meta"]);
            self.body.append(&l);
            let current = self.gen.clone();
            let weak = l.downgrade();
            util::bg(
                move || std::fs::read_dir(&path).map(|rd| rd.count()).ok(),
                move |n| {
                    if current.get() != gen {
                        return;
                    }
                    if let Some(l) = weak.upgrade() {
                        l.set_text(&match n {
                            Some(1) => "1 item".to_string(),
                            Some(n) => format!("{n} items"),
                            None => "can't open this folder".to_string(),
                        });
                    }
                },
            );
            return;
        }

        self.head(it.icon, it.tint, &it.name, &meta);
        if is_text(it) && it.size > 0 {
            let lbl = ui::label("", &["preview-text"]);
            lbl.set_wrap(true);
            lbl.set_wrap_mode(pango::WrapMode::Char);
            lbl.set_yalign(0.0);
            lbl.set_selectable(true);
            lbl.set_margin_top(8);
            self.body.append(&lbl);
            let current = self.gen.clone();
            let weak = lbl.downgrade();
            util::bg(move || head_text(&path), move |t| {
                if current.get() != gen {
                    return;
                }
                if let (Some(l), Some(t)) = (weak.upgrade(), t) {
                    l.set_text(&t);
                }
            });
        }
    }
}

fn head_text(p: &PathBuf) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; TEXT_MAX];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    if buf.contains(&0) {
        return None; // binary
    }
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    let lines: Vec<&str> = s.lines().take(120).collect();
    s = lines.join("\n");
    if n == TEXT_MAX {
        s.push_str("\n…");
    }
    Some(s)
}

/// Decode at most IMG_MAX px on the long side (worker thread; only plain bytes come back).
fn decode(p: &PathBuf) -> Option<(i32, i32, bool, usize, glib::Bytes)> {
    let pb = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, IMG_MAX, IMG_MAX, true).ok()?;
    let pb = pb.apply_embedded_orientation().unwrap_or(pb);
    if pb.bits_per_sample() != 8 {
        return None;
    }
    Some((pb.width(), pb.height(), pb.has_alpha(), pb.rowstride() as usize, pb.read_pixel_bytes()))
}
