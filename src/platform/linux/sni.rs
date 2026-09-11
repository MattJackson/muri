//! The StatusNotifierItem bridge (Path 1 — the native-menu fallback).
//!
//! [`MuriSni`] implements the [`ksni::Tray`] trait: it exports the tray icon and
//! a `com.canonical.dbusmenu` menu **built from muri's own [`Menu`]**, which the
//! SNI host (GNOME Shell, KDE Plasma, an XEmbed shim, …) draws in *its* process.
//! This is the honest, portable Linux tray: it works on X11 and Wayland alike and
//! is accessible over AT-SPI for free because the host draws a real native menu.
//! The cost is that muri's custom styling (`Segment`/`Flex`/`Align`/`Color`/
//! `Font`) is dropped — dbusmenu carries only label + icon + state (spec 22 §2).
//! Two consequences are worth calling out, because they surface as "muri doesn't
//! look like the native macOS menu" reports (issue #15) even though muri's own
//! painter renders both correctly (see `render::paint`'s `issue15_*` test):
//!
//!   1. **No tab-stop column.** A `label\tvalue` row is flattened to one string
//!      via [`Row::accessible_name`] (space-joined). dbusmenu has no right-aligned
//!      secondary column (the only trailing field is `shortcut`, for key
//!      accelerators, which GNOME Shell's appindicator does not render), so the
//!      value cannot be aligned into a shared column the way an `NSMenu` tab stop
//!      — or muri's own styled popup — does. muda/tray-icon share this host menu
//!      on Linux and have the same limitation.
//!   2. **Icon side is the host's.** A leading [`Icon::Png`] is exported as the
//!      item's `icon_data`; whether the host draws it leading or trailing (and at
//!      what size) is GNOME Shell / KDE's call, not muri's.
//!
//! Callers that need muri's exact OEM row layout on Linux should show the menu as
//! a pointer-anchored [`ContextMenu::open_at`](crate::ContextMenu::open_at), which
//! renders through muri's X11 styled popup (`render::paint`) and honors every
//! `Segment`/`Flex`/`Align` — identical to the macOS/Windows tray popup.
//!
//! Activations arrive as `ksni` `activate` callbacks on the menu items; each
//! carries the row's [`MenuId`], which is dispatched straight through the muri
//! [`Tray`]'s handler ([`Tray::dispatch`]) — the same unified dispatch path every
//! backend uses.

use super::LinuxMenuPresenter;
use crate::menu::{Icon, Item, Menu, MenuId};
use crate::Tray;

/// The muri → SNI adapter handed to `ksni`. Owns the muri [`Tray`] (its menu,
/// icon, tooltip and click handler) plus a best-effort visibility flag mapped
/// onto the SNI `Status`. `ksni` runs this on a background D-Bus service thread,
/// re-reading its properties whenever a
/// [`Handle::update`](ksni::blocking::Handle::update) mutates it.
pub(crate) struct MuriSni {
    /// The muri tray: source of the menu, icon, tooltip and dispatch handler.
    pub(crate) tray: Tray,
    /// Best-effort show/hide, surfaced as the SNI `Status` (Active/Passive).
    pub(crate) visible: bool,
    /// Which presenter draws the menu on this session (deliverable #1/#2). Picked
    /// once by [`super::detect_linux_presenter`]. Governs whether a click opens
    /// muri's own styled popup or defers to the host-drawn dbusmenu.
    pub(crate) presenter: LinuxMenuPresenter,
}

impl MuriSni {
    /// Route a row activation through the muri tray's unified dispatch path so
    /// `on_click` fires. Inert ids ([`MenuId::none`]) never dispatch.
    fn dispatch(&self, id: &MenuId) {
        if !id.is_none() {
            self.tray.dispatch(id);
        }
    }
}

/// Decode icon bytes into a `ksni::Icon` (ARGB32, network byte order — i.e.
/// `[A,R,G,B]` per pixel, straight alpha). Tries the PNG codec first, then the
/// SVG rasterizer, so both `Icon::Png` and `Icon::Svg` tray icons render.
/// Returns `None` if the bytes decode as neither; symbol/checkmark icons have no
/// bytes and yield `None` here, so the host falls back to a themed/blank icon
/// (spec 22 §2, best-effort).
fn icon_to_argb32(bytes: &[u8]) -> Option<ksni::Icon> {
    let (rgba, width, height) = crate::render::decode_icon_bytes(bytes)?;
    let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
    // `decode_png` yields straight-alpha RGBA; SNI wants straight ARGB32.
    for px in rgba.chunks_exact(4) {
        data.push(px[3]); // A
        data.push(px[0]); // R
        data.push(px[1]); // G
        data.push(px[2]); // B
    }
    Some(ksni::Icon {
        width: width as i32,
        height: height as i32,
        data,
    })
}

/// The PNG bytes of a leading [`Icon`], if any — dbusmenu item icons take raw PNG
/// bytes directly (unlike the SNI item icon, which wants ARGB32).
fn leading_png(icon: &Option<Icon>) -> Vec<u8> {
    match icon {
        Some(Icon::Png(bytes)) => bytes.to_vec(),
        _ => Vec::new(),
    }
}

