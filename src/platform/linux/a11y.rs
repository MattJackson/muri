//! Accessibility seam for the **self-drawn** Linux popups (X11 override-redirect
//! + Wayland layer-shell) — **SCAFFOLD ONLY** (0.11.0).
//!
//! ## Why this exists
//!
//! The native dbusmenu presenter ([`super::sni`]) is accessible over AT-SPI **for
//! free**: the SNI host draws a real native menu, so Orca walks it with no work
//! from muri. The moment muri draws its *own* styled popup (the X11 or Wayland
//! custom presenter), that free accessibility is gone — the compositor sees only
//! an opaque surface of pixels. To stay accessible, muri must publish a semantic
//! tree itself, exactly as it already does on macOS/Windows via `accesskit`.
//!
//! ## The plan (research doc §13)
//!
//! `accesskit_unix` implements the AT-SPI2 D-Bus interfaces via `zbus`. Crucially
//! **AT-SPI2 rides D-Bus, not the display protocol**, so the adapter is identical
//! under X11 and Wayland and never touches the compositor — it only needs a
//! logical `Window`-role node plus the menu items as `Role::MenuItem` /
//! `Role::MenuItemCheckBox`, and the app reporting focus. muri already builds this
//! exact tree for the macOS/Windows self-drawn menus behind the `a11y` feature;
//! the Unix adapter reuses it.
//!
//! **Known Wayland caveat:** `Adapter::set_root_window_bounds()` is X11-only (a
//! Wayland client can't read its window position), so absolute-screen AT
//! hit-testing won't be exact on Wayland — roles, text, focus, and actions all
//! still work. This is the documented, bounded cost of the styled path; it is a
//! reason the dbusmenu presenter stays the default where styling isn't essential.
//!
//! ## Status
//!
//! This is the **seam only**. Wiring `accesskit_unix` is a device-side task: it
//! adds the dependency (behind the `a11y` feature, `cfg(unix, not macos)`) and
//! feeds a `TreeUpdate` built from the popup's laid-out rows to the adapter on the
//! popup's own thread. Until then these hooks are inert no-ops so the X11/Wayland
//! popups run without accessibility rather than not at all.

#![allow(dead_code)] // Scaffold seam: wired device-side under the `a11y` feature.

use crate::menu::Menu;

/// A handle to the AT-SPI adapter driving one self-drawn popup session. Owns the
/// (future) `accesskit_unix::Adapter`; created when a styled popup opens and
/// dropped when it dismisses, mirroring the macOS/Windows adapters' lifetimes.
pub(super) struct PopupA11y {
    // DEVICE-VERIFY(0.11.0): hold the real adapter here, e.g.
    //   adapter: accesskit_unix::Adapter,
    // behind the `a11y` feature. Kept a ZST placeholder so the seam type exists
    // and the call sites compile today.
    _private: (),
}

impl PopupA11y {
    /// Attach an AT-SPI adapter to a freshly-opened styled popup and publish its
    /// initial tree (menu items as `Role::MenuItem`, the panel as `Role::Window`).
    /// Returns `None` when the `a11y` feature is off or no assistive tech is
    /// listening (the adapter lazily no-ops), so callers run unconditionally.
    ///
    /// DEVICE-VERIFY(0.11.0): build the `accesskit::TreeUpdate` from `menu` and
    /// hand it to `accesskit_unix::Adapter::new`. On X11 also call
    /// `set_root_window_bounds` with the popup's screen rect (it is X11-only).
    pub(super) fn attach(menu: &Menu) -> Option<Self> {
        let _ = menu;
        None
    }

    /// Report a hover/keyboard-focus change to the AT so Orca announces the row.
    ///
    /// DEVICE-VERIFY(0.11.0): push a focused-node `TreeUpdate` to the adapter.
    pub(super) fn focus_row(&self, _index: Option<usize>) {}

    /// Report that the popup surface gained/lost focus (AccessKit needs this
    /// explicitly on Unix).
    ///
    /// DEVICE-VERIFY(0.11.0): call `adapter.update_window_focus_state(focused)`.
    pub(super) fn set_focused(&self, _focused: bool) {}
}
