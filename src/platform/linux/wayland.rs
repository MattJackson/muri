//! Wayland `wlr-layer-shell` styled-popup presenter — **SCAFFOLD ONLY** (0.11.0).
//!
//! This is the third [`LinuxMenuPresenter`](super::LinuxMenuPresenter) impl: the
//! self-drawn, pixel-identical styled popup on the compositors that expose
//! `zwlr_layer_shell_v1` — every wlroots compositor (sway, Hyprland, river,
//! Wayfire, labwc, cosmic-comp) **and** KWin/Plasma. GNOME/Mutter refuses
//! layer-shell as an architectural stance (research doc §6, mutter#973), so it
//! stays on the native dbusmenu presenter — see [`super::detect_linux_presenter`].
//!
//! ## Why this module carries no code yet
//!
//! The real implementation needs the Wayland client stack
//! (`smithay-client-toolkit` + `wayland-client`), which is **Linux-only** and
//! does not build on the macOS host this crate is currently gated against (the
//! `cargo build --all-features` gate must stay green on macOS). So the whole
//! module is gated behind BOTH `all(unix, not(target_os = "macos"))` (via its
//! parent [`super`]) **and** the off-by-default `wayland-styled` cargo feature,
//! and every function that would touch a live Wayland connection has a
//! `todo!("DEVICE-VERIFY: …")` body describing the exact device-side step. The
//! signatures + lifecycle are fixed here so the seam is real; the bytes are added
//! on a Linux session (ADR-0003, first device task).
//!
//! ## The recipe this module will implement (research doc §3, §12)
//!
//! 1. Bind `wl_compositor`, `wl_shm`, `wl_seat`, and `zwlr_layer_shell_v1` from
//!    the registry. If `zwlr_layer_shell_v1` is absent → not our path (the caller
//!    already fell back to the dbusmenu presenter via detection).
//! 2. Create a **full-output overlay** layer surface: anchor to all four edges,
//!    `exclusive_zone(-1)`, `KeyboardInteractivity::OnDemand`, an empty input
//!    region except under the menu. Because it covers the output, `wl_pointer`
//!    motion coordinates *are* output coordinates (the only pointer-position
//!    channel Wayland gives a client).
//! 3. `get_popup` a child `xdg_popup` positioned by an `xdg_positioner` with a
//!    1×1 anchor rect at the pointer (for `ContextMenu::open_at`) or at the
//!    SNI-reported coordinate (Plasma/Waybar tray-anchored). The positioner's
//!    constraint-adjustment gives on-screen flip/slide for free.
//! 4. Blit muri's premultiplied-RGBA [`RasterDrawer`](crate::render::RasterDrawer)
//!    framebuffer into a `wl_shm` `Argb8888` buffer. `Argb8888` is BGRA in memory
//!    (little-endian), premultiplied — so the per-pixel work is a single **R↔B
//!    swap**; alpha is already premultiplied (see [`blit_argb8888`]). Unlike the
//!    X11 path (`super::x11::encode_framebuffer`, which composites over an opaque
//!    background), layer-shell keeps true alpha so rounded corners / shadow
//!    survive — the compositor composites the ARGB surface.
//! 5. Drive pointer/keyboard from the `wl_seat` handlers, reusing the shared,
//!    display-server-agnostic `flyout` + `keynav` state machines exactly as the
//!    X11 backend does; only the transport (`wl_shm` attach/damage/commit) and
//!    positioning (layer-surface + xdg-popup) differ.

#![allow(dead_code)] // Scaffold: the seam is defined here; the bodies are the
                     // first device-side task (ADR-0003). Every item below is
                     // referenced by the real implementation added on Linux.

use crate::error::{Error, Result};
use crate::geometry::{Edge, LogicalPoint, LogicalRect};
use crate::menu::{Menu, MenuId};
use crate::theme::MenuOptions;

/// Whether the current Wayland compositor advertises `zwlr_layer_shell_v1` — the
/// authoritative test for whether the styled layer-shell presenter is usable
/// (research doc §10, detection step 2). A live answer must bind the registry on
/// a real `wl_display`, which cannot be done on the macOS gate host, so the body
/// is deferred; [`super::detect_linux_presenter`] uses a cheap env heuristic
/// ([`super::wayland_env_suggests_layer_shell`]) to select the presenter and this
/// function is the device-side confirmation before a surface is actually created.
///
/// DEVICE-VERIFY(0.11.0): connect to `$WAYLAND_DISPLAY`, `wl_registry.global`
/// enumerate, return whether `zwlr_layer_shell_v1` appears. Add
/// `wayland-client`/`smithay-client-toolkit` first (see ADR-0003).
pub(super) fn layer_shell_available() -> bool {
    todo!(
        "DEVICE-VERIFY: add wayland-client + smithay-client-toolkit, connect to \
         $WAYLAND_DISPLAY, and return whether the registry advertises \
         zwlr_layer_shell_v1"
    )
}

