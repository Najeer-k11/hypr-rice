//! Recursive search under a folder (Explorer's "search in subfolders"), on a worker thread.
//! Results stream back in batches; a newer search or leaving the folder cancels it.

use crate::item::{self, Item};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MAX_RESULTS: usize = 5000;

pub enum Msg {
    Batch(Vec<Item>),
    /// finished (true = stopped at MAX_RESULTS)
    Done(bool),
}

/// Pseudo filesystems that are never worth walking.
fn skip_dir(p: &Path) -> bool {
    matches!(p.to_str(), Some("/proc" | "/sys" | "/dev" | "/run" | "/tmp/.X11-unix"))
}

/// Walk `root` breadth-first; names containing `query` (already lowercase) are reported.
/// Stays on root's filesystem and never follows symlinked folders (no loops, no network mounts).
pub fn run(root: PathBuf, query: String, hidden: bool, cancel: Arc<AtomicBool>, tx: Sender<Msg>) {
    let dev = std::fs::metadata(&root).map(|m| m.dev()).ok();
    let mut queue = std::collections::VecDeque::from([root]);
    let mut descs = HashMap::new();
    let mut batch = Vec::new();
    let mut found = 0;
    let mut last = Instant::now();
    let mut capped = false;
    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let parent: Arc<Path> = Arc::from(dir.as_path());
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                let p = e.path();
                let same_fs = dev.is_none() || e.metadata().map(|m| Some(m.dev()) == dev).unwrap_or(false);
                if same_fs && !skip_dir(&p) {
                    queue.push_back(p);
                }
            }
            if name.to_lowercase().contains(&query) {
                if let Some(it) = item::stat_with(&parent, &name, &mut descs) {
                    batch.push(it);
                    found += 1;
                }
                if found >= MAX_RESULTS {
                    capped = true;
                    break;
                }
            }
        }
        if !batch.is_empty() && (batch.len() >= 200 || last.elapsed() > Duration::from_millis(120)) {
            if tx.send(Msg::Batch(std::mem::take(&mut batch))).is_err() {
                return;
            }
            last = Instant::now();
        }
        if capped {
            break;
        }
    }
    if !batch.is_empty() {
        let _ = tx.send(Msg::Batch(batch));
    }
    let _ = tx.send(Msg::Done(capped));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_and_skips_hidden() {
        let d = std::env::temp_dir().join(format!("slap-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b/.secret")).unwrap();
        std::fs::write(d.join("a/b/Report-final.txt"), "x").unwrap();
        std::fs::write(d.join("a/b/.secret/report.txt"), "x").unwrap();
        std::fs::write(d.join("other.txt"), "x").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        run(d.clone(), "report".into(), false, Arc::default(), tx);
        let names: Vec<String> = rx
            .iter()
            .flat_map(|m| match m {
                Msg::Batch(v) => v.into_iter().map(|i| i.name).collect(),
                Msg::Done(_) => vec![],
            })
            .collect();
        assert_eq!(names, vec!["Report-final.txt"]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
