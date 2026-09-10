//! The StatusNotifierItem bridge (Path 1 — the native-menu fallback).
//!
//! [`MuriSni`] implements the [`ksni::Tray`] trait: it exports the tray icon and
//! a `com.canonical.dbusmenu` menu **built from muri's own [`Menu`]**, which the
//! SNI host (GNOME Shell, KDE Plasma, an XEmbed shim, …) draws in *its* process.
//! This is the honest, portable Linux tray: it works on X11 and Wayland alike and
//! is accessible over AT-SPI for free because the host draws a real native menu.
//! The cost is that muri's custom styling (`Segment`/`Flex`/`Align`/`Color`/
//! `Font`) is dropped — dbusmenu carries only label + icon + state (spec 22 §2).
//!
//! Activations arrive as `ksni` `activate` callbacks on the menu items; each
//! carries the row's [`MenuId`], which is dispatched straight through the muri
//! [`Tray`]'s handler ([`Tray::dispatch`]) — the same unified dispatch path every
//! backend uses.

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

/// Decode a PNG into a `ksni::Icon` (ARGB32, network byte order — i.e. `[A,R,G,B]`
/// per pixel, straight alpha). Returns `None` if the bytes are not a decodable
/// raster PNG. SVG/symbol/checkmark icons have no PNG bytes and yield `None`
/// here; the host falls back to a themed/blank icon (spec 22 §2, best-effort).
fn png_to_argb32(bytes: &[u8]) -> Option<ksni::Icon> {
    let (rgba, width, height) = crate::render::decode_png(bytes)?;
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
            Icon::Png(bytes) | Icon::Svg(bytes) => png_to_argb32(bytes).into_iter().collect(),
            // Checkmark/Symbol have no raster bytes here; the host shows its
            // default/blank icon. Themed-name mapping is future work.
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

    fn menu(&self) -> Vec<ksni::menu::MenuItem<Self>> {
        build_items(&self.tray.menu)
    }
}
