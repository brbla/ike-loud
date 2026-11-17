//! The tray icon: a StatusNotifierItem with a com.canonical.dbusmenu menu,
//! over GIO's D-Bus (no extra library).
//!
//! KDE Plasma shows it natively; GNOME only with the AppIndicator
//! extension. Without a tray host [`Tray::start`] still succeeds and the
//! icon simply does not appear: the window and the notifications carry the
//! status (spec: the tray icon is an extra).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gio;
use gtk::glib::prelude::*;
use gtk::glib::{self, Variant};

use crate::icons::Badge;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";

const ITEM_XML: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus"><arg name="status" type="s"/></signal>
  </interface>
</node>"#;

const MENU_XML: &str = r#"<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg name="parentId" type="i" direction="in"/>
      <arg name="recursionDepth" type="i" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="revision" type="u" direction="out"/>
      <arg name="layout" type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="properties" type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg name="id" type="i" direction="in"/>
      <arg name="name" type="s" direction="in"/>
      <arg name="value" type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg name="id" type="i" direction="in"/>
      <arg name="eventId" type="s" direction="in"/>
      <arg name="data" type="v" direction="in"/>
      <arg name="timestamp" type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg name="events" type="a(isvu)" direction="in"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg name="id" type="i" direction="in"/>
      <arg name="needUpdate" type="b" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="updatesNeeded" type="ai" direction="out"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <signal name="ItemsPropertiesUpdated">
      <arg name="updatedProps" type="a(ia{sv})"/>
      <arg name="removedProps" type="a(ias)"/>
    </signal>
    <signal name="LayoutUpdated">
      <arg name="revision" type="u"/>
      <arg name="parent" type="i"/>
    </signal>
  </interface>
</node>"#;

/// What the user picked in the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Left click or "Show".
    Show,
    /// "Open folder".
    OpenFolder,
    /// "Pause" / "Resume".
    TogglePause,
    /// "Quit".
    Quit,
}

/// One menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// A clickable entry.
    Entry {
        /// Label.
        label: String,
        /// What it does.
        action: TrayAction,
    },
    /// A separator line.
    Separator,
}

struct Shared {
    badge: Badge,
    title: String,
    tooltip: String,
    menu: Vec<MenuItem>,
    revision: u32,
    theme_path: String,
    pixmaps: HashMap<Badge, Vec<(i32, i32, Vec<u8>)>>,
}

/// The registered tray icon.
pub struct Tray {
    connection: gio::DBusConnection,
    shared: Rc<RefCell<Shared>>,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").finish_non_exhaustive()
    }
}

impl Tray {
    /// Registers the icon on the session bus. `on_action` runs on the main
    /// thread. `theme_path` holds the icons ([`crate::icons::install`]).
    pub fn start(
        theme_path: &std::path::Path,
        menu: Vec<MenuItem>,
        on_action: impl Fn(TrayAction) + 'static,
    ) -> Result<Self, glib::Error> {
        let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
        let shared = Rc::new(RefCell::new(Shared {
            badge: Badge::Quiet,
            title: String::from("ike loud"),
            tooltip: String::new(),
            menu,
            revision: 1,
            theme_path: theme_path.to_string_lossy().into_owned(),
            pixmaps: Badge::ALL.iter().map(|b| (*b, pixmaps(*b))).collect(),
        }));
        let on_action: Rc<dyn Fn(TrayAction)> = Rc::new(on_action);

        let item_info = interface(ITEM_XML, ITEM_IFACE)?;
        let state = Rc::clone(&shared);
        let action = Rc::clone(&on_action);
        connection
            .register_object(ITEM_PATH, &item_info)
            .method_call(move |_, _, _, _, method, _, invocation| {
                if method == "Activate" {
                    action(TrayAction::Show);
                }
                invocation.return_value(None);
            })
            .property(move |_, _, _, _, name| item_property(&state.borrow(), name))
            .build()?;

        let menu_info = interface(MENU_XML, MENU_IFACE)?;
        let state = Rc::clone(&shared);
        let menu_state = Rc::clone(&shared);
        connection
            .register_object(MENU_PATH, &menu_info)
            .method_call(move |_, _, _, _, method, params, invocation| {
                let reply = menu_call(&state.borrow(), method, &params, &on_action);
                invocation.return_value(reply.as_ref());
            })
            .property(move |_, _, _, _, name| match name {
                "Version" => 3u32.to_variant(),
                "TextDirection" => "ltr".to_variant(),
                "IconThemePath" => vec![menu_state.borrow().theme_path.clone()].to_variant(),
                _ => "normal".to_variant(),
            })
            .build()?;

        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        connection.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            Some(&(name.as_str(), 4u32).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        )?;
        // Register now and again whenever a tray host (re)appears, e.g.
        // after Plasma restarts.
        let watcher_name = name.clone();
        // The watch stays for the life of the app (its id is not a guard).
        let _watch = gio::bus_watch_name_on_connection(
            &connection,
            WATCHER,
            gio::BusNameWatcherFlags::NONE,
            move |connection, _, _| register_with_watcher(&connection, &watcher_name),
            |_, _| {},
        );
        Ok(Self { connection, shared })
    }

