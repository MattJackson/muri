//! The StatusNotifierItem bridge (Path 1 — the native-menu fallback).
//!
//! [`MuriSni`] implements [`ksni::Tray`]: it exports the tray icon and a
//! `com.canonical.dbusmenu` menu built from muri's own [`Menu`], which the SNI
//! host draws in *its* process — portable and accessible over AT-SPI for free,
//! but muri's custom styling (`Segment`/`Flex`/`Align`/`Color`/`Font`) is
//! dropped since dbusmenu carries only label + icon + state (spec 22 §2). Two
//! consequences (issue #15): no tab-stop column (a `label\tvalue` row flattens
//! to one string; dbusmenu has no right-aligned secondary column), and the icon
//! side/size is the host's call, not muri's.
//!
//! Callers that need muri's exact row layout on Linux should show the menu as a
//! pointer-anchored [`ContextMenu::open_at`](crate::ContextMenu::open_at)
//! instead, which renders through muri's own styled popup.
//!
//! Activations arrive as `ksni` `activate` callbacks; each carries the row's
//! [`MenuId`], dispatched through [`Tray::dispatch`] like every other backend.

use super::LinuxMenuPresenter;
use crate::menu::{Icon, Item, Menu, MenuId};
use crate::Tray;

/// The muri → SNI adapter handed to `ksni`. Owns the muri [`Tray`] (its menu,
/// icon, tooltip and click handler) plus a best-effort visibility flag mapped
/// onto the SNI `Status`. `ksni` runs this on a background D-Bus service thread.
///
/// The `MENU_ACTIVATE` const parameter is threaded into ksni's
/// [`ksni::Tray::MENU_ON_ACTIVATE`] (a type-level const it derives the SNI
/// `ItemIsMenu` property from) — the load-bearing fix for the styled presenters
/// (deliverable #2, ADR-0003 §3): `true` for the
/// [`NativeDbusMenu`](LinuxMenuPresenter::NativeDbusMenu) baseline (host draws
/// the exported dbusmenu, [`Self::activate`] never called); `false` for the
/// styled [`X11Popup`](LinuxMenuPresenter::X11Popup)/
/// [`WaylandLayerShell`](LinuxMenuPresenter::WaylandLayerShell) presenters
/// (empty exported menu, host forwards the click as `Activate`).
///
/// The two instantiations are distinct types, resolved once by
/// [`super::detect_linux_presenter`] before the `ksni` service is spawned.
pub(crate) struct MuriSni<const MENU_ACTIVATE: bool> {
    /// The muri tray: source of the menu, icon, tooltip and dispatch handler.
    pub(crate) tray: Tray,
    /// Best-effort show/hide, surfaced as the SNI `Status` (Active/Passive).
    pub(crate) visible: bool,
    /// Which presenter draws the menu on this session (deliverable #1/#2). Picked
    /// once by [`super::detect_linux_presenter`]. Governs whether a click opens
    /// muri's own styled popup or defers to the host-drawn dbusmenu.
    pub(crate) presenter: LinuxMenuPresenter,
}

impl<const MENU_ACTIVATE: bool> MuriSni<MENU_ACTIVATE> {
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
///
/// An [`Icon::Svg`] is rasterized to PNG (the DEFAULT GNOME dbusmenu path, which
/// otherwise silently dropped SVG leading icons — #F7); a [`Icon::Checkmark`] /
/// [`Icon::Symbol`] carries no raster bytes, so the host draws its own glyph. The
/// match is exhaustive (no `_`) so a future [`Icon`] variant is a compile error
/// here rather than another silently-dropped icon.
fn leading_png(icon: &Option<Icon>) -> Vec<u8> {
    match icon {
        Some(Icon::Png(bytes)) => bytes.to_vec(),
        Some(Icon::Svg(bytes)) => svg_to_png(bytes).unwrap_or_default(),
        Some(Icon::Checkmark) | Some(Icon::Symbol(_)) | None => Vec::new(),
    }
}

/// Rasterize muri's restricted `Icon::Svg` subset and re-encode it as PNG, so a
/// dbusmenu item icon (which consumes PNG bytes, not the SVG subset) can carry an
/// SVG leading icon — the Linux twin of the macOS `NSImage` SVG→PNG bridge.
/// `None` for non-SVG / unparseable bytes.
fn svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let (rgba, w, h) = crate::render::rasterize_svg(bytes)?;
    crate::render::encode_rgba_png(&rgba, w, h)
}

