//! Linux backend (M5) — the honest carve-out plus the recommended default path.
//!
//! The modern Linux tray is StatusNotifierItem / AppIndicator over D-Bus: the
//! *host* (GNOME Shell, KDE Plasma, an XEmbed shim) owns and draws the icon in
//! its own process and renders the menu from a `com.canonical.dbusmenu`
//! description. The application is never told the icon's on-screen rectangle and
//! never receives the click coordinate, and Wayland additionally forbids a client
//! from positioning its own toplevel. A tray-*anchored* styled popup is therefore
//! **architecturally impossible** here — a permanent, honest non-goal — so
//! [`LinuxPlatform::tray_anchor_rect`] returns
//! [`Unsupported::TrayAnchor`] and
//! [`LinuxPlatform::supports_tray_anchor`] returns `false` (spec 22 §1, §6).
//!
//! ## What this backend delivers (spec 22 §2, Path 1 — the recommended default)
//!
//! [`Platform::run_tray`] registers an SNI item (via the pure-Rust [`ksni`]
//! crate — no GTK/`libdbus` runtime dependency) and exports a native
//! `com.canonical.dbusmenu` menu **built from muri's own [`Menu`]** (see the
//! `sni` submodule). The host draws it on X11 and Wayland alike, and it is accessible over
//! AT-SPI **for free** because it is a real native menu — so Path 1 needs no
//! AccessKit bridge. Row activations dispatch through the muri tray's unified
//! handler. Runtime `TrayCommand`s from a
//! [`TrayHandle`](crate::TrayHandle) are drained on the run-loop thread and applied
//! via [`ksni::blocking::Handle::update`], which re-exports the SNI properties and
//! the dbusmenu tree — so `set_menu` rebuilds the menu, `set_icon`/`set_tooltip`
//! update the item.
//!
//! ## The styled pointer-anchored popup (spec 22 §2, Path 2)
//!
//! [`Platform::open_popup_session`] wires the styled `ContextMenu::open_at`
//! pointer path, degrading honestly per display server:
//!
//! - **X11 (incl. XWayland):** where an X server is reachable (`$DISPLAY` set),
//!   muri opens an **override-redirect** `_NET_WM_WINDOW_TYPE_POPUP_MENU` window at
//!   the caller's absolute pointer coordinate — X11 permits client positioning —
//!   paints the shared [`RasterDrawer`](crate::render::RasterDrawer) framebuffer via
//!   `PutImage`, grabs the pointer + keyboard, and runs a local hover / click /
//!   keyboard-nav loop with the full N-level flyout stack (decision #8). See the
//!   `x11` submodule.
//! - **Wayland (no X server):** returns
//!   [`Unsupported::ClientPositioning`]. This
//!   is the honest protocol limit, not a stub: a Wayland client cannot self-position
//!   a popup without a parent surface + input serial (spec 22 §3), and
//!   `open_popup_session` carries neither. The styled path on Wayland must go
//!   through the caller's own surface (a future `raw-window-handle` parameter, spec
//!   22 §2), so muri refuses rather than fabricating a mis-placed toplevel.
//!
//! The Linux styled surface is an **opaque** panel (spec 22 §6: the compositor owns
//! blur; there is no client-controllable vibrancy in any portable protocol).

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::error::{Error, Result, Unsupported};
use crate::geometry::{Edge, LogicalPoint, LogicalRect, LogicalSize};
use crate::menu::{Icon, Menu, MenuId};
use crate::platform::{Appearance, Platform};
use crate::theme::MenuOptions;
use crate::{Tray, TrayCommand};

mod a11y;
mod sni;
#[cfg(feature = "wayland-styled")]
mod wayland;
#[cfg(feature = "x11-popup")]
mod x11;

use ksni::blocking::{Handle, TrayMethods};
use sni::MuriSni;

