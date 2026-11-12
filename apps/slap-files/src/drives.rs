//! Drives via udisks2 over the system D-Bus (no gvfs needed): list volumes, mount, unlock
//! LUKS, and a careful eject:
//!   1. flush the drive's cached writes (syncfs) and report what's left
//!   2. unmount every filesystem on the drive — never forced; if busy, say who holds it
//!   3. lock unlocked encrypted containers
//!   4. power the drive off (or eject the medium)
//! Data is only "safe to remove" after step 2 succeeded for every filesystem.

use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BUS: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const IFACE_BLOCK: &str = "org.freedesktop.UDisks2.Block";
const IFACE_FS: &str = "org.freedesktop.UDisks2.Filesystem";
const IFACE_DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const IFACE_CRYPT: &str = "org.freedesktop.UDisks2.Encrypted";
const IFACE_LOOP: &str = "org.freedesktop.UDisks2.Loop";
const NO_TIMEOUT: i32 = i32::MAX; // unmounting a big dirty drive can take minutes

#[derive(Clone, Debug, Default)]
pub struct Volume {
    /// block object path (what we mount)
    pub obj: String,
    pub device: String,
    pub label: String,
    pub fstype: String,
    pub size: u64,
    pub mounts: Vec<PathBuf>,
    /// drive object path ("/" = none)
    pub drive: String,
    pub model: String,
    pub removable: bool,
    pub can_power_off: bool,
    pub ejectable: bool,
    /// encrypted container that isn't unlocked yet
    pub locked: bool,
    /// (free, total) bytes when mounted
    pub usage: Option<(u64, u64)>,
    /// a mounted disk image (loop device); `drive` is then the loop block itself
    pub backing: Option<PathBuf>,
}

impl Volume {
    pub fn title(&self) -> String {
        if !self.label.is_empty() {
            return self.label.clone();
        }
        let size = crate::util::human_bytes(self.size);
        if !self.model.is_empty() {
            format!("{size} {}", self.model.trim())
        } else {
            format!("{size} volume")
        }
    }

    pub fn mount_point(&self) -> Option<&PathBuf> {
        self.mounts.first()
    }

    /// Something you'd unplug (usb disk, sd card…), so it gets an eject button.
    pub fn is_external(&self) -> bool {
        self.backing.is_some() || (self.drive != "/" && (self.removable || self.can_power_off || self.ejectable))
    }

    pub fn icon(&self) -> &'static str {
        if self.locked {
            "\u{f033e}"
        } else if self.backing.is_some() {
            "\u{f0b48}"
        } else {
            "\u{f02ca}"
        }
    }
}

thread_local! {
    static CONN: RefCell<Option<gio::DBusConnection>> = const { RefCell::new(None) };
    static VOLS: RefCell<Vec<Volume>> = const { RefCell::new(Vec::new()) };
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DEB: crate::ui::Debounce = crate::ui::Debounce::default();
    static SUB: RefCell<Option<gio::SignalSubscription>> = const { RefCell::new(None) };
}

fn conn() -> Option<gio::DBusConnection> {
    CONN.with(|c| c.borrow().clone())
}

/// Connect once, load the list, and follow udisks signals (plug/unplug/mount/unmount).
pub fn init() {
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    let Ok(c) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) else { return };
    let sub = c.subscribe_to_signal(Some(BUS), None, None, None, None, gio::DBusSignalFlags::NONE, |_| {
        DEB.with(|d| d.run(250, refresh));
    });
    SUB.with(|x| *x.borrow_mut() = Some(sub)); // unsubscribes when dropped, so keep it
    CONN.with(|x| *x.borrow_mut() = Some(c));
    refresh();
}

pub fn volumes() -> Vec<Volume> {
    VOLS.with(|v| v.borrow().clone())
}

pub fn find(obj: &str) -> Option<Volume> {
    VOLS.with(|v| v.borrow().iter().find(|x| x.obj == obj).cloned())
}

/// The mounted volume containing `p` (deepest mount wins): (mount point, title, icon).
pub fn mount_containing(p: &Path) -> Option<(PathBuf, String, &'static str)> {
    VOLS.with(|v| {
        v.borrow()
            .iter()
            .filter_map(|x| x.mount_point().filter(|m| p.starts_with(m)).map(|m| (m.clone(), x.title(), x.icon())))
            .max_by_key(|(m, _, _)| m.as_os_str().len())
    })
}

