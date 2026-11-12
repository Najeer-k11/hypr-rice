//! Small helpers: paths, running work off the UI thread, formatting.
//! (Taken from rice-settings and trimmed to what the file manager needs.)

use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

pub fn cfg(rel: &str) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
        .join(rel)
}

/// `~/foo` style path for display.
pub fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let h = home().to_string_lossy().into_owned();
    match s.strip_prefix(&h) {
        Some(rest) if !h.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => s,
    }
}

/// Expand a typed path: `~`, `~/x`, `file://` URIs, relative to `base`.
pub fn expand(input: &str, base: &Path) -> PathBuf {
    let t = input.trim();
    if t.starts_with("file://") {
        if let Some(p) = gio::File::for_uri(t).path() {
            return p;
        }
    }
    let p = if t == "~" {
        home()
    } else if let Some(rest) = t.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(t)
    };
    if p.is_absolute() { p } else { base.join(p) }
}

/// Run a command and return stdout if it succeeded. Blocking: fast commands only.
pub fn sh(prog: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(prog).args(args).stdin(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Run a closure on a worker thread, deliver its result on the GTK thread.
pub fn bg<T, W, F>(work: W, cb: F)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    F: FnOnce(T) + 'static,
{
    glib::spawn_future_local(async move {
        if let Ok(v) = gio::spawn_blocking(work).await {
            cb(v);
        }
    });
}

/// Fire and forget (reaped by a tiny thread so no zombies are left behind).
#[allow(dead_code)]
pub fn spawn(prog: &str, args: &[&str]) {
    if let Ok(mut c) = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

pub fn human_bytes(b: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else if v < 10.0 { format!("{v:.1} {}", units[i]) } else { format!("{v:.0} {}", units[i]) }
}

pub fn fmt_time(unix: i64) -> String {
    glib::DateTime::from_unix_local(unix)
        .ok()
        .and_then(|d| d.format("%Y-%m-%d  %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
    fn mallopt(param: i32, value: i32) -> i32;
}

/// glibc gives every thread its own heap arena (up to 8 per core), and memory freed in
/// one stays parked there. Two arenas are plenty for this app's few worker threads.
pub fn limit_arenas() {
    const M_ARENA_MAX: i32 = -8;
    // SAFETY: plain glibc tuning call, made before any worker thread exists.
    unsafe {
        mallopt(M_ARENA_MAX, 2);
    }
}

/// Hand freed heap pages back to the OS (glibc keeps them otherwise), e.g. after leaving
/// a folder with tens of thousands of files.
pub fn trim_heap() {
    // SAFETY: glibc's malloc_trim only releases unused pages; it has no preconditions.
    unsafe {
        malloc_trim(0);
    }
}