/// Which presenter draws the tray/context menu on this Linux session — the
/// runtime seam between the three Linux menu-presentation strategies (deliverable
/// #1, research doc §10). Selected once by [`detect_linux_presenter`] and carried
/// on the SNI adapter so a right-click / activate routes to the right surface.
///
/// - [`NativeDbusMenu`](LinuxMenuPresenter::NativeDbusMenu) — the SNI host draws a
///   `com.canonical.dbusmenu` tree ([`sni`]). Universal baseline: every SNI host
///   incl. GNOME, accessible for free over AT-SPI. Also the honest fallback
///   anywhere the custom path isn't available.
/// - [`X11Popup`](LinuxMenuPresenter::X11Popup) — muri's own override-redirect
///   styled popup at the pointer ([`x11`]), on a real X11 session.
/// - [`WaylandLayerShell`](LinuxMenuPresenter::WaylandLayerShell) — muri's own
///   `wlr-layer-shell` styled popup ([`wayland`]) on wlroots + KWin/Plasma.
///   **Scaffold only** in 0.11.0 (see the `wayland` module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// `WaylandLayerShell` is only *constructed* under the off-by-default
// `wayland-styled` feature; keep the variant present (and matched) unconditionally
// so the seam type is stable, and silence the never-constructed lint on the
// default Linux build.
#[allow(dead_code)]
pub(crate) enum LinuxMenuPresenter {
    /// Native `com.canonical.dbusmenu`, host-drawn (GNOME + universal fallback).
    NativeDbusMenu,
    /// muri's own X11 override-redirect styled popup at the pointer.
    X11Popup,
    /// muri's own `wlr-layer-shell` styled popup (wlroots + KWin). Scaffold.
    WaylandLayerShell,
}

/// Pick the Linux menu presenter for the current session from the environment
/// (deliverable #2, research doc §10 "Runtime detection strategy"):
///
/// 1. `$WAYLAND_DISPLAY` set → **Wayland**. If the desktop is GNOME/Mutter (which
///    refuses layer-shell — research doc §6) → [`NativeDbusMenu`]. Otherwise, when
///    the `wayland-styled` scaffold is compiled *and* the env suggests a
///    layer-shell compositor, prefer [`WaylandLayerShell`] (the live
///    `zwlr_layer_shell_v1` registry bind, [`wayland::layer_shell_available`], is
///    the device-side confirmation before a surface is actually created). Without
///    the feature (today's default) Wayland always falls back to [`NativeDbusMenu`].
/// 2. Else `$DISPLAY` set → real **X11** → [`X11Popup`] when the `x11-popup`
///    feature is compiled, else [`NativeDbusMenu`].
/// 3. Else (no display env: headless/CI) → [`NativeDbusMenu`].
///
/// This is the cheap, non-blocking, never-panicking env logic; it never opens a
/// Wayland/X connection itself, so it is safe to call on the SNI service thread.
///
/// [`NativeDbusMenu`]: LinuxMenuPresenter::NativeDbusMenu
/// [`X11Popup`]: LinuxMenuPresenter::X11Popup
/// [`WaylandLayerShell`]: LinuxMenuPresenter::WaylandLayerShell
pub(crate) fn detect_linux_presenter() -> LinuxMenuPresenter {
    if wayland_display_present() {
        if desktop_is_gnome() {
            // Mutter refuses layer-shell as policy (research doc §6): the styled
            // path is impossible for a client, so use the native dbusmenu.
            return LinuxMenuPresenter::NativeDbusMenu;
        }
        #[cfg(feature = "wayland-styled")]
        {
            if wayland_env_suggests_layer_shell() {
                return LinuxMenuPresenter::WaylandLayerShell;
            }
        }
        // No layer-shell scaffold compiled (default), or the env doesn't look like
        // a layer-shell compositor: the honest fallback is the native dbusmenu.
        return LinuxMenuPresenter::NativeDbusMenu;
    }

    // No Wayland display: a real X server (incl. a pure-X11 session) allows the
    // custom override-redirect popup where the `x11-popup` path is compiled.
    #[cfg(feature = "x11-popup")]
    {
        if x11::is_available() {
            return LinuxMenuPresenter::X11Popup;
        }
    }
    LinuxMenuPresenter::NativeDbusMenu
}

/// Whether `$WAYLAND_DISPLAY` names a live Wayland session.
fn wayland_display_present() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Whether the current desktop is GNOME/Mutter, via `$XDG_CURRENT_DESKTOP` /
/// `$XDG_SESSION_DESKTOP` containing "GNOME" (case-insensitive). GNOME-Wayland is
/// forced onto the native dbusmenu presenter (research doc §6).
fn desktop_is_gnome() -> bool {
    [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok())
    .any(|v| v.to_ascii_lowercase().contains("gnome"))
}