/// Called after every refresh (sidebars, this-pc pages).
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

pub fn refresh() {
    let Some(c) = conn() else { return };
    glib::spawn_future_local(async move {
        let r = c
            .call_future(Some(BUS), ROOT, "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", None, None, gio::DBusCallFlags::NONE, 10_000)
            .await;
        let Ok(v) = r else { return };
        let mut vols = parse(&v);
        // free space: statfs off the UI thread (a sleeping USB disk can take a moment)
        let mounts: Vec<PathBuf> = vols.iter().filter_map(|v| v.mount_point().cloned()).collect();
        let usage = gio::spawn_blocking(move || mounts.iter().map(|m| (m.clone(), fs_usage(m))).collect::<Vec<_>>()).await.unwrap_or_default();
        for v in vols.iter_mut() {
            if let Some(m) = v.mount_point() {
                v.usage = usage.iter().find(|(p, _)| p == m).and_then(|(_, u)| *u);
            }
        }
        VOLS.with(|x| *x.borrow_mut() = vols);
        LISTENERS.with(|l| l.borrow().iter().for_each(|f| f()));
    });
}

/// (free, total) bytes of the filesystem holding `p`.
pub fn fs_usage(p: &Path) -> Option<(u64, u64)> {
    let info = gio::File::for_path(p)
        .query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE)
        .ok()?;
    Some((info.attribute_uint64("filesystem::free"), info.attribute_uint64("filesystem::size")))
}

// ---------------------------------------------------------------- parsing --

type Ifaces = HashMap<String, glib::VariantDict>;

fn s(d: &glib::VariantDict, k: &str) -> String {
    d.lookup_value(k, None).and_then(|v| v.str().map(String::from)).unwrap_or_default()
}
fn b(d: &glib::VariantDict, k: &str) -> bool {
    d.lookup_value(k, None).and_then(|v| v.get::<bool>()).unwrap_or(false)
}
fn t(d: &glib::VariantDict, k: &str) -> u64 {
    d.lookup_value(k, None).and_then(|v| v.get::<u64>()).unwrap_or(0)
}
fn bytes(v: &glib::Variant) -> String {
    let a = v.fixed_array::<u8>().unwrap_or(&[]);
    let a = a.strip_suffix(&[0]).unwrap_or(a);
    String::from_utf8_lossy(a).into_owned()
}
fn aay(d: &glib::VariantDict, k: &str) -> Vec<PathBuf> {
    d.lookup_value(k, None)
        .map(|v| (0..v.n_children()).map(|i| PathBuf::from(bytes(&v.child_value(i)))).collect())
        .unwrap_or_default()
}

/// Mounted where the OS itself lives? Those aren't "drives" to the user.
fn os_mount(m: &Path) -> bool {
    let s = m.to_string_lossy();
    s == "/" || ["/boot", "/efi", "/usr", "/var", "/etc", "/opt", "/srv", "/nix", "/gnu", "/snap", "/home"]
        .iter()
        .any(|p| s == *p || s.starts_with(&format!("{p}/")) && !s.starts_with("/home/"))
        || s == "/home"
}

