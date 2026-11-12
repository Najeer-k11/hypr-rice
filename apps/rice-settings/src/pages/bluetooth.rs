//! Bluetooth via bluetoothctl: power, scan, pair, connect / disconnect, remove.

use crate::{deps, ui, util};
use gtk::prelude::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
struct Dev {
    mac: String,
    name: String,
    icon: String,
    paired: bool,
    connected: bool,
    battery: Option<u32>,
}

struct Data {
    controller: bool,
    powered: bool,
    alias: String,
    devs: Vec<Dev>,
}

fn fetch() -> Data {
    let show = util::sh("bluetoothctl", &["show"]).unwrap_or_default();
    if !show.contains("Controller") {
        return Data { controller: false, powered: false, alias: String::new(), devs: vec![] };
    }
    let field = |txt: &str, k: &str| {
        txt.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default()
    };
    let powered = field(&show, "Powered:") == "yes";
    let alias = field(&show, "Alias:");
    let mut devs = Vec::new();
    for l in util::sh("bluetoothctl", &["devices"]).unwrap_or_default().lines() {
        let mut it = l.splitn(3, ' ');
        let (Some("Device"), Some(mac)) = (it.next(), it.next()) else { continue };
        let info = util::sh("bluetoothctl", &["info", mac]).unwrap_or_default();
        let name = field(&info, "Name:");
        if name.is_empty() {
            continue; // nameless beacons: noise
        }
        let battery = field(&info, "Battery Percentage:")
            .split(['(', ')'])
            .nth(1)
            .and_then(|v| v.parse().ok());
        devs.push(Dev {
            mac: mac.to_string(),
            name,
            icon: field(&info, "Icon:"),
            paired: field(&info, "Paired:") == "yes",
            connected: field(&info, "Connected:") == "yes",
            battery,
        });
    }
    devs.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.cmp(&b.name)));
    Data { controller: true, powered, alias, devs }
}

fn dev_icon(i: &str) -> &'static str {
    match i {
        s if s.starts_with("audio-head") => "󰋋",
        s if s.starts_with("audio") => "󰓃",
        "input-mouse" => "󰍽",
        "input-keyboard" => "󰌌",
        "input-gaming" => "󰊴",
        "phone" => "󰏲",
        "computer" => "󰌢",
        _ => "󰂯",
    }
}

struct Ui {
    switch: gtk::Switch,
    paired: gtk::Box,
    found: gtk::Box,
    status: gtk::Label,
    sub: gtk::Label,
    loading: Cell<bool>,
    busy: Cell<bool>,
}

fn refresh(ui: &Rc<Ui>) {
    if ui.busy.replace(true) {
        return;
    }
    let u = ui.clone();
    util::bg(fetch, move |d| {
        u.busy.set(false);
        render(&u, d);
    });
}

fn act(ui: &Rc<Ui>, msg: &str, prog: &str, args: &[&str]) {
    ui.status.set_text(msg);
    let u = ui.clone();
    util::run(prog, args, move |o| {
        u.status.set_text(&if o.ok { String::new() } else { format!("failed: {}", o.msg()) });
        refresh(&u);
    });
}

