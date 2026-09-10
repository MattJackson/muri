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

mod sni;
mod x11;

use ksni::blocking::{Handle, TrayMethods};
use sni::MuriSni;

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
    service: Option<Handle<MuriSni>>,
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

impl Platform for LinuxPlatform {
    fn install_tray(&mut self, icon: &Icon, tooltip: Option<&str>) -> Result<()> {
        // Register a bare SNI item (icon + tooltip, empty menu). The full menu
        // and click handler only exist on the owning `Tray`, so the rich menu is
        // delivered through `run_tray`; this method is for consumers that manage
        // their own loop and just want the icon visible.
        let mut tray = Tray::new(icon.clone());
        if let Some(tip) = tooltip {
            tray = tray.tooltip(tip);
        }
        let handle = MuriSni {
            tray,
            visible: true,
        }
        .spawn()
        .map_err(|e| Error::Platform(format!("SNI/StatusNotifierItem registration failed: {e}")))?;
        self.service = Some(handle);
        Ok(())
    }

    fn tray_anchor_rect(&self) -> Result<LogicalRect> {
        self.anchor.anchor_rect()
    }

    fn supports_tray_anchor(&self) -> bool {
        self.anchor.supports_tray_anchor()
    }

    fn appearance(&self) -> Appearance {
        system_appearance()
    }

    fn work_area(&self) -> LogicalRect {
        // No portable, portal-free query for the work area exists on Linux; the
        // SNI host owns placement anyway, so a sensible default is enough here.
        // DEVICE-VERIFY(0.9.0): a real multi-monitor work area (would need the
        // caller's surface + output geometry, only available on the styled path).
        LogicalRect::new(LogicalPoint::new(0.0, 0.0), LogicalSize::new(1440.0, 900.0))
    }

    fn run_tray(self, tray: Tray) -> Result<()> {
        run_sni_loop(tray)
    }

    fn spawn_tray(self, tray: Tray) -> Result<()> {
        // `ksni` already runs its D-Bus service on its own thread; `run_sni_loop`
        // only parks draining `TrayHandle` commands, so hosting that drain on a
        // dedicated background thread is self-consistent and lets build() return
        // immediately.
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
        if x11::is_available() {
            let dark = self.appearance().is_dark();
            x11::open_popup_session(menu, options, on_click, anchor, edge, dark)
        } else {
            Err(x11::wayland_unsupported())
        }
    }
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
fn run_sni_loop(tray: Tray) -> Result<()> {
    // Keep the shared command queue + waker slot; the rest of the tray moves into
    // the SNI adapter.
    let commands = Arc::clone(&tray.commands);
    let waker_slot = Arc::clone(&tray.waker);

    let handle = MuriSni {
        tray,
        visible: true,
    }
    .spawn()
    .map_err(|e| Error::Platform(format!("SNI/StatusNotifierItem registration failed: {e}")))?;

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

    loop {
        // Apply everything posted so far (posts made before the loop came up are
        // buffered here already).
        let pending: Vec<TrayCommand> = commands
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        for command in pending {
            if matches!(command, TrayCommand::Shutdown) {
                // ksni's `Handle` has no `Drop` that unregisters — simply
                // dropping it leaves ksni's own service thread and the live SNI
                // item running forever. Explicitly shut the service down (closes
                // the D-Bus connection and ends that thread) and wait for it to
                // complete, then end this drain thread. Removes the tray item,
                // matching tray-icon's drop-removes contract.
                handle.shutdown().wait();
                return Ok(());
            }
            apply_command(&handle, command);
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
fn apply_command(handle: &Handle<MuriSni>, command: TrayCommand) {
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
    }
}

/// Best-effort system light/dark appearance via the `org.freedesktop.appearance`
/// `color-scheme` desktop-portal setting (spec 22 §4). Falls back to
/// [`Appearance::Light`] whenever the portal, the session bus, or the value is
/// unavailable — a headless/CI environment always takes the fallback.
///
/// DEVICE-VERIFY(0.9.0): the live portal read against a real desktop session.
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
