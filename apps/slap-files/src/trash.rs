//! The freedesktop trash: the home trash plus per-drive `.Trash-$uid` / `.Trash/$uid`
//! folders on mounted drives (files trashed from a USB disk stay on that disk).
//! List, restore, delete for good, empty. Blocking: call from worker threads.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    /// where the data lives now (…/Trash/files/name)
    pub stored: PathBuf,
    /// its .trashinfo
    pub info: PathBuf,
    /// where it came from
    pub original: PathBuf,
    /// unix time of deletion
    pub deleted: i64,
}

fn uid() -> u32 {
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
}

pub fn home_trash() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::util::home().join(".local/share"))
        .join("Trash")
}

/// Mount points of real, user-visible filesystems (where per-drive trashes may exist).
fn user_mounts() -> Vec<PathBuf> {
    let txt = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    txt.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let dev = f.next()?;
            let mp = f.next()?.replace("\\040", " ");
            (dev.starts_with("/dev/") && (mp.starts_with("/run/media/") || mp.starts_with("/media/") || mp.starts_with("/mnt/")))
                .then(|| PathBuf::from(mp))
        })
        .collect()
}

/// (trash dir, top dir for relative paths)
pub fn dirs() -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut v = vec![(home_trash(), None)];
    let u = uid();
    for m in user_mounts() {
        for t in [m.join(format!(".Trash-{u}")), m.join(".Trash").join(u.to_string())] {
            if t.join("info").is_dir() {
                v.push((t, Some(m.clone())));
            }
        }
    }
    v
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_date(s: &str) -> i64 {
    gtk::glib::DateTime::from_iso8601(s, Some(&gtk::glib::TimeZone::local()))
        .map(|d| d.to_unix())
        .unwrap_or(0)
}

pub fn list() -> Vec<Entry> {
    let mut out = Vec::new();
    for (dir, top) in dirs() {
        let Ok(rd) = std::fs::read_dir(dir.join("info")) else { continue };
        for e in rd.flatten() {
            let info = e.path();
            if info.extension().and_then(|x| x.to_str()) != Some("trashinfo") {
                continue;
            }
            let Ok(txt) = std::fs::read_to_string(&info) else { continue };
            let (mut path, mut date) = (None, 0);
            for l in txt.lines() {
                if let Some(v) = l.strip_prefix("Path=") {
                    let p = PathBuf::from(decode(v));
                    path = Some(match (&top, p.is_absolute()) {
                        (Some(t), false) => t.join(p),
                        _ => p,
                    });
                } else if let Some(v) = l.strip_prefix("DeletionDate=") {
                    date = parse_date(v);
                }
            }
            let Some(stem) = info.file_stem() else { continue };
            let stored = dir.join("files").join(stem);
            if let (Some(original), true) = (path, std::fs::symlink_metadata(&stored).is_ok()) {
                out.push(Entry { stored, info, original, deleted: date });
            }
        }
    }
    out
}

pub fn count() -> usize {
    dirs()
        .iter()
        .filter_map(|(d, _)| std::fs::read_dir(d.join("info")).ok())
        .map(|rd| rd.count())
        .sum()
}

/// Put it back where it came from (a free "name (2)" if something is there now).
pub fn restore(e: &Entry) -> Result<PathBuf, String> {
    let parent = e.original.parent().ok_or("bad original path")?;
    std::fs::create_dir_all(parent).map_err(|x| x.to_string())?;
    let name = e.original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let target = if std::fs::symlink_metadata(&e.original).is_ok() {
        crate::ops::unique_name(parent, &name, false)
    } else {
        e.original.clone()
    };
    match std::fs::rename(&e.stored, &target) {
        Ok(()) => {}
        Err(x) if x.raw_os_error() == Some(18) => {
            // trash on another drive than the original folder: copy back
            let h = crate::ops::Handle::default();
            let out = crate::ops::transfer(crate::ops::Kind::Move, vec![e.stored.clone()], parent.to_path_buf(), crate::ops::Conflict::KeepBoth, &h);
            if let Some(err) = out.errors.first() {
                return Err(err.clone());
            }
        }
        Err(x) => return Err(x.to_string()),
    }
    let _ = std::fs::remove_file(&e.info);
    Ok(target)
}

pub fn delete(e: &Entry) -> Result<(), String> {
    let m = std::fs::symlink_metadata(&e.stored).map_err(|x| x.to_string())?;
    let r = if m.is_dir() { std::fs::remove_dir_all(&e.stored) } else { std::fs::remove_file(&e.stored) };
    r.map_err(|x| x.to_string())?;
    let _ = std::fs::remove_file(&e.info);
    Ok(())
}

/// Most recently trashed entry that came from `original`.
pub fn find(original: &Path) -> Option<Entry> {
    list().into_iter().filter(|e| e.original == original).max_by_key(|e| e.deleted)
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes() {
        assert_eq!(super::decode("/a%20b/%C3%A9"), "/a b/é");
        assert!(super::parse_date("2026-11-03T15:05:01") > 1_700_000_000);
    }
}
