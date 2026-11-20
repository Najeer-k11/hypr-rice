//! Wi-Fi via nmcli: radio toggle, network list, connect (with inline password), disconnect, forget.

use crate::{deps, ui, util};
use gtk::prelude::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
struct Net {
    ssid: String,
    signal: u32,
    secure: bool,
    active: bool,
    saved: bool,
}

struct Data {
    radio: bool,
    nets: Vec<Net>,
    active_con: Option<String>,
    ip: String,
}

fn fetch(rescan: bool) -> Data {
    let radio = util::sh("nmcli", &["-t", "radio", "wifi"]).is_some_and(|s| s.trim() == "enabled");
    if !radio {
        return Data { radio, nets: vec![], active_con: None, ip: String::new() };
    }
    if rescan {
        let _ = util::sh("nmcli", &["dev", "wifi", "rescan"]);
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
    let saved: Vec<String> = util::sh("nmcli", &["-t", "-f", "NAME,TYPE", "con", "show"])
        .unwrap_or_default()
        .lines()
        .map(util::split_terse)
        .filter(|f| f.len() >= 2 && f[1] == "802-11-wireless")
        .map(|f| f[0].clone())
        .collect();
    let mut active_con = None;
    let mut dev = String::new();
    for f in util::sh("nmcli", &["-t", "-f", "NAME,TYPE,DEVICE", "con", "show", "--active"])
        .unwrap_or_default()
        .lines()
        .map(util::split_terse)
    {
        if f.len() >= 3 && f[1] == "802-11-wireless" {
            active_con = Some(f[0].clone());
            dev = f[2].clone();
        }
    }
    let ip = if dev.is_empty() {
        String::new()
    } else {
        util::sh("nmcli", &["-t", "-g", "IP4.ADDRESS", "dev", "show", &dev])
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    };
    let mut nets: Vec<Net> = Vec::new();
    for f in util::sh("nmcli", &["-t", "-f", "IN-USE,SIGNAL,SECURITY,SSID", "dev", "wifi", "list", "--rescan", "no"])
        .unwrap_or_default()
        .lines()
        .map(util::split_terse)
    {
        if f.len() < 4 || f[3].is_empty() {
            continue;
        }
        let n = Net {
            ssid: f[3].clone(),
            signal: f[1].parse().unwrap_or(0),
            secure: !f[2].is_empty() && f[2] != "--",
            active: f[0] == "*",
            saved: saved.contains(&f[3]),
        };
        match nets.iter_mut().find(|x| x.ssid == n.ssid) {
            Some(x) => {
                x.signal = x.signal.max(n.signal);
                x.active |= n.active;
            }
            None => nets.push(n),
        }
    }
    nets.sort_by(|a, b| b.active.cmp(&a.active).then(b.saved.cmp(&a.saved)).then(b.signal.cmp(&a.signal)));
    Data { radio, nets, active_con, ip }
}

fn strength_icon(s: u32) -> &'static str {
    match s {
        75.. => "󰤨",
        50.. => "󰤥",
        25.. => "󰤢",
        _ => "󰤟",
    }
}

struct Ui {
    switch: gtk::Switch,
    list: gtk::Box,
    status: gtk::Label,
    sub: gtk::Label,
    loading: Cell<bool>,
    busy: Cell<bool>,
}

fn refresh(ui: &Rc<Ui>, rescan: bool) {
    if ui.busy.replace(true) {
        return;
    }
    if rescan {
        ui.status.set_text("Scanning…");
    }
    let u = ui.clone();
    util::bg(move || fetch(rescan), move |d| {
        u.busy.set(false);
        render(&u, d);
    });
}

fn act(ui: &Rc<Ui>, msg: &str, prog: &str, args: &[&str]) {
    ui.status.set_text(msg);
    let u = ui.clone();
    util::run(prog, args, move |o| {
        u.status.set_text(&if o.ok { String::new() } else { format!("Failed: {}", o.msg()) });
        refresh(&u, false);
    });
}

