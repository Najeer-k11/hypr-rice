//! Battery (charge, health, draw, time left, charge limit, history), power profiles and
//! busiest apps. History and the auto switch are shared with `rice-battery`, which logs to
//! ~/.local/state/rice/battery.csv and reads ~/.config/rice/battery.json.

use crate::{deps, theme, ui, util};
use gtk::prelude::*;
use gtk::{cairo, glib, pango};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fs,
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

struct Bat {
    capacity: u32,
    status: String,
    now_wh: f64,
    full_wh: f64,
    design_wh: f64,
    power_w: f64,
    cycles: Option<u32>,
    model: String,
    limit: Option<u32>,
    ac: bool,
    gpu: Option<String>,
}

fn rd(dir: &PathBuf, f: &str) -> Option<String> {
    fs::read_to_string(dir.join(f)).ok().map(|s| s.trim().to_string())
}

fn num(dir: &PathBuf, f: &str) -> Option<f64> {
    rd(dir, f)?.parse().ok()
}

fn supplies() -> Vec<PathBuf> {
    fs::read_dir("/sys/class/power_supply")
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

pub fn battery_dir() -> Option<PathBuf> {
    supplies().into_iter().find(|d| {
        rd(d, "type").as_deref() == Some("Battery") && rd(d, "scope").as_deref() != Some("Device")
    })
}

fn fetch() -> Option<Bat> {
    let d = battery_dir()?;
    // energy_* in µWh, or charge_* in µAh × design voltage in µV
    let volt = num(&d, "voltage_min_design").or_else(|| num(&d, "voltage_now")).unwrap_or(0.0);
    let wh = |e: &str, c: &str| num(&d, e).map(|v| v / 1e6).or_else(|| num(&d, c).map(|v| v * volt / 1e12)).unwrap_or(0.0);
    let power_w = num(&d, "power_now")
        .map(|v| v / 1e6)
        .or_else(|| Some(num(&d, "current_now")? * num(&d, "voltage_now")? / 1e12))
        .unwrap_or(0.0);
    let ac = supplies().iter().any(|s| rd(s, "type").as_deref() == Some("Mains") && rd(s, "online").as_deref() == Some("1"));
    Some(Bat {
        capacity: num(&d, "capacity").unwrap_or(0.0) as u32,
        status: rd(&d, "status").unwrap_or_default(),
        now_wh: wh("energy_now", "charge_now"),
        full_wh: wh("energy_full", "charge_full"),
        design_wh: wh("energy_full_design", "charge_full_design"),
        power_w,
        cycles: num(&d, "cycle_count").map(|c| c as u32).filter(|c| *c > 0),
        model: [rd(&d, "manufacturer"), rd(&d, "model_name"), rd(&d, "technology")]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty() && s != "Unknown")
            .collect::<Vec<_>>()
            .join(" "),
        limit: num(&d, "charge_control_end_threshold").map(|v| v as u32),
        ac,
        gpu: nvidia_state(),
    })
}

/// Reading runtime_status doesn't wake a suspended GPU (nvidia-smi would).
fn nvidia_state() -> Option<String> {
    let devices = fs::read_dir("/sys/bus/pci/devices").ok()?;
    devices.flatten().map(|e| e.path()).find_map(|d| {
        if rd(&d, "vendor").as_deref() != Some("0x10de") || !rd(&d, "class")?.starts_with("0x03") {
            return None;
        }
        Some(match rd(&d, "power/runtime_status").as_deref() {
            Some("suspended") => "asleep · saving power".to_string(),
            Some("active") => "awake · using extra power".to_string(),
            other => other.unwrap_or("unknown").to_string(),
        })
    })
}