/// Cheap env heuristic for "this Wayland session is a layer-shell compositor"
/// (wlroots family or KWin/Plasma), used to *pre-select*
/// [`LinuxMenuPresenter::WaylandLayerShell`] before the authoritative live
/// registry bind ([`wayland::layer_shell_available`]) runs device-side. GNOME is
/// excluded by the caller before this is consulted.
///
/// Recognizes the common `$XDG_CURRENT_DESKTOP` / `$XDG_SESSION_DESKTOP` values
/// for the layer-shell world (sway, Hyprland, river, Wayfire, labwc, cosmic, KDE).
#[cfg(feature = "wayland-styled")]
fn wayland_env_suggests_layer_shell() -> bool {
    const LAYER_SHELL_DESKTOPS: &[&str] = &[
        "sway", "hyprland", "river", "wayfire", "labwc", "cosmic", "kde", "plasma", "wlroots",
    ];
    [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok())
    .any(|v| {
        let v = v.to_ascii_lowercase();
        LAYER_SHELL_DESKTOPS.iter().any(|d| v.contains(d))
    })
}

/// The Linux SNI/AppIndicator anchor. It registers an icon over D-Bus (via
/// [`ksni`]) but, by design, cannot report an anchor rectangle: SNI hosts never
/// expose the icon's geometry (spec 22 §1).
#[derive(Default)]
#[non_exhaustive]
pub struct LinuxAnchor;

impl LinuxAnchor {
    /// Create the Linux anchor.
    pub fn new() -> Self {
        LinuxAnchor
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        Err(Error::Unsupported(Unsupported::TrayAnchor))
    }

    fn supports_tray_anchor(&self) -> bool {
        false
    }
}

impl std::fmt::Debug for LinuxAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinuxAnchor")
    }
}

/// The Linux [`Platform`] implementation: the honest tray-anchor carve-out plus
/// the SNI/`dbusmenu` native-menu fallback (spec 22 §2, Path 1). Every Linux
/// dependency (`ksni`, `zbus`) lives behind this module and the [`Platform`]
/// trait — no D-Bus or SNI type crosses the seam.
#[derive(Default)]
pub struct LinuxPlatform {
    anchor: LinuxAnchor,
    /// A live SNI service registered by [`Platform::install_tray`], kept alive as
    /// long as the platform handle lives. The managed [`Platform::run_tray`] path
    /// spawns its own service instead and does not use this.
    ///
    /// Pinned to the `MENU_ON_ACTIVATE = true` (native-dbusmenu) variant of
    /// [`MuriSni`]: `install_tray` registers a bare icon with an empty menu for
    /// consumers running their own loop, so there is no styled popup to route an
    /// `Activate` into — the host-drawn baseline is the right behavior here (the
    /// styled-presenter split lives on the managed [`run_sni_loop`] path).
    service: Option<Handle<MuriSni<true>>>,
}

impl std::fmt::Debug for LinuxPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxPlatform")
            .field("anchor", &self.anchor)
            .field("installed", &self.service.is_some())
            .finish()
    }
}

impl LinuxPlatform {
    /// Create the Linux platform.
    pub fn new() -> Self {
        LinuxPlatform {
            anchor: LinuxAnchor::new(),
            service: None,
        }
    }
}

impl Drop for LinuxPlatform {
    /// Tear down a standalone [`Platform::install_tray`] SNI service on the
    /// platform's final drop. `ksni`'s blocking `Handle` has no unregistering
    /// `Drop` of its own (the same reason the re-install path in `install_tray`
    /// shuts the prior handle down explicitly, #35), so without this the D-Bus
    /// service + its thread + the tray icon would leak until process exit — this
    /// mirrors `MacosAnchor`/`WindowsAnchor` tearing their OS registration down on
    /// `Drop`. The managed `run_tray` path owns its own service and never
    /// populates `self.service`, so this only fires for the standalone API.
    fn drop(&mut self) {
        if let Some(service) = self.service.take() {
            service.shutdown().wait();
        }
    }
}