fn render(ui: &Rc<Ui>, d: Data) {
    ui.loading.set(true);
    ui.switch.set_active(d.radio);
    ui.loading.set(false);
    ui.sub.set_text(&match (&d.active_con, d.radio) {
        (_, false) => "Wi-Fi is off".to_string(),
        (Some(c), _) => format!("Connected to {c}{}", if d.ip.is_empty() { String::new() } else { format!(" · {}", d.ip) }),
        (None, _) => "Not connected".to_string(),
    });
    ui::clear(&ui.list);
    if !d.radio {
        ui.list.append(&ui::label("Turn Wi-Fi on to see networks", &["empty"]));
        return;
    }
    if d.nets.is_empty() {
        ui.list.append(&ui::label("No networks found · try rescan", &["empty"]));
        return;
    }
    for n in d.nets {
        let wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let sub = match (n.active, n.saved, n.secure) {
            (true, _, _) => format!("connected{}", if d.ip.is_empty() { String::new() } else { format!(" · {}", d.ip) }),
            (_, true, _) => "saved".into(),
            (_, _, true) => "secured".into(),
            _ => "open".into(),
        };
        let r = ui::row(strength_icon(n.signal), &n.ssid, &sub);
        if n.active {
            r.icon.add_css_class("on");
        }
        let meta = ui::label(&format!("{}{}%", if n.secure { "\u{f023}  " } else { "" }, n.signal), &["dim"]);
        r.suffix(&meta);

        // inline password
        let rev = gtk::Revealer::new();
        let pw_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pw_row.add_css_class("row");
        pw_row.set_margin_start(36);
        let pw = gtk::PasswordEntry::new();
        pw.set_show_peek_icon(true);
        pw.set_hexpand(true);
        pw.set_placeholder_text(Some("Password"));
        let go = ui::button("Connect", &["suggested"]);
        pw_row.append(&pw);
        pw_row.append(&go);
        rev.set_child(Some(&pw_row));

        if n.active {
            let forget = ui::button("Forget", &["flat", "danger"]);
            let dis = ui::button("Disconnect", &[]);
            let (u, con) = (ui.clone(), d.active_con.clone().unwrap_or(n.ssid.clone()));
            dis.connect_clicked(move |_| act(&u, "Disconnecting…", "nmcli", &["con", "down", "id", &con]));
            let (u, s) = (ui.clone(), n.ssid.clone());
            forget.connect_clicked(move |_| act(&u, "Forgetting…", "nmcli", &["con", "delete", "id", &s]));
            r.suffix(&forget);
            r.suffix(&dis);
        } else {
            if n.saved {
                let forget = ui::button("Forget", &["flat", "danger"]);
                let (u, s) = (ui.clone(), n.ssid.clone());
                forget.connect_clicked(move |_| act(&u, "Forgetting…", "nmcli", &["con", "delete", "id", &s]));
                r.suffix(&forget);
            }
            let con = ui::button("Connect", &[]);
            let (u, n2, rv, pwf) = (ui.clone(), n.clone(), rev.clone(), pw.clone());
            con.connect_clicked(move |_| {
                if n2.saved {
                    act(&u, &format!("Connecting to {}…", n2.ssid), "nmcli", &["con", "up", "id", &n2.ssid]);
                } else if n2.secure {
                    rv.set_reveal_child(!rv.reveals_child());
                    pwf.grab_focus();
                } else {
                    act(&u, &format!("Connecting to {}…", n2.ssid), "nmcli", &["dev", "wifi", "connect", &n2.ssid]);
                }
            });
            r.suffix(&con);
            let submit = {
                let (u, s, pw) = (ui.clone(), n.ssid.clone(), pw.clone());
                move || {
                    let t = pw.text().to_string();
                    if t.is_empty() {
                        return;
                    }
                    act(&u, &format!("Connecting to {s}…"), "nmcli", &["dev", "wifi", "connect", &s, "password", &t]);
                }
            };
            let s2 = submit.clone();
            go.connect_clicked(move |_| s2());
            pw.connect_activate(move |_| submit());
        }
        wrap.append(&r.row);
        wrap.append(&rev);
        ui::add(&ui.list, &wrap);
    }
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰤨", "Wi-Fi", "");


    if let Some(b) = deps::banner(&[&deps::NETWORKMANAGER], || crate::reload_page("wifi")) {
        p.body.append(&b);
    }
    if !deps::installed(&deps::NETWORKMANAGER) {
        return p.root.upcast();
    }
    let rescan = ui::button("󰑐  rescan", &[]);
    let adv = ui::button("\u{f013}  advanced", &["flat"]);
    adv.connect_clicked(|_| util::spawn("nm-connection-editor", &[]));
    p.actions.append(&adv);
    p.actions.append(&rescan);
    let top = ui::card();
    let sw = ui::switch(false);
    let r = ui::row("󰖩", "Wi-Fi", "Wireless radio");
    r.icon.add_css_class("on");
    r.suffix(&sw);
    top.append(&r.row);
    p.body.append(&top);

    let status = ui::label("", &["status"]);
    p.body.append(&ui::label("Networks", &["section"]));
    let list = ui::card();
    list.append(&ui::label("Loading…", &["empty"]));
    p.body.append(&list);
    p.body.append(&status);

    let ui = Rc::new(Ui { switch: sw.clone(), list, status, sub: p.sub.clone(), loading: Cell::new(false), busy: Cell::new(false) });

    let u = ui.clone();
    sw.connect_active_notify(move |s| {
        if u.loading.get() {
            return;
        }
        let on = s.is_active();
        act(&u, if on { "Turning on…" } else { "Turning off…" }, "nmcli", &["radio", "wifi", if on { "on" } else { "off" }]);
    });
    let u = ui.clone();
    rescan.connect_clicked(move |_| refresh(&u, true));

    let u = ui.clone();
    p.root.connect_map(move |_| refresh(&u, false));
    let u = ui.clone();
    ui::every_while_mapped(&p.root, 10, move || refresh(&u, false));
    p.root.upcast()
}
