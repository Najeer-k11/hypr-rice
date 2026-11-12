//! Thumbnails, freedesktop style: shared cache ~/.cache/thumbnails/normal/<md5(uri)>.png with
//! Thumb::URI / Thumb::MTime (reuses what other apps made, and they reuse ours).
//! Made by: gdk-pixbuf in-process for images, ffmpeg for video, pdftoppm for PDF, otherwise
//! a system .thumbnailer (glycin…). Failures are remembered in fail/slap-files/.
//!
//! RAM: two worker threads, newest request first (what's on screen now), a bounded queue,
//! and at most MEM_MAX textures (128 px ≈ 64 KB each) kept in memory.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SIZE: i32 = 128;
const MEM_MAX: usize = 96;
const QUEUE_MAX: usize = 300;
const MAX_FILE: u64 = 512 << 20; // don't decode huge files just for a thumbnail

type Raw = (i32, i32, bool, usize, glib::Bytes);
type Cb = Box<dyn FnOnce(&gdk::Texture)>;

struct Req {
    path: PathBuf,
    mtime: i64,
}

struct Queue {
    stack: Mutex<Vec<Req>>,
    cv: Condvar,
}

thread_local! {
    static MEM: RefCell<(HashMap<PathBuf, (i64, gdk::Texture)>, VecDeque<PathBuf>)> = RefCell::default();
    static PENDING: RefCell<HashMap<PathBuf, Vec<Cb>>> = RefCell::default();
    static RESULTS: RefCell<Option<mpsc::Receiver<(PathBuf, i64, Option<Raw>)>>> = const { RefCell::new(None) };
    static POLLING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static QUEUE: OnceLock<(Arc<Queue>, Mutex<mpsc::Sender<(PathBuf, i64, Option<Raw>)>>)> = OnceLock::new();

/// Worth asking for? (by content type; cheap, no disk access)
pub fn wanted(name: &str) -> bool {
    let (ct, _) = gio::content_type_guess(Some(name), None::<&[u8]>);
    let ct = ct.as_str();
    ct.starts_with("image/") || ct.starts_with("video/") || ct == "application/pdf" || thumbnailers().iter().any(|t| t.mimes.iter().any(|m| m == ct))
}

/// Get a thumbnail texture for `path`; `cb` runs on the UI thread if/when one exists.
pub fn request(path: &Path, mtime: i64, cb: impl FnOnce(&gdk::Texture) + 'static) {
    let hit = MEM.with(|m| {
        let mut m = m.borrow_mut();
        let hit = m.0.get(path).filter(|(t, _)| *t == mtime).map(|(_, tex)| tex.clone());
        if hit.is_some() {
            // most recently used goes to the back
            m.1.retain(|p| p != path);
            m.1.push_back(path.to_path_buf());
        }
        hit
    });
    if let Some(t) = hit {
        cb(&t);
        return;
    }
    let first = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let v = p.entry(path.to_path_buf()).or_default();
        v.push(Box::new(cb));
        v.len() == 1
    });
    if !first {
        return;
    }
    let (q, _) = QUEUE.get_or_init(start);
    {
        let mut st = q.stack.lock().unwrap();
        st.push(Req { path: path.to_path_buf(), mtime });
        if st.len() > QUEUE_MAX {
            // scrolled far away: forget the oldest requests
            let drop_n = st.len() - QUEUE_MAX;
            for r in st.drain(..drop_n) {
                PENDING.with(|p| p.borrow_mut().remove(&r.path));
            }
        }
    }
    q.cv.notify_one();
    poll();
}

fn start() -> (Arc<Queue>, Mutex<mpsc::Sender<(PathBuf, i64, Option<Raw>)>>) {
    let q = Arc::new(Queue { stack: Mutex::new(Vec::new()), cv: Condvar::new() });
    let (tx, rx) = mpsc::channel();
    RESULTS.with(|r| *r.borrow_mut() = Some(rx));
    for _ in 0..2 {
        let (q, tx) = (q.clone(), tx.clone());
        std::thread::spawn(move || loop {
            let req = {
                let mut st = q.stack.lock().unwrap();
                loop {
                    if let Some(r) = st.pop() {
                        break r; // LIFO: newest = currently visible
                    }
                    st = q.cv.wait(st).unwrap();
                }
            };
            let raw = make(&req.path, req.mtime);
            if tx.send((req.path, req.mtime, raw)).is_err() {
                return;
            }
        });
    }
    (q, Mutex::new(tx))
}