fn parse(v: &glib::Variant) -> Vec<Volume> {
    let objs = v.child_value(0);
    let mut all: HashMap<String, Ifaces> = HashMap::new();
    for i in 0..objs.n_children() {
        let e = objs.child_value(i);
        let path = e.child_value(0).str().unwrap_or_default().to_string();
        let ifs = e.child_value(1);
        let mut m = Ifaces::new();
        for j in 0..ifs.n_children() {
            let x = ifs.child_value(j);
            let name = x.child_value(0).str().unwrap_or_default().to_string();
            m.insert(name, glib::VariantDict::new(Some(&x.child_value(1))));
        }
        all.insert(path, m);
    }
    let drive_of = |blk: &glib::VariantDict| -> String {
        let d = s(blk, "Drive");
        if d != "/" && !d.is_empty() {
            return d;
        }
        // unlocked LUKS: the cleartext block has no drive; its backing device does
        let back = s(blk, "CryptoBackingDevice");
        all.get(&back).and_then(|i| i.get(IFACE_BLOCK)).map(|bb| s(bb, "Drive")).unwrap_or_else(|| "/".into())
    };

    let mut out = Vec::new();
    for (path, ifs) in &all {
        let Some(blk) = ifs.get(IFACE_BLOCK) else { continue };
        if b(blk, "HintIgnore") {
            continue;
        }
        let usage = s(blk, "IdUsage");
        let fs = ifs.get(IFACE_FS);
        let crypt = ifs.get(IFACE_CRYPT);
        let locked = match crypt {
            Some(c) if usage == "crypto" => {
                let clear = s(c, "CleartextDevice");
                if !clear.is_empty() && clear != "/" {
                    continue; // shown through its unlocked filesystem
                }
                true
            }
            _ => false,
        };
        if fs.is_none() && !locked {
            continue;
        }
        let mounts = fs.map(|f| aay(f, "MountPoints")).unwrap_or_default();
        let lp = ifs.get(IFACE_LOOP);
        let backing = lp.map(|l| PathBuf::from(l.lookup_value("BackingFile", None).map(|v| bytes(&v)).unwrap_or_default()));
        // a disk image is its own "drive" (ejecting it detaches the loop device)
        let drive = if lp.is_some() { path.clone() } else { drive_of(blk) };
        let dr = all.get(&drive).and_then(|i| i.get(IFACE_DRIVE));
        let system = b(blk, "HintSystem");
        // disk images count as removable: udisks flags loop devices as "system"
        let removable = lp.is_some() || dr.is_some_and(|d| b(d, "Removable") || b(d, "MediaRemovable") || s(d, "ConnectionBus") == "usb");
        // hide the OS's own partitions; keep data partitions and anything external
        if mounts.iter().any(|m| os_mount(m)) {
            continue;
        }
        if system && !removable && mounts.is_empty() && (t(blk, "Size") < (1 << 30) || s(blk, "IdType") == "swap") {
            continue;
        }
        out.push(Volume {
            obj: path.clone(),
            device: bytes(&blk.lookup_value("PreferredDevice", None).or_else(|| blk.lookup_value("Device", None)).unwrap_or_else(|| Vec::<u8>::new().to_variant())),
            label: s(blk, "IdLabel"),
            fstype: s(blk, "IdType"),
            size: t(blk, "Size"),
            mounts,
            drive: drive.clone(),
            model: match &backing {
                Some(b) => format!("image {}", b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                None => dr.map(|d| format!("{} {}", s(d, "Vendor"), s(d, "Model")).trim().to_string()).unwrap_or_default(),
            },
            removable,
            can_power_off: dr.is_some_and(|d| b(d, "CanPowerOff")),
            ejectable: dr.is_some_and(|d| b(d, "Ejectable")),
            locked,
            usage: None,
            backing,
        });
    }
    out.sort_by(|a, b| (!a.is_external(), &a.device).cmp(&(!b.is_external(), &b.device)));
    out
}

// ------------------------------------------------------------------ actions --

fn opts() -> glib::Variant {
    // allow polkit to ask for a password if needed
    let d = glib::VariantDict::new(None);
    d.insert("auth.no_user_interaction", false);
    d.end()
}

/// A D-Bus argument tuple whose members are the given values (not boxed in "v").
fn args(items: &[glib::Variant]) -> glib::Variant {
    glib::Variant::tuple_from_iter(items.iter().cloned())
}

async fn call(obj: &str, iface: &str, method: &str, args: glib::Variant) -> Result<glib::Variant, String> {
    let c = conn().ok_or("udisks2 isn't available")?;
    c.call_future(Some(BUS), obj, iface, method, Some(&args), None, gio::DBusCallFlags::NONE, NO_TIMEOUT)
        .await
        .map_err(|e| {
            let m = e.message().to_string();
            // "GDBus.Error:org.freedesktop.UDisks2.Error.DeviceBusy: Error unmounting …: target is busy"
            m.rsplit_once(": ").map(|(_, tail)| tail.to_string()).filter(|t| !t.is_empty()).unwrap_or(m)
        })
}

/// Mount a filesystem volume; returns where it went.
pub async fn mount(obj: &str) -> Result<PathBuf, String> {
    if let Some(m) = find(obj).and_then(|v| v.mount_point().cloned()) {
        return Ok(m);
    }
    let r = call(obj, IFACE_FS, "Mount", args(&[opts()])).await;
    match r {
        Ok(v) => {
            refresh();
            Ok(PathBuf::from(v.child_value(0).str().unwrap_or_default()))
        }
        // someone else mounted it meanwhile
        Err(e) if e.contains("already mounted") => {
            refresh();
            Err(e)
        }
        Err(e) => Err(e),
    }
}

/// Unlock a LUKS container, then mount what's inside. Returns the mount point.
pub async fn unlock_and_mount(obj: &str, passphrase: &str) -> Result<PathBuf, String> {
    let v = call(obj, IFACE_CRYPT, "Unlock", args(&[passphrase.to_variant(), opts()])).await?;
    let clear = v.child_value(0).str().unwrap_or_default().to_string();
    mount(&clear).await
}

#[derive(Clone, Debug)]
pub enum Stage {
    /// flushing cached writes: bytes still waiting system-wide
    Flushing(u64),
    Unmounting(String),
    Locking,
    PoweringOff,
}

pub enum EjectError {
    /// a filesystem is in use; who's using it (from fuser), may be empty if unknown
    Busy(PathBuf, Vec<String>),
    Other(String),
}

/// Dirty + under-writeback page cache, system wide (the kernel doesn't expose per-disk
/// numbers to normal users). It shrinks to ~0 while syncfs runs.
pub fn pending_writes() -> u64 {
    let txt = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    txt.lines()
        .filter(|l| l.starts_with("Dirty:") || l.starts_with("Writeback:"))
        .filter_map(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        .sum::<u64>()
        * 1024
}

extern "C" {
    fn syncfs(fd: i32) -> i32;
}

/// Who has files open on a mount (only our own user's processes are visible).
fn holders(mount: &Path) -> Vec<String> {
    // fuser prints PIDs on stdout and the rest of the table on stderr: merge them
    let out = std::process::Command::new("sh").arg("-c").arg("fuser -vm \"$1\" 2>&1").arg("sh").arg(mount).output();
    let Ok(o) = out else { return vec![] };
    parse_fuser(&String::from_utf8_lossy(&o.stdout))
}

fn parse_fuser(txt: &str) -> Vec<String> {
    let mut v: Vec<String> = txt
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // "user pid access command"; the kernel's own mount entry has no pid
            (f.len() >= 4 && f[1].parse::<u32>().is_ok()).then(|| format!("{} (pid {})", f[3], f[1]))
        })
        .collect();
    v.dedup();
    v
}