impl Platform for LinuxPlatform {
    fn install_tray(&mut self, icon: &Icon, tooltip: Option<&str>) -> Result<()> {
        // Scan installed fonts in the background so the first menu open is instant.
        crate::render::prewarm_system_fonts();
        // Register a bare SNI item (icon + tooltip, empty menu). The full menu
        // and click handler only exist on the owning `Tray`, so the rich menu is
        // delivered through `run_tray`; this method is for consumers that manage
        // their own loop and just want the icon visible.
        let mut tray = Tray::new(icon.clone());
        if let Some(tip) = tooltip {
            tray = tray.tooltip(tip);
        }
        let handle = MuriSni::<true> {
            tray,
            visible: true,
            presenter: detect_linux_presenter(),
        }
        .spawn()
        .map_err(|e| {
            Error::TrayInstall(format!("SNI/StatusNotifierItem registration failed: {e}"))
        })?;
        // Re-install: shut the prior ksni service down before replacing it (#35).
        // ksni's blocking `Handle` has no `Drop` that unregisters, so simply
        // overwriting `self.service` would leak the old D-Bus service + thread and
        // leave a stale, unremovable tray icon. The new handle spawned first, so a
        // failed re-install leaves the existing icon intact.
        if let Some(old) = self.service.take() {
            old.shutdown().wait();
        }
        self.service = Some(handle);
        Ok(())
    }

    fn tray_anchor_rect(&self) -> Result<LogicalRect> {
        self.anchor.anchor_rect()
    }

    fn supports_tray_anchor(&self) -> bool {
        self.anchor.supports_tray_anchor()
    }

    fn cursor_position(&self) -> Option<LogicalPoint> {
        // X11 (incl. XWayland) can report the global pointer; a pure-Wayland
        // session cannot (no client pointer query) — `None` there is honest. Also
        // `None` in a tray-only build that compiled out the X11 path (#1).
        #[cfg(feature = "x11-popup")]
        {
            if x11::is_available() {
                return x11::cursor_position();
            }
        }
        // A pure-Wayland session has no client pointer query (research doc §5): the
        // styled layer-shell popup discovers the pointer within its own overlay
        // surface instead, so this is honestly `None` here.
        #[cfg(feature = "wayland-styled")]
        {
            return wayland::cursor_position();
        }
        #[allow(unreachable_code)]
        None
    }

    fn appearance(&self) -> Appearance {
        system_appearance()
    }

    fn system_menu_font(&self) -> Option<crate::platform::SystemFont> {
        system_menu_font()
    }

    fn work_area(&self) -> LogicalRect {
        // No portable, portal-free query for the work area exists on Linux; the
        // SNI host owns placement anyway, so a sensible default is enough here.
        // DEVICE-VERIFY(0.9.0): a real multi-monitor work area (would need the
        // caller's surface + output geometry, only available on the styled path).
        LogicalRect::new(LogicalPoint::new(0.0, 0.0), LogicalSize::new(1440.0, 900.0))
    }

    fn run_tray(self, tray: Tray) -> Result<()> {
        // Blocking path: a registration failure surfaces directly through the
        // return value, so the install handshake fires into a local channel we
        // don't consume (`_wait` stays alive for the duration of the call).
        let (report, _wait) = std::sync::mpsc::channel();
        run_sni_loop(tray, &report)
    }

    fn spawn_tray(self, tray: Tray) -> Result<()> {
        // `ksni` already runs its D-Bus service on its own thread; `run_sni_loop`
        // only parks draining `TrayHandle` commands, so hosting that drain on a
        // dedicated background thread is self-consistent and lets build() return
        // immediately. `spawn_tray_thread` blocks until `run_sni_loop` has fired
        // the install handshake, so a real SNI registration failure is surfaced
        // synchronously here rather than only `eprintln!`'d.
        super::spawn_tray_thread(tray, run_sni_loop)
    }

