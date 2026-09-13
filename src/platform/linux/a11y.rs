//! Accessibility bridge for the **self-drawn** Linux popups (X11 override-redirect
//! + Wayland layer-shell), wired to `accesskit_unix` behind the `a11y` feature.
//!
//! The native dbusmenu presenter ([`super::sni`]) is accessible over AT-SPI for
//! free; a self-drawn popup is just opaque pixels to the compositor, so muri
//! publishes its own semantic tree, as it already does on macOS/Windows.
//!
//! `accesskit_unix` implements AT-SPI2 over D-Bus (via `zbus`), not the display
//! protocol, so the adapter is identical under X11 and Wayland and reuses
//! [`crate::a11y::accesskit::tree_update`]. **Wayland caveat:**
//! `Adapter::set_root_window_bounds()` is X11-only, so absolute-screen AT
//! hit-testing isn't exact there — roles/text/focus/actions still work; muri
//! never calls it.
//!
//! Fully wired: [`PopupA11y::attach`]/`focus_row`/`set_focused` push tree and
//! focus updates; the adapter lazily no-ops until a real AT (Orca) is listening
//! (`DEVICE-VERIFY(0.11.1)`). With the `a11y` feature off the type degrades to
//! inert no-ops.

use crate::menu::Menu;

/// The AT-SPI action handler for a popup. AccessKit action requests (e.g. a screen
/// reader's "click"/"focus") arrive here on the adapter's own thread.
///
/// DEVICE-VERIFY(0.11.1): AT-driven *activation* would need to hop the request
/// back onto the popup's single-threaded event loop to fire the row. Focus/
/// label/state announcement — the bulk of screen-reader value — works through
/// the pushed `TreeUpdate`s without it, so the handler is a no-op for now.
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