/// Open a styled, self-drawn popup on a layer-shell compositor and block until it
/// dismisses — the Wayland analogue of [`super::x11::open_popup_session`].
///
/// `anchor` is the pointer (or SNI-reported) point the popup grows from relative
/// to `edge`; `on_click` receives the activated row's id. Reuses the shared
/// `render_menu` layout + `flyout`/`keynav` machinery; only transport +
/// positioning are Wayland-specific.
///
/// DEVICE-VERIFY(0.11.0): the full lifecycle — bind globals, create the
/// full-output overlay layer surface, `get_popup` at `anchor`, blit via
/// [`blit_argb8888`], run the seat event loop, tear down on dismiss.
pub(super) fn open_popup_session(
    menu: Menu,
    options: MenuOptions,
    on_click: &(dyn Fn(&MenuId) + '_),
    anchor: LogicalRect,
    edge: Edge,
    dark: bool,
) -> Result<()> {
    let _ = (menu, options, on_click, anchor, edge, dark);
    todo!(
        "DEVICE-VERIFY: layer-shell overlay + xdg_popup lifecycle over \
         smithay-client-toolkit (research doc §3, §12)"
    )
}

/// The global pointer position via the full-output overlay surface's own
/// `wl_pointer` motion — the *only* pointer channel a Wayland client has
/// (research doc §5). Meaningful only while an overlay surface is mapped and the
/// pointer is over it; `None` otherwise. The tray-anchored case instead uses the
/// out-of-band SNI `ContextMenu`/`Activate` coordinate (Plasma/Waybar).
///
/// DEVICE-VERIFY(0.11.0): map the overlay, cache the last `wl_pointer` motion
/// coordinate, return it here.
pub(super) fn cursor_position() -> Option<LogicalPoint> {
    None
}

/// Blit muri's premultiplied-RGBA framebuffer into a `wl_shm` `Argb8888`
/// (BGRA-in-memory, premultiplied) destination buffer: a per-pixel R↔B swap,
/// alpha copied through unchanged (already premultiplied). Pure byte-shuffling —
/// no Wayland handle — so it is implemented **now** and unit-testable on the host
/// once the transport lands; only the destination `canvas` (a mapped `wl_shm`
/// slot) is device-side.
///
/// `src` is muri's framebuffer (`RasterDrawer::framebuffer().pixels()`, RGBA
/// premultiplied); `dst` is the mapped `wl_shm` `Argb8888` canvas. Both are
/// `4 * width * height` bytes.
pub(super) fn blit_argb8888(src: &[u8], dst: &mut [u8]) -> Result<()> {
    if src.len() != dst.len() {
        return Err(Error::Platform(format!(
            "wl_shm blit size mismatch: src {} bytes, dst {} bytes",
            src.len(),
            dst.len()
        )));
    }
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        d[0] = s[2]; // B
        d[1] = s[1]; // G
        d[2] = s[0]; // R
        d[3] = s[3]; // A (already premultiplied — no change)
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::blit_argb8888;

    /// The one piece of Wayland glue that is real today: the RGBA→BGRA (Argb8888
    /// LE) channel swap with alpha passed through. Verifies the R↔B swap and the
    /// size-mismatch guard so the device-side transport can rely on it.
    #[test]
    fn blit_swaps_r_and_b_and_keeps_alpha() {
        // One pixel: R=10, G=20, B=30, A=40 (premultiplied) -> B,G,R,A.
        let src = [10u8, 20, 30, 40];
        let mut dst = [0u8; 4];
        blit_argb8888(&src, &mut dst).unwrap();
        assert_eq!(dst, [30, 20, 10, 40]);
    }

    #[test]
    fn blit_rejects_size_mismatch() {
        let src = [0u8; 8];
        let mut dst = [0u8; 4];
        assert!(blit_argb8888(&src, &mut dst).is_err());
    }
}
