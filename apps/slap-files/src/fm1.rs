//! org.freedesktop.FileManager1 — what browsers and other apps call for "show in folder".
//! Owned while the app runs (taking over from e.g. Dolphin); `slap-files --service` lets
//! D-Bus start us on demand (see install.sh --default).

use crate::window;
use gtk::prelude::*;
use gtk::gio;
use std::path::PathBuf;

const XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
    <method name="ShowItems"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
    <method name="ShowItemProperties"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
  </interface>
</node>"#;

pub fn own(app: &gtk::Application) {
    let app = app.downgrade();
    gio::bus_own_name(
        gio::BusType::Session,
        "org.freedesktop.FileManager1",
        gio::BusNameOwnerFlags::ALLOW_REPLACEMENT | gio::BusNameOwnerFlags::REPLACE,
        move |conn, _| {
            let Some(info) = gio::DBusNodeInfo::for_xml(XML).ok().and_then(|n| n.lookup_interface("org.freedesktop.FileManager1")) else { return };
            let app = app.clone();
            let _ = conn
                .register_object("/org/freedesktop/FileManager1", &info)
                .method_call(move |_, _, _, _, method, params, inv| {
                    let uris: Vec<String> = params.child_value(0).get().unwrap_or_default();
                    let paths: Vec<PathBuf> = uris.iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
                    if let Some(app) = app.upgrade() {
                        for p in paths {
                            match method {
                                "ShowFolders" => window::open(&app, Some(p)),
                                // select the item itself (a folder too) in its parent
                                _ => window::reveal(&app, &p, method == "ShowItemProperties"),
                            }
                        }
                    }
                    inv.return_value(None);
                })
                .build();
        },
        |_, _| {},
        |_, _| {},
    );
}
