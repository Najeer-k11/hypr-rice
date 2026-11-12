//! One directory entry, how to read a folder, and how files look (glyph + tint + type name).

use crate::util;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::sync::Arc;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    /// parent folder, shared by every item read from it
    pub dir: Arc<Path>,
    pub is_dir: bool,
    pub is_link: bool,
    pub hidden: bool,
    pub size: u64,
    pub mtime: i64,
    /// type description, shared by every item of the same content type
    pub kind: Arc<str>,
    pub icon: &'static str,
    pub tint: &'static str,
    /// set for items shown in the trash view (boxed: 8 bytes for everyone else)
    pub trash: Option<Box<crate::trash::Entry>>,
}

impl Item {
    /// Where the data actually is (for trashed items: inside the trash).
    pub fn path(&self) -> PathBuf {
        match &self.trash {
            Some(t) => t.stored.clone(),
            None => self.dir.join(&self.name),
        }
    }

    /// A trash entry shown under its original name, dated by deletion.
    pub fn from_trash(e: crate::trash::Entry, descs: &mut HashMap<String, Arc<str>>) -> Option<Item> {
        let files: Arc<Path> = Arc::from(e.stored.parent()?);
        let stored_name = e.stored.file_name()?.to_string_lossy().into_owned();
        let mut it = stat_with(&files, &stored_name, descs)?;
        it.name = e.original.file_name()?.to_string_lossy().into_owned();
        it.hidden = false;
        it.mtime = e.deleted;
        it.trash = Some(Box::new(e));
        Some(it)
    }
    /// The file's own modification time (trash items show deletion time in `mtime`).
    pub fn mtime_file(&self) -> i64 {
        match &self.trash {
            Some(t) => std::fs::symlink_metadata(&t.stored).map(|m| std::os::unix::fs::MetadataExt::mtime(&m)).unwrap_or(0),
            None => self.mtime,
        }
    }

    pub fn date_s(&self) -> String {
        util::fmt_time(self.mtime)
    }
    pub fn size_s(&self) -> String {
        if self.is_dir { String::new() } else { util::human_bytes(self.size) }
    }
}

/// Borrow the Item inside a list-model object.
pub fn of(o: &glib::Object) -> std::cell::Ref<'_, Item> {
    o.downcast_ref::<glib::BoxedAnyObject>().expect("item object").borrow::<Item>()
}

pub fn obj(it: Item) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(it)
}

thread_local! {
    static FOLDER: Arc<str> = Arc::from("folder");
}

/// Read a folder. Runs on a worker thread.
pub fn read(dir: &Path) -> Result<Vec<Item>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => "you don't have permission to open this folder".to_string(),
        std::io::ErrorKind::NotFound => "this folder doesn't exist any more".to_string(),
        _ => e.to_string(),
    })?;
    let mut descs = HashMap::new();
    let parent: Arc<Path> = Arc::from(dir);
    let mut v: Vec<Item> = rd.flatten().filter_map(|e| stat_with(&parent, &e.file_name().to_string_lossy(), &mut descs)).collect();
    v.shrink_to_fit();
    Ok(v)
}

#[allow(dead_code)]
pub fn stat(path: &Path) -> Option<Item> {
    let parent: Arc<Path> = Arc::from(path.parent()?);
    stat_with(&parent, &path.file_name()?.to_string_lossy(), &mut HashMap::new())
}

