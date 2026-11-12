//! Read live Hyprland options (rounding, border…) so the window matches the rest of the rice.

use crate::util;
use serde_json::Value;

fn getopt(name: &str) -> Option<Value> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return None;
    }
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