    /// Open the styled, pointer-anchored `ContextMenu::open_at` popup (spec 22 §2
    /// Path 2). On X11 (incl. XWayland) this draws muri's own surface at the
    /// pointer via an override-redirect window (the `x11` submodule); on a Wayland-only session
    /// it returns [`Unsupported::ClientPositioning`] — the honest protocol limit
    /// (spec 22 §3), since this API carries no parent surface + input serial.
    fn open_popup_session(
        &mut self,
        menu: Menu,
        options: MenuOptions,
        on_click: &(dyn Fn(&MenuId) + '_),
        anchor: LogicalRect,
        edge: Edge,
    ) -> Result<()> {
        #[cfg(feature = "x11-popup")]
        {
            if x11::is_available() {
                let dark = self.appearance().is_dark();
                x11::open_popup_session(menu, options, on_click, anchor, edge, dark)
            } else {
                Err(x11::wayland_unsupported())
            }
        }
        // Tray-only build (`default-features = false`, no `x11-popup`): the styled
        // pointer-anchored popup is compiled out, so report the same honest
        // protocol limit a Wayland-only session gets — the caller can still use the
        // SNI tray + native dbusmenu (Path 1).
        #[cfg(not(feature = "x11-popup"))]
        {
            let _ = (menu, options, on_click, anchor, edge);
            Err(Error::Unsupported(Unsupported::ClientPositioning))
        }
    }
}

/// Open muri's OWN styled popup at the current pointer for the
/// [`X11Popup`](LinuxMenuPresenter::X11Popup) presenter (deliverable #3): ignore
/// any host-supplied SNI coordinate (unreliable / often `0,0`) and use
/// [`x11::cursor_position`] (`XQueryPointer`) as the anchor, then route to the
/// shared [`x11::open_popup_session`]. The menu, options and click dispatch come
/// straight off the live [`Tray`], so the popup shows muri's exact styled rows and
/// activations flow through the tray's `on_click` — the very path
/// [`ContextMenu::open_at`](crate::ContextMenu::open_at) uses. Blocks the calling
/// (SNI service) thread until the popup dismisses.
///
/// DEVICE-VERIFY(0.11.0): the end-to-end SNI-click → cursor read → styled popup on
/// a real X11 session, including the ksni `ItemIsMenu`/`activate` interplay noted
/// on `MuriSni::activate` (ksni 0.3.6 only calls `activate` when `ItemIsMenu` is
/// false, and its `ContextMenu` right-click D-Bus method is a hard `UnknownMethod`
/// — so a fully host-agnostic right-click hook needs an upstream ksni capability
/// or a raw-`zbus` SNI impl; see ADR-0003).
#[cfg(feature = "x11-popup")]
pub(super) fn open_x11_popup_at_cursor(tray: &Tray) {
    let Some(point) = x11::cursor_position() else {
        return;
    };
    let anchor = LogicalRect::new(point, LogicalSize::new(0.0, 0.0));
    let dark = system_appearance().is_dark();
    let dispatch = |id: &MenuId| tray.dispatch(id);
    // Reuse the exact styled-popup backend `ContextMenu::open_at` funnels into (it
    // attaches the AT-SPI adapter itself — deliverable #5).
    //
    // The `Result` is deliberately discarded (#F10/F11): ksni's `activate(&mut
    // self, x, y)` callback returns nothing, so there is no channel back to the SNI
    // host or the app for a popup-open failure; the crate also has a no-`log` rule,
    // so there is no honest place to surface it. A failed open on this background
    // SNI service thread has no consumer-visible consequence beyond "no popup
    // appeared" — the swallow is intended, not an oversight.
    let _ = x11::open_popup_session(
        tray.menu.clone(),
        tray.options.clone(),
        &dispatch,
        anchor,
        Edge::Bottom,
        dark,
    );
}

/// Open muri's OWN `wlr-layer-shell` styled popup for the
/// [`WaylandLayerShell`](LinuxMenuPresenter::WaylandLayerShell) presenter
/// (deliverable #1): anchor at the SNI-reported `(x, y)` — the one real coordinate
/// channel on Wayland (research doc §5), meaningful on Plasma/Waybar and a
/// best-effort top-left fallback (0,0) elsewhere — and route to the shared
/// [`wayland::open_popup_session`], with the menu, options and click dispatch off the
/// live [`Tray`]. Blocks the calling (SNI service) thread until the popup dismisses.
///
/// DEVICE-VERIFY(0.11.1): end-to-end SNI-click → layer-shell popup on a real
/// wlroots/KWin session; the SNI coordinate quality is the host's (research doc §5).
#[cfg(feature = "wayland-styled")]
pub(super) fn open_wayland_popup_at(tray: &Tray, x: i32, y: i32) {
    // Authoritative confirmation (beyond the env heuristic used to pre-select the
    // presenter) that this compositor actually advertises `zwlr_layer_shell_v1`
    // before a surface is created (ADR-0003 §2). Refuse cleanly otherwise.
    if !wayland::layer_shell_available() {
        return;
    }
    let anchor = LogicalRect::new(
        LogicalPoint::new(x.max(0) as f32, y.max(0) as f32),
        LogicalSize::new(0.0, 0.0),
    );
    let dark = system_appearance().is_dark();
    let dispatch = |id: &MenuId| tray.dispatch(id);
    // Deliberately discard the `Result` (#F10/F11): as with the X11 path above,
    // ksni's `activate` callback is void — no return channel to the host or the app
    // — and the crate does not log, so a failed layer-shell open on this SNI service
    // thread has nowhere honest to be reported. The swallow is intentional.
    let _ = wayland::open_popup_session(
        tray.menu.clone(),
        tray.options.clone(),
        &dispatch,
        anchor,
        Edge::Bottom,
        dark,
    );
}

/// Register the SNI item + `dbusmenu` and run the command-drain loop to
/// completion (spec 22 §2, Path 1).
///
/// `ksni` runs the D-Bus service on its own background thread; this function
/// blocks the caller's thread draining [`TrayCommand`]s posted by any
/// [`TrayHandle`](crate::TrayHandle) and applying them via
/// [`Handle::update`], which re-exports the affected SNI properties / dbusmenu.
/// A condvar woken by the tray's installed waker keeps the drain responsive, with
/// a periodic timeout as a backstop so a missed wake never wedges the loop.
fn run_sni_loop(tray: Tray, report: &super::InstallReport) -> Result<()> {
    // Resolve the presenter — and therefore ksni's type-level `MENU_ON_ACTIVATE` /
    // `ItemIsMenu` — *before* the service is spawned (deliverable #2, ADR-0003 §3):
    // the two `MuriSni<const>` instantiations are distinct types, and
    // `spawn_tray_thread` handed us a non-generic `fn` pointer, so the const must be
    // fixed here, at the seam. The styled presenters (X11/Wayland) want
    // `MENU_ON_ACTIVATE = false` so the host forwards `Activate` to muri's own popup;
    // the native-dbusmenu baseline wants `true` so the host draws the exported menu.
    let presenter = detect_linux_presenter();
    if presenter_uses_custom_popup(presenter) {
        run_sni_loop_impl::<false>(tray, presenter, report)
    } else {
        run_sni_loop_impl::<true>(tray, presenter, report)
    }
}

/// Whether `presenter` draws muri's OWN styled popup (so ksni must expose the tray
/// as a plain activatable item — `ItemIsMenu = false` — and forward `Activate`),
/// rather than deferring to the host-drawn dbusmenu.
fn presenter_uses_custom_popup(presenter: LinuxMenuPresenter) -> bool {
    matches!(
        presenter,
        LinuxMenuPresenter::X11Popup | LinuxMenuPresenter::WaylandLayerShell
    )
}

/// The `MuriSni<M>`-monomorphized SNI service + command-drain loop. `M` is ksni's
/// `MENU_ON_ACTIVATE`, fixed by [`run_sni_loop`] from the resolved presenter.
fn run_sni_loop_impl<const M: bool>(
    tray: Tray,
    presenter: LinuxMenuPresenter,
    report: &super::InstallReport,
) -> Result<()> {
    // Keep the shared command queue + waker slot; the rest of the tray moves into
    // the SNI adapter.
    let commands = Arc::clone(&tray.commands);
    let waker_slot = Arc::clone(&tray.waker);

    // Install handshake (EH-1): a spawn failure is reported synchronously — the
    // success half of the handshake is deferred until *after* the waker is
    // installed (below) so a spawn-path caller (the native `Tray::spawn`) learns
    // the item never registered instead of seeing a false `Ok`. The blocking
    // `run_tray` path also propagates the error via the return value.
    let handle = match (MuriSni::<M> {
        tray,
        visible: true,
        presenter,
    }
    .spawn())
    {
        Ok(handle) => handle,
        Err(e) => {
            let err =
                Error::TrayInstall(format!("SNI/StatusNotifierItem registration failed: {e}"));
            let _ = report.send(Err(Error::TrayInstall(format!(
                "SNI/StatusNotifierItem registration failed: {e}"
            ))));
            return Err(err);
        }
    };

    // Wake primitive: the waker flips the flag + notifies; the loop parks on it.
    let wake: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(false), Condvar::new()));
    {
        let wake = Arc::clone(&wake);
        if let Ok(mut slot) = waker_slot.lock() {
            *slot = Some(Box::new(move || {
                let (lock, cvar) = &*wake;
                if let Ok(mut flag) = lock.lock() {
                    *flag = true;
                }
                cvar.notify_all();
            }));
        }
    }

    // Report success only NOW — after the waker closure is stored — so a command
    // posted by a TrayHandle obtained before this point still wakes the drain
    // (otherwise a post landing between the send and the waker install would sit
    // unserviced until the ~500ms backstop timeout) (#F2/F3).
    let _ = report.send(Ok(()));

    loop {
        // Apply everything posted so far (posts made before the loop came up are
        // buffered here already).
        let pending: Vec<TrayCommand> = commands
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        // Apply every non-Shutdown command in the batch FIRST, then tear down if a
        // Shutdown was present — so a `[Shutdown, SetIcon]` interleaving still
        // applies SetIcon before teardown instead of discarding it (#F1).
        let mut shutdown = false;
        for command in pending {
            if matches!(command, TrayCommand::Shutdown) {
                shutdown = true;
                continue;
            }
            apply_command(&handle, command);
        }
        if shutdown {
            // ksni's `Handle` has no `Drop` that unregisters — simply dropping it
            // leaves ksni's own service thread and the live SNI item running
            // forever. Explicitly shut the service down (closes the D-Bus
            // connection and ends that thread) and wait for it to complete, then
            // end this drain thread. Removes the tray item, matching tray-icon's
            // drop-removes contract.
            handle.shutdown().wait();
            return Ok(());
        }
        if handle.is_closed() {
            break;
        }

        // Park until woken by a handle post (or the backstop timeout), then
        // re-drain.
        let (lock, cvar) = &*wake;
        let Ok(mut flag) = lock.lock() else { break };
        while !*flag {
            // Recover the guard on poison rather than panicking the tray worker:
            // a waker thread that panicked while holding this mutex must not take
            // the host app's tray loop down with it (consistent with the graceful
            // poison handling at the `lock()` sites above).
            let (guard, timeout) = cvar
                .wait_timeout(flag, Duration::from_millis(500))
                .unwrap_or_else(|e| e.into_inner());
            flag = guard;
            if timeout.timed_out() {
                break;
            }
        }
        *flag = false;
    }
    Ok(())
}