fn render(ui: &Rc<Ui>, d: Data) {
    ui.loading.set(true);
    ui.switch.set_active(d.powered);
    ui.switch.set_sensitive(d.controller);
    ui.loading.set(false);
    let n_conn = d.devs.iter().filter(|x| x.connected).count();
    ui.sub.set_text(&match (d.controller, d.powered) {
        (false, _) => "no bluetooth adapter found".into(),
        (_, false) => "bluetooth is off".into(),
        _ => format!("{} · {} connected", d.alias, n_conn),
    });
    ui::clear(&ui.paired);
    ui::clear(&ui.found);
    if !d.powered {
        ui.paired.append(&ui::label("turn bluetooth on to see devices", &["empty"]));
        ui.found.append(&ui::label("—", &["empty"]));
        return;
    }
    for dv in d.devs {
        let sub = match (dv.connected, dv.paired) {
            (true, _) => format!("connected{}", dv.battery.map(|b| format!(" · 󰥉 {b}%")).unwrap_or_default()),
            (_, true) => "paired".into(),
            _ => dv.mac.clone(),
        };
        let r = ui::row(dev_icon(&dv.icon), &dv.name, &sub);
        if dv.connected {
            r.icon.add_css_class("on");
        }
        let m = dv.mac.clone();
        if dv.paired {
            let rm = ui::button("󰆴", &["flat", "danger", "icon"]);
            rm.set_tooltip_text(Some("remove / unpair"));
            let (u, m2) = (ui.clone(), m.clone());
            rm.connect_clicked(move |_| act(&u, "removing…", "bluetoothctl", &["remove", &m2]));
            r.suffix(&rm);
            let (label, verb, msg) = if dv.connected { ("disconnect", "disconnect", "disconnecting…") } else { ("connect", "connect", "connecting…") };
            let b = ui::button(label, &[]);
            let u = ui.clone();
            b.connect_clicked(move |_| act(&u, msg, "bluetoothctl", &[verb, &m]));
            r.suffix(&b);
            ui::add(&ui.paired, &r.row);
        } else {
            let b = ui::button("pair", &["suggested"]);
            let u = ui.clone();
            b.connect_clicked(move |_| {
                let cmd = format!("bluetoothctl --timeout 20 pair {m} && bluetoothctl trust {m} && bluetoothctl connect {m}");
                act(&u, "pairing… (confirm on the device if asked)", "sh", &["-c", &cmd]);
            });
            r.suffix(&b);
            ui::add(&ui.found, &r.row);
        }
    }
    if ui.paired.first_child().is_none() {
        ui.paired.append(&ui::label("no paired devices", &["empty"]));
    }
    if ui.found.first_child().is_none() {
        ui.found.append(&ui::label("nothing nearby yet · press scan", &["empty"]));
    }
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰂯", "bluetooth", "");
    let scan = ui::button("󰂰  scan", &[]);
    p.actions.append(&scan);

    if let Some(b) = deps::banner(&[&deps::BLUEZ], || crate::reload_page("bluetooth")) {
        p.body.append(&b);
    }
    if !deps::installed(&deps::BLUEZ) {
        return p.root.upcast();
    }
    let top = ui::card();
    let sw = ui::switch(false);
    let r = ui::row("󰂯", "bluetooth", "adapter power");
    r.icon.add_css_class("on");
    r.suffix(&sw);
    top.append(&r.row);
    p.body.append(&top);

    let paired = ui::section(&p.body, "my devices");
    paired.append(&ui::label("loading…", &["empty"]));
    let found = ui::section(&p.body, "nearby");
    let status = ui::label("", &["status"]);
    p.body.append(&status);

    let ui = Rc::new(Ui { switch: sw.clone(), paired, found, status, sub: p.sub.clone(), loading: Cell::new(false), busy: Cell::new(false) });

    let u = ui.clone();
    sw.connect_active_notify(move |s| {
        if u.loading.get() {
            return;
        }
        if s.is_active() {
            act(&u, "turning on…", "sh", &["-c", "rfkill unblock bluetooth; sleep 0.3; bluetoothctl power on"]);
        } else {
            act(&u, "turning off…", "bluetoothctl", &["power", "off"]);
        }
    });
    let u = ui.clone();
    scan.connect_clicked(move |b| {
        b.set_sensitive(false);
        u.status.set_text("scanning for 10 s…");
        let (u2, b2) = (u.clone(), b.clone());
        util::run("bluetoothctl", &["--timeout", "10", "scan", "on"], move |_| {
            b2.set_sensitive(true);
            u2.status.set_text("");
            refresh(&u2);
        });
    });

    let u = ui.clone();
    p.root.connect_map(move |_| refresh(&u));
    let u = ui.clone();
    ui::every_while_mapped(&p.root, 8, move || refresh(&u));
    p.root.upcast()
}