/// Build the `ksni` menu items for one [`Menu`] level. Recurses through
/// [`Item::Submenu`], so muri's N-level nesting (decision #8) is carried for free
/// — dbusmenu nests arbitrarily deep and the host draws the whole tree.
fn build_items(menu: &Menu) -> Vec<ksni::menu::MenuItem<MuriSni>> {
    use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};

    menu.items
        .iter()
        .map(|item| match item {
            Item::Separator => MenuItem::Separator,
            Item::SectionHeader(row) => {
                // A styled group heading has no dbusmenu equivalent; degrade to a
                // disabled (non-interactive) label item.
                StandardItem {
                    label: row.accessible_name(),
                    enabled: false,
                    icon_data: leading_png(&row.leading),
                    ..Default::default()
                }
                .into()
            }
            // A rich content row (#44) has no dbusmenu/SNI equivalent (the host
            // draws plain menu items); degrade to a disabled, empty label so the
            // native tray menu stays well-formed.
            Item::Content(_) => StandardItem {
                label: String::new(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            Item::Row(row) => {
                let id = row.id.clone();
                let label = row.accessible_name();
                let icon_data = leading_png(&row.leading);
                let enabled = row.enabled && !row.id.is_none();
                if let Some(checked) = row.checked {
                    CheckmarkItem {
                        label,
                        enabled,
                        checked,
                        icon_data,
                        activate: Box::new(move |s: &mut MuriSni| s.dispatch(&id)),
                        ..Default::default()
                    }
                    .into()
                } else {
                    StandardItem {
                        label,
                        enabled,
                        icon_data,
                        activate: Box::new(move |s: &mut MuriSni| s.dispatch(&id)),
                        ..Default::default()
                    }
                    .into()
                }
            }
            Item::Submenu { label, menu } => SubMenu {
                label: label.accessible_name(),
                enabled: label.enabled,
                icon_data: leading_png(&label.leading),
                submenu: build_items(menu),
                ..Default::default()
            }
            .into(),
        })
        .collect()
}

impl ksni::Tray for MuriSni {
    /// A left-click should surface the same menu the host draws on right-click:
    /// a menu-only tray has no other primary action.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        // A stable, process-wide SNI id. Not user-visible.
        "muri".to_owned()
    }

    fn title(&self) -> String {
        self.tray
            .tooltip
            .clone()
            .unwrap_or_else(|| "muri".to_owned())
    }

    fn icon_name(&self) -> String {
        // We supply pixmaps (see `icon_pixmap`), not a freedesktop theme name.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        match &self.tray.icon {
            // PNG and SVG both rasterize to an ARGB32 pixmap; Checkmark/Symbol
            // have no raster bytes (the host shows its default/blank icon —
            // themed-name mapping is future work).
            Icon::Png(bytes) | Icon::Svg(bytes) => icon_to_argb32(bytes).into_iter().collect(),
            Icon::Checkmark | Icon::Symbol(_) => Vec::new(),
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.tray.tooltip.clone().unwrap_or_default(),
            ..Default::default()
        }
    }

    fn status(&self) -> ksni::Status {
        if self.visible {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    /// A left-click (SNI `Activate`). For the custom
    /// [`X11Popup`](LinuxMenuPresenter::X11Popup) presenter, open muri's OWN
    /// styled popup: **ignore** the host-supplied `(x, y)` (an SNI hint that is
    /// unreliable / often `0,0` off KDE/Waybar — research doc §5) and anchor at
    /// the live pointer via `XQueryPointer` instead (deliverable #3). For every
    /// other presenter this is a no-op: the host draws the exported `dbusmenu`.
    ///
    /// ksni-0.3.6 constraint (DEVICE-VERIFY, ADR-0003): ksni derives `ItemIsMenu`
    /// from the type-level `MENU_ON_ACTIVATE` const and, when it is `true`, makes
    /// `Activate` return `UnknownMethod` so the host shows the dbusmenu instead of
    /// calling this method. So to actually *receive* this activate for the X11
    /// presenter, the device-side task must select a `MENU_ON_ACTIVATE = false`
    /// adapter for `X11Popup` (e.g. a const-generic split of `MuriSni`, threaded
    /// through the non-fn-pointer `run_sni_loop` seam) or move to a raw-`zbus` SNI
    /// impl. The routing itself is wired here and is correct the moment activate
    /// fires; right-click `ContextMenu` is unreachable in ksni 0.3.6 (hard
    /// `UnknownMethod`) and is the same upstream/raw-zbus task.
    fn activate(&mut self, _x: i32, _y: i32) {
        #[cfg(feature = "x11-popup")]
        if self.presenter == LinuxMenuPresenter::X11Popup {
            super::open_x11_popup_at_cursor(&self.tray);
        }
    }

    fn menu(&self) -> Vec<ksni::menu::MenuItem<Self>> {
        match self.presenter {
            // When muri draws its own styled popup (the X11 custom presenter),
            // don't *also* export a host-drawn dbusmenu tree — return an empty menu
            // so the custom popup is the only surface (deliverable #3).
            LinuxMenuPresenter::X11Popup => Vec::new(),
            // The native dbusmenu baseline (GNOME + universal fallback), and — until
            // the layer-shell path is wired device-side — the Wayland presenter too:
            // export the full native menu (the accessible, host-drawn baseline).
            LinuxMenuPresenter::NativeDbusMenu | LinuxMenuPresenter::WaylandLayerShell => {
                build_items(&self.tray.menu)
            }
        }
    }
}