/// Apply one [`TrayCommand`] to the live SNI item. Each
/// [`Handle::update`] mutates the adapter and triggers `ksni` to re-export the
/// changed SNI properties / dbusmenu tree.
fn apply_command<const M: bool>(handle: &Handle<MuriSni<M>>, command: TrayCommand) {
    match command {
        TrayCommand::SetMenu(menu) => {
            handle.update(move |s| s.tray.menu = menu);
        }
        TrayCommand::SetIcon(icon) => {
            handle.update(move |s| s.tray.icon = icon);
        }
        TrayCommand::SetTooltip(tooltip) => {
            handle.update(move |s| s.tray.tooltip = tooltip);
        }
        TrayCommand::SetTitle(title) => {
            // The SNI panel has no free-text menu-bar label (that is a macOS
            // concept); retain it on the tray, with no host-visible effect.
            handle.update(move |s| s.tray.title = title);
        }
        TrayCommand::SetVisible(visible) => {
            handle.update(move |s| s.visible = visible);
        }
        // The SNI host owns menu presentation; muri cannot force-open or close a
        // host-drawn menu from the app side (spec 22 §1). Honest no-op.
        TrayCommand::Open | TrayCommand::Close => {}
        // Intercepted in `run_sni_loop`'s drain before reaching here (it needs to
        // end the loop + call `Handle::shutdown`), so this arm is never taken.
        // The debug assertion catches a future second `apply_command` caller that
        // forgets to intercept Shutdown (which would silently swallow it and
        // regress the drop-removes contract); it stays a no-op in release.
        TrayCommand::Shutdown => debug_assert!(
            false,
            "TrayCommand::Shutdown must be intercepted by run_sni_loop's drain, \
             not routed through apply_command"
        ),
        // The persistent Linux tray is a native `dbusmenu` the SNI host renders,
        // so muri's theme/options don't apply to it — they govern only the styled
        // `ContextMenu::open_at` popups, which read `MenuOptions` per call. Honest
        // no-op on the tray; the payloads are consumed (and dropped) here (issue #45).
        TrayCommand::SetTheme(theme) => drop(theme),
        TrayCommand::SetOptions(options) => drop(options),
        // The SNI host never exposes the icon's geometry (spec 22 §1), so there is
        // no anchor rectangle to report — reply `None` (issue #48).
        TrayCommand::QueryAnchorRect(reply) => {
            let _ = reply.send(None);
        }
    }
}