pub fn stat_with(dir: &Arc<Path>, name: &str, descs: &mut HashMap<String, Arc<str>>) -> Option<Item> {
    let name = name.to_string();
    let path = &dir.join(&name);
    let lmeta = std::fs::symlink_metadata(path).ok()?;
    let is_link = lmeta.file_type().is_symlink();
    let meta = if is_link { std::fs::metadata(path).unwrap_or(lmeta) } else { lmeta };
    let is_dir = meta.is_dir();
    let exec = !is_dir && meta.is_file() && meta.permissions().mode() & 0o111 != 0;
    let size = if is_dir { 0 } else { meta.len() };
    let mtime = meta.mtime();

    let (kind, ct) = if is_dir {
        (FOLDER.with(|f| f.clone()), String::new())
    } else {
        let (ct, _) = gio::content_type_guess(Some(&name), None::<&[u8]>);
        let ct = ct.to_string();
        let d = descs
            .entry(ct.clone())
            .or_insert_with(|| Arc::from(gio::content_type_get_description(&ct).to_lowercase()))
            .clone();
        if gio::content_type_is_unknown(&ct) {
            let e = ext(&name);
            (if e.is_empty() { Arc::from("file") } else { Arc::from(format!("{e} file")) }, ct)
        } else {
            (d, ct)
        }
    };
    let (icon, tint) = look(&name, is_dir, is_link, exec, &ct);
    Some(Item {
        hidden: name.starts_with('.') || name.ends_with('~'),
        name,
        dir: dir.clone(),
        is_dir,
        is_link,
        size,
        mtime,
        kind,
        icon,
        tint,
        trash: None,
    })
}

fn ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Nerd Font glyph + css tint class for a file.
pub fn look(name: &str, is_dir: bool, is_link: bool, exec: bool, ct: &str) -> (&'static str, &'static str) {
    if is_dir {
        return (if is_link { "\u{f0251}" } else { "\u{f024b}" }, "t-folder"); // folder-network / folder
    }
    let e = ext(name);
    let by_ext = match e.as_str() {
        "zip" | "tar" | "gz" | "tgz" | "xz" | "txz" | "zst" | "bz2" | "7z" | "rar" | "lz4" | "lzma" | "cab" => Some(("\u{f05c4}", "t-archive")),
        "deb" | "rpm" | "apk" | "appimage" | "flatpak" => Some(("\u{f03d7}", "t-archive")), // package
        "iso" | "img" => Some(("\u{f02ca}", "t-archive")),
        "pdf" => Some(("\u{f0226}", "t-doc")),
        "doc" | "docx" | "odt" | "rtf" => Some(("\u{f022c}", "t-doc")),
        "xls" | "xlsx" | "ods" | "csv" | "tsv" => Some(("\u{f021b}", "t-doc")),
        "ppt" | "pptx" | "odp" => Some(("\u{f0227}", "t-doc")),
        "md" | "markdown" | "rst" => Some(("\u{f0354}", "t-doc")),
        "rs" => Some(("\u{e7a8}", "t-code")),
        "py" => Some(("\u{e73c}", "t-code")),
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => Some(("\u{e74e}", "t-code")),
        "lua" => Some(("\u{e620}", "t-code")),
        "go" => Some(("\u{e627}", "t-code")),
        "c" | "h" | "cpp" | "cc" | "hpp" | "cxx" | "java" | "kt" | "zig" | "nim" | "hs" | "rb" | "php" | "swift" | "cs" => Some(("\u{f022e}", "t-code")),
        "html" | "htm" | "css" | "scss" | "xml" | "svg" => Some(("\u{f022e}", "t-code")),
        "json" | "jsonc" | "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "env" | "lock" => Some(("\u{f0493}", "t-code")), // cog file
        "sh" | "bash" | "zsh" | "fish" => Some(("\u{f489}", "t-exec")),
        "desktop" => Some(("\u{f0379}", "t-exec")),
        "ttf" | "otf" | "woff" | "woff2" => Some(("\u{f0b34}", "t-doc")), // format-font
        "torrent" => Some(("\u{f01da}", "t-doc")),
        _ => None,
    };
    if let Some(x) = by_ext {
        return x;
    }
    if ct.starts_with("image/") {
        ("\u{f021f}", "t-image")
    } else if ct.starts_with("video/") {
        ("\u{f022b}", "t-media")
    } else if ct.starts_with("audio/") {
        ("\u{f0223}", "t-media")
    } else if exec {
        ("\u{f489}", "t-exec")
    } else if is_link {
        ("\u{f0337}", "t-link")
    } else if ct.starts_with("text/") {
        ("\u{f0219}", "t-doc")
    } else {
        ("\u{f0214}", "t-file")
    }
}

/// Explorer-style ordering: case-insensitive, numbers compared by value ("file2" < "file10").
pub fn natural(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a", "B", "file02b"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, vec!["a", "B", "file1", "File2", "file02b", "file10"]);
    }
}
