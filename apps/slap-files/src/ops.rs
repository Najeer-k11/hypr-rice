//! File operations engine (no GTK widgets): copy / move with progress + cancel, trash,
//! delete, restore from trash, the undo log, and clipboard formats.
//! Blocking functions here run on worker threads.

use gtk::gio;
use gtk::prelude::*;
use std::cell::RefCell;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Copy,
    Move,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conflict {
    Replace,
    Skip,
    KeepBoth,
}

#[derive(Default, Clone)]
pub struct Progress {
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub files_total: u64,
    pub files_done: u64,
    pub current: String,
    pub scanning: bool,
}

#[derive(Clone, Default)]
pub struct Handle {
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
}

impl Handle {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn with(&self, f: impl FnOnce(&mut Progress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }
}

#[derive(Default)]
pub struct Outcome {
    /// (source, destination, destination did not exist before)
    pub done: Vec<(PathBuf, PathBuf, bool)>,
    pub errors: Vec<String>,
    pub cancelled: bool,
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

fn same_dev(a: &Path, b: &Path) -> bool {
    match (fs::symlink_metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

/// Would a drop/paste of `src` into `dest` be a plain rename (same filesystem)?
pub fn is_same_device(src: &Path, dest: &Path) -> bool {
    same_dev(src, dest)
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// "name (copy).ext" / "name (2).ext" … first free name in `dir`.
pub fn unique_name(dir: &Path, name: &str, copy: bool) -> PathBuf {
    let is_dir = is_real_dir(&dir.join(name));
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !is_dir => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1.. {
        let cand = match (copy, n) {
            (true, 1) => format!("{stem} (copy){ext}"),
            (true, n) => format!("{stem} (copy {n}){ext}"),
            (false, n) => format!("{stem} ({}){ext}", n + 1),
        };
        let p = dir.join(&cand);
        if fs::symlink_metadata(&p).is_err() {
            return p;
        }
    }
    unreachable!()
}

fn walk_size(p: &Path, h: &Handle, bytes: &mut u64, files: &mut u64) {
    if h.cancelled() {
        return;
    }
    let Ok(m) = fs::symlink_metadata(p) else { return };
    if m.is_dir() {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                walk_size(&e.path(), h, bytes, files);
            }
        }
    } else {
        *bytes += m.len();
        *files += 1;
    }
}

/// Copy or move `srcs` into the folder `dest`. Blocking.
pub fn transfer(kind: Kind, srcs: Vec<PathBuf>, dest: PathBuf, conflict: Conflict, h: &Handle) -> Outcome {
    let mut out = Outcome::default();

    // size up what has to be physically copied (renames are instant)
    h.with(|p| p.scanning = true);
    let (mut bytes, mut files) = (0, 0);
    for s in &srcs {
        if kind == Kind::Copy || !same_dev(s, &dest) {
            walk_size(s, h, &mut bytes, &mut files);
        } else {
            files += 1;
        }
    }
    h.with(|p| {
        p.scanning = false;
        p.bytes_total = bytes;
        p.files_total = files;
    });

    for s in srcs {
        if h.cancelled() {
            out.cancelled = true;
            break;
        }
        let Some(name) = s.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let same_dir = s.parent() == Some(dest.as_path());
        if kind == Kind::Move && same_dir {
            continue;
        }
        if is_real_dir(&s) && dest.starts_with(&s) {
            out.errors.push(format!("{name}: can't put a folder inside itself"));
            continue;
        }
        let mut dst = dest.join(&name);
        let mut fresh = true;
        if same_dir {
            dst = unique_name(&dest, &name, true);
        } else if fs::symlink_metadata(&dst).is_ok() {
            match conflict {
                Conflict::Skip => continue,
                Conflict::KeepBoth => dst = unique_name(&dest, &name, false),
                Conflict::Replace => fresh = false,
            }
        }
        h.with(|p| p.current = name.clone());
        let r = match kind {
            Kind::Copy => copy_tree(&s, &dst, h),
            Kind::Move => move_tree(&s, &dst, h),
        };
        match r {
            Ok(()) => out.done.push((s, dst, fresh)),
            Err(_) if h.cancelled() => {
                out.cancelled = true;
                break;
            }
            Err(e) => out.errors.push(format!("{name}: {e}")),
        }
    }
    out
}

fn remove_any(p: &Path) -> io::Result<()> {
    if is_real_dir(p) { fs::remove_dir_all(p) } else { fs::remove_file(p) }
}

fn copy_tree(src: &Path, dst: &Path, h: &Handle) -> io::Result<()> {
    if h.cancelled() {
        return Err(interrupted());
    }
    let m = fs::symlink_metadata(src)?;
    let existing = fs::symlink_metadata(dst).ok();
    if m.file_type().is_symlink() {
        if existing.is_some() {
            remove_any(dst)?;
        }
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
        h.with(|p| p.files_done += 1);
    } else if m.is_dir() {
        match existing {
            Some(e) if e.is_dir() => {} // merge into it
            Some(_) => {
                fs::remove_file(dst)?;
                fs::create_dir(dst)?;
            }
            None => fs::create_dir(dst)?,
        }
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()), h)?;
        }
        let _ = fs::set_permissions(dst, m.permissions());
    } else {
        if existing.is_some_and(|e| e.is_dir()) {
            fs::remove_dir_all(dst)?;
        }
        h.with(|p| p.current = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        if let Err(e) = copy_file(src, dst, &m, h) {
            let _ = fs::remove_file(dst); // no half-written files
            return Err(e);
        }
        h.with(|p| p.files_done += 1);
    }
    Ok(())
}

extern "C" {
    fn copy_file_range(fd_in: i32, off_in: *mut i64, fd_out: i32, off_out: *mut i64, len: usize, flags: u32) -> isize;
}

/// Kernel-side copy (reflinks on btrfs/xfs, no userspace buffer), in chunks so progress
/// and cancel stay responsive. Falls back to a small read/write loop.
fn copy_file(src: &Path, dst: &Path, m: &fs::Metadata, h: &Handle) -> io::Result<()> {
    let mut fin = fs::File::open(src)?;
    let mut fout = fs::File::create(dst)?;
    let mut kernel = true;
    loop {
        if h.cancelled() {
            return Err(interrupted());
        }
        let n = if kernel {
            // SAFETY: valid open fds; null offsets mean "use and advance the file positions".
            let r = unsafe { copy_file_range(fin.as_raw_fd(), std::ptr::null_mut(), fout.as_raw_fd(), std::ptr::null_mut(), 16 << 20, 0) };
            if r < 0 {
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    // not supported here: fall back (nothing was written by the failed call)
                    Some(18) | Some(22) | Some(38) | Some(95) | Some(1) => {
                        kernel = false;
                        continue;
                    }
                    _ => return Err(e),
                }
            }
            r as u64
        } else {
            let mut buf = [0u8; 256 * 1024];
            let r = fin.read(&mut buf)?;
            fout.write_all(&buf[..r])?;
            r as u64
        };
        if n == 0 {
            break;
        }
        h.with(|p| p.bytes_done += n);
    }
    let _ = fout.set_permissions(m.permissions());
    if let Ok(t) = m.modified() {
        let _ = fout.set_times(fs::FileTimes::new().set_modified(t));
    }
    Ok(())
}

