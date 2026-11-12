//! Hyprland options: read live values with `hyprctl getoption`, apply them live, and persist
//! them to an override file that the main config loads last. Works with both config styles:
//!   Lua  (hyprland.lua)  -> `hyprctl eval 'hl.config{…}'`, ~/.config/hypr/settings.lua
//!   conf (hyprland.conf) -> `hyprctl keyword`,           ~/.config/hypr/rice-settings.conf
//! App state lives in ~/.config/rice/settings.json.

use crate::{ui, util};
use gtk::prelude::*;
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

thread_local! {
    static STATE: RefCell<Map<String, Value>> = RefCell::new(load());
}

fn state_path() -> PathBuf {
    util::cfg("rice/settings.json")
}

pub fn is_lua() -> bool {
    util::cfg("hypr/hyprland.lua").exists()
}

pub fn main_config() -> PathBuf {
    util::cfg(if is_lua() { "hypr/hyprland.lua" } else { "hypr/hyprland.conf" })
}

/// The override file this app owns.
pub fn override_path() -> PathBuf {
    util::cfg(if is_lua() { "hypr/settings.lua" } else { "hypr/rice-settings.conf" })
}

/// Does the main config load our override file? (otherwise changes vanish on restart)
pub fn include_present() -> bool {
    let name = override_path().file_name().unwrap().to_string_lossy().into_owned();
    fs::read_to_string(main_config()).is_ok_and(|t| t.contains(&name))
}

/// Append the line that loads the override file to the main config.
pub fn enable_include() -> bool {
    let line = if is_lua() {
        "\n-- overrides written by rice-settings (loaded last so they win)\npcall(dofile, os.getenv(\"HOME\") .. \"/.config/hypr/settings.lua\")\n"
    } else {
        "\n# overrides written by rice-settings (keep this last)\nsource = ~/.config/hypr/rice-settings.conf\n"
    };
    let path = main_config();
    let Ok(mut t) = fs::read_to_string(&path) else { return false };
    let _ = fs::write(path.with_extension(format!("{}.pre-rice-settings", path.extension().unwrap().to_string_lossy())), &t);
    t.push_str(line);
    // make sure the file exists before Hyprland sources it
    STATE.with(|s| write_override(&s.borrow()));
    fs::write(&path, t).is_ok()
}

fn load() -> Map<String, Value> {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn save(st: &Map<String, Value>) {
    let p = state_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _ = fs::write(&p, serde_json::to_string_pretty(st).unwrap_or_default() + "\n");
}

/// Generic app state (used by other pages, e.g. the bar module snapshot).
pub fn state_get(key: &str) -> Option<Value> {
    STATE.with(|s| s.borrow().get(key).cloned())
}

pub fn state_set(key: &str, v: Value) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.insert(key.into(), v);
        save(&s);
    });
}

/// Banner shown when the main config doesn't load our override file yet.
pub fn persist_banner() -> Option<gtk::Box> {
    if include_present() {
        return None;
    }
    let card = ui::card();
    card.add_css_class("banner");
    let r = ui::row(
        "󰆓",
        "changes won't survive a restart yet",
        &format!("add one line to {} so it loads {}", util::tilde(&main_config()), util::tilde(&override_path())),
    );
    r.icon.add_css_class("accent");
    let b = ui::button("add it", &["suggested"]);
    let (c, sub) = (card.clone(), r.sub.clone());
    b.connect_clicked(move |_| {
        if enable_include() {
            c.set_visible(false);
        } else {
            sub.set_text("couldn't edit the config file");
        }
    });
    r.suffix(&b);
    card.append(&r.row);
    Some(card)
}

// ------------------------------------------------------------------ read --

fn getopt(name: &str) -> Option<Value> {
    serde_json::from_str(&util::sh("hyprctl", &["getoption", name, "-j"])?).ok()
}

pub fn opt_i(name: &str, def: i64) -> i64 {
    getopt(name)
        .and_then(|v| {
            v["int"].as_i64().or_else(|| {
                // gaps come back as css-style "3 3 3 3"
                v["css"].as_str()?.split_whitespace().next()?.parse().ok()
            })
        })
        .unwrap_or(def)
}

pub fn opt_f(name: &str, def: f64) -> f64 {
    getopt(name)
        .and_then(|v| v["float"].as_f64().or_else(|| v["int"].as_f64()))
        .unwrap_or(def)
}

pub fn opt_b(name: &str, def: bool) -> bool {
    getopt(name)
        .and_then(|v| v["bool"].as_bool().or_else(|| v["int"].as_i64().map(|i| i != 0)))
        .unwrap_or(def)
}

pub fn opt_s(name: &str, def: &str) -> String {
    getopt(name)
        .and_then(|v| v["str"].as_str().map(String::from))
        .unwrap_or_else(|| def.to_string())
}

// ----------------------------------------------------------------- write --

/// Apply an option live and remember it.
pub fn set(name: &str, v: Value) {
    if !matches!(v, Value::Bool(_) | Value::Number(_) | Value::String(_)) {
        return;
    }
    if is_lua() {
        // Lua-config Hyprland has no `keyword`; evaluate an hl.config() for just this option.
        let mut one = Map::new();
        insert_path(&mut one, name, v.clone());
        let mut lua = String::from("hl.config(");
        emit(&one, 0, &mut lua);
        lua.push(')');
        util::hyprctl(&["eval", &lua]);
    } else {
        util::hyprctl(&["keyword", name, &conf_val(&v)]);
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let opts = s.entry("hypr").or_insert_with(|| json!({}));
        if let Some(o) = opts.as_object_mut() {
            o.insert(name.into(), v);
        }
        save(&s);
        write_override(&s);
    });
}

