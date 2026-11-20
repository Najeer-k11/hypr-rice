//! Small helpers: paths, running commands off the UI thread, hyprctl.

use gtk::{gio, glib};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn cfg(rel: &str) -> PathBuf {
    home().join(".config").join(rel)
}

/// `~/foo` style path for display.
pub fn tilde(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let h = home().to_string_lossy().into_owned();
    match s.strip_prefix(&h) {
        Some(rest) if !h.is_empty() => format!("~{rest}"),
        _ => s,
    }
}

/// Run a command and return stdout if it succeeded. Blocking — use from worker threads
/// or for very fast commands only.
pub fn sh(prog: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(prog).args(args).stdin(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    /// Last meaningful line of output, for error toasts.
    pub fn msg(&self) -> String {
        let t = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        let line = t.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("failed");
        line.trim().trim_start_matches("Error: ").to_string()
    }
}

/// Run a command on a worker thread; `cb` runs back on the GTK thread.
pub fn run<F: FnOnce(Out) + 'static>(prog: &str, args: &[&str], cb: F) {
    let prog = prog.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    glib::spawn_future_local(async move {
        let res = gio::spawn_blocking(move || {
            Command::new(&prog).args(&args).stdin(Stdio::null()).output()
        })
        .await;
        let out = match res {
            Ok(Ok(o)) => Out {
                ok: o.status.success(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            },
            Ok(Err(e)) => Out { ok: false, stdout: String::new(), stderr: e.to_string() },
            Err(_) => Out { ok: false, stdout: String::new(), stderr: "Worker crashed".into() },
        };
        cb(out);
    });
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

pub fn notify(title: &str, body: &str) {
    spawn("notify-send", &["-a", "settings", "-t", "2500", title, body]);
}

static HYPR_TX: OnceLock<Mutex<mpsc::Sender<Vec<String>>>> = OnceLock::new();

/// hyprctl calls go through one ordered queue so rapid slider drags land in order.
pub fn hyprctl(args: &[&str]) {
    let tx = HYPR_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Vec<String>>();
        std::thread::spawn(move || {
            for a in rx {
                let _ = Command::new("hyprctl").args(&a).stdout(Stdio::null()).output();
            }
        });
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap().send(args.iter().map(|s| s.to_string()).collect());
}

/// Split one line of `nmcli -t` output (':' separated, '\:' escaped).
pub fn split_terse(line: &str) -> Vec<String> {
    let mut v = vec![String::new()];
    let mut esc = false;
    for ch in line.chars() {
        if esc {
            v.last_mut().unwrap().push(ch);
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == ':' {
            v.push(String::new());
        } else {
            v.last_mut().unwrap().push(ch);
        }
    }
    v
}

pub fn human_bytes(b: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i >= 3 { format!("{v:.1} {}", units[i]) } else { format!("{v:.0} {}", units[i]) }
}

/// "personalise" -> "Personalise": sentence case for labels that double as ids.
pub fn sentence(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