fn move_tree(src: &Path, dst: &Path, h: &Handle) -> io::Result<()> {
    if h.cancelled() {
        return Err(interrupted());
    }
    if let Ok(dm) = fs::symlink_metadata(dst) {
        if dm.is_dir() && is_real_dir(src) {
            // replace = merge folders
            for e in fs::read_dir(src)? {
                let e = e?;
                move_tree(&e.path(), &dst.join(e.file_name()), h)?;
            }
            return fs::remove_dir(src);
        }
        remove_any(dst)?;
    }
    match fs::rename(src, dst) {
        Ok(()) => {
            h.with(|p| p.files_done += 1);
            Ok(())
        }
        Err(e) if e.raw_os_error() == Some(18) => {
            // EXDEV: other filesystem -> copy then delete
            copy_tree(src, dst, h)?;
            remove_any(src)
        }
        Err(e) => Err(e),
    }
}

/// Move to trash. Returns the paths that made it and error messages for the rest.
pub fn trash(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let (mut ok, mut errs) = (Vec::new(), Vec::new());
    for p in paths {
        match gio::File::for_path(p).trash(gio::Cancellable::NONE) {
            Ok(()) => ok.push(p.clone()),
            Err(e) => errs.push(format!("{}: {}", name_of(p), e.message())),
        }
    }
    (ok, errs)
}

