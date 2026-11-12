//! Small persisted preferences: ~/.config/slap/files.json

use crate::util;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

thread_local! {
    static STATE: RefCell<Map<String, Value>> = RefCell::new(load());
}

fn path() -> PathBuf {
    util::cfg("slap/files.json")
}

fn load() -> Map<String, Value> {
    fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub fn get_bool(key: &str, def: bool) -> bool {
    STATE.with(|s| s.borrow().get(key).and_then(Value::as_bool).unwrap_or(def))
}

pub fn get_str(key: &str, def: &str) -> String {
    STATE.with(|s| s.borrow().get(key).and_then(Value::as_str).unwrap_or(def).to_string())
}

pub fn set(key: &str, v: impl Into<Value>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.insert(key.to_string(), v.into());
        let p = path();
        let _ = fs::create_dir_all(p.parent().unwrap());
        if let Ok(txt) = serde_json::to_string_pretty(&Value::Object(s.clone())) {
            let _ = fs::write(p, txt);
        }
    });
}

pub fn get_list(key: &str) -> Vec<String> {
    STATE.with(|s| {
        s.borrow()
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    })
}
