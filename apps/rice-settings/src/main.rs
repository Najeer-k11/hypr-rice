//! rice-settings — one place for wallpaper, colours, window look, the bar, displays,
//! wi-fi, bluetooth and system info on a Hyprland rice. Themed from the matugen palette.
//!
//!   rice-settings [page]       page = wallpaper | colours | appearance | panel | wifi | bluetooth |
//!                                     display | power | keyboard | about
//!   rice-settings --sync-keyboard   keyboard RGB follows the wallpaper accent (for wallpaper hooks)

mod deps;
mod hypr;
mod pages;
mod theme;
mod ui;
mod util;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;

struct PageDef {
    id: &'static str,
    aliases: &'static [&'static str],
    icon: &'static str,
    title: &'static str,
    group: &'static str,
    build: fn() -> gtk::Widget,
}

const PAGES: &[PageDef] = &[
    PageDef { id: "wallpaper", aliases: &["wall", "wallpapers"], icon: "󰸉", title: "wallpaper", group: "personalise", build: pages::wallpaper::build },
    PageDef { id: "colours", aliases: &["colors", "palette", "color", "colour"], icon: "󰏘", title: "colours", group: "personalise", build: pages::colours::build },
    PageDef { id: "appearance", aliases: &["windows", "look", "radius"], icon: "\u{f1fc}", title: "appearance", group: "personalise", build: pages::appearance::build },
    PageDef { id: "panel", aliases: &["bar", "waybar", "panels"], icon: "󰕮", title: "panel", group: "personalise", build: pages::panel::build },
    PageDef { id: "display", aliases: &["monitor", "monitors", "scale"], icon: "󰍹", title: "display", group: "system", build: pages::display::build },
    PageDef { id: "wifi", aliases: &["wi-fi", "network", "net"], icon: "󰤨", title: "wi-fi", group: "connections", build: pages::wifi::build },
    PageDef { id: "bluetooth", aliases: &["bt"], icon: "󰂯", title: "bluetooth", group: "connections", build: pages::bluetooth::build },
    PageDef { id: "power", aliases: &["battery", "profile", "profiles"], icon: "󰁹", title: "power", group: "system", build: pages::power::build },
    PageDef { id: "keyboard", aliases: &["kbd", "rgb", "backlight"], icon: "󰌌", title: "keyboard", group: "system", build: pages::keyboard::build },
    PageDef { id: "about", aliases: &["system", "info"], icon: "󰋽", title: "about", group: "system", build: pages::about::build },
];

const GROUPS: &[&str] = &["personalise", "connections", "system"];

thread_local! {
    static NAV: RefCell<Option<gtk::ListBox>> = const { RefCell::new(None) };
    static STACK: RefCell<Option<gtk::Stack>> = const { RefCell::new(None) };
}

/// Throw away a page and build it again (after installing something it needed).
pub fn reload_page(id: &str) {
    STACK.with(|s| {
        let s = s.borrow();
        let Some(stack) = s.as_ref() else { return };
        let Some(p) = PAGES.iter().find(|p| p.id == id) else { return };
        if let Some(holder) = stack.child_by_name(p.id).and_downcast::<gtk::Box>() {
            if let Some(old) = holder.first_child() {
                holder.remove(&old);
                holder.append(&(p.build)());
            }
        }
    });
}

fn main() -> glib::ExitCode {
    if std::env::args().any(|a| a == "--sync-keyboard") {
        return if pages::keyboard::sync_follow() { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE };
    }
    // The CPU renderer keeps RAM way down (no GL context / shader caches);
    // a settings window doesn't need GPU compositing. Override with GSK_RENDERER=ngl.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    // The desktop portal check costs ~420 ms of GTK startup, and nothing here uses portals.
    if std::env::var_os("GDK_DEBUG").is_none() {
        std::env::set_var("GDK_DEBUG", "no-portals");
    }
    let app = gtk::Application::builder()
        .application_id("rice.settings")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_command_line(|app, cmd| {
        let page = cmd.arguments().get(1).and_then(|a| a.to_str().map(str::to_lowercase));
        present(app, page.as_deref());
        glib::ExitCode::SUCCESS
    });
    app.run()
}

fn present(app: &gtk::Application, page: Option<&str>) {
    let win = app.active_window().unwrap_or_else(|| build_window(app));
    let want = page
        .and_then(|p| PAGES.iter().find(|d| d.id == p || d.aliases.contains(&p)))
        .map(|d| d.id);
    NAV.with(|n| {
        let n = n.borrow();
        let Some(list) = n.as_ref() else { return };
        if want.is_none() && list.selected_row().is_some() {
            return;
        }
        let target = want.unwrap_or(PAGES[0].id);
        let mut i = 0;
        while let Some(row) = list.row_at_index(i) {
            if row.widget_name() == target {
                list.select_row(Some(&row));
                break;
            }
            i += 1;
        }
    });
    win.present();
}

fn build_window(app: &gtk::Application) -> gtk::Window {
    theme::init();

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("settings")
        .default_width(960)
        .default_height(640)
        .decorated(false) // Hyprland draws the border; no CSD title bar
        .build();
    win.add_css_class("rice");

    // ---- sidebar
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("sidebar");
    side.set_size_request(196, -1);

    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    brand.add_css_class("brand");
    brand.append(&ui::label("\u{f013}", &["brand-icon"]));
    brand.append(&ui::label("settings", &["brand-title"]));
    side.append(&brand);

    let nav = gtk::ListBox::new();
    nav.add_css_class("nav");
    nav.set_selection_mode(gtk::SelectionMode::Single);
    for g in GROUPS {
        let head = gtk::ListBoxRow::new();
        head.add_css_class("nav-head");
        head.set_selectable(false);
        head.set_activatable(false);
        head.set_focusable(false);
        head.set_child(Some(&ui::label(g, &[])));
        nav.append(&head);
        for p in PAGES.iter().filter(|p| p.group == *g) {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(p.id);
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let ic = ui::label(p.icon, &["nav-icon"]);
            ic.set_xalign(0.5);
            b.append(&ic);
            b.append(&ui::label(p.title, &[]));
            row.set_child(Some(&b));
            nav.append(&row);
        }
    }
    let navscroll = gtk::ScrolledWindow::new();
    navscroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    navscroll.set_vexpand(true);
    navscroll.set_child(Some(&nav));
    side.append(&navscroll);
    side.append(&ui::label("esc to close", &["sidebar-foot"]));

    // ---- pages: placeholders now, real widgets on first visit
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(110);
    stack.set_hexpand(true);
    for p in PAGES {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&holder, Some(p.id));
    }
    let st = stack.clone();
    nav.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let id = row.widget_name();
        let Some(p) = PAGES.iter().find(|p| p.id == id.as_str()) else { return };
        if let Some(holder) = st.child_by_name(p.id).and_downcast::<gtk::Box>() {
            if holder.first_child().is_none() {
                holder.append(&(p.build)());
            }
        }
        st.set_visible_child_name(p.id);
    });

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.append(&side);
    root.append(&stack);
    win.set_child(Some(&root));

    // esc / ctrl+q / ctrl+w close
    let keys = gtk::EventControllerKey::new();
    let w = win.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        if key == gdk::Key::Escape || (ctrl && (key == gdk::Key::q || key == gdk::Key::w)) {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(keys);

    NAV.with(|n| *n.borrow_mut() = Some(nav));
    STACK.with(|s| *s.borrow_mut() = Some(stack));
    win.upcast()
}