    /// Shows a new state.
    pub fn update(&self, badge: Badge, tooltip: &str) {
        let changed = {
            let mut s = self.shared.borrow_mut();
            let changed = s.badge != badge || s.tooltip != tooltip;
            s.badge = badge;
            s.tooltip = tooltip.to_string();
            changed
        };
        if changed {
            for signal in ["NewIcon", "NewToolTip"] {
                let _ = self
                    .connection
                    .emit_signal(None, ITEM_PATH, ITEM_IFACE, signal, None);
            }
            let status = if matches!(badge, Badge::Attention | Badge::Error) {
                "NeedsAttention"
            } else {
                "Active"
            };
            let _ = self.connection.emit_signal(
                None,
                ITEM_PATH,
                ITEM_IFACE,
                "NewStatus",
                Some(&(status,).to_variant()),
            );
        }
    }

    /// Replaces the menu (e.g. "Pause" becomes "Resume").
    pub fn set_menu(&self, menu: Vec<MenuItem>) {
        let revision = {
            let mut s = self.shared.borrow_mut();
            if s.menu == menu {
                return;
            }
            s.menu = menu;
            s.revision = s.revision.wrapping_add(1);
            s.revision
        };
        let _ = self.connection.emit_signal(
            None,
            MENU_PATH,
            MENU_IFACE,
            "LayoutUpdated",
            Some(&(revision, 0i32).to_variant()),
        );
    }
}

fn interface(xml: &str, name: &str) -> Result<gio::DBusInterfaceInfo, glib::Error> {
    gio::DBusNodeInfo::for_xml(xml)?
        .lookup_interface(name)
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::NotFound, "D-Bus interface missing"))
}

