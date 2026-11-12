//! One browsing pane: toolbar (history, breadcrumb address bar, search, view switches),
//! the details / grid views over one shared model, the operations panel and the status bar.
//! File operations, menus and drag & drop live in `browser/actions.rs`.
//! Each tab is one of these; every signal handler holds it weakly so a closed tab is freed.

mod actions;

pub use actions::attach_path_drop;

use crate::item::{self, Item};
use crate::{driveui, preview, search, state, trash, ui, util};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Virtual locations (not real paths): the drives page and the trash.
pub const PC: &str = "slap:pc";
pub const TRASH: &str = "slap:trash";

pub fn is_pc(p: &Path) -> bool {
    p == Path::new(PC)
}
pub fn is_trash(p: &Path) -> bool {
    p == Path::new(TRASH)
}
pub fn is_virtual(p: &Path) -> bool {
    is_pc(p) || is_trash(p)
}

/// Late-bound reference to the browser for list-item factories (built before the browser).
#[derive(Clone, Default)]
struct BRef(Rc<RefCell<Weak<Browser>>>);

impl BRef {
    fn get(&self) -> Option<Rc<Browser>> {
        self.0.borrow().upgrade()
    }
}

pub struct Browser {
    pub root: gtk::Box,
    cwd: RefCell<PathBuf>,
    back: RefCell<Vec<PathBuf>>,
    fwd: RefCell<Vec<PathBuf>>,
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    sel: gtk::MultiSelection,
    query: Rc<RefCell<String>>,
    show_hidden: Rc<Cell<bool>>,
    gen: Cell<u64>,
    error: RefCell<Option<String>>,
    select_after: RefCell<Vec<String>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    deb: ui::Debounce,
    flash_deb: ui::Debounce,
    on_nav: RefCell<Vec<Box<dyn Fn(&Path)>>>,
    open_tab: RefCell<Option<Box<dyn Fn(PathBuf)>>>,
    actions: gio::SimpleActionGroup,
    // recursive search
    searching: Cell<bool>,
    search_running: Cell<bool>,
    search_capped: Cell<bool>,
    search_cancel: RefCell<Option<Arc<AtomicBool>>>,
    search_deb: ui::Debounce,
    loc_col: gtk::ColumnViewColumn,
    date_col: gtk::ColumnViewColumn,
    pc: RefCell<Option<Rc<driveui::Page>>>,
    // preview pane
    preview: Rc<preview::Pane>,
    preview_deb: ui::Debounce,
    btn_preview: gtk::ToggleButton,
    // widgets
    btn_back: gtk::Button,
    btn_fwd: gtk::Button,
    btn_up: gtk::Button,
    address: gtk::Box,
    addr_stack: gtk::Stack,
    crumbs: gtk::Box,
    crumb_scroll: gtk::ScrolledWindow,
    addr_entry: gtk::Entry,
    search: gtk::SearchEntry,
    views: gtk::Stack,
    view_area: gtk::Overlay,
    colview: gtk::ColumnView,
    grid: gtk::GridView,
    btn_list: gtk::ToggleButton,
    btn_grid: gtk::ToggleButton,
    btn_hidden: gtk::ToggleButton,
    empty: gtk::Box,
    empty_icon: gtk::Label,
    empty_label: gtk::Label,
    ops_box: gtk::Box,
    status_l: gtk::Label,
    status_r: gtk::Label,
}

