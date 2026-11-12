//! System overview.

use crate::{deps, ui, util};
use gtk::prelude::*;
use std::fs;

struct Info {
    host: String,
    model: String,
    user: String,
    os: String,
    kernel: String,
    hypr: String,
    cpu: String,
    gpu: String,
    mem: (u64, u64),
    swap: (u64, u64),
    disk: (u64, u64),
    uptime: String,
    display: String,
    pkgs: String,
    shell: String,
    wall: String,
    self_rss: u64,
}

fn meminfo() -> (u64, u64, u64, u64) {
    let t = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let get = |k: &str| {
        t.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    (get("MemTotal:"), get("MemAvailable:"), get("SwapTotal:"), get("SwapFree:"))
}

fn self_rss() -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

/// GPU names from sysfs + pci.ids. Unlike `lspci`, this never wakes a sleeping dGPU.
fn gpus() -> String {
    let ids = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())
        .unwrap_or_default();
    let lookup = |ven: &str, dev: &str| -> Option<String> {
        let mut vname = None;
        for l in ids.lines() {
            if vname.is_none() {
                if let Some(rest) = l.strip_prefix(ven) {
                    vname = Some(rest.trim().replace(" Corporation", "").replace("Advanced Micro Devices, Inc. ", ""));
                }
            } else if let Some(rest) = l.strip_prefix('\t').and_then(|r| r.strip_prefix(dev)) {
                if !l.starts_with("\t\t") {
                    return Some(format!("{} {}", vname.unwrap(), rest.trim()));
                }
            } else if !l.starts_with('\t') && !l.starts_with('#') && !l.is_empty() {
                break; // next vendor
            }
        }
        vname
    };
    let mut out = Vec::new();
    for d in fs::read_dir("/sys/bus/pci/devices").map(|r| r.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default() {
        let rd = |f: &str| fs::read_to_string(d.join(f)).map(|s| s.trim().trim_start_matches("0x").to_string()).unwrap_or_default();
        if !rd("class").starts_with("03") {
            continue;
        }
        let (v, dv) = (rd("vendor"), rd("device"));
        out.push(lookup(&v, &dv).unwrap_or(format!("{v}:{dv}")));
    }
    out.sort();
    out.join("\n")
}

fn gather() -> Info {
    let os = fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        .unwrap_or_else(|| "Linux".into());
    let host = fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| util::sh("uname", &["-n"]).map(|s| s.trim().to_string()))
        .unwrap_or_default();
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_string()))
        .unwrap_or_default();
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    let gpu = gpus();
    let hypr = util::sh("hyprctl", &["version", "-j"])
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["version"].as_str().or(v["tag"].as_str()).map(String::from))
        .unwrap_or_default();
    let (mt, ma, st, sf) = meminfo();
    let disk = util::sh("df", &["-B1", "--output=size,used", "/"])
        .and_then(|s| {
            let l = s.lines().nth(1)?.to_string();
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .unwrap_or((0, 0));
    let up = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    let uptime = match (up / 86400, (up % 86400) / 3600, (up % 3600) / 60) {
        (0, 0, m) => format!("{m} min"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, h, _) => format!("{d} d {h} h"),
    };
    let display = util::sh("hyprctl", &["monitors", "-j"])
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|m| {
            format!(
                "{}  {}×{} @ {:.0} Hz · scale {}",
                m["name"].as_str().unwrap_or(""),
                m["width"], m["height"],
                m["refreshRate"].as_f64().unwrap_or(0.0),
                (m["scale"].as_f64().unwrap_or(1.0) * 100.0).round() / 100.0
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    // count package dirs directly: instant, unlike `pacman -Qq`
    let pkgs = fs::read_dir("/var/lib/pacman/local")
        .map(|r| format!("{} (pacman)", r.flatten().filter(|e| e.path().is_dir()).count()))
        .or_else(|_| fs::read_dir("/var/lib/dpkg/info").map(|r| format!("{} (dpkg)", r.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "list")).count())))
        .unwrap_or_default();
    let shell = std::env::var("SHELL").unwrap_or_default().rsplit('/').next().unwrap_or("").to_string();
    let wall = std::path::Path::new(&super::wallpaper::current())
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dmi = |f: &str| fs::read_to_string(format!("/sys/class/dmi/id/{f}")).map(|s| s.trim().to_string()).unwrap_or_default();
    let vendor = dmi("sys_vendor").replace("ASUSTeK COMPUTER INC.", "ASUS").replace("LENOVO", "Lenovo");
    let product = dmi("product_name");
    // ASUS puts "Family Model_Model" in product_name; Lenovo hides the name in product_version
    let product = product.split('_').next().unwrap_or("").to_string();
    let product = if vendor == "Lenovo" { dmi("product_version") } else { product };
    let machine = if product.to_lowercase().starts_with(&vendor.to_lowercase()) || vendor.is_empty() {
        product
    } else {
        format!("{vendor} {product}")
    };
    Info {
        host,
        model: machine.trim().to_string(),
        user: std::env::var("USER").unwrap_or_default(),
        os,
        kernel: util::sh("uname", &["-r"]).unwrap_or_default().trim().to_string(),
        hypr,
        cpu: if threads > 0 { format!("{model} · {threads} threads") } else { model },
        gpu,
        mem: (mt.saturating_sub(ma), mt),
        swap: (st.saturating_sub(sf), st),
        disk: (disk.1, disk.0),
        uptime,
        display,
        pkgs,
        shell,
        wall,
        self_rss: self_rss(),
    }
}

fn usage_row(card: &gtk::Box, icon: &str, title: &str, (used, total): (u64, u64)) {
    let r = ui::row(icon, title, &format!("{} of {}", util::human_bytes(used), util::human_bytes(total)));
    // plain boxes instead of GtkLevelBar: exact 6px pill, no theme surprises
    const W: i32 = 180;
    let frac = if total > 0 { used as f64 / total as f64 } else { 0.0 };
    let lb = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    lb.add_css_class("meter");
    lb.set_size_request(W, 6);
    lb.set_valign(gtk::Align::Center);
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.add_css_class("meter-fill");
    fill.set_size_request(((W as f64 * frac).round() as i32).max(6), 6);
    fill.set_halign(gtk::Align::Start);
    lb.append(&fill);
    let pct = ui::label(&format!("{:>3.0}%", if total > 0 { used as f64 * 100.0 / total as f64 } else { 0.0 }), &["dim"]);
    r.suffix(&lb);
    r.suffix(&pct);
    ui::add(card, &r.row);
}

fn render(body: &gtk::Box, i: Info) {
    ui::clear(body);
    // hero
    let hero = ui::card();
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    h.add_css_class("hero");
    let logo = ui::label(if i.os.to_lowercase().contains("arch") { "\u{f303}" } else { "\u{f17c}" }, &["hero-logo"]);
    logo.set_xalign(0.5);
    h.append(&logo);
    let t = gtk::Box::new(gtk::Orientation::Vertical, 6);
    t.set_valign(gtk::Align::Center);
    t.append(&ui::label(&i.host, &["hero-title"]));
    t.append(&ui::label(&if i.model.is_empty() { i.os.clone() } else { format!("{} · {}", i.model, i.os) }, &["dim"]));
    let badges = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for b in [format!("hyprland {}", i.hypr), format!("up {}", i.uptime)] {
        let l = ui::label(&b, &["badge"]);
        badges.append(&l);
    }
    t.append(&badges);
    h.append(&t);
    hero.append(&h);
    body.append(&hero);

    // ---- laptop name (hostname)
    let c = ui::section(body, "name");
    let r = ui::row("󰌢", "device name", "how this machine shows up on networks and bluetooth");
    let entry = gtk::Entry::new();
    entry.set_text(&i.host);
    entry.set_width_chars(18);
    entry.set_valign(gtk::Align::Center);
    let save = ui::button("rename", &[]);
    save.set_sensitive(false);
    r.suffix(&entry);
    r.suffix(&save);
    ui::add(&c, &r.row);
    let valid = |t: &str| !t.is_empty() && t.len() <= 63 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !t.starts_with('-');
    let (sv, cur) = (save.clone(), i.host.clone());
    entry.connect_changed(move |e| {
        let t = e.text();
        sv.set_sensitive(valid(&t) && t.as_str() != cur);
    });
    let (e2, sub) = (entry.clone(), r.sub.clone());
    let do_rename = move || {
        let name = e2.text().to_string();
        if !valid(&name) {
            sub.set_text("letters, numbers and - only");
            return;
        }
        sub.set_text("renaming… (you may be asked for your password)");
        let sub2 = sub.clone();
        let n2 = name.clone();
        util::run("hostnamectl", &["set-hostname", &n2], move |o| {
            sub2.set_text(&if o.ok { format!("renamed to {name} · new terminals pick it up") } else { format!("failed: {}", o.msg()) });
        });
    };
    let dr = std::rc::Rc::new(do_rename);
    let d = dr.clone();
    save.connect_clicked(move |b| {
        b.set_sensitive(false);
        d();
    });
    entry.connect_activate(move |_| dr());

    let c = ui::section(body, "hardware");
    if !i.model.is_empty() {
        let r = ui::row("󰌢", "model", &i.model);
        ui::add(&c, &r.row);
    }
    for (ic, k, v) in [("\u{f4bc}", "processor", &i.cpu), ("󰢮", "graphics", &i.gpu), ("󰍹", "display", &i.display)] {
        if !v.is_empty() {
            let r = ui::row(ic, k, v);
            r.sub.set_ellipsize(gtk::pango::EllipsizeMode::None);
            r.sub.set_wrap(true);
            ui::add(&c, &r.row);
        }
    }
    usage_row(&c, "󰍛", "memory", i.mem);
    if i.swap.1 > 0 {
        usage_row(&c, "󰾴", "swap", i.swap);
    }
    usage_row(&c, "󰋊", "disk  /", i.disk);

    let c = ui::section(body, "software");
    for (ic, k, v) in [
        ("\u{f303}", "os", i.os.clone()),
        ("\u{f17c}", "kernel", i.kernel.clone()),
        ("\u{f2d0}", "compositor", format!("Hyprland {}", i.hypr)),
        ("󰏖", "packages", i.pkgs.clone()),
        ("\u{f120}", "shell", i.shell.clone()),
        ("󰸉", "wallpaper", i.wall.clone()),
        ("󰘚", "this app", format!("{} RAM", util::human_bytes(i.self_rss))),
    ] {
        if !v.is_empty() {
            let r = ui::row(ic, k, "");
            r.suffix(&ui::label(&v, &["dim"]));
            ui::add(&c, &r.row);
        }
    }

    // ---- what this app can use, and what's missing
    let c = ui::section(body, "components");
    for d in deps::ALL {
        if d.name == "asusctl" && !fs::read_to_string("/sys/class/dmi/id/sys_vendor").is_ok_and(|v| v.contains("ASUS")) {
            continue;
        }
        let ok = deps::installed(d);
        let r = ui::row(if ok { "󰄬" } else { "󰏗" }, d.name, d.why);
        if ok {
            r.icon.add_css_class("on");
            r.suffix(&ui::label("installed", &["dim"]));
        } else if let Some(cmd) = deps::install_cmd(d) {
            let b = ui::button("install", &[]);
            b.set_tooltip_text(Some(&cmd));
            b.connect_clicked(move |b| {
                b.set_sensitive(false);
                let b2 = b.clone();
                deps::run_in_terminal(&cmd, move || {
                    b2.set_sensitive(true);
                    crate::reload_page("about");
                });
            });
            r.suffix(&b);
        } else {
            r.suffix(&ui::label("not available", &["dim"]));
        }
        ui::add(&c, &r.row);
    }
}

pub fn build() -> gtk::Widget {
    let p = ui::page("󰋽", "about", "this machine");
    let copy = ui::button("󰆏  copy", &["flat"]);
    copy.set_tooltip_text(Some("copy a plain-text summary"));
    p.actions.append(&copy);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.append(&ui::label("loading…", &["status"]));
    p.body.append(&body);

    let load = {
        let body = body.clone();
        move || {
            let b = body.clone();
            util::bg(gather, move |i| {
                render(&b, i);
                // keep the page at the top after a re-render
                if let Some(sw) = b.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>() {
                    sw.vadjustment().set_value(0.0);
                }
            });
        }
    };
    load();
    let l = load.clone();
    p.root.connect_map(move |_| l());

    copy.connect_clicked(|b| {
        let b = b.clone();
        util::bg(gather, move |i| {
            let txt = format!(
                "{}@{}\nos: {}\nkernel: {}\nhyprland: {}\ncpu: {}\ngpu: {}\nmemory: {} / {}\ndisplay: {}\nuptime: {}\n",
                i.user, i.host, i.os, i.kernel, i.hypr, i.cpu, i.gpu.replace('\n', "; "),
                util::human_bytes(i.mem.0), util::human_bytes(i.mem.1), i.display.replace('\n', "; "), i.uptime
            );
            b.clipboard().set_text(&txt);
            b.set_label("󰄬  copied");
        });
    });
    p.root.upcast()
}