fn register_with_watcher(connection: &gio::DBusConnection, name: &str) {
    connection.call(
        Some(WATCHER),
        "/StatusNotifierWatcher",
        WATCHER,
        "RegisterStatusNotifierItem",
        Some(&(name,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        |_| {},
    );
}

fn item_property(s: &Shared, name: &str) -> Variant {
    let icon = s.badge.icon_name();
    match name {
        "Category" => "ApplicationStatus".to_variant(),
        "Id" => "ikeloud".to_variant(),
        "Title" => s.title.to_variant(),
        "Status" => if matches!(s.badge, Badge::Attention | Badge::Error) {
            "NeedsAttention"
        } else {
            "Active"
        }
        .to_variant(),
        "WindowId" => 0i32.to_variant(),
        "IconName" | "AttentionIconName" => icon.to_variant(),
        "IconThemePath" => s.theme_path.to_variant(),
        "IconPixmap" => s
            .pixmaps
            .get(&s.badge)
            .cloned()
            .unwrap_or_default()
            .to_variant(),
        "ToolTip" => (
            icon,
            Vec::<(i32, i32, Vec<u8>)>::new(),
            s.title.clone(),
            s.tooltip.clone(),
        )
            .to_variant(),
        "ItemIsMenu" => false.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH.to_string())
            .map(|p| p.to_variant())
            .unwrap_or_else(|_| "/".to_variant()),
        _ => "".to_variant(),
    }
}

/// Menu ids: 0 is the root, entries are 1, 2, … in order.
fn entry_props(item: &MenuItem) -> HashMap<String, Variant> {
    let mut props = HashMap::new();
    match item {
        MenuItem::Entry { label, .. } => {
            props.insert("label".to_string(), label.to_variant());
            props.insert("enabled".to_string(), true.to_variant());
        }
        MenuItem::Separator => {
            props.insert("type".to_string(), "separator".to_variant());
        }
    }
    props.insert("visible".to_string(), true.to_variant());
    props
}

fn node(id: i32, props: HashMap<String, Variant>, children: Vec<Variant>) -> Variant {
    (id, props, children).to_variant()
}

fn layout(s: &Shared) -> Variant {
    let children: Vec<Variant> = s
        .menu
        .iter()
        .zip(1i32..)
        .map(|(item, id)| Variant::from_variant(&node(id, entry_props(item), Vec::new())))
        .collect();
    let mut root = HashMap::new();
    root.insert("children-display".to_string(), "submenu".to_variant());
    // (u(ia{sv}av)): a tuple of the revision and the root node itself, not
    // the node boxed in a variant.
    Variant::tuple_from_iter([s.revision.to_variant(), node(0, root, children)])
}

fn item_by_id(s: &Shared, id: i32) -> Option<&MenuItem> {
    let index = usize::try_from(id.checked_sub(1)?).ok()?;
    s.menu.get(index)
}

fn menu_call(
    s: &Shared,
    method: &str,
    params: &Variant,
    on_action: &Rc<dyn Fn(TrayAction)>,
) -> Option<Variant> {
    match method {
        "GetLayout" => Some(layout(s)),
        "GetGroupProperties" => {
            let ids: Vec<i32> = params
                .try_child_value(0)
                .and_then(|v| v.get::<Vec<i32>>())
                .unwrap_or_default();
            let props: Vec<(i32, HashMap<String, Variant>)> = ids
                .into_iter()
                .filter_map(|id| item_by_id(s, id).map(|item| (id, entry_props(item))))
                .collect();
            Some((props,).to_variant())
        }
        "GetProperty" => {
            let id = params.try_child_value(0).and_then(|v| v.get::<i32>());
            let name = params.try_child_value(1).and_then(|v| v.get::<String>());
            let value = id
                .and_then(|id| item_by_id(s, id))
                .zip(name)
                .and_then(|(item, name)| entry_props(item).remove(&name))
                .unwrap_or_else(|| "".to_variant());
            Some((value,).to_variant())
        }
        "Event" => {
            let id = params.try_child_value(0).and_then(|v| v.get::<i32>());
            let event = params.try_child_value(1).and_then(|v| v.get::<String>());
            if event.as_deref() == Some("clicked")
                && let Some(MenuItem::Entry { action, .. }) = id.and_then(|id| item_by_id(s, id))
            {
                let action = *action;
                let on_action = Rc::clone(on_action);
                // Not inside the D-Bus call: the action may change the menu.
                glib::idle_add_local_once(move || on_action(action));
            }
            None
        }
        "EventGroup" => Some((Vec::<i32>::new(),).to_variant()),
        "AboutToShow" => Some((false,).to_variant()),
        "AboutToShowGroup" => Some((Vec::<i32>::new(), Vec::<i32>::new()).to_variant()),
        _ => None,
    }
}

/// The icon in pixels (ARGB32, network byte order) for hosts without SVG
/// lookup by name.
fn pixmaps(badge: Badge) -> Vec<(i32, i32, Vec<u8>)> {
    let bytes = glib::Bytes::from_owned(badge.svg().into_bytes());
    [22, 32, 64]
        .into_iter()
        .filter_map(|size| {
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
                &stream,
                size,
                size,
                true,
                gio::Cancellable::NONE,
            )
            .ok()?;
            Some((pixbuf.width(), pixbuf.height(), argb(&pixbuf)?))
        })
        .collect()
}

fn argb(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> Option<Vec<u8>> {
    let pixbuf = pixbuf.add_alpha(false, 0, 0, 0).ok()?;
    let width = usize::try_from(pixbuf.width()).ok()?;
    let height = usize::try_from(pixbuf.height()).ok()?;
    let stride = usize::try_from(pixbuf.rowstride()).ok()?;
    let data = pixbuf.read_pixel_bytes();
    let mut out = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for row in 0..height {
        for col in 0..width {
            let at = row.checked_mul(stride)?.checked_add(col.checked_mul(4)?)?;
            let px = data.get(at..at.checked_add(4)?)?;
            if let [r, g, b, a] = *px {
                out.extend_from_slice(&[a, r, g, b]);
            }
        }
    }
    Some(out)
}