// ---- history (written by `rice-battery daemon`)
type Sample = (f64, f64, f64); // time, percent, watts
const DAY: f64 = 86400.0;
const LOG_GAP: f64 = 300.0; // a longer hole (suspend, powered off) breaks the line

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn read_history(window: f64) -> Vec<Sample> {
    let cutoff = now_secs() - window;
    let text = fs::read_to_string(util::home().join(".local/state/rice/battery.csv")).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut f = l.split(',').map(|v| v.parse::<f64>().ok());
            Some((f.next()??, f.next()??, f.next()??))
        })
        .filter(|s| s.0 >= cutoff)
        .collect()
}

fn rgb(name: &str) -> (f64, f64, f64) {
    let hex = theme::hex(name);
    let part = |i: usize| hex.get(i..i + 2).and_then(|p| u8::from_str_radix(p, 16).ok()).unwrap_or(128) as f64 / 255.0;
    (part(1), part(3), part(5))
}

fn set_rgb(cr: &cairo::Context, name: &str) {
    let (r, g, b) = rgb(name);
    cr.set_source_rgb(r, g, b);
}

fn draw_history(cr: &cairo::Context, w: f64, h: f64, rows: &[Sample], window: f64) {
    let (top, ph) = (4.0, h - 20.0);
    let now = now_secs();
    let x_of = |t: f64| w * (t - (now - window)) / window;
    cr.select_font_face("GeistMono Nerd Font", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(10.0);

    set_rgb(cr, "outline");
    cr.move_to(0.0, h - 3.0);
    let _ = cr.show_text(if window > DAY { "7 days ago" } else { "24 hours ago" });
    if let Ok(e) = cr.text_extents("now") {
        cr.move_to(w - e.width() - 1.0, h - 3.0);
        let _ = cr.show_text("now");
    }
    if rows.len() < 2 {
        cr.set_font_size(12.0);
        let msg = "collecting history · check back soon";
        if let Ok(e) = cr.text_extents(msg) {
            cr.move_to((w - e.width()) / 2.0, top + ph / 2.0);
            let _ = cr.show_text(msg);
        }
        return;
    }

    set_rgb(cr, "outline_variant");
    cr.set_line_width(1.0);
    for frac in [0.0, 0.25, 0.5, 0.75, 1.0] {
        cr.move_to(0.0, top + ph * frac + 0.5);
        cr.line_to(w, top + ph * frac + 0.5);
    }
    let _ = cr.stroke();

    let gap = LOG_GAP.max(window / w * 2.0);
    let mut runs: Vec<&[Sample]> = Vec::new();
    let mut start = 0;
    for i in 1..rows.len() {
        if rows[i].0 - rows[i - 1].0 > gap {
            runs.push(&rows[start..i]);
            start = i;
        }
    }
    runs.push(&rows[start..]);

    let (pr, pg, pb) = rgb("primary");
    let y_pct = |p: f64| top + ph * (1.0 - p / 100.0);
    for run in runs.iter().filter(|r| r.len() > 1) {
        cr.move_to(x_of(run[0].0), top + ph);
        for s in run.iter() {
            cr.line_to(x_of(s.0), y_pct(s.1));
        }
        cr.line_to(x_of(run[run.len() - 1].0), top + ph);
        cr.close_path();
        cr.set_source_rgba(pr, pg, pb, 0.18);
        let _ = cr.fill();
        for s in run.iter() {
            cr.line_to(x_of(s.0), y_pct(s.1));
        }
        cr.set_source_rgb(pr, pg, pb);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }
    let peak = rows.iter().map(|s| s.2).fold(20.0, f64::max); // scale to the busiest moment, at least 20 W
    set_rgb(cr, "tertiary");
    cr.set_line_width(1.0);
    for run in runs.iter() {
        for s in run.iter() {
            cr.line_to(x_of(s.0), top + ph * (1.0 - s.2 / peak));
        }
        let _ = cr.stroke();
    }
}

fn history_section(body: &gtk::Box) {
    let rows: Rc<RefCell<Vec<Sample>>> = Rc::new(RefCell::new(read_history(DAY)));
    let window = Rc::new(Cell::new(DAY));

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let title = ui::label("history", &["section"]);
    title.set_hexpand(true);
    head.append(&title);
    let day = ui::button("24h", &["flat", "selected"]);
    let week = ui::button("7d", &["flat"]);
    head.append(&day);
    head.append(&week);
    body.append(&head);

    let chart = gtk::DrawingArea::new();
    chart.set_content_height(130);
    chart.set_hexpand(true);
    chart.set_margin_top(10);
    chart.set_margin_bottom(6);
    chart.set_margin_start(14);
    chart.set_margin_end(14);
    let (r, win) = (rows.clone(), window.clone());
    chart.set_draw_func(move |_, cr, w, h| draw_history(cr, w as f64, h as f64, &r.borrow(), win.get()));
    let card = ui::card();
    card.append(&chart);
    let legend = ui::label("", &["status"]);
    legend.set_use_markup(true);
    legend.set_margin_start(14);
    legend.set_margin_bottom(8);
    legend.set_markup(&format!(
        "<span foreground='{}'>━</span> charge   <span foreground='{}'>━</span> power draw",
        theme::hex("primary"),
        theme::hex("tertiary")
    ));
    card.append(&legend);
    body.append(&card);

    let reload = {
        let (rows, window, chart) = (rows.clone(), window.clone(), chart.clone());
        Rc::new(move || {
            let (rows, chart, secs) = (rows.clone(), chart.clone(), window.get());
            util::bg(move || read_history(secs), move |v| {
                *rows.borrow_mut() = v;
                chart.queue_draw();
            });
        })
    };
    for (btn, other, secs) in [(&day, &week, DAY), (&week, &day, 7.0 * DAY)] {
        let (other, window, reload) = (other.clone(), window.clone(), reload.clone());
        btn.connect_clicked(move |b| {
            b.add_css_class("selected");
            other.remove_css_class("selected");
            window.set(secs);
            reload();
        });
    }
    ui::every_while_mapped(&chart, 30, move || reload());
}

// ---- auto switch (done by `rice-battery daemon`, which reads this file on each plug/unplug)
fn auto_settings() -> serde_json::Value {
    let text = fs::read_to_string(util::cfg("rice/battery.json")).unwrap_or_default();
    match serde_json::from_str(&text) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        _ => serde_json::json!({}),
    }
}