/// Best-effort system light/dark appearance via the `org.freedesktop.appearance`
/// `color-scheme` desktop-portal setting (spec 22 §4). Falls back to
/// [`Appearance::Light`] whenever the portal, the session bus, or the value is
/// unavailable — a headless/CI environment always takes the fallback.
///
/// DEVICE-VERIFY(0.9.0): the live portal read against a real desktop session.
/// The GNOME UI font from gsettings `org.gnome.desktop.interface font-name`
/// (a Pango `"Family [Styles] Size"` spec, e.g. `"Cantarell 11"`), as a
/// [`SystemFont`](crate::platform::SystemFont) the renderer pins by family name
/// (fontdb already has the installed desktop font). Best-effort: `None` when
/// gsettings is absent/fails or the value can't be parsed (#10, #14).
fn system_menu_font() -> Option<crate::platform::SystemFont> {
    use crate::platform::{SystemFont, SystemFontSource};
    const STYLES: &[&str] = &[
        "Bold",
        "Italic",
        "Oblique",
        "Light",
        "Medium",
        "Regular",
        "Thin",
        "Black",
        "Semilight",
        "Semibold",
        "Heavy",
        "Condensed",
    ];
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "font-name"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let spec = raw.trim().trim_matches(['\'', '"']).trim();
    let mut parts: Vec<&str> = spec.split_whitespace().collect();
    let point_size = parts.last().and_then(|s| s.parse::<f32>().ok());
    if point_size.is_some() {
        parts.pop();
    }
    while parts
        .last()
        .is_some_and(|w| STYLES.iter().any(|s| s.eq_ignore_ascii_case(w)))
    {
        parts.pop();
    }
    let family = parts.join(" ");
    if family.is_empty() {
        return None;
    }
    Some(SystemFont {
        source: SystemFontSource::Family(family),
        point_size: point_size.unwrap_or(0.0),
    })
}