/// Build the `ksni` menu items for one [`Menu`] level. Recurses through
/// [`Item::Submenu`], so muri's N-level nesting (decision #8) is carried for free
/// — dbusmenu nests arbitrarily deep and the host draws the whole tree.
fn build_items<const M: bool>(menu: &Menu) -> Vec<ksni::menu::MenuItem<MuriSni<M>>> {
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
                        activate: Box::new(move |s: &mut MuriSni<M>| s.dispatch(&id)),
                        ..Default::default()
                    }
                    .into()
                } else {
                    StandardItem {
                        label,
                        enabled,
                        icon_data,
                        activate: Box::new(move |s: &mut MuriSni<M>| s.dispatch(&id)),
                        ..Default::default()
                    }
                    .into()
                }
            }
            Item::Submenu { label, menu } => SubMenu {
                label: label.accessible_name(),
                enabled: label.enabled,
                icon_data: leading_png(&label.leading),
                submenu: build_items::<M>(menu),
                ..Default::default()
            }
            .into(),
        })
        .collect()
}

impl<const MENU_ACTIVATE: bool> ksni::Tray for MuriSni<MENU_ACTIVATE> {
    /// Threaded from the type parameter into ksni's `ItemIsMenu` derivation (see the
    /// [`MuriSni`] type docs): `true` for the native-dbusmenu baseline (host draws
    /// the menu on left-click), `false` for the styled X11/Wayland presenters (the
    /// host forwards `Activate` and muri draws its own popup).
    const MENU_ON_ACTIVATE: bool = MENU_ACTIVATE;

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

    /// A left-click (SNI `Activate`), reached only when `ItemIsMenu` is `false`
    /// (the styled presenters, deliverable #2):
    ///
    /// - [`X11Popup`](LinuxMenuPresenter::X11Popup): ignores the unreliable
    ///   SNI-supplied `(x, y)` and anchors at the live pointer via
    ///   `XQueryPointer` instead (research doc §5).
    /// - [`WaylandLayerShell`](LinuxMenuPresenter::WaylandLayerShell): anchors at
    ///   the SNI-reported `(x, y)`, the one real coordinate channel on Wayland.
    ///
    /// The [`NativeDbusMenu`](LinuxMenuPresenter::NativeDbusMenu) baseline never
    /// reaches here.
    ///
    /// Right-click `ContextMenu(x, y)` is a hard `UnknownMethod` in ksni 0.3.6,
    /// so it cannot be intercepted without a raw-`zbus` SNI impl or an upstream
    /// ksni capability (ADR-0003 §3).
    fn activate(&mut self, x: i32, y: i32) {
        // Only the wayland presenter consumes the coordinate; keep the params live
        // for every other (feature/variant) configuration.
        let _ = (x, y);
        match self.presenter {
            #[cfg(feature = "x11-popup")]
            LinuxMenuPresenter::X11Popup => super::open_x11_popup_at_cursor(&self.tray),
            #[cfg(feature = "wayland-styled")]
            LinuxMenuPresenter::WaylandLayerShell => super::open_wayland_popup_at(&self.tray, x, y),
            // The host draws the exported dbusmenu (`ItemIsMenu = true`), so its
            // `activate` is a genuine no-op — spelled out (like the sibling
            // `menu()`) rather than swept into `_`, narrowing the silent-variant
            // blind spot to only the feature-gated presenters below.
            LinuxMenuPresenter::NativeDbusMenu => {}
            // Reached only when a styled presenter's feature is compiled out
            // (X11Popup without `x11-popup`, WaylandLayerShell without
            // `wayland-styled`); unreachable once both features are on.
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    fn menu(&self) -> Vec<ksni::menu::MenuItem<Self>> {
        match self.presenter {
            // When muri draws its own styled popup (the X11 or Wayland custom
            // presenter), don't *also* export a host-drawn dbusmenu tree — return an
            // empty menu so the custom popup is the only surface (deliverable #2/#3).
            LinuxMenuPresenter::X11Popup | LinuxMenuPresenter::WaylandLayerShell => Vec::new(),
            // The native dbusmenu baseline (GNOME + universal fallback): export the
            // full native menu (the accessible, host-drawn baseline).
            LinuxMenuPresenter::NativeDbusMenu => build_items(&self.tray.menu),
        }
    }
}