impl Browser {
    pub fn new() -> Rc<Self> {
        let bref = BRef::default();

        // ---------------- model: store -> filter -> sort -> selection
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let query: Rc<RefCell<String>> = Rc::default();
        let show_hidden = Rc::new(Cell::new(state::get_bool("show_hidden", false)));
        let filter = {
            let (q, h) = (query.clone(), show_hidden.clone());
            gtk::CustomFilter::new(move |o| {
                let it = item::of(o);
                (h.get() || !it.hidden) && {
                    let q = q.borrow();
                    q.is_empty() || it.name.to_lowercase().contains(q.as_str())
                }
            })
        };
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

        let colview = gtk::ColumnView::new(None::<gtk::MultiSelection>);
        colview.set_show_row_separators(false);
        colview.set_show_column_separators(false);
        colview.set_enable_rubberband(true);
        colview.set_reorderable(false);

        let name_col = column(&bref, "name", true, true, name_setup, name_bind, |a, b| item::natural(&a.name, &b.name));
        let date_col = column(&bref, "date modified", false, false, text_setup, |l, it| l.set_text(&it.date_s()), |a, b| a.mtime.cmp(&b.mtime));
        let type_col = column(&bref, "type", false, false, text_setup, |l, it| l.set_text(&it.kind), |a, b| {
            a.kind.cmp(&b.kind).then_with(|| item::natural(&a.name, &b.name))
        });
        let size_col = column(&bref, "size", false, false, size_setup, |l, it| l.set_text(&it.size_s()), |a, b| a.size.cmp(&b.size));
        let loc_col = {
            let f = gtk::SignalListItemFactory::new();
            let br = bref.clone();
            f.connect_setup(move |_, o| {
                let li = o.downcast_ref::<gtk::ListItem>().unwrap();
                text_setup(li);
                if let Some(w) = li.child() {
                    actions::attach(&br, li, &w, false);
                }
            });
            let br = bref.clone();
            f.connect_bind(move |_, o| {
                let li = o.downcast_ref::<gtk::ListItem>().unwrap();
                let (Some(w), Some(obj), Some(b)) = (li.child(), li.item(), br.get()) else { return };
                let it = item::of(&obj);
                if let Some(t) = &it.trash {
                    w.set_text(&t.original.parent().map(util::tilde).unwrap_or_default());
                    return;
                }
                let cwd = b.cwd();
                let rel = match it.dir.strip_prefix(&cwd) {
                    Ok(r) if r.as_os_str().is_empty() => "·".to_string(),
                    Ok(r) => r.to_string_lossy().into_owned(),
                    Err(_) => util::tilde(&it.dir),
                };
                w.set_text(&rel);
            });
            let c = gtk::ColumnViewColumn::new(Some("folder"), Some(f));
            c.set_resizable(true);
            c.set_fixed_width(200);
            c.set_visible(false);
            c.set_sorter(Some(&gtk::CustomSorter::new(|a, b| item::of(a).dir.cmp(&item::of(b).dir).into())));
            c
        };
        date_col.set_fixed_width(150);
        type_col.set_fixed_width(140);
        size_col.set_fixed_width(84);
        for c in [&name_col, &loc_col, &date_col, &type_col, &size_col] {
            colview.append_column(c);
        }

        // folders always first (independent of the clicked column's direction), then the column
        let sorter = gtk::MultiSorter::new();
        sorter.append(gtk::CustomSorter::new(|a, b| item::of(b).is_dir.cmp(&item::of(a).is_dir).into()));
        if let Some(s) = colview.sorter() {
            sorter.append(s);
        }
        sorter.append(gtk::CustomSorter::new(|a, b| item::natural(&item::of(a).name, &item::of(b).name).into()));
        let sorted = gtk::SortListModel::new(Some(filtered), Some(sorter));
        let sel = gtk::MultiSelection::new(Some(sorted));
        colview.set_model(Some(&sel));
        colview.sort_by_column(Some(&name_col), gtk::SortType::Ascending);

        let gfac = gtk::SignalListItemFactory::new();
        let br = bref.clone();
        gfac.connect_setup(move |_, o| {
            let li = o.downcast_ref::<gtk::ListItem>().unwrap();
            let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
            b.add_css_class("drop-cell");
            let ic = ui::label("", &["grid-icon"]);
            ic.set_xalign(0.5);
            ic.set_vexpand(true);
            // fixed-height icon area: glyph, or the thumbnail once it arrives
            let area = gtk::Box::new(gtk::Orientation::Vertical, 0);
            area.set_size_request(-1, 84);
            let pic = gtk::Picture::new();
            pic.add_css_class("grid-thumb");
            pic.set_can_shrink(true);
            pic.set_content_fit(gtk::ContentFit::ScaleDown);
            pic.set_vexpand(true);
            pic.set_visible(false);
            area.append(&ic);
            area.append(&pic);
            let nm = gtk::Label::new(None);
            nm.add_css_class("grid-name");
            nm.set_wrap(true);
            nm.set_wrap_mode(pango::WrapMode::WordChar);
            nm.set_lines(2);
            nm.set_ellipsize(pango::EllipsizeMode::End);
            nm.set_justify(gtk::Justification::Center);
            nm.set_max_width_chars(12);
            nm.set_width_chars(12);
            b.append(&area);
            b.append(&nm);
            li.set_child(Some(&b));
            actions::attach(&br, li, b.upcast_ref(), true);
        });
        gfac.connect_bind(|_, o| {
            let li = o.downcast_ref::<gtk::ListItem>().unwrap();
            let (Some(b), Some(obj)) = (li.child(), li.item()) else { return };
            let it = item::of(&obj);
            let area = b.first_child().unwrap();
            let ic = area.first_child().and_downcast::<gtk::Label>().unwrap();
            let pic = area.last_child().and_downcast::<gtk::Picture>().unwrap();
            let nm = b.last_child().and_downcast::<gtk::Label>().unwrap();
            ic.set_text(it.icon);
            ic.set_css_classes(&["grid-icon", it.tint]);
            nm.set_text(&it.name);
            b.set_tooltip_text(Some(&tooltip(&it)));
            set_class(&b, "hidden-item", it.hidden);
            // thumbnail: tiles are recycled, so only apply it if this tile still shows the file
            pic.set_paintable(None::<&gdk::Paintable>);
            pic.set_visible(false);
            ic.set_visible(true);
            let path = it.path();
            b.set_widget_name(&path.to_string_lossy());
            if !it.is_dir && crate::thumbs::wanted(&it.name) {
                let (bw, pw, iw) = (b.downgrade(), pic.downgrade(), ic.downgrade());
                let key = path.to_string_lossy().into_owned();
                crate::thumbs::request(&path, it.mtime_file(), move |tex| {
                    let (Some(b), Some(pic), Some(ic)) = (bw.upgrade(), pw.upgrade(), iw.upgrade()) else { return };
                    if b.widget_name() == key.as_str() {
                        pic.set_paintable(Some(tex));
                        pic.set_visible(true);
                        ic.set_visible(false);
                    }
                });
            }
        });
        let grid = gtk::GridView::new(Some(sel.clone()), Some(gfac));
        grid.set_max_columns(40);
        grid.set_min_columns(1);
        grid.set_enable_rubberband(true);

        // ---------------- views + empty overlay
        let views = gtk::Stack::new();
        views.set_transition_type(gtk::StackTransitionType::Crossfade);
        views.set_transition_duration(90);
        views.add_named(&scrolled(&colview), Some("list"));
        views.add_named(&scrolled(&grid), Some("grid"));

        let empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.set_can_target(false);
        let empty_icon = ui::label("", &["empty-icon"]);
        empty_icon.set_xalign(0.5);
        let empty_label = ui::label("", &["empty-view"]);
        empty_label.set_xalign(0.5);
        empty_label.set_wrap(true);
        empty_label.set_justify(gtk::Justification::Center);
        empty.append(&empty_icon);
        empty.append(&empty_label);
        let view_area = gtk::Overlay::new();
        view_area.add_css_class("drop-zone");
        view_area.set_child(Some(&views));
        view_area.add_overlay(&empty);
        view_area.set_vexpand(true);

        // ---------------- toolbar
        let btn_back = ui::tool("\u{f004d}", "back (alt+←)");
        let btn_fwd = ui::tool("\u{f0054}", "forward (alt+→)");
        let btn_up = ui::tool("\u{f005d}", "up (alt+↑)");
        let btn_refresh = ui::tool("\u{f0450}", "refresh (f5)");

        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        crumbs.set_valign(gtk::Align::Center);
        let crumb_scroll = gtk::ScrolledWindow::new();
        crumb_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        crumb_scroll.set_child(Some(&crumbs));
        crumb_scroll.set_hexpand(true);
        crumb_scroll.set_propagate_natural_width(true);
        crumb_scroll.set_min_content_width(60);

        let addr_entry = gtk::Entry::new();
        addr_entry.set_hexpand(true);
        let addr_stack = gtk::Stack::new();
        addr_stack.add_named(&crumb_scroll, Some("crumbs"));
        addr_stack.add_named(&addr_entry, Some("entry"));
        addr_stack.set_hexpand(true);
        addr_stack.set_hhomogeneous(false);
        let address = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        address.add_css_class("address");
        address.set_hexpand(true);
        address.append(&addr_stack);

        let search = gtk::SearchEntry::new();
        search.set_width_chars(6);
        search.set_max_width_chars(22);
        search.set_valign(gtk::Align::Center);

        let btn_list = ui::tool_toggle("\u{f0572}", "details (ctrl+1)");
        let btn_grid = ui::tool_toggle("\u{f0570}", "large icons (ctrl+2)");
        btn_grid.set_group(Some(&btn_list));
        let btn_preview = ui::tool_toggle("\u{f10ab}", "preview pane (alt+p)");
        let btn_hidden = ui::tool_toggle(if show_hidden.get() { "\u{f0208}" } else { "\u{f0209}" }, "show hidden files (ctrl+h)");
        btn_hidden.set_active(show_hidden.get());

        let tb = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tb.add_css_class("fm-toolbar");
        for b in [&btn_back, &btn_fwd, &btn_up, &btn_refresh] {
            tb.append(b);
        }
        address.set_margin_start(6);
        address.set_margin_end(8);
        tb.append(&address);
        tb.append(&search);
        let right = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        right.set_margin_start(8);
        right.append(&btn_list);
        right.append(&btn_grid);
        right.append(&btn_hidden);
        right.append(&btn_preview);
        tb.append(&right);

        // ---------------- operations + status bar
        let ops_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        ops_box.add_css_class("ops");
        ops_box.set_visible(false);

        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("statusbar");
        let status_l = ui::label("", &[]);
        status_l.set_hexpand(true);
        let status_r = ui::label("", &[]);
        status_r.set_ellipsize(pango::EllipsizeMode::Middle);
        status.append(&status_l);
        status.append(&status_r);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        let preview = preview::Pane::new();
        let show_preview = state::get_bool("preview", false);
        preview.root.set_visible(show_preview);
        btn_preview.set_active(show_preview);
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&view_area));
        paned.set_end_child(Some(&preview.root));
        paned.set_resize_end_child(false);
        paned.set_shrink_end_child(false);
        paned.set_shrink_start_child(false);
        paned.set_vexpand(true);
        preview.root.set_width_request(230);

        root.append(&tb);
        root.append(&paned);
        root.append(&ops_box);
        root.append(&status);

        let me = Rc::new(Browser {
            root,
            cwd: RefCell::new(util::home()),
            back: RefCell::default(),
            fwd: RefCell::default(),
            store,
            filter,
            sel,
            query,
            show_hidden,
            gen: Cell::new(0),
            error: RefCell::default(),
            select_after: RefCell::default(),
            monitor: RefCell::default(),
            deb: ui::Debounce::default(),
            flash_deb: ui::Debounce::default(),
            on_nav: RefCell::default(),
            open_tab: RefCell::default(),
            actions: gio::SimpleActionGroup::new(),
            searching: Cell::new(false),
            search_running: Cell::new(false),
            search_capped: Cell::new(false),
            search_cancel: RefCell::default(),
            search_deb: ui::Debounce::default(),
            loc_col,
            date_col: date_col.clone(),
            pc: RefCell::default(),
            preview,
            preview_deb: ui::Debounce::default(),
            btn_preview,
            btn_back,
            btn_fwd,
            btn_up,
            address,
            addr_stack,
            crumbs,
            crumb_scroll,
            addr_entry,
            search,
            views,
            view_area,
            colview,
            grid,
            btn_list,
            btn_grid,
            btn_hidden,
            empty,
            empty_icon,
            empty_label,
            ops_box,
            status_l,
            status_r,
        });
        *bref.0.borrow_mut() = Rc::downgrade(&me);
        me.wire(&btn_refresh);
        me.install_actions();
        me.set_view(&state::get_str("view", "list"));
        me
    }

    fn wire(self: &Rc<Self>, btn_refresh: &gtk::Button) {
        // every handler holds the pane weakly: closing a tab frees it
        macro_rules! weak {
            ($m:ident; $($arg:pat),* => $body:expr) => {{
                let w = Rc::downgrade(self);
                move |$($arg),*| {
                    if let Some($m) = w.upgrade() {
                        $body
                    }
                }
            }};
        }
        self.btn_back.connect_clicked(weak!(m; _ => m.go_back()));
        self.btn_fwd.connect_clicked(weak!(m; _ => m.go_forward()));
        self.btn_up.connect_clicked(weak!(m; _ => m.go_up()));
        btn_refresh.connect_clicked(weak!(m; _ => m.refresh()));

        self.colview.connect_activate(weak!(m; _, pos => m.activate(pos)));
        self.grid.connect_activate(weak!(m; _, pos => m.activate(pos)));

        self.sel.connect_selection_changed(weak!(m; _, _, _ => {
            m.update_status();
            m.queue_preview();
        }));
        self.sel.connect_items_changed(weak!(m; _, _, _, _ => m.update_status()));

        // view / hidden / preview toggles
        self.btn_list.connect_toggled(weak!(m; b => if b.is_active() { m.set_view("list") }));
        self.btn_grid.connect_toggled(weak!(m; b => if b.is_active() { m.set_view("grid") }));
        self.btn_hidden.connect_toggled(weak!(m; b => m.set_hidden(b.is_active())));
        self.btn_preview.connect_toggled(weak!(m; b => m.set_preview(b.is_active())));

        // search: instant filter of this folder, then (after a pause) a recursive search
        self.search.connect_search_changed(weak!(m; s => m.query_changed(&s.text())));
        self.search.connect_stop_search(weak!(m; s => {
            s.set_text("");
            m.focus_view();
        }));
        // down arrow from the search box moves into the results
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, k, _, _| {
            let Some(m) = w.upgrade() else { return glib::Propagation::Proceed };
            if matches!(k, gdk::Key::Down | gdk::Key::KP_Down) && m.sel.n_items() > 0 {
                m.focus_view();
                m.scroll_to(0, true);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.search.add_controller(keys);
        self.search.connect_activate(weak!(m; _ => {
            // enter: search subfolders now, or open the only/first match
            if !m.searching.get() && m.query.borrow().chars().count() >= 1 {
                m.start_deep_search();
            } else if m.sel.n_items() > 0 {
                m.activate(0);
            }
        }));

        // address bar: click empty space (or ctrl+l) to type a path
        let click = gtk::GestureClick::new();
        click.connect_released(weak!(m; g, _, _, _ => {
            g.set_state(gtk::EventSequenceState::Claimed);
            m.edit_address();
        }));
        self.address.add_controller(click);
        self.addr_entry.connect_activate(weak!(m; e => {
            let target = util::expand(&e.text(), &m.cwd());
            m.end_edit();
            m.focus_view();
            if target.is_dir() {
                m.go(target);
            } else if target.exists() {
                m.open_file(&target);
            } else {
                m.flash(&format!("can't find '{}'", e.text()));
            }
        }));
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, k, _, _| {
            let Some(m) = w.upgrade() else { return glib::Propagation::Proceed };
            if k == gdk::Key::Escape {
                m.end_edit();
                m.focus_view();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.addr_entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(weak!(m; _ => m.end_edit()));
        self.addr_entry.add_controller(focus);

        // right-click on empty space: folder menu (items claim their own right-clicks)
        let rc = gtk::GestureClick::new();
        rc.set_button(3);
        rc.connect_pressed(weak!(m; g, _, x, y => {
            m.sel.unselect_all();
            if let Some(w) = g.widget() {
                m.context_menu(&w, x, y);
            }
        }));
        self.view_area.add_controller(rc);
        actions::attach_drop_zone(self);
    }

    // ------------------------------------------------------------ navigation --

    pub fn cwd(&self) -> PathBuf {
        self.cwd.borrow().clone()
    }

    /// Run `f` whenever the folder changes (sidebar highlight, window title…).
    pub fn connect_navigated(&self, f: impl Fn(&Path) + 'static) {
        self.on_nav.borrow_mut().push(Box::new(f));
    }

    /// Go to a folder, recording history.
    pub fn go(self: &Rc<Self>, p: PathBuf) {
        if p == self.cwd() && self.gen.get() > 0 {
            self.refresh();
            return;
        }
        if self.gen.get() > 0 {
            self.back.borrow_mut().push(self.cwd());
            self.fwd.borrow_mut().clear();
        }
        self.enter(p);
    }

    /// Open a file's folder with that file selected.
    pub fn reveal(self: &Rc<Self>, file: &Path) {
        if let (Some(dir), Some(name)) = (file.parent(), file.file_name()) {
            *self.select_after.borrow_mut() = vec![name.to_string_lossy().into_owned()];
            self.go(dir.to_path_buf());
        }
    }

    pub fn go_back(self: &Rc<Self>) {
        let Some(p) = self.back.borrow_mut().pop() else { return };
        self.fwd.borrow_mut().push(self.cwd());
        self.enter(p);
    }

    pub fn go_forward(self: &Rc<Self>) {
        let Some(p) = self.fwd.borrow_mut().pop() else { return };
        self.back.borrow_mut().push(self.cwd());
        self.enter(p);
    }

    pub fn go_up(self: &Rc<Self>) {
        let cwd = self.cwd();
        if cwd == Path::new("/") {
            self.go(PathBuf::from(PC));
            return;
        }
        if let Some(parent) = cwd.parent().filter(|_| !is_virtual(&cwd)) {
            // like Explorer: land on the folder we came out of
            *self.select_after.borrow_mut() = cwd.file_name().map(|n| n.to_string_lossy().into_owned()).into_iter().collect();
            self.go(parent.to_path_buf());
        }
    }

    pub fn refresh(self: &Rc<Self>) {
        if is_pc(&self.cwd()) {
            crate::drives::refresh();
        } else if self.searching.get() {
            self.start_deep_search();
        } else {
            self.load(true);
        }
    }

    /// Ask the window to open a folder in a new tab.
    pub fn connect_open_tab(&self, f: impl Fn(PathBuf) + 'static) {
        *self.open_tab.borrow_mut() = Some(Box::new(f));
    }

    pub fn open_in_tab(&self, p: PathBuf) {
        if let Some(f) = self.open_tab.borrow().as_ref() {
            f(p);
        }
    }

    /// The drives page lives in the views stack; built on first visit.
    fn show_pc(self: &Rc<Self>) {
        self.gen.set(self.gen.get() + 1);
        self.sel.unselect_all();
        self.store.remove_all(); // nothing listed behind the page
        if self.pc.borrow().is_none() {
            let (w1, w2) = (Rc::downgrade(self), Rc::downgrade(self));
            let page = driveui::Page::new(driveui::PcHooks {
                open: Box::new(move |p, new_tab| {
                    let Some(b) = w1.upgrade() else { return };
                    if new_tab { b.open_in_tab(p) } else { b.go(p) }
                }),
                eject: Box::new(move |anchor, drive, name| {
                    if w2.upgrade().is_some() {
                        crate::window::eject(anchor, &drive, &name);
                    }
                }),
            });
            self.views.add_named(&page.root, Some("pc"));
            *self.pc.borrow_mut() = Some(page);
        } else if let Some(p) = self.pc.borrow().as_ref() {
            p.rebuild();
        }
        self.views.set_visible_child_name("pc");
        self.update_status();
    }

    // ---------------------------------------------------------------- search --

    fn query_changed(self: &Rc<Self>, text: &str) {
        *self.query.borrow_mut() = text.to_lowercase();
        self.filter.changed(gtk::FilterChange::Different);
        if text.is_empty() {
            self.search_deb.run(0, || {});
            if self.searching.get() {
                self.stop_search();
                self.load(false);
            }
            return;
        }
        // like Explorer: after a short pause, look in subfolders too
        if text.chars().count() >= 2 && !is_virtual(&self.cwd()) {
            let w = Rc::downgrade(self);
            self.search_deb.run(450, move || {
                if let Some(m) = w.upgrade() {
                    m.start_deep_search();
                }
            });
        }
    }

    fn stop_search(&self) {
        if let Some(c) = self.search_cancel.borrow_mut().take() {
            c.store(true, Ordering::Relaxed);
        }
        self.searching.set(false);
        self.search_running.set(false);
        self.loc_col.set_visible(false);
    }

    fn start_deep_search(self: &Rc<Self>) {
        let q = self.query.borrow().clone();
        if q.is_empty() {
            return;
        }
        self.stop_search();
        self.searching.set(true);
        self.search_running.set(true);
        self.search_capped.set(false);
        self.loc_col.set_visible(true);
        let gen = self.gen.get() + 1; // stale folder loads are ignored from here on
        self.gen.set(gen);
        *self.error.borrow_mut() = None;
        self.sel.unselect_all();
        self.store.remove_all();
        let cancel = Arc::new(AtomicBool::new(false));
        *self.search_cancel.borrow_mut() = Some(cancel.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, hidden) = (self.cwd(), self.show_hidden.get());
        let c2 = cancel.clone();
        std::thread::spawn(move || search::run(root, q, hidden, c2, tx));
        let w = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let Some(m) = w.upgrade() else { return glib::ControlFlow::Break };
            if cancel.load(Ordering::Relaxed) || m.gen.get() != gen {
                return glib::ControlFlow::Break;
            }
            loop {
                match rx.try_recv() {
                    Ok(search::Msg::Batch(v)) => {
                        let objs: Vec<glib::BoxedAnyObject> = v.into_iter().map(item::obj).collect();
                        m.store.extend_from_slice(&objs);
                    }
                    Ok(search::Msg::Done(capped)) => {
                        m.search_running.set(false);
                        m.search_capped.set(capped);
                        m.update_status();
                        return glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        m.search_running.set(false);
                        m.update_status();
                        return glib::ControlFlow::Break;
                    }
                }
            }
            m.update_status();
            glib::ControlFlow::Continue
        });
        self.update_status();
    }

    pub fn is_searching(&self) -> bool {
        self.searching.get()
    }

    fn enter(self: &Rc<Self>, p: PathBuf) {
        *self.cwd.borrow_mut() = p.clone();
        self.stop_search();
        if !self.search.text().is_empty() {
            self.search.set_text("");
            *self.query.borrow_mut() = String::new();
            self.filter.changed(gtk::FilterChange::LessStrict);
        }
        self.search.set_placeholder_text(Some(&format!("search {}", display_name(&p))));
        self.btn_back.set_sensitive(!self.back.borrow().is_empty());
        self.btn_fwd.set_sensitive(!self.fwd.borrow().is_empty());
        // up from "/" goes to this pc, like Explorer's C:\ -> This PC
        self.btn_up.set_sensitive(!is_virtual(&p));
        self.search.set_sensitive(!is_pc(&p));
        let trash_mode = is_trash(&p);
        self.loc_col.set_visible(trash_mode);
        self.loc_col.set_title(Some(if trash_mode { "original location" } else { "folder" }));
        self.date_col.set_title(Some(if trash_mode { "date deleted" } else { "date modified" }));
        self.build_crumbs();
        self.watch();
        if is_pc(&p) {
            self.show_pc();
        } else {
            self.views.set_visible_child_name(if state::get_str("view", "list") == "grid" { "grid" } else { "list" });
            self.load(false);
        }
        for f in self.on_nav.borrow().iter() {
            f(&p);
        }
    }

    fn activate(self: &Rc<Self>, pos: u32) {
        let Some(o) = self.sel.item(pos) else { return };
        let (path, is_dir) = {
            let it = item::of(&o);
            (it.path(), it.is_dir)
        };
        if is_dir {
            self.go(path);
        } else {
            self.open_file(&path);
        }
    }

    /// Open the selection: folders navigate (first one), files open with their default app.
    pub fn open_selection(self: &Rc<Self>) {
        let items = self.selected();
        if let Some(d) = items.iter().find(|i| i.is_dir) {
            self.go(d.path());
            return;
        }
        for it in items {
            self.open_file(&it.path());
        }
    }

    pub fn open_file(&self, p: &Path) {
        let win = self.root.root().and_downcast::<gtk::Window>();
        let l = gtk::FileLauncher::new(Some(&gio::File::for_path(p)));
        let name = display_name(p);
        let st = self.status_r.clone();
        l.launch(win.as_ref(), gio::Cancellable::NONE, move |r| {
            if let Err(e) = r {
                if !e.matches(gtk::DialogError::Dismissed) && !e.matches(gtk::DialogError::Cancelled) {
                    st.set_text(&format!("couldn't open {name}: {}", e.message()));
                }
            }
        });
    }

    // ---------------------------------------------------------------- loading --

    fn load(self: &Rc<Self>, keep: bool) {
        let gen = self.gen.get() + 1;
        self.gen.set(gen);
        let dir = self.cwd();
        if is_pc(&dir) {
            return;
        }
        let m = self.clone();
        let read = move || {
            if is_trash(&dir) {
                let mut descs = HashMap::new();
                Ok(trash::list().into_iter().filter_map(|e| Item::from_trash(e, &mut descs)).collect())
            } else {
                item::read(&dir)
            }
        };
        util::bg(read, move |res| {
            if m.gen.get() != gen {
                return; // a newer navigation won
            }
            match res {
                Ok(items) => {
                    *m.error.borrow_mut() = None;
                    if keep { m.merge(items) } else { m.replace(items) }
                }
                Err(e) => {
                    *m.error.borrow_mut() = Some(e);
                    m.store.remove_all();
                }
            }
            m.after_load();
        });
    }

    fn replace(&self, items: Vec<Item>) {
        let objs: Vec<glib::BoxedAnyObject> = items.into_iter().map(item::obj).collect();
        let old = self.store.n_items();
        self.sel.unselect_all();
        self.store.splice(0, old, &objs);
        drop(objs);
        if old > 2000 {
            util::trim_heap();
        }
        if self.sel.n_items() > 0 && self.select_after.borrow().is_empty() {
            self.scroll_to(0, false);
        }
    }

    /// Apply a re-read in place (keeps scroll position and most of the selection).
    fn merge(&self, items: Vec<Item>) {
        let mut fresh: HashMap<String, Item> = items.into_iter().map(|i| (i.name.clone(), i)).collect();
        let mut i = 0;
        while i < self.store.n_items() {
            let o = self.store.item(i).unwrap();
            let (name, changed) = {
                let old = item::of(&o);
                let changed = fresh
                    .get(&old.name)
                    .map(|n| n.mtime != old.mtime || n.size != old.size || n.is_dir != old.is_dir || n.kind != old.kind);
                (old.name.clone(), changed)
            };
            match changed {
                None => {
                    self.store.remove(i);
                    continue;
                }
                Some(true) => {
                    let n = fresh.remove(&name).unwrap();
                    self.store.splice(i, 1, &[item::obj(n)]);
                }
                Some(false) => {
                    fresh.remove(&name);
                }
            }
            i += 1;
        }
        let objs: Vec<glib::BoxedAnyObject> = fresh.into_values().map(item::obj).collect();
        self.store.extend_from_slice(&objs);
    }

    fn after_load(&self) {
        let want = std::mem::take(&mut *self.select_after.borrow_mut());
        if !want.is_empty() {
            let mut first = true;
            for i in 0..self.sel.n_items() {
                if self.sel.item(i).is_some_and(|o| want.contains(&item::of(&o).name)) {
                    if first {
                        self.sel.unselect_all();
                        self.scroll_to(i, true);
                        first = false;
                    } else {
                        self.sel.select_item(i, false);
                    }
                }
            }
        }
        self.update_status();
    }

    fn scroll_to(&self, pos: u32, select: bool) {
        let flags = if select {
            gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS
        } else {
            gtk::ListScrollFlags::NONE
        };
        if self.is_grid() {
            self.grid.scroll_to(pos, flags, None);
        } else {
            self.colview.scroll_to(pos, None, flags, None);
        }
    }

    /// Watch the folder so changes made elsewhere show up live.
    fn watch(self: &Rc<Self>) {
        let cwd = self.cwd();
        if is_pc(&cwd) {
            if let Some(old) = self.monitor.replace(None) {
                old.cancel();
            }
            return;
        }
        let f = gio::File::for_path(if is_trash(&cwd) { trash::home_trash().join("info") } else { cwd });
        let mon = f.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE).ok();
        if let Some(mon) = &mon {
            let w = Rc::downgrade(self);
            mon.connect_changed(move |_, _, _, ev| {
                if matches!(ev, gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::PreUnmount) {
                    return;
                }
                if let Some(m) = w.upgrade() {
                    if m.searching.get() {
                        return;
                    }
                    let w2 = Rc::downgrade(&m);
                    m.deb.run(200, move || {
                        if let Some(m) = w2.upgrade() {
                            m.load(true);
                        }
                    });
                }
            });
        }
        if let Some(old) = self.monitor.replace(mon) {
            old.cancel();
        }
    }

    // --------------------------------------------------------------- view bits --

    fn is_grid(&self) -> bool {
        self.views.visible_child_name().as_deref() == Some("grid")
    }

    pub fn set_view(&self, v: &str) {
        let grid = v == "grid";
        let had_focus = self.root.root().and_then(|r| r.focus()).is_some_and(|f| f.is_ancestor(&self.views));
        if !is_pc(&self.cwd()) {
            self.views.set_visible_child_name(if grid { "grid" } else { "list" });
        }
        if grid { self.btn_grid.set_active(true) } else { self.btn_list.set_active(true) }
        state::set("view", if grid { "grid" } else { "list" });
        if had_focus {
            self.focus_view();
        }
    }

    pub fn toggle_hidden(&self) {
        self.btn_hidden.set_active(!self.btn_hidden.is_active());
    }

    fn set_hidden(&self, on: bool) {
        if self.show_hidden.get() == on {
            return;
        }
        self.show_hidden.set(on);
        self.btn_hidden.set_label(if on { "\u{f0208}" } else { "\u{f0209}" });
        self.filter.changed(if on { gtk::FilterChange::LessStrict } else { gtk::FilterChange::MoreStrict });
        state::set("show_hidden", on);
    }

    pub fn toggle_preview(&self) {
        self.btn_preview.set_active(!self.btn_preview.is_active());
    }

    /// Show/hide the preview pane (the window keeps every tab in step).
    pub fn set_preview(&self, on: bool) {
        if self.btn_preview.is_active() != on {
            self.btn_preview.set_active(on); // re-enters via the toggled handler
            return;
        }
        self.preview.root.set_visible(on);
        state::set("preview", on);
        if on {
            self.preview.show(&self.selected());
        } else {
            self.preview.show(&[]); // free any texture
        }
    }

    fn queue_preview(self: &Rc<Self>) {
        if !self.preview.root.get_visible() {
            return;
        }
        let w = Rc::downgrade(self);
        self.preview_deb.run(90, move || {
            if let Some(m) = w.upgrade() {
                m.preview.show(&m.selected());
            }
        });
    }

    pub fn focus_view(&self) {
        if self.is_grid() {
            self.grid.grab_focus();
        } else {
            self.colview.grab_focus();
        }
    }

    pub fn focus_search(&self, prefill: Option<char>) {
        self.search.grab_focus();
        if let Some(c) = prefill {
            let t = format!("{}{c}", self.search.text());
            self.search.set_text(&t);
            self.search.set_position(-1);
        }
    }

    /// Esc: clear the filter first, then the selection.
    pub fn escape(&self) -> bool {
        if !self.search.text().is_empty() {
            self.search.set_text("");
            self.focus_view();
            true
        } else if self.sel.selection().size() > 0 {
            self.sel.unselect_all();
            true
        } else {
            false
        }
    }

    pub fn select_all(&self) {
        self.sel.select_all();
    }

    pub fn edit_address(&self) {
        if self.addr_stack.visible_child_name().as_deref() == Some("entry") {
            return;
        }
        let p = self.cwd();
        self.addr_entry.set_text(&p.to_string_lossy());
        self.addr_stack.set_visible_child_name("entry");
        self.address.add_css_class("editing");
        self.addr_entry.grab_focus();
        self.addr_entry.select_region(0, -1);
    }

    fn end_edit(&self) {
        self.addr_stack.set_visible_child_name("crumbs");
        self.address.remove_css_class("editing");
    }

    fn build_crumbs(self: &Rc<Self>) {
        ui::clear(&self.crumbs);
        let p = self.cwd();
        if is_virtual(&p) {
            let b = gtk::Button::with_label(&format!("{}  {}", if is_pc(&p) { "\u{f0322}" } else { "\u{f01b4}" }, display_name(&p)));
            b.add_css_class("crumb");
            b.add_css_class("current");
            b.set_focus_on_click(false);
            self.crumbs.append(&b);
            return;
        }
        let home = util::home();
        let vol = crate::drives::mount_containing(&p);
        let (mut acc, label0, rest): (PathBuf, String, PathBuf) = match (p.strip_prefix(&home), vol) {
            // on a drive: "this pc > DRIVE > …" instead of "/ > run > media > user > DRIVE"
            (_, Some((m, title, icon))) => (m.clone(), format!("{icon}  {title}"), p.strip_prefix(&m).unwrap_or(&p).to_path_buf()),
            (Ok(rest), None) if home != Path::new("/") => (home.clone(), "\u{f02dc}  home".into(), rest.to_path_buf()),
            _ => (PathBuf::from("/"), "\u{f02ca}  /".into(), p.strip_prefix("/").unwrap_or(&p).to_path_buf()),
        };
        let mut segs = vec![];
        if !acc.starts_with(&home) || home == Path::new("/") {
            segs.push(("\u{f0322}  this pc".to_string(), PathBuf::from(PC)));
        }
        segs.push((label0, acc.clone()));
        for c in rest.components() {
            acc.push(c);
            segs.push((c.as_os_str().to_string_lossy().into_owned(), acc.clone()));
        }
        let n = segs.len();
        for (i, (label, target)) in segs.into_iter().enumerate() {
            if i > 0 {
                self.crumbs.append(&ui::label("\u{f0142}", &["crumb-sep"]));
            }
            let b = gtk::Button::with_label(&label);
            b.add_css_class("crumb");
            b.set_focus_on_click(false);
            if i == n - 1 {
                b.add_css_class("current");
            }
            let w = Rc::downgrade(self);
            let t = target.clone();
            b.connect_clicked(move |_| {
                if let Some(m) = w.upgrade() {
                    m.go(t.clone());
                }
            });
            actions::attach_crumb_drop(self, &b, target);
            self.crumbs.append(&b);
        }
        // keep the deepest folder in view: wait for the new trail to be measured
        // (a few frames, then the callback stops so the frame clock can idle)
        let sw = self.crumb_scroll.clone();
        let frames = Cell::new(0);
        self.crumbs.add_tick_callback(move |_, _| {
            let a = sw.hadjustment();
            if a.upper() > a.page_size() {
                a.set_value(a.upper() - a.page_size());
            }
            frames.set(frames.get() + 1);
            if frames.get() < 4 { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }

    /// Snapshot of the selected items.
    pub fn selected(&self) -> Vec<Item> {
        let bs = self.sel.selection();
        (0..bs.size())
            .filter_map(|i| self.sel.item(bs.nth(i as u32)))
            .map(|o| item::of(&o).clone())
            .collect()
    }

    fn update_status(&self) {
        let shown = self.sel.n_items();
        let total = self.store.n_items();
        let err = self.error.borrow().clone();
        let mut l = format!("{shown} item{}", if shown == 1 { "" } else { "s" });
        let cwd = self.cwd();
        if is_pc(&cwd) {
            let n = self.pc.borrow().as_ref().map(|p| p.count()).unwrap_or(1);
            self.status_l.set_text(&format!("{n} drive{}", if n == 1 { "" } else { "s" }));
            self.status_r.set_text("");
            self.empty.set_visible(false);
            return;
        }
        if is_trash(&cwd) {
            l = format!("{shown} item{} in trash", if shown == 1 { "" } else { "s" });
        } else if self.searching.get() {
            l = format!(
                "{shown} found in {} and subfolders{}",
                display_name(&self.cwd()),
                if self.search_running.get() { "  ·  searching…" } else if self.search_capped.get() { "  ·  first 5000 shown" } else { "" }
            );
        } else if !self.query.borrow().is_empty() {
            l = format!("{shown} of {total} match");
        } else if total > shown {
            l.push_str(&format!("  ·  {} hidden", total - shown));
        }
        self.status_l.set_text(&l);
        self.selection_status();

        // empty / error overlay
        let (icon, text) = if let Some(e) = err {
            ("\u{f0026}", e)
        } else if shown > 0 {
            ("", String::new())
        } else if self.searching.get() && self.search_running.get() {
            ("\u{f0968}", "searching…".into())
        } else if !self.query.borrow().is_empty() {
            ("\u{f0349}", "no items match your search".into())
        } else if is_trash(&cwd) {
            ("\u{f0a7a}", "the trash is empty".into())
        } else if total > 0 {
            ("\u{f0209}", "only hidden files here  ·  ctrl+h to show them".into())
        } else {
            ("\u{f024b}", "this folder is empty".into())
        };
        self.empty.set_visible(!text.is_empty());
        self.empty_icon.set_text(icon);
        self.empty_label.set_text(&text);
        set_class(&self.empty_label, "err", self.error.borrow().is_some());
    }

    fn selection_status(&self) {
        if self.flash_deb.pending() {
            return; // a message is showing
        }
        let bs = self.sel.selection();
        let n = bs.size();
        if n == 0 {
            self.status_r.set_text("");
            return;
        }
        let mut bytes = 0u64;
        let mut files = 0;
        for i in 0..n {
            if let Some(o) = self.sel.item(bs.nth(i as u32)) {
                let it = item::of(&o);
                if !it.is_dir {
                    bytes += it.size;
                    files += 1;
                }
            }
        }
        let mut r = format!("{n} selected");
        if files > 0 {
            r.push_str(&format!("  ·  {}", util::human_bytes(bytes)));
        }
        self.status_r.set_text(&r);
    }

    /// Show a short message in the status bar for a few seconds.
    fn flash(self: &Rc<Self>, msg: &str) {
        self.status_r.set_text(msg);
        let m = self.clone();
        self.flash_deb.run(3500, move || m.selection_status());
    }
}

// --------------------------------------------------------------------- helpers --

pub fn display_name(p: &Path) -> String {
    if is_pc(p) {
        return "this pc".into();
    }
    if is_trash(p) {
        return "trash".into();
    }
    if p == util::home() {
        return "home".into();
    }
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn tooltip(it: &Item) -> String {
    let mut t = format!("{}\n{}", it.name, it.kind);
    if !it.is_dir {
        t.push_str(&format!("  ·  {}", it.size_s()));
    }
    t.push_str(&format!("\nmodified {}", it.date_s()));
    if it.is_link {
        if let Ok(target) = std::fs::read_link(it.path()) {
            t.push_str(&format!("\n→ {}", target.to_string_lossy()));
        }
    }
    t
}

fn set_class(w: &impl IsA<gtk::Widget>, c: &str, on: bool) {
    if on { w.add_css_class(c) } else { w.remove_css_class(c) }
}

fn scrolled(w: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    s.set_vexpand(true);
    s.set_hexpand(true);
    s.set_child(Some(w));
    s
}

fn column(
    bref: &BRef,
    title: &str,
    expand: bool,
    drop: bool,
    setup: fn(&gtk::ListItem),
    bind: fn(&gtk::Widget, &Item),
    cmp: fn(&Item, &Item) -> std::cmp::Ordering,
) -> gtk::ColumnViewColumn {
    let f = gtk::SignalListItemFactory::new();
    let br = bref.clone();
    f.connect_setup(move |_, o| {
        let li = o.downcast_ref::<gtk::ListItem>().unwrap();
        setup(li);
        if let Some(w) = li.child() {
            actions::attach(&br, li, &w, drop);
        }
    });
    f.connect_bind(move |_, o| {
        let li = o.downcast_ref::<gtk::ListItem>().unwrap();
        if let (Some(w), Some(obj)) = (li.child(), li.item()) {
            bind(&w, &item::of(&obj));
        }
    });
    let c = gtk::ColumnViewColumn::new(Some(title), Some(f));
    c.set_expand(expand);
    c.set_resizable(true);
    c.set_sorter(Some(&gtk::CustomSorter::new(move |a, b| cmp(&item::of(a), &item::of(b)).into())));
    c
}

fn name_setup(li: &gtk::ListItem) {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.add_css_class("drop-cell");
    let ic = ui::label("", &["file-icon"]);
    ic.set_xalign(0.5);
    let nm = ui::label("", &[]);
    nm.set_ellipsize(pango::EllipsizeMode::Middle);
    nm.set_hexpand(true);
    nm.set_width_chars(16); // names stay readable; narrow windows scroll sideways instead
    b.append(&ic);
    b.append(&nm);
    li.set_child(Some(&b));
}

fn name_bind(w: &gtk::Widget, it: &Item) {
    let ic = w.first_child().and_downcast::<gtk::Label>().unwrap();
    let nm = w.last_child().and_downcast::<gtk::Label>().unwrap();
    ic.set_text(it.icon);
    ic.set_css_classes(&["file-icon", it.tint]);
    nm.set_text(&it.name);
    w.set_tooltip_text(Some(&tooltip(it)));
    set_class(w, "hidden-item", it.hidden);
}

fn text_setup(li: &gtk::ListItem) {
    let l = ui::label("", &["cell-dim"]);
    l.set_ellipsize(pango::EllipsizeMode::End);
    li.set_child(Some(&l));
}

fn size_setup(li: &gtk::ListItem) {
    let l = ui::label("", &["cell-dim"]);
    l.set_xalign(1.0);
    li.set_child(Some(&l));
}

// `column()` hands binders a &Widget; labels downcast here.
trait SetText {
    fn set_text(&self, s: &str);
}
impl SetText for gtk::Widget {
    fn set_text(&self, s: &str) {
        if let Some(l) = self.downcast_ref::<gtk::Label>() {
            l.set_text(s);
        }
    }
}