/// The whole safe-eject sequence for the drive `drive` (object path).
pub async fn safe_eject(drive: &str, progress: Rc<dyn Fn(Stage)>) -> Result<(), EjectError> {
    let vols: Vec<Volume> = volumes().into_iter().filter(|v| v.drive == drive).collect();
    let mounted: Vec<(String, PathBuf)> = vols.iter().filter_map(|v| v.mount_point().map(|m| (v.obj.clone(), m.clone()))).collect();

    // 1. flush: syncfs on each mounted filesystem, reporting pending writes meanwhile
    if !mounted.is_empty() {
        let done = Arc::new(AtomicBool::new(false));
        let d2 = done.clone();
        let paths: Vec<PathBuf> = mounted.iter().map(|(_, m)| m.clone()).collect();
        std::thread::spawn(move || {
            for p in paths {
                if let Ok(f) = std::fs::File::open(&p) {
                    // SAFETY: valid fd for the duration of the call
                    unsafe { syncfs(f.as_raw_fd()) };
                }
            }
            d2.store(true, Ordering::Relaxed);
        });
        while !done.load(Ordering::Relaxed) {
            progress(Stage::Flushing(pending_writes()));
            glib::timeout_future(Duration::from_millis(300)).await;
        }
        progress(Stage::Flushing(0));
    }

    // 2. unmount everything on the drive (no force: a busy fs stays mounted and we say why)
    for (obj, m) in &mounted {
        progress(Stage::Unmounting(m.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        if let Err(e) = call(obj, IFACE_FS, "Unmount", args(&[opts()])).await {
            refresh();
            if e.to_lowercase().contains("busy") {
                let m2 = m.clone();
                let who = gio::spawn_blocking(move || holders(&m2)).await.unwrap_or_default();
                return Err(EjectError::Busy(m.clone(), who));
            }
            return Err(EjectError::Other(e));
        }
    }

    // 3. lock unlocked encrypted containers on this drive
    let objs = conn().ok_or(EjectError::Other("udisks2 isn't available".into()))?;
    let r = objs
        .call_future(Some(BUS), ROOT, "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", None, None, gio::DBusCallFlags::NONE, 10_000)
        .await
        .map_err(|e| EjectError::Other(e.message().to_string()))?;
    let lockable = lockable_on(&r, drive);
    if !lockable.is_empty() {
        progress(Stage::Locking);
        for obj in lockable {
            call(&obj, IFACE_CRYPT, "Lock", args(&[opts()])).await.map_err(EjectError::Other)?;
        }
    }

    // 4. power off (spins the disk down, the usb port lets go) or eject the medium
    let drive_info = vols.first().cloned().unwrap_or_default();
    progress(Stage::PoweringOff);
    let r = if drive_info.backing.is_some() {
        call(drive, IFACE_LOOP, "Delete", args(&[opts()])).await
    } else if drive_info.can_power_off {
        call(drive, IFACE_DRIVE, "PowerOff", args(&[opts()])).await
    } else if drive_info.ejectable {
        call(drive, IFACE_DRIVE, "Eject", args(&[opts()])).await
    } else {
        Ok(().to_variant())
    };
    refresh();
    // everything is unmounted and flushed by now, so a failed power-off is not data loss
    if let Err(e) = r {
        eprintln!("slap-files: power off failed (already safe, unmounted): {e}");
    }
    Ok(())
}

/// Attach a disk image (iso/img) as a loop device and mount its filesystem.
pub async fn mount_image(file: &Path) -> Result<PathBuf, String> {
    let ro = file.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("iso"));
    let f = file.to_path_buf();
    let out = gio::spawn_blocking(move || {
        let mut c = std::process::Command::new("udisksctl");
        c.arg("loop-setup").arg("--no-user-interaction").arg("-f").arg(&f);
        if ro {
            c.arg("-r");
        }
        c.output()
    })
    .await
    .map_err(|_| "worker failed".to_string())?
    .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    // "Mapped file X as /dev/loop0."
    let txt = String::from_utf8_lossy(&out.stdout);
    let dev = txt.split_whitespace().last().unwrap_or_default().trim_end_matches('.').to_string();
    // wait for udisks to see it (and its partitions)
    for _ in 0..30 {
        refresh();
        glib::timeout_future(Duration::from_millis(150)).await;
        let found = volumes().into_iter().find(|v| v.device == dev || v.device.starts_with(&format!("{dev}p")));
        if let Some(v) = found {
            let r = mount(&v.obj).await;
            if r.is_err() {
                let _ = std::process::Command::new("udisksctl").args(["loop-delete", "--no-user-interaction", "-b", &dev]).output();
                refresh();
            }
            return r;
        }
    }
    // nothing mountable inside: don't leave the loop device behind
    let _ = std::process::Command::new("udisksctl").args(["loop-delete", "--no-user-interaction", "-b", &dev]).output();
    refresh();
    Err(format!("{} has no filesystem that can be mounted", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}

/// Encrypted containers on `drive` that are currently unlocked.
fn lockable_on(v: &glib::Variant, drive: &str) -> Vec<String> {
    let objs = v.child_value(0);
    let mut out = Vec::new();
    for i in 0..objs.n_children() {
        let e = objs.child_value(i);
        let path = e.child_value(0).str().unwrap_or_default().to_string();
        let ifs = e.child_value(1);
        let (mut blk, mut crypt) = (None, None);
        for j in 0..ifs.n_children() {
            let x = ifs.child_value(j);
            match x.child_value(0).str() {
                Some(IFACE_BLOCK) => blk = Some(glib::VariantDict::new(Some(&x.child_value(1)))),
                Some(IFACE_CRYPT) => crypt = Some(glib::VariantDict::new(Some(&x.child_value(1)))),
                _ => {}
            }
        }
        if let (Some(bk), Some(c)) = (blk, crypt) {
            let clear = s(&c, "CleartextDevice");
            if s(&bk, "Drive") == drive && !clear.is_empty() && clear != "/" {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuser_table() {
        let t = "                     USER        PID ACCESS COMMAND\n/run/media/v/X:\n                     root     kernel mount /run/media/v/X\n                     v    77514 ..c.. bash\n                     v    77516 ..c.. sleep\n";
        assert_eq!(parse_fuser(t), vec!["bash (pid 77514)", "sleep (pid 77516)"]);
    }

    #[test]
    fn os_mounts() {
        for m in ["/", "/boot", "/boot/efi", "/home", "/var/log", "/usr"] {
            assert!(os_mount(Path::new(m)), "{m}");
        }
        for m in ["/run/media/user/BIG", "/mnt/data", "/media/usb", "/home/user/mnt/disk"] {
            assert!(!os_mount(Path::new(m)), "{m}");
        }
    }
}