fn set_auto(on: bool) -> bool {
    let mut v = auto_settings();
    v["auto_profile"] = on.into();
    let path = util::cfg("rice/battery.json");
    path.parent().is_some_and(|d| fs::create_dir_all(d).is_ok())
        && fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default() + "\n").is_ok()
}

fn auto_row(card: &gtk::Box) {
    let v = auto_settings();
    let text = |key: &str, default: &'static str| v[key].as_str().unwrap_or(default).replace('-', " ");
    let r = ui::row(
        "󰚥",
        "switch automatically",
        &format!("{} on battery · {} when plugged in", text("on_battery", "power-saver"), text("on_ac", "balanced")),
    );
    let s = ui::switch(v["auto_profile"].as_bool().unwrap_or(true));
    s.connect_state_set(|_, on| {
        if !set_auto(on) {
            util::notify("power", "couldn't save the auto switch setting");
        }
        glib::Propagation::Proceed
    });
    r.suffix(&s);
    ui::add(card, &r.row);
}

// ---- busiest apps. Linux has no per-app watts, so CPU share is the closest measure.
type CpuTicks = HashMap<u32, (String, u64)>;

fn read_cpu_ticks() -> CpuTicks {
    let mut out = HashMap::new();
    for e in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(raw) = fs::read_to_string(e.path().join("stat")) else { continue };
        let (Some(open), Some(close)) = (raw.find('('), raw.rfind(')')) else { continue };
        let f: Vec<&str> = raw[close + 1..].split_whitespace().collect();
        if f.len() < 13 || f[1] == "2" || pid == 2 {
            continue; // kernel threads
        }
        let ticks = f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0);
        out.insert(pid, (raw[open + 1..close].to_string(), ticks));
    }
    out
}

