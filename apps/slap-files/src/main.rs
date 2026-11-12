//! Slap File Manager — an Explorer-style file manager for Hyprland, themed live from the
//! wallpaper's matugen palette (same look as rice-settings).
//!
//!   slap-files [path]     open a folder (or a file's folder, with the file selected)
//!   slap-files --service  stay in the background for D-Bus (FileManager1) requests

mod apps;
mod archive;
mod browser;
mod driveui;
mod drives;
mod fm1;
mod hypr;
mod item;
mod ops;
mod preview;
mod props;
mod search;
mod sidebar;
mod state;
mod theme;
mod thumbs;
mod trash;
mod ui;
mod util;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    util::limit_arenas();
    // The CPU renderer keeps RAM way down (no GL context / shader caches).
    // Override with GSK_RENDERER=ngl.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    let app = gtk::Application::builder()
        .application_id("slap.files")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(fm1::own);
    app.connect_command_line(|app, cmd| {
        // started by D-Bus for "show in folder": no window until a request arrives
        if cmd.arguments().iter().any(|a| a == "--service") {
            let hold = app.hold();
            glib::timeout_add_local_once(std::time::Duration::from_secs(30), move || drop(hold));
            return glib::ExitCode::SUCCESS;
        }
        let cwd = cmd.cwd().unwrap_or_else(util::home);
        let target = cmd
            .arguments()
            .get(1)
            .and_then(|a| a.to_str().map(|s| util::expand(s, &cwd)));
        window::open(app, target);
        glib::ExitCode::SUCCESS
    });
    app.run()
}
