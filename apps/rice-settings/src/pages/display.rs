//! Monitors: resolution / refresh and scale. Only scales Hyprland accepts are offered
//! (ones that give a whole-pixel logical size), so nothing silently snaps.

use crate::{hypr, ui, util};
use gtk::prelude::*;
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

fn monitors() -> Vec<Value> {
    util::sh("hyprctl", &["monitors", "-j"])
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Scales k/120 in 0.5..=3 where width/scale and height/scale are whole numbers.
fn valid_scales(w: i64, h: i64) -> Vec<f64> {
    (60..=360)
        .filter(|k| (w * 120) % k == 0 && (h * 120) % k == 0)
        .map(|k| k as f64 / 120.0)
        .collect()
}

fn fmt_scale(s: f64) -> String {
    let t = format!("{s:.3}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰍹", "display", "resolution, refresh rate and scale");
    if let Some(b) = hypr::persist_banner() {
        p.body.append(&b);
    }
    let mons = monitors();
    if mons.is_empty() {
        let c = ui::card();
        c.append(&ui::label("no monitors reported by hyprctl", &["empty"]));
        p.body.append(&c);
    }

    for m in mons {
        let name = m["name"].as_str().unwrap_or("?").to_string();
        let desc = format!("{} {}", m["make"].as_str().unwrap_or(""), m["model"].as_str().unwrap_or("")).trim().to_string();
        let (w, h) = (m["width"].as_i64().unwrap_or(1920), m["height"].as_i64().unwrap_or(1080));
        let hz = m["refreshRate"].as_f64().unwrap_or(60.0);
        let scale = m["scale"].as_f64().unwrap_or(1.0);
        let pos = format!("{}x{}", m["x"].as_i64().unwrap_or(0), m["y"].as_i64().unwrap_or(0));

        let c = ui::section(&p.body, &name);
        let head = ui::row("󰍹", &desc, &format!("{w}×{h} @ {hz:.0} Hz · scale {} · looks like {}×{}", fmt_scale(scale), (w as f64 / scale).round(), (h as f64 / scale).round()));
        head.icon.add_css_class("on");
        ui::add(&c, &head.row);

        // modes like "1920x1080@144.00Hz"
        let modes: Vec<String> = m["availableModes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let mut uniq: Vec<String> = Vec::new();
        for md in &modes {
            if !uniq.contains(md) {
                uniq.push(md.clone());
            }
        }
        let labels: Vec<String> = uniq.iter().map(|s| s.replace('x', "×").replace('@', " @ ").replace("Hz", " Hz")).collect();
        let lrefs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let cur_i = uniq.iter().position(|md| {
            let (res, rate) = md.split_once('@').unwrap_or((md, "0"));
            res == format!("{w}x{h}") && (rate.trim_end_matches("Hz").parse::<f64>().unwrap_or(0.0) - hz).abs() < 0.5
        });
        let mode_dd = ui::dropdown(&lrefs, cur_i.unwrap_or(0) as u32);
        let r = ui::row("󰹑", "resolution", "");
        r.suffix(&mode_dd);
        ui::add(&c, &r.row);

        let scale_dd = gtk::DropDown::from_strings(&[]);
        scale_dd.set_valign(gtk::Align::Center);
        let r = ui::row("󰁌", "scale", "bigger = larger text and windows");
        r.suffix(&scale_dd);
        ui::add(&c, &r.row);

        // scale list depends on the chosen resolution
        let scales_now: Rc<RefCell<Vec<f64>>> = Rc::default();
        let fill_scales = {
            let scale_dd = scale_dd.clone();
            let cell = scales_now.clone();
            let uniq = uniq.clone();
            move |mode_i: u32, want: f64| {
                let (mw, mh) = uniq
                    .get(mode_i as usize)
                    .and_then(|m| m.split('@').next())
                    .and_then(|r| r.split_once('x'))
                    .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                    .unwrap_or((w, h));
                let scales = valid_scales(mw, mh);
                let labels: Vec<String> = scales
                    .iter()
                    .map(|s| format!("{}  ·  {}×{}", fmt_scale(*s), (mw as f64 / s).round(), (mh as f64 / s).round()))
                    .collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                scale_dd.set_model(Some(&gtk::StringList::new(&refs)));
                let i = scales
                    .iter()
                    .enumerate()
                    .min_by(|a, b| (a.1 - want).abs().total_cmp(&(b.1 - want).abs()))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                scale_dd.set_selected(i as u32);
                *cell.borrow_mut() = scales;
            }
        };
        fill_scales(mode_dd.selected(), scale);
        let fill_scales = Rc::new(fill_scales);
        let (fs, sd, cell) = (fill_scales.clone(), scale_dd.clone(), scales_now.clone());
        mode_dd.connect_selected_notify(move |d| {
            let cur = cell.borrow().get(sd.selected() as usize).copied().unwrap_or(1.0);
            fs(d.selected(), cur);
        });

        let apply = ui::button("apply", &["suggested"]);
        let status = ui::label("", &["row-sub"]);
        status.set_hexpand(true);
        let ar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        ar.add_css_class("row");
        ar.append(&status);
        ar.append(&apply);
        ui::add(&c, &ar);

        let (md, sd, st, hd) = (mode_dd.clone(), scale_dd.clone(), status.clone(), head.sub.clone());
        let name2 = name.clone();
        let cell = scales_now.clone();
        apply.connect_clicked(move |_| {
            let mode = uniq.get(md.selected() as usize).map(|m| m.trim_end_matches("Hz").to_string()).unwrap_or_else(|| "preferred".into());
            let s = cell.borrow().get(sd.selected() as usize).copied().unwrap_or(1.0);
            hypr::set_monitor(&name2, &mode, &pos, (s * 1000.0).round() / 1000.0);
            st.set_text(&format!("applied · saved to {}", util::tilde(&hypr::override_path())));
            let (hd, name3) = (hd.clone(), name2.clone());
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
                if let Some(m) = monitors().into_iter().find(|m| m["name"] == name3.as_str()) {
                    let (w, h) = (m["width"].as_f64().unwrap_or(0.0), m["height"].as_f64().unwrap_or(0.0));
                    let sc = m["scale"].as_f64().unwrap_or(1.0);
                    hd.set_text(&format!("{w}×{h} @ {:.0} Hz · scale {} · looks like {}×{}", m["refreshRate"].as_f64().unwrap_or(0.0), fmt_scale(sc), (w / sc).round(), (h / sc).round()));
                }
            });
        });
    }
    p.root.upcast()
}