/// Drop every window-look override (monitors are kept) and reload Hyprland.
pub fn reset_options() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.remove("hypr");
        save(&s);
        write_override(&s);
    });
    util::hyprctl(&["reload"]);
}

pub fn set_monitor(output: &str, mode: &str, position: &str, scale: f64) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mons = s.entry("monitors").or_insert_with(|| json!({}));
        if let Some(o) = mons.as_object_mut() {
            o.insert(
                output.into(),
                json!({ "mode": mode, "position": position, "scale": scale }),
            );
        }
        save(&s);
        write_override(&s);
    });
    util::hyprctl(&["reload"]);
}

fn lua_key(k: &str) -> String {
    let ident = !k.is_empty()
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !k.chars().next().unwrap().is_ascii_digit();
    if ident { k.to_string() } else { format!("[{k:?}]") }
}

fn lua_val(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("{s:?}"),
        _ => "nil".into(),
    }
}

fn emit(m: &Map<String, Value>, ind: usize, out: &mut String) {
    out.push_str("{\n");
    let pad = "    ".repeat(ind + 1);
    for (k, v) in m {
        out.push_str(&pad);
        out.push_str(&lua_key(k));
        out.push_str(" = ");
        match v {
            Value::Object(o) => emit(o, ind + 1, out),
            _ => out.push_str(&lua_val(v)),
        }
        out.push_str(",\n");
    }
    out.push_str(&"    ".repeat(ind));
    out.push('}');
}

/// "decoration:blur:enabled" -> { decoration = { blur = { enabled = v } } }
fn insert_path(tree: &mut Map<String, Value>, key: &str, v: Value) {
    let parts: Vec<&str> = key.split(':').collect();
    let mut cur = tree;
    for p in &parts[..parts.len() - 1] {
        let e = cur.entry(p.to_string()).or_insert_with(|| json!({}));
        if !e.is_object() {
            *e = json!({});
        }
        cur = e.as_object_mut().unwrap();
    }
    cur.insert(parts[parts.len() - 1].to_string(), v);
}

fn write_override(st: &Map<String, Value>) {
    let out = if is_lua() { render_lua(st) } else { render_conf(st) };
    let _ = fs::write(override_path(), out);
}

fn conf_val(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn emit_conf(m: &Map<String, Value>, ind: usize, out: &mut String) {
    let pad = "    ".repeat(ind);
    for (k, v) in m {
        match v {
            Value::Object(o) => {
                out.push_str(&format!("{pad}{k} {{\n"));
                emit_conf(o, ind + 1, out);
                out.push_str(&format!("{pad}}}\n"));
            }
            _ => out.push_str(&format!("{pad}{k} = {}\n", conf_val(v))),
        }
    }
}

fn render_conf(st: &Map<String, Value>) -> String {
    let mut tree = Map::new();
    if let Some(Value::Object(opts)) = st.get("hypr") {
        for (k, v) in opts {
            insert_path(&mut tree, k, v.clone());
        }
    }
    let mut out = String::from("# written by rice-settings: change things in the app,\n# or delete this file to go back to hyprland.conf values.\n");
    emit_conf(&tree, 0, &mut out);
    if let Some(Value::Object(mons)) = st.get("monitors") {
        for (name, m) in mons {
            out.push_str(&format!(
                "monitor = {}, {}, {}, {}\n",
                name,
                m["mode"].as_str().unwrap_or("preferred"),
                m["position"].as_str().unwrap_or("auto"),
                m["scale"].as_f64().unwrap_or(1.0)
            ));
        }
    }
    out
}

fn render_lua(st: &Map<String, Value>) -> String {
    // "decoration:blur:enabled" -> { decoration = { blur = { enabled = … } } }
    let mut tree = Map::new();
    if let Some(Value::Object(opts)) = st.get("hypr") {
        for (k, v) in opts {
            insert_path(&mut tree, k, v.clone());
        }
    }
    let mut out = String::from(
        "-- written by rice-settings: change things in the app,\n-- or delete this file to go back to hyprland.lua values.\n",
    );
    if !tree.is_empty() {
        out.push_str("hl.config(");
        emit(&tree, 0, &mut out);
        out.push_str(")\n");
    }
    if let Some(Value::Object(mons)) = st.get("monitors") {
        for (name, m) in mons {
            out.push_str(&format!(
                "hl.monitor({{ output = {:?}, mode = {:?}, position = {:?}, scale = {} }})\n",
                name,
                m["mode"].as_str().unwrap_or("preferred"),
                m["position"].as_str().unwrap_or("auto"),
                m["scale"].as_f64().unwrap_or(1.0),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn lua_render() {
        let st: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{
            "hypr": {"decoration:rounding": 10, "decoration:rounding_power": 2.5,
                     "decoration:blur:enabled": false, "general:gaps_in": 3, "general:layout": "dwindle"},
            "monitors": {"eDP-1": {"mode": "1920x1080@144.00", "position": "0x0", "scale": 1.2}}
        }"#).unwrap();
        let out = super::render_lua(&st);
        std::fs::write(std::env::var("LUA_OUT").unwrap_or("/dev/null".into()), &out).unwrap();
        assert!(out.contains("rounding = 10"));
        assert!(out.contains("blur = {"));
        let conf = super::render_conf(&st);
        assert!(conf.contains("decoration {\n    rounding = 10\n"));
        assert!(conf.contains("monitor = eDP-1, 1920x1080@144.00, 0x0, 1.2"));
    }
}
