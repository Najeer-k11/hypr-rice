//! Compress (zip / tar.zst / tar.gz / tar.xz) and extract (anything libarchive's bsdtar
//! reads: zip, tar.*, 7z, rar, iso, …). External tools, cancellable, run on worker threads.
//! bsdtar refuses absolute paths and `..` entries by default (no writing outside the target).

use crate::ops;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

const MULTI: &[&str] = &[".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tar.lz4", ".tar.lzma"];
const SINGLE: &[&str] = &[".zip", ".tar", ".tgz", ".txz", ".tzst", ".tbz2", ".7z", ".rar", ".iso", ".cab", ".jar", ".apk", ".cpio", ".xpi"];

pub fn is_archive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    MULTI.iter().chain(SINGLE).any(|e| n.ends_with(e) && n.len() > e.len())
}

/// "photos.tar.zst" -> "photos"
pub fn stem(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    for e in MULTI.iter().chain(SINGLE) {
        if n.ends_with(e) && n.len() > e.len() {
            return name[..name.len() - e.len()].to_string();
        }
    }
    name.to_string()
}

pub const FORMATS: &[(&str, &str)] = &[("zip", ".zip"), ("tar.zst  (fast, small)", ".tar.zst"), ("tar.gz", ".tar.gz"), ("tar.xz  (smallest, slow)", ".tar.xz")];

fn have(tool: &str) -> bool {
    std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(tool).is_file())
}

/// Pack `names` (entries of `dir`) into `out`. Removes a partial archive on failure/cancel.
pub fn compress(dir: &Path, names: &[String], out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let o = out.to_string_lossy().to_ascii_lowercase();
    let mut cmd;
    if o.ends_with(".zip") {
        if !have("zip") {
            return Err("the 'zip' tool isn't installed".into());
        }
        cmd = Command::new("zip");
        cmd.args(["-r", "-y", "-q"]).arg(out).arg("--");
    } else {
        cmd = Command::new("tar");
        let flag = if o.ends_with(".tar.zst") {
            "--zstd"
        } else if o.ends_with(".tar.gz") {
            "--gzip"
        } else if o.ends_with(".tar.xz") {
            "--xz"
        } else {
            return Err("unknown archive type".into());
        };
        cmd.arg(flag).arg("-cf").arg(out).arg("--");
    }
    cmd.args(names).current_dir(dir);
    let r = ops::run_cmd(cmd, cancel);
    if r.is_err() {
        let _ = std::fs::remove_file(out);
    }
    r
}

/// Extract next to the archive. One top-level entry inside -> just that entry;
/// otherwise a folder named after the archive. Returns what was created.
pub fn extract(archive: &Path, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let parent = archive.parent().ok_or("bad path")?;
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = stem(&name);
    let tmp = ops::unique_name(parent, &format!(".{base}.extracting"), false);
    std::fs::create_dir(&tmp).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("bsdtar");
    cmd.arg("-xf").arg(archive).arg("-C").arg(&tmp);
    if let Err(e) = ops::run_cmd(cmd, cancel) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(if e.contains("passphrase") || e.contains("Encrypted") { "the archive is password protected".into() } else { e });
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    let result = if let [only] = entries.as_slice() {
        let n = only.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dst = if std::fs::symlink_metadata(parent.join(&n)).is_ok() { ops::unique_name(parent, &n, false) } else { parent.join(&n) };
        std::fs::rename(only, &dst).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir(&tmp);
        dst
    } else {
        let dst = if std::fs::symlink_metadata(parent.join(&base)).is_ok() { ops::unique_name(parent, &base, false) } else { parent.join(&base) };
        std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
        dst
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_archive("a.tar.zst") && is_archive("B.ZIP") && !is_archive(".zip") && !is_archive("a.txt"));
        assert_eq!(stem("photos.tar.zst"), "photos");
        assert_eq!(stem("x.zip"), "x");
    }

    #[test]
    fn roundtrip() {
        let d = std::env::temp_dir().join(format!("slap-arch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/inner")).unwrap();
        std::fs::write(d.join("src/inner/a.txt"), "hello").unwrap();
        std::fs::write(d.join("b.txt"), "b").unwrap();
        let c = AtomicBool::new(false);
        for ext in [".zip", ".tar.zst"] {
            // one folder -> extracts as that folder
            let out = d.join(format!("one{ext}"));
            compress(&d, &["src".into()], &out, &c).unwrap();
            std::fs::rename(d.join("src"), d.join("src-orig")).unwrap();
            let got = extract(&out, &c).unwrap();
            assert_eq!(got, d.join("src"));
            assert_eq!(std::fs::read_to_string(d.join("src/inner/a.txt")).unwrap(), "hello");
            std::fs::remove_dir_all(d.join("src-orig")).unwrap();
            // two entries -> wrapped in a folder named after the archive
            let out2 = d.join(format!("two{ext}"));
            compress(&d, &["src".into(), "b.txt".into()], &out2, &c).unwrap();
            let got = extract(&out2, &c).unwrap();
            assert_eq!(got, d.join("two"));
            assert!(d.join("two/b.txt").exists() && d.join("two/src/inner/a.txt").exists());
            std::fs::remove_dir_all(d.join("two")).unwrap();
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