pub fn delete(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|p| remove_any(p).err().map(|e| format!("{}: {e}", name_of(p))))
        .collect()
}

pub fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Put the most recently trashed copy of `original` back (any trash, incl. drives').
pub fn restore(original: &Path) -> Result<PathBuf, String> {
    let e = crate::trash::find(original).ok_or_else(|| format!("{} isn't in the trash", name_of(original)))?;
    crate::trash::restore(&e)
}

// ------------------------------------------------------------ running jobs --
// Paths that running copies/moves/archive jobs read or write. Eject refuses a drive while
// any of these lie on it.

type Job = (u64, Vec<PathBuf>, Arc<AtomicBool>);

thread_local! {
    static ACTIVE: RefCell<Vec<Job>> = const { RefCell::new(Vec::new()) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// Register a job's paths (and its cancel flag); drop the returned guard when it ends.
pub fn register(paths: Vec<PathBuf>, cancel: Arc<AtomicBool>) -> ActiveGuard {
    let id = NEXT.with(|n| {
        let v = n.get();
        n.set(v + 1);
        v
    });
    ACTIVE.with(|a| a.borrow_mut().push((id, paths, cancel)));
    ActiveGuard(id)
}

pub struct ActiveGuard(u64);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let id = self.0;
        ACTIVE.with(|a| a.borrow_mut().retain(|(i, _, _)| *i != id));
    }
}

/// Is any running job touching something under `root`?
pub fn busy_under(root: &Path) -> bool {
    ACTIVE.with(|a| a.borrow().iter().any(|(_, ps, _)| ps.iter().any(|p| p.starts_with(root))))
}

pub fn active_count() -> usize {
    ACTIVE.with(|a| a.borrow().len())
}

/// Ask every running job to stop (each cleans up its partial output).
pub fn cancel_all() {
    ACTIVE.with(|a| a.borrow().iter().for_each(|(_, _, c)| c.store(true, Ordering::Relaxed)));
}

/// Run an external command; kill it if `cancel` is set. Ok(()) on exit status 0.
pub fn run_cmd(mut cmd: std::process::Command, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(st)) => {
                if st.success() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                let last = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("failed").trim().to_string();
                return Err(last);
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(80)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

// ------------------------------------------------------------------- undo --

pub enum Undo {
    Rename { from: PathBuf, to: PathBuf },
    Move(Vec<(PathBuf, PathBuf)>),
    Copy(Vec<PathBuf>),
    Trash(Vec<PathBuf>),
    Create(PathBuf),
}

thread_local! {
    static UNDO: RefCell<Vec<Undo>> = const { RefCell::new(Vec::new()) };
}

pub fn push_undo(u: Undo) {
    UNDO.with(|s| {
        let mut s = s.borrow_mut();
        s.push(u);
        if s.len() > 50 {
            s.remove(0);
        }
    });
}

pub fn can_undo() -> bool {
    UNDO.with(|s| !s.borrow().is_empty())
}

pub fn pop_undo() -> Option<Undo> {
    UNDO.with(|s| s.borrow_mut().pop())
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// Reverse an operation. Blocking. Ok(message) or Err(message).
pub fn run_undo(u: Undo) -> Result<String, String> {
    match u {
        Undo::Rename { from, to } => {
            if fs::symlink_metadata(&from).is_ok() {
                return Err(format!("{} already exists", name_of(&from)));
            }
            fs::rename(&to, &from).map_err(|e| e.to_string())?;
            Ok(format!("undid rename of {}", name_of(&from)))
        }
        Undo::Move(pairs) => {
            let h = Handle::default();
            let mut errs = Vec::new();
            for (src, dst) in pairs.iter().rev() {
                if let Some(p) = src.parent() {
                    let _ = fs::create_dir_all(p);
                }
                if let Err(e) = move_tree(dst, src, &h) {
                    errs.push(format!("{}: {e}", name_of(dst)));
                }
            }
            if errs.is_empty() { Ok(format!("undid move of {}", plural(pairs.len(), "item"))) } else { Err(errs.join("\n")) }
        }
        Undo::Copy(created) => {
            let (ok, errs) = trash(&created);
            if errs.is_empty() { Ok(format!("undid copy · {} moved to trash", plural(ok.len(), "item"))) } else { Err(errs.join("\n")) }
        }
        Undo::Trash(paths) => {
            let errs: Vec<String> = paths.iter().filter_map(|p| restore(p).err()).collect();
            if errs.is_empty() { Ok(format!("restored {}", plural(paths.len(), "item"))) } else { Err(errs.join("\n")) }
        }
        Undo::Create(p) => {
            let (_, errs) = trash(std::slice::from_ref(&p));
            if errs.is_empty() { Ok(format!("undid new {}", name_of(&p))) } else { Err(errs.join("\n")) }
        }
    }
}

// -------------------------------------------------------------- clipboard --

pub const MIME_GNOME: &str = "x-special/gnome-copied-files";
pub const MIME_URIS: &str = "text/uri-list";

pub fn uri_of(p: &Path) -> String {
    gio::File::for_path(p).uri().to_string()
}

/// Parse clipboard text in either format -> (is cut, paths).
pub fn parse_clip(text: &str, mime: &str) -> (bool, Vec<PathBuf>) {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).peekable();
    let mut cut = false;
    if mime == MIME_GNOME {
        match lines.peek() {
            Some(&"cut") => {
                cut = true;
                lines.next();
            }
            Some(&"copy") => {
                lines.next();
            }
            _ => {}
        }
    }
    let paths = lines
        .filter_map(|l| if l.starts_with('/') { Some(PathBuf::from(l)) } else { gio::File::for_uri(l).path() })
        .collect();
    (cut, paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("slap-ops-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn copy_move_conflicts() {
        let d = tmp("cm");
        let (a, b) = (d.join("a"), d.join("b"));
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("sub/x.txt"), b"hello").unwrap();
        fs::write(a.join("f.txt"), vec![7u8; 3 << 20]).unwrap();
        let h = Handle::default();

        let out = transfer(Kind::Copy, vec![a.clone()], b.clone(), Conflict::Skip, &h);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(fs::read(b.join("a/sub/x.txt")).unwrap(), b"hello");
        assert_eq!(fs::metadata(b.join("a/f.txt")).unwrap().len(), 3 << 20);
        assert_eq!(h.progress.lock().unwrap().bytes_done, (3 << 20) + 5);

        // again: keep both -> "a (2)"; duplicate in place -> "f (copy).txt"
        transfer(Kind::Copy, vec![a.clone()], b.clone(), Conflict::KeepBoth, &h);
        assert!(b.join("a (2)/sub/x.txt").exists());
        transfer(Kind::Copy, vec![a.join("f.txt")], a.clone(), Conflict::Skip, &h);
        assert!(a.join("f (copy).txt").exists());

        // a folder into itself is refused
        let out = transfer(Kind::Copy, vec![a.clone()], a.join("sub"), Conflict::Skip, &h);
        assert_eq!(out.errors.len(), 1);

        // move with replace merges folders
        fs::write(a.join("sub/y.txt"), b"y").unwrap();
        let out = transfer(Kind::Move, vec![a.clone()], b.clone(), Conflict::Replace, &h);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(!a.exists());
        assert!(b.join("a/sub/y.txt").exists() && b.join("a/sub/x.txt").exists());

        // undo the move
        run_undo(Undo::Move(out.done.iter().map(|(s, d, _)| (s.clone(), d.clone())).collect())).unwrap();
        assert!(a.join("sub/y.txt").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn job_registry_blocks_eject() {
        let c = Arc::new(AtomicBool::new(false));
        let g = register(vec![PathBuf::from("/home/u/a.txt"), PathBuf::from("/run/media/u/BIG")], c.clone());
        assert!(busy_under(Path::new("/run/media/u/BIG")));
        assert!(!busy_under(Path::new("/run/media/u/OTHER")));
        cancel_all();
        assert!(c.load(Ordering::Relaxed));
        drop(g);
        assert!(!busy_under(Path::new("/run/media/u/BIG")));
        assert_eq!(active_count(), 0);
    }

    #[test]
    fn clipboard_formats() {
        let (cut, p) = parse_clip("cut\nfile:///tmp/a%20b\nfile:///etc", MIME_GNOME);
        assert!(cut);
        assert_eq!(p, vec![PathBuf::from("/tmp/a b"), PathBuf::from("/etc")]);
        let (cut, p) = parse_clip("# comment\r\nfile:///x\r\n", MIME_URIS);
        assert!(!cut);
        assert_eq!(p, vec![PathBuf::from("/x")]);
    }
}