/// The GNOME accent color from gsettings `org.gnome.desktop.interface
/// accent-color` (GNOME 47+, a named accent), mapped to the libadwaita accent
/// RGB, or `None` if unavailable/unrecognized. Injected into `Color::Accent`
/// (#14). Best-effort.
///
/// Only the styled popup paths read the accent (the SNI tray defers all styling to
/// the host's dbusmenu renderer), so this is gated to the styled-popup features — a
/// tray-only build (`default-features = false`, no styled popup) compiles neither
/// the `x11`/`wayland` submodules nor this reader (#21).
#[cfg(any(feature = "x11-popup", feature = "wayland-styled"))]
pub(super) fn system_accent() -> Option<(u8, u8, u8, u8)> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "accent-color"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let (r, g, b) = match raw.trim().trim_matches(['\'', '"']).trim() {
        "blue" => (0x35, 0x84, 0xe4),
        "teal" => (0x21, 0x90, 0xa4),
        "green" => (0x3a, 0x94, 0x4a),
        "yellow" => (0xc8, 0x88, 0x00),
        "orange" => (0xed, 0x5b, 0x00),
        "red" => (0xe6, 0x2d, 0x42),
        "pink" => (0xd5, 0x61, 0x99),
        "purple" => (0x91, 0x41, 0xac),
        "slate" => (0x6f, 0x83, 0x96),
        _ => return None,
    };
    Some((r, g, b, 255))
}

fn system_appearance() -> Appearance {
    portal_color_scheme_is_dark()
        .map(Appearance::from_is_dark)
        .unwrap_or(Appearance::Light)
}

/// Query the desktop portal's `color-scheme` (0 = no preference, 1 = prefer dark,
/// 2 = prefer light). `Some(true)` means prefer-dark. Pure-Rust `zbus`, so no
/// system `libdbus` is needed.
fn portal_color_scheme_is_dark() -> Option<bool> {
    use zbus::blocking::Connection;
    use zbus::zvariant::Value;

    /// Unwrap the nested variant the portal returns down to its `u32`.
    fn scheme_is_dark(value: &Value) -> Option<bool> {
        match value {
            Value::U32(n) => Some(*n == 1),
            Value::Value(inner) => scheme_is_dark(inner),
            _ => None,
        }
    }

    let connection = Connection::session().ok()?;
    let reply = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "ReadOne",
            &("org.freedesktop.appearance", "color-scheme"),
        )
        .ok()?;
    let body = reply.body();
    let value: Value = body.deserialize().ok()?;
    scheme_is_dark(&value)
}