fn top_apps(prev: &CpuTicks, cur: &CpuTicks) -> Vec<(String, f64)> {
    let mut by_name: HashMap<&str, u64> = HashMap::new();
    for (pid, (name, ticks)) in cur {
        let was = prev.get(pid).map_or(*ticks, |p| p.1);
        if *ticks > was && name != "rice-settings" {
            *by_name.entry(name).or_default() += ticks - was;
        }
    }
    let busy: u64 = by_name.values().sum();
    let mut top: Vec<(String, f64)> =
        by_name.into_iter().map(|(n, t)| (n.to_string(), 100.0 * t as f64 / busy as f64)).collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1));
    top.truncate(5);
    top
}

fn render_apps(card: &gtk::Box, apps: &[(String, f64)]) {
    ui::clear(card);
    if apps.is_empty() {
        card.append(&ui::label("measuring…", &["empty"]));
    }
    for (name, share) in apps {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("app-row");
        let n = ui::label(name, &[]);
        n.set_width_chars(18);
        n.set_max_width_chars(18);
        n.set_ellipsize(pango::EllipsizeMode::End);
        let bar = gtk::ProgressBar::new();
        bar.set_fraction(share / 100.0);
        bar.set_hexpand(true);
        bar.set_valign(gtk::Align::Center);
        let v = ui::label(&format!("{share:.0}%"), &[]);
        v.set_width_chars(4);
        v.set_xalign(1.0);
        row.append(&n);
        row.append(&bar);
        row.append(&v);
        card.append(&row);
    }
}

fn apps_section(body: &gtk::Box) {
    let card = ui::section(body, "busiest apps");
    render_apps(&card, &[]);
    body.append(&ui::label("share of cpu use, not measured watts", &["status"]));
    let prev = Rc::new(RefCell::new(read_cpu_ticks()));
    let refresh = {
        let card = card.clone();
        Rc::new(move || {
            let (card, prev) = (card.clone(), prev.clone());
            util::bg(read_cpu_ticks, move |cur| {
                let apps = top_apps(&prev.borrow(), &cur);
                *prev.borrow_mut() = cur;
                if !apps.is_empty() {
                    render_apps(&card, &apps);
                }
            });
        })
    };
    let first = refresh.clone();
    glib::timeout_add_seconds_local_once(1, move || first());
    ui::every_while_mapped(&card, 5, move || refresh());
}

fn bat_icon(c: u32, charging: bool) -> &'static str {
    if charging {
        return "󰂄";
    }
    ["󰂎", "󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"][(c.min(100) / 10) as usize]
}

fn hm(h: f64) -> String {
    let m = (h * 60.0).round() as u64;
    if m >= 60 { format!("{} h {} min", m / 60, m % 60) } else { format!("{m} min") }
}

fn asus_backend() -> bool {
    deps::have("asusctl") && deps::service_active("asusd")
}

const PROFILES: &[(&str, &str, &str)] = &[
    ("power-saver", "󰾆", "quiet & cool · longest battery"),
    ("balanced", "󰾅", "the everyday default"),
    ("performance", "󰓅", "full speed · fans and heat"),
];

struct Ui {
    icon: gtk::Label,
    pct: gtk::Label,
    state: gtk::Label,
    health: ui::Row,
    draw: ui::Row,
    cycles: ui::Row,
    gpu: ui::Row,
    limit: Option<(gtk::Scale, Rc<Cell<bool>>)>,
    profiles: Vec<(String, gtk::Button)>,
}

