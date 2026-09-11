//! Accessibility bridge for the **self-drawn** Linux popups (X11 override-redirect
//! + Wayland layer-shell), wired to `accesskit_unix` behind the `a11y` feature.
//!
//! ## Why this exists
//!
//! The native dbusmenu presenter ([`super::sni`]) is accessible over AT-SPI **for
//! free**: the SNI host draws a real native menu, so Orca walks it with no work
//! from muri. The moment muri draws its *own* styled popup (the X11 or Wayland
//! custom presenter), that free accessibility is gone — the compositor sees only
//! an opaque surface of pixels. To stay accessible, muri publishes a semantic tree
//! itself, exactly as it already does on macOS/Windows via `accesskit`.
//!
//! ## How it works (research doc §13)
//!
//! `accesskit_unix` implements the AT-SPI2 D-Bus interfaces via `zbus`. Crucially
//! **AT-SPI2 rides D-Bus, not the display protocol**, so the adapter is identical
//! under X11 and Wayland and never touches the compositor — it only needs a logical
//! `Window`/`Menu`-role node plus the menu items as `Role::MenuItem` /
//! `Role::MenuItemCheckBox`, and the app reporting focus. muri already builds this
//! exact tree for the macOS/Windows self-drawn menus behind the `a11y` feature
//! ([`crate::a11y`]); the Unix adapter reuses [`crate::a11y::accesskit::tree_update`].
//!
//! **Known Wayland caveat:** `Adapter::set_root_window_bounds()` is X11-only (a
//! Wayland client can't read its window position), so absolute-screen AT
//! hit-testing won't be exact on Wayland — roles, text, focus, and actions all
//! still work. muri never calls it (the popup carries no reliable screen rect on
//! either backend), which is the documented, bounded cost of the styled path.
//!
//! ## Status
//!
//! Fully wired: [`PopupA11y::attach`] creates an `accesskit_unix::Adapter` seeded
//! with the popup's menu tree, and [`PopupA11y::focus_row`] /
//! [`PopupA11y::set_focused`] push focus/activation updates. The adapter lazily
//! no-ops until a real AT-SPI bus + assistive technology (Orca) is listening —
//! `DEVICE-VERIFY(0.11.1)`: the live bus handshake + Orca announcement can only be
//! confirmed on a real Linux session. When the `a11y` feature is off the whole type
//! degrades to inert no-ops so the popups run without accessibility rather than not
//! at all.

use crate::menu::Menu;

/// The AT-SPI action handler for a popup. AccessKit action requests (e.g. a screen
/// reader's "click"/"focus") arrive here on the adapter's own thread.
///
/// DEVICE-VERIFY(0.11.1): AT-driven *activation* would need to hop the request back
/// onto the popup's event-loop thread to fire the row (the self-drawn loops are
/// single-threaded and own the dispatch closure). Focus/label/state announcement —
/// the bulk of screen-reader value — works through the pushed `TreeUpdate`s without
/// it, so the handler is a no-op for now.
#[cfg(feature = "a11y")]
struct PopupActions;

#[cfg(feature = "a11y")]
impl accesskit::ActionHandler for PopupActions {
    fn do_action(&mut self, _request: accesskit::ActionRequest) {}
}

/// Serves the initial AccessKit tree the moment an assistive technology attaches
/// (AccessKit is lazy — nothing is built until an AT asks).
#[cfg(feature = "a11y")]
struct PopupActivation {
    menu: Menu,
}

#[cfg(feature = "a11y")]
impl accesskit::ActivationHandler for PopupActivation {
    fn request_initial_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        let tree = crate::a11y::build_tree(&self.menu);
        Some(crate::a11y::accesskit::tree_update(&tree, None))
    }
}

/// Notified when the last AT detaches; nothing to tear down here.
#[cfg(feature = "a11y")]
struct PopupDeactivation;

#[cfg(feature = "a11y")]
impl accesskit::DeactivationHandler for PopupDeactivation {
    fn deactivate_accessibility(&mut self) {}
}

/// A handle to the AT-SPI adapter driving one self-drawn popup session. Owns the
/// `accesskit_unix::Adapter` (behind `a11y`); created when a styled popup opens and
/// dropped when it dismisses, mirroring the macOS/Windows adapters' lifetimes. When
/// the `a11y` feature is off this is a ZST whose methods are no-ops.
pub(super) struct PopupA11y {
    /// The live AT-SPI adapter. Present only under the `a11y` feature.
    #[cfg(feature = "a11y")]
    adapter: accesskit_unix::Adapter,
    /// The menu the popup renders, kept so focus updates can rebuild the tree.
    #[cfg(feature = "a11y")]
    menu: Menu,
    /// Keeps the type non-empty (and the seam stable) when `a11y` is off.
    #[cfg(not(feature = "a11y"))]
    _private: (),
}

impl PopupA11y {
    /// Attach an AT-SPI adapter to a freshly-opened styled popup and register its
    /// initial tree (menu items as `Role::MenuItem`, the container as `Role::Menu`).
    /// Returns `None` when the `a11y` feature is off, so callers run unconditionally.
    /// The adapter itself lazily no-ops until an AT is listening.
    #[cfg(feature = "a11y")]
    pub(super) fn attach(menu: &Menu) -> Option<Self> {
        let adapter = accesskit_unix::Adapter::new(
            PopupActivation { menu: menu.clone() },
            PopupActions,
            PopupDeactivation,
        );
        Some(PopupA11y {
            adapter,
            menu: menu.clone(),
        })
    }

    /// Inert when the `a11y` feature is off: the popups run without accessibility.
    #[cfg(not(feature = "a11y"))]
    pub(super) fn attach(menu: &Menu) -> Option<Self> {
        let _ = menu;
        None
    }

    /// Report a hover / keyboard-focus change to the AT so Orca announces the row.
    /// `top` is the focused top-level [`Menu::items`] index (submenu-nested focus is
    /// resolved to its parent row — the per-window one-level model, spec 30 §3.2).
    pub(super) fn focus_row(&mut self, top: Option<usize>) {
        #[cfg(feature = "a11y")]
        {
            let menu = self.menu.clone();
            self.adapter.update_if_active(move || {
                let tree = crate::a11y::build_tree(&menu);
                let focus = crate::keynav::MenuFocus {
                    top,
                    flyout: Vec::new(),
                };
                let fid = crate::a11y::focused_id(&tree, &focus);
                crate::a11y::accesskit::tree_update(&tree, fid)
            });
        }
        #[cfg(not(feature = "a11y"))]
        {
            let _ = top;
        }
    }

    /// Report that the popup surface gained/lost keyboard focus (AccessKit needs
    /// this explicitly on Unix — research doc §13 caveat 3).
    pub(super) fn set_focused(&mut self, focused: bool) {
        #[cfg(feature = "a11y")]
        {
            self.adapter.update_window_focus_state(focused);
        }
        #[cfg(not(feature = "a11y"))]
        {
            let _ = focused;
        }
    }
}