/// Deliver finished thumbnails on the UI thread (only polls while work is pending).
fn poll() {
    if POLLING.with(|p| p.replace(true)) {
        return;
    }
    glib::timeout_add_local(Duration::from_millis(40), || {
        RESULTS.with(|r| {
            let r = r.borrow();
            let Some(rx) = r.as_ref() else { return };
            while let Ok((path, mtime, raw)) = rx.try_recv() {
                let cbs = PENDING.with(|p| p.borrow_mut().remove(&path)).unwrap_or_default();
                let Some((w, h, alpha, stride, bytes)) = raw else { continue };
                let fmt = if alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
                let tex: gdk::Texture = gdk::MemoryTexture::new(w, h, fmt, &bytes, stride).upcast();
                MEM.with(|m| {
                    let mut m = m.borrow_mut();
                    m.0.insert(path.clone(), (mtime, tex.clone()));
                    m.1.retain(|p| *p != path);
                    m.1.push_back(path.clone());
                    while m.1.len() > MEM_MAX {
                        if let Some(old) = m.1.pop_front() {
                            m.0.remove(&old);
                        }
                    }
                });
                for cb in cbs {
                    cb(&tex);
                }
            }
        });
        let more = PENDING.with(|p| !p.borrow().is_empty());
        if !more {
            POLLING.with(|p| p.set(false));
            // decoders leave big temporary buffers behind: give them back
            glib::timeout_add_local_once(Duration::from_millis(500), crate::util::trim_heap);
        }
        if more { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
    });
}

// ------------------------------------------------------------- worker side --

fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::util::home().join(".cache"))
        .join("thumbnails")
}

fn private_dir(p: &Path) {
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p);
}

fn raw_of(pb: &gtk::gdk_pixbuf::Pixbuf) -> Option<Raw> {
    (pb.bits_per_sample() == 8).then(|| (pb.width(), pb.height(), pb.has_alpha(), pb.rowstride() as usize, pb.read_pixel_bytes()))
}

fn fit(pb: gtk::gdk_pixbuf::Pixbuf) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let (w, h) = (pb.width(), pb.height());
    if w <= SIZE && h <= SIZE {
        return Some(pb);
    }
    let s = SIZE as f64 / w.max(h) as f64;
    pb.scale_simple(((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1), gtk::gdk_pixbuf::InterpType::Bilinear)
}

/// Run a helper with a time limit (a stuck thumbnailer must not block the queue).
fn run(mut c: Command, secs: u64) -> bool {
    let Ok(mut child) = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let t = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if t.elapsed() < Duration::from_secs(secs) => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

struct Thumbnailer {
    exec: String,
    mimes: Vec<String>,
}

fn thumbnailers() -> &'static Vec<Thumbnailer> {
    static T: OnceLock<Vec<Thumbnailer>> = OnceLock::new();
    T.get_or_init(|| {
        let mut v = Vec::new();
        let dirs = ["/usr/share/thumbnailers", "/usr/local/share/thumbnailers"];
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let txt = std::fs::read_to_string(e.path()).unwrap_or_default();
                let get = |k: &str| txt.lines().find_map(|l| l.strip_prefix(k).map(|s| s.trim().to_string()));
                let (Some(exec), Some(mimes)) = (get("Exec="), get("MimeType=")) else { continue };
                if let Some(te) = get("TryExec=") {
                    if !Path::new(&te).exists() {
                        continue;
                    }
                }
                v.push(Thumbnailer { exec, mimes: mimes.split(';').filter(|m| !m.is_empty()).map(String::from).collect() });
            }
        }
        v
    })
}