fn render(u: &Ui, b: Option<Bat>) {
    if let Some(b) = b {
        let charging = b.status == "Charging";
        u.icon.set_text(bat_icon(b.capacity, charging));
        u.pct.set_text(&format!("{}%", b.capacity));
        let lim = b.limit.filter(|l| *l < 100);
        let mut s = match b.status.as_str() {
            "Charging" => "charging".to_string(),
            "Discharging" => "on battery".to_string(),
            "Full" => "fully charged".to_string(),
            "Not charging" if b.ac => match lim {
                Some(l) if b.capacity + 2 >= l => format!("plugged in · held at the {l}% limit"),
                _ => "plugged in · not charging".to_string(),
            },
            other => other.to_lowercase(),
        };
        if b.power_w > 0.3 {
            if b.status == "Discharging" {
                s.push_str(&format!(" · {} left", hm(b.now_wh / b.power_w)));
            } else if charging {
                let target = b.full_wh * lim.unwrap_or(100) as f64 / 100.0;
                if target > b.now_wh {
                    s.push_str(&format!(" · full in {}", hm((target - b.now_wh) / b.power_w)));
                }
            }
        }
        u.state.set_text(&s);
        if b.design_wh > 0.0 {
            u.health.set_sub(&format!(
                "{:.0}% · holds {:.1} of {:.1} Wh when new{}",
                b.full_wh / b.design_wh * 100.0,
                b.full_wh,
                b.design_wh,
                if b.model.is_empty() { String::new() } else { format!(" · {}", b.model) }
            ));
        }
        u.draw.set_sub(&if b.power_w > 0.05 { format!("{:.1} W", b.power_w) } else { "idle".into() });
        u.cycles.set_sub(&b.cycles.map(|c| c.to_string()).unwrap_or_else(|| "not reported".into()));
        u.gpu.row.set_visible(b.gpu.is_some());
        u.gpu.set_sub(b.gpu.as_deref().unwrap_or(""));
        if let (Some((s, loading)), Some(l)) = (&u.limit, b.limit) {
            loading.set(true);
            s.set_value(l as f64);
            loading.set(false);
        }
    }
    if deps::have("powerprofilesctl") {
        let cur = util::sh("powerprofilesctl", &["get"]).unwrap_or_default().trim().to_string();
        for (id, btn) in &u.profiles {
            if *id == cur { btn.add_css_class("selected") } else { btn.remove_css_class("selected") }
        }
    }
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰁹", "power", "battery and power profiles");
    let bat = fetch();

    // ---- battery hero
    let hero = ui::card();
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    h.add_css_class("hero");
    let icon = ui::label("󰁹", &["hero-logo"]);
    icon.set_xalign(0.5);
    let t = gtk::Box::new(gtk::Orientation::Vertical, 4);
    t.set_valign(gtk::Align::Center);
    let pct = ui::label("—", &["hero-title"]);
    let state = ui::label("", &["dim"]);
    t.append(&pct);
    t.append(&state);
    h.append(&icon);
    h.append(&t);
    hero.append(&h);

    let details = ui::card();
    let health = ui::row("󰂑", "health", "");
    let draw = ui::row("󱐋", "power draw", "");
    let cycles = ui::row("󰑓", "charge cycles", "");
    ui::add(&details, &health.row);
    ui::add(&details, &draw.row);
    ui::add(&details, &cycles.row);
    let gpu = ui::row("󰢮", "nvidia gpu", "");
    ui::add(&details, &gpu.row);

    if bat.is_some() {
        p.body.append(&hero);
        p.body.append(&ui::label("battery", &["section"]));
        p.body.append(&details);
        if deps::have("rice-battery") {
            history_section(&p.body);
        }
    } else {
        let c = ui::card();
        c.append(&ui::label("no battery found · this looks like a desktop", &["empty"]));
        p.body.append(&c);
    }

    // ---- charge limit
    let mut limit = None;
    if let Some(cur) = bat.as_ref().and_then(|b| b.limit) {
        let c = ui::section(&p.body, "charge limit");
        let asus = asus_backend();
        let s = ui::slider(20.0, 100.0, 5.0, 0, cur as f64);
        let r = ui::row(
            "󰂃",
            "stop charging at",
            if asus { "80% is kinder to the battery" } else { "80% is kinder to the battery · resets at boot" },
        );
        r.suffix(&s);
        ui::add(&c, &r.row);
        let status = ui::label("", &["status"]);
        let loading = Rc::new(Cell::new(false));
        let deb = ui::Debounce::default();
        let (l, st) = (loading.clone(), status.clone());
        s.connect_value_changed(move |s| {
            if l.get() {
                return;
            }
            let v = ((s.value() / 5.0).round() * 5.0) as u32;
            let st = st.clone();
            deb.run(700, move || {
                st.set_text("setting limit…");
                let cb = move |o: util::Out| st.set_text(&if o.ok { format!("charging stops at {v}%") } else { format!("failed: {}", o.msg()) });
                if asus_backend() {
                    util::run("asusctl", &["battery", "limit", &v.to_string()], cb);
                } else if let Some(d) = battery_dir() {
                    let f = d.join("charge_control_end_threshold");
                    util::run("pkexec", &["sh", "-c", &format!("echo {v} > '{}'", f.display())], cb);
                }
            });
        });
        if asus {
            let once = ui::button("charge to 100% once", &["flat"]);
            once.set_tooltip_text(Some("one full charge, then back to the limit"));
            let st = status.clone();
            once.connect_clicked(move |_| {
                let st = st.clone();
                util::run("asusctl", &["battery", "oneshot"], move |o| {
                    st.set_text(if o.ok { "charging to 100% this once" } else { "failed" })
                });
            });
            r.suffix(&once);
        }
        p.body.append(&status);
        limit = Some((s, loading));
    }

    // ---- power profiles
    p.body.append(&ui::label("power profile", &["section"]));
    if let Some(b) = deps::banner(&[&deps::PPD], || crate::reload_page("power")) {
        p.body.append(&b);
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_homogeneous(true);
    let mut profiles = Vec::new();
    if deps::have("powerprofilesctl") {
        let avail = util::sh("powerprofilesctl", &["list"]).unwrap_or_default();
        for (id, ic, desc) in PROFILES {
            if !avail.contains(&format!("{id}:")) {
                continue;
            }
            let b = gtk::Button::new();
            b.add_css_class("profile");
            let v = gtk::Box::new(gtk::Orientation::Vertical, 4);
            v.append(&ui::label(ic, &["profile-icon"]));
            v.append(&ui::label(&id.replace('-', " "), &["profile-name"]));
            let d = ui::label(desc, &["row-sub"]);
            d.set_wrap(true);
            v.append(&d);
            b.set_child(Some(&v));
            let id2 = id.to_string();
            b.connect_clicked(move |b| {
                let row = b.parent().unwrap();
                let mut c = row.first_child();
                while let Some(w) = c {
                    w.remove_css_class("selected");
                    c = w.next_sibling();
                }
                b.add_css_class("selected");
                util::run("powerprofilesctl", &["set", &id2], |o| {
                    if !o.ok {
                        util::notify("power profile", &o.msg());
                    }
                    util::spawn("pkill", &["-RTMIN+8", "waybar"]); // rice bar shows the profile
                });
            });
            row.append(&b);
            profiles.push((id.to_string(), b));
        }
        p.body.append(&row);
        if bat.is_some() && deps::have("rice-battery") {
            let c = ui::card();
            auto_row(&c);
            c.set_margin_top(8);
            p.body.append(&c);
        }
    }

    apps_section(&p.body);

    let u = Rc::new(Ui { icon, pct, state, health, draw, cycles, gpu, limit, profiles });
    render(&u, bat);
    let u2 = u.clone();
    ui::every_while_mapped(&p.root, 5, move || {
        let u3 = u2.clone();
        util::bg(fetch, move |b| render(&u3, b));
    });
    p.root.upcast()
}