/// Produce a thumbnail (from the cache if valid). Worker thread.
fn make(path: &Path, mtime: i64) -> Option<Raw> {
    let root = cache_root();
    if path.starts_with(&root) {
        return None;
    }
    let uri = gio::File::for_path(path).uri().to_string();
    let md5 = glib::compute_checksum_for_string(glib::ChecksumType::Md5, &uri)?.to_string();
    let cached = root.join("normal").join(format!("{md5}.png"));
    let failed = root.join("fail/slap-files").join(format!("{md5}.png"));
    let mt = mtime.to_string();
    let valid = |p: &Path| gtk::gdk_pixbuf::Pixbuf::from_file(p).ok().filter(|pb| pb.option("tEXt::Thumb::MTime").as_deref() == Some(mt.as_str()));
    if let Some(pb) = valid(&cached) {
        return raw_of(&pb);
    }
    if valid(&failed).is_some() {
        return None;
    }
    let made = generate(path, &uri, &root, &md5);
    let opts = [("tEXt::Thumb::URI", uri.as_str()), ("tEXt::Thumb::MTime", mt.as_str()), ("tEXt::Software", "slap-files")];
    match made {
        Some(pb) => {
            private_dir(&root.join("normal"));
            // write-then-rename so readers never see a half-written file
            let tmp = root.join("normal").join(format!(".{md5}.{}.tmp", std::process::id()));
            if pb.savev(&tmp, "png", &opts).is_ok() {
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
                let _ = std::fs::rename(&tmp, &cached);
            }
            raw_of(&pb)
        }
        None => {
            private_dir(&root.join("fail/slap-files"));
            if let Some(marker) = gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 1, 1) {
                let _ = marker.savev(&failed, "png", &opts);
            }
            None
        }
    }
}

fn generate(path: &Path, uri: &str, root: &Path, md5: &str) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    if std::fs::metadata(path).map(|m| m.len() > MAX_FILE).unwrap_or(true) {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (ct, _) = gio::content_type_guess(Some(&name), None::<&[u8]>);
    let ct = ct.to_string();
    let tmp = root.join(format!(".slap-{md5}-{}", std::process::id()));
    let out = tmp.with_extension("png");
    let from_tmp = |p: &Path| {
        let pb = gtk::gdk_pixbuf::Pixbuf::from_file(p).ok().and_then(fit);
        let _ = std::fs::remove_file(p);
        pb
    };

    // 1. images: in-process, decoded straight at thumbnail size
    if ct.starts_with("image/") {
        if let Ok(pb) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, SIZE, SIZE, true) {
            return Some(pb.apply_embedded_orientation().unwrap_or(pb));
        }
    }
    private_dir(root);
    // 2. video: a frame a few seconds in (or the first frame for short clips)
    if ct.starts_with("video/") {
        for at in ["3", "0"] {
            let mut c = Command::new("ffmpeg");
            c.args(["-v", "error", "-y", "-ss", at, "-i"]).arg(path).args(["-frames:v", "1", "-vf", "scale=128:128:force_original_aspect_ratio=decrease"]).arg(&out);
            if run(c, 15) && out.exists() {
                return from_tmp(&out);
            }
        }
    }
    // 3. pdf: first page
    if ct == "application/pdf" {
        let mut c = Command::new("pdftoppm");
        c.args(["-png", "-singlefile", "-f", "1", "-l", "1", "-scale-to", "128"]).arg(path).arg(&tmp);
        if run(c, 15) && out.exists() {
            return from_tmp(&out);
        }
    }
    // 4. system thumbnailers (glycin for heif/jxl/svg…)
    for t in thumbnailers().iter().filter(|t| t.mimes.contains(&ct)) {
        let args: Vec<String> = t
            .exec
            .split_whitespace()
            .map(|a| a.replace("%i", &path.to_string_lossy()).replace("%u", uri).replace("%o", &out.to_string_lossy()).replace("%s", &SIZE.to_string()))
            .collect();
        let Some((prog, rest)) = args.split_first() else { continue };
        let mut c = Command::new(prog);
        c.args(rest);
        if run(c, 15) && out.exists() {
            return from_tmp(&out);
        }
    }
    let _ = std::fs::remove_file(&out);
    None
}
