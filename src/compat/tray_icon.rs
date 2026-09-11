//! The `muri::compat::tray_icon` facade — the `tray-icon` crate's surface,
//! subsumed by muri's [`Tray`](crate::Tray) (spec `02` §8).
//!
//! `TrayIconBuilder::build()` returns immediately with a live handle (as
//! tray-icon's does); it does **not** block the caller (spec `02` §2.1). Unlike
//! tray-icon — which registers the icon and leans on the host's own event loop —
//! muri installs a **live** tray driven on a background UI thread (macOS: on the
//! host's main-thread run loop) via [`Tray::spawn`](crate::Tray::spawn), so the
//! icon actually appears (issues #6, #7). The important behavior change: raw
//! tray-icon's click opens the OS's native menu, whereas muri's opens the
//! **custom-drawn** popup — the point of migrating.
//!
//! ### Divergences (spec `02` §8)
//!
//! - `TrayIconEvent` is emitted on macOS/Windows and **not on Linux** (matching
//!   tray-icon's own contract — the SNI host never delivers the click). The
//!   facade never fabricates Linux events.
//! - The tray icon is carried as raw RGBA in the facade [`Icon`]. It is encoded
//!   to PNG and handed to the muri [`Tray`](crate::Tray) as
//!   [`Icon::Png`](crate::menu::Icon::Png) at build time (and on
//!   [`TrayIcon::set_icon`]), so the pixels reach the drawn tray. (Previously the
//!   facade left the tray on a placeholder symbol and the RGBA never reached it —
//!   a facade-specific gap, distinct from the spec's `NativeIcon`/`Icon::Symbol`
//!   divergence D6, which is about menu-item glyphs.) On Linux this populates the
//!   SNI `icon_pixmap` GNOME's appindicator extension needs (issue #6); the
//!   placeholder symbol is used only when no icon is configured.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::menu::Icon as MuriIcon;
use crate::{MenuOptions, ThemeSource};

// Re-export the shared icon/error types so a `tray_icon::Icon` import resolves.
pub use super::muda::{BadIcon, Icon, Menu};

/// Mirrors `tray_icon::menu` — real `tray-icon` does `pub use muda as menu`, and
/// that is the canonical path apps use to reach the menu-item types (e.g.
/// `tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem,
/// Submenu}`). Re-exporting the facade's muda module here so those
/// `tray_icon::menu::*` paths migrate with a pure `use tray_icon` → `use
/// muri::compat::tray_icon` swap (issue #5).
pub use super::muda as menu;

/// A tray icon identifier, mirroring `tray-icon`'s `TrayIconId`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrayIconId(pub String);

impl<T: Into<String>> From<T> for TrayIconId {
    fn from(value: T) -> Self {
        TrayIconId(value.into())
    }
}

/// A physical (device-pixel) position, mirroring `tray-icon`'s use of
/// `dpi::PhysicalPosition`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalPosition {
    /// The x coordinate in physical pixels.
    pub x: f64,
    /// The y coordinate in physical pixels.
    pub y: f64,
}

/// A rectangle in physical pixels (the tray icon's on-screen rect).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// The rect's top-left position.
    pub position: PhysicalPosition,
    /// The rect's size, `(width, height)` in physical pixels.
    pub size: (f64, f64),
}

/// Convert the native, DPI-independent [`LogicalRect`](crate::LogicalRect) into
/// the facade's physical-pixel [`Rect`] (issue #48). muri reports anchor
/// geometry in logical points; the facade widens to `f64` to match
/// `tray-icon`'s `PhysicalPosition`/`Rect` field types. No DPI scale is applied
/// (muri does not track a scale factor at this layer), so on a HiDPI display
/// this is numerically the logical rect, not the true physical-pixel rect —
/// documented best-effort, matching the rest of this facade's geometry story.
impl From<crate::LogicalRect> for Rect {
    fn from(r: crate::LogicalRect) -> Self {
        Rect {
            position: PhysicalPosition {
                x: r.origin.x as f64,
                y: r.origin.y as f64,
            },
            size: (r.size.width as f64, r.size.height as f64),
        }
    }
}

/// A mouse button, mirroring `tray-icon`'s `MouseButton`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// The left button.
    Left,
    /// The right button.
    Right,
    /// The middle button.
    Middle,
}

/// A mouse button state, mirroring `tray-icon`'s `MouseButtonState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButtonState {
    /// The button was released.
    Up,
    /// The button was pressed.
    Down,
}

/// A tray icon event, mirroring `tray-icon`'s `TrayIconEvent`.
///
/// **Not yet emitted by the backend.** The type, the process-global
/// [`receiver`](TrayIconEvent::receiver), and [`set_event_handler`] exist for
/// source/API parity with `tray-icon`, but muri's tray backends do not yet
/// surface icon-level pointer events (click/enter/leave/move) into this channel —
/// so a handler installed here currently never fires. Row *activations* inside
/// the popup are delivered through the muda [`MenuEvent`](crate::MenuEvent)
/// channel instead. When wired, events will (per spec `02` §8) fire on
/// macOS/Windows and **not on Linux** (the SNI host never reports icon clicks).
/// Tracked as a follow-up; do not rely on this channel for click handling yet.
///
/// [`set_event_handler`]: TrayIconEvent::set_event_handler
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TrayIconEvent {
    /// A click on the tray icon.
    Click {
        /// The tray icon's id.
        id: TrayIconId,
        /// The pointer position.
        position: PhysicalPosition,
        /// The icon's on-screen rect.
        rect: Rect,
        /// The button clicked.
        button: MouseButton,
        /// The button state.
        button_state: MouseButtonState,
    },
    /// A double-click on the tray icon.
    DoubleClick {
        /// The tray icon's id.
        id: TrayIconId,
        /// The pointer position.
        position: PhysicalPosition,
        /// The icon's on-screen rect.
        rect: Rect,
        /// The button double-clicked.
        button: MouseButton,
    },
    /// The pointer entered the tray icon.
    Enter {
        /// The tray icon's id.
        id: TrayIconId,
        /// The pointer position.
        position: PhysicalPosition,
        /// The icon's on-screen rect.
        rect: Rect,
    },
    /// The pointer moved over the tray icon.
    Move {
        /// The tray icon's id.
        id: TrayIconId,
        /// The pointer position.
        position: PhysicalPosition,
        /// The icon's on-screen rect.
        rect: Rect,
    },
    /// The pointer left the tray icon.
    Leave {
        /// The tray icon's id.
        id: TrayIconId,
        /// The pointer position.
        position: PhysicalPosition,
        /// The icon's on-screen rect.
        rect: Rect,
    },
}

/// The receive half of the process-global [`TrayIconEvent`] channel.
pub type TrayIconEventReceiver = Receiver<TrayIconEvent>;

/// The optional global tray handler. Stored behind an [`Arc`] (not a bare
/// `Box`) so [`TrayIconEvent::emit`] can clone it out and drop the slot lock
/// before invoking it — a handler that re-enters
/// [`TrayIconEvent::set_event_handler`] from its own body would otherwise
/// deadlock against the same mutex.
type TrayEventHandler = Arc<dyn Fn(TrayIconEvent) + Send + Sync + 'static>;

struct TrayChannel {
    sender: Sender<TrayIconEvent>,
    receiver: TrayIconEventReceiver,
}

fn tray_channel() -> &'static TrayChannel {
    static CHANNEL: OnceLock<TrayChannel> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = unbounded();
        TrayChannel { sender, receiver }
    })
}

fn tray_handler_slot() -> &'static Mutex<Option<TrayEventHandler>> {
    static HANDLER: OnceLock<Mutex<Option<TrayEventHandler>>> = OnceLock::new();
    HANDLER.get_or_init(|| Mutex::new(None))
}

impl TrayIconEvent {
    /// The process-global receiver for tray-icon events, mirroring
    /// `tray_icon::TrayIconEvent::receiver()`.
    pub fn receiver() -> &'static TrayIconEventReceiver {
        &tray_channel().receiver
    }

    /// Install (or clear, with `None`) a global handler invoked for every
    /// tray-icon event.
    ///
    /// Generic over the closure type (`Option<F>`), matching
    /// `tray_icon::TrayIconEvent::set_event_handler`, so a migrated app's
    /// bare-closure call compiles unchanged. Clear with a type-annotated `None`,
    /// e.g. `set_event_handler(None::<fn(TrayIconEvent)>)`.
    pub fn set_event_handler<F>(handler: Option<F>)
    where
        F: Fn(TrayIconEvent) + Send + Sync + 'static,
    {
        // Wrap into the `Arc` the slot holds so `emit` can clone-and-drop the lock
        // before dispatching.
        let handler: Option<TrayEventHandler> = handler.map(|f| Arc::new(f) as TrayEventHandler);
        if let Ok(mut slot) = tray_handler_slot().lock() {
            *slot = handler;
        }
    }

    /// Project a tray-icon event onto the global channel + handler. Called by the
    /// platform backend on macOS/Windows (never on Linux).
    // Not yet called by any backend — the tray backends don't surface icon-level
    // pointer events into this channel yet (see the type-level doc). Kept + tested
    // so the channel plumbing is ready to wire; `allow(dead_code)` until then.
    #[allow(dead_code)]
    pub(crate) fn emit(event: TrayIconEvent) {
        let _ = tray_channel().sender.send(event.clone());
        // Clone the `Arc` handler out under the lock, then release the lock
        // before invoking it, so a handler that re-enters `set_event_handler`
        // does not self-deadlock.
        let handler = tray_handler_slot()
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(handler) = handler {
            handler(event);
        }
    }
}

/// A builder for a [`TrayIcon`], mirroring `tray-icon`'s `TrayIconBuilder`.
#[derive(Default)]
pub struct TrayIconBuilder {
    id: Option<TrayIconId>,
    icon: Option<Icon>,
    tooltip: Option<String>,
    title: Option<String>,
    menu: Option<Menu>,
    /// **muri extension (issue #45):** popup options (theme, width bounds,
    /// gutter policy) threaded into the built [`Tray`](crate::Tray) instead of
    /// always leaving it on [`MenuOptions::default`]. Not muda/tray-icon's API.
    options: Option<MenuOptions>,
}

impl TrayIconBuilder {
    /// A new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the tray icon id.
    pub fn with_id(mut self, id: impl Into<TrayIconId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the tray icon image (raw RGBA; see the module note on icon encoding).
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the tooltip / accessible name.
    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Set the title shown beside the icon (macOS).
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Attach the menu. Routes the menu to a muri **custom** surface (spec
    /// `02` §2). Takes a concrete `Box<Menu>`, not a trait object like real
    /// tray-icon's `Box<dyn ContextMenu + Send + Sync>` (issue #53, item 1):
    /// the facade's `Menu` is `Rc`-shared (`!Send`), so it cannot satisfy that
    /// `Send + Sync` bound. Making `ContextMenu` object-safe *and* dropping
    /// `Send + Sync` here would ripple into every surface method's signature
    /// across both compat modules — a real trait-object refactor, not a
    /// same-file fix — so this facade instead documents the divergence and
    /// keeps the concrete type; there is exactly one facade `Menu` type, so
    /// nothing is actually lost by not being generic over it.
    // `Box<Menu>` is deliberate signature parity with tray-icon's `with_menu`.
    #[allow(clippy::boxed_local)]
    pub fn with_menu(mut self, menu: Box<Menu>) -> Self {
        self.menu = Some(*menu);
        self
    }

    /// **muri extension (issue #45):** set the popup options (theme, width
    /// bounds, gutter policy) the built [`Tray`](crate::Tray) opens with,
    /// instead of leaving it on [`MenuOptions::default`]. Not muda/tray-icon's
    /// API — the escape hatch for reaching muri-only tuning through the facade.
    pub fn with_options(mut self, options: MenuOptions) -> Self {
        self.options = Some(options);
        self
    }

    /// **muri extension (issue #45):** sugar for `with_options` that only sets
    /// the theme source, preserving any width/gutter settings already staged.
    /// Not muda/tray-icon's API.
    pub fn with_theme(mut self, theme: ThemeSource) -> Self {
        let options = self.options.take().unwrap_or_default();
        self.options = Some(options.theme(theme));
        self
    }

    /// Build the configured native [`crate::Tray`] — menu, icon, tooltip, title,
    /// and staged [`MenuOptions`] (#45) — **without** spawning it. Factored out of
    /// [`build`](Self::build) so the option/theme threading is observable in a
    /// test (via [`crate::Tray::menu_options`]) rather than only after the tray is
    /// spawned and consumed.
    fn configured_tray(&self) -> crate::Tray {
        let muri_menu = if let Some(menu) = &self.menu {
            // Route to a muri custom surface (marks the menu Custom).
            menu.build_custom_surface().menu().clone()
        } else {
            crate::Menu::new()
        };

        // Encode the facade's raw RGBA into muri's own `Icon::Png` so the pixels
        // actually reach the drawn tray — on Linux this populates the SNI
        // `icon_pixmap` GNOME's appindicator extension needs (#6), and on Windows
        // the `HICON` (#7). Falls back to the placeholder symbol only when no icon
        // was configured or the RGBA is malformed.
        let mut tray = crate::Tray::new(icon_to_muri(&self.icon)).menu(muri_menu);
        if let Some(tooltip) = &self.tooltip {
            tray = tray.tooltip(tooltip.clone());
        }
        // The macOS menu-bar text (e.g. usagio's live "45%"); drawn on the
        // NSStatusItem, retained-only on Windows/Linux (#8 part 3).
        if let Some(title) = &self.title {
            tray = tray.title(title.clone());
        }
        // muri extension (#45): thread staged options into the built tray
        // instead of always leaving it on `MenuOptions::default()`.
        if let Some(options) = self.options.clone() {
            tray = tray.options(options);
        }
        tray
    }

    /// Build the tray icon. Returns immediately (passive model, spec `02` §2.1):
    /// it does **not** block the caller, but — unlike tray-icon, which relies on
    /// the host's event loop — it installs a **live** muri tray driven on a
    /// background UI thread (macOS: on the host's main-thread run loop), so the
    /// icon actually appears (issues #6, #7). The returned handle mutates it
    /// (`set_icon` / `set_menu` / `set_tooltip`) via the same cross-thread path.
    ///
    /// **Infallible-degrade:** matching `tray-icon`'s practically-infallible
    /// `build()`, this returns `Ok` even when the underlying `Tray::spawn` fails —
    /// a headless/off-main-thread environment, but also a *real* failure such as an
    /// unavailable Linux session D-Bus. In that case the tray is not live: the
    /// returned `TrayIcon` records state but its post-construction setters are
    /// no-ops. A consumer that must detect install failure should drive the native
    /// [`crate::Tray::spawn`] directly (which surfaces the `Result`) instead of the
    /// facade.
    pub fn build(self) -> super::muda::Result<TrayIcon> {
        // Infallible-degrade for tray-icon parity: a spawn failure yields a
        // passive facade (`handle: None`) rather than an error. Use
        // [`build_result`](Self::build_result) or [`TrayIcon::is_live`] to detect
        // a real failure.
        let handle = self.spawn_handle().ok();
        Ok(self.into_tray_icon(handle))
    }

    /// Like [`build`](Self::build) but **surfaces** a real spawn failure instead
    /// of degrading to a passive facade — returns `Err` when the platform backend
    /// can't install the tray (issue: `build()` swallowed every error). Off the
    /// macOS main thread / in a headless session this returns `Err` too, so use
    /// [`build`](Self::build) for the tray-icon-parity infallible behavior and this
    /// when you must know the tray is live.
    pub fn build_result(self) -> super::muda::Result<TrayIcon> {
        let handle = self.spawn_handle()?;
        Ok(self.into_tray_icon(Some(handle)))
    }

    /// Spawn the configured tray, returning the live handle or the real error.
    fn spawn_handle(&self) -> super::muda::Result<crate::TrayHandle> {
        let tray = self.configured_tray();
        let marker = crate::MainThreadMarker::new().ok_or(super::muda::Error::MainThread)?;
        // Preserve the structured failure kind (EH-1/EH-2): on Windows/Linux the
        // install handshake makes `spawn` return `Error::TrayInstall` when the OS
        // tray never installed, so `build_result` surfaces that exact variant
        // instead of a stringly-typed `Platform` catch-all.
        tray.spawn(marker).map_err(super::muda::Error::from)
    }

    /// Assemble the facade `TrayIcon` around an (optional) live handle.
    fn into_tray_icon(self, handle: Option<crate::TrayHandle>) -> TrayIcon {
        let id = self.id.unwrap_or_else(|| TrayIconId(next_tray_id()));
        TrayIcon {
            id,
            icon: RefCell::new(self.icon),
            tooltip: RefCell::new(self.tooltip),
            title: RefCell::new(self.title),
            handle,
        }
    }
}

/// Convert a facade [`Icon`] (raw straight-alpha RGBA) into a muri
/// [`Icon`](crate::menu::Icon) by encoding it to PNG — the form every muri
/// backend consumes (the Linux SNI `icon_pixmap`, the macOS/Windows image
/// paths). Returns the placeholder tray symbol when there is no icon or the
/// RGBA cannot be encoded, so the tray is never left with an empty pixmap.
fn icon_to_muri(icon: &Option<Icon>) -> MuriIcon {
    icon.as_ref()
        .and_then(|i| super::encode_rgba_cached(&i.rgba, i.width, i.height))
        .map(MuriIcon::Png)
        .unwrap_or(MuriIcon::Symbol("tray"))
}

fn next_tray_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
}

/// A live tray icon, mirroring `tray-icon`'s `TrayIcon`. Drives a spawned muri
/// [`Tray`](crate::Tray) via a [`TrayHandle`](crate::TrayHandle); the
/// post-construction setters mirror tray-icon's API and post cross-thread to the
/// running tray. The `handle` is `None` only when the tray could not be spawned
/// (a headless session or off the macOS main thread); the facade then records
/// state without a live tray.
pub struct TrayIcon {
    id: TrayIconId,
    icon: RefCell<Option<Icon>>,
    tooltip: RefCell<Option<String>>,
    title: RefCell<Option<String>>,
    handle: Option<crate::TrayHandle>,
}

impl TrayIcon {
    /// This tray icon's id.
    pub fn id(&self) -> &TrayIconId {
        &self.id
    }

    /// Whether a live OS tray is backing this facade. `false` means
    /// [`TrayIconBuilder::build`] degraded (a headless/off-main-thread
    /// environment, or a real spawn failure): the icon isn't shown and the
    /// setters (`set_icon`/`set_tooltip`/…) are no-ops. Use
    /// [`TrayIconBuilder::build_result`] to get the underlying error instead.
    pub fn is_live(&self) -> bool {
        self.handle.is_some()
    }

    /// Replace the tray icon image.
    ///
    /// The new icon is recorded on the facade (previously the argument was
    /// silently discarded and `Ok(())` returned unconditionally — a no-op that
    /// falsely claimed success).
    ///
    /// The facade's raw RGBA is encoded to PNG and posted to the live muri
    /// [`Tray`](crate::Tray): the new pixels reach the drawn tray — on Linux the
    /// SNI `icon_pixmap` is re-registered, on Windows the `HICON` is replaced. A
    /// `None` icon (or RGBA that fails to encode)
    /// clears the facade record and reverts the tray to the placeholder symbol.
    pub fn set_icon(&self, icon: Option<Icon>) -> super::muda::Result<()> {
        if let Some(handle) = &self.handle {
            handle.set_icon(icon_to_muri(&icon));
        }
        *self.icon.borrow_mut() = icon;
        Ok(())
    }

    /// Replace the tooltip / accessible name (posted to the live tray).
    pub fn set_tooltip(&self, tooltip: Option<impl Into<String>>) -> super::muda::Result<()> {
        let tooltip = tooltip.map(Into::into);
        if let Some(handle) = &self.handle {
            handle.set_tooltip(tooltip.clone());
        }
        *self.tooltip.borrow_mut() = tooltip;
        Ok(())
    }

    /// Replace the status-item text title (the macOS menu-bar text). Posted to
    /// the live tray; a no-op on the drawn item on Windows/Linux.
    pub fn set_title(&self, title: Option<impl Into<String>>) {
        let title = title.map(Into::into);
        if let Some(handle) = &self.handle {
            handle.set_title(title.clone());
        }
        *self.title.borrow_mut() = title;
    }

    /// Show or hide the tray icon (posted to the live tray as an SNI/status
    /// change).
    pub fn set_visible(&self, visible: bool) -> super::muda::Result<()> {
        if let Some(handle) = &self.handle {
            handle.set_visible(visible);
        }
        Ok(())
    }

    /// Replace the attached menu. Routes to a muri custom surface and posts it to
    /// the live tray.
    pub fn set_menu(&self, menu: Option<Box<Menu>>) {
        let muri_menu = match &menu {
            Some(menu) => menu.build_custom_surface().menu().clone(),
            None => crate::Menu::new(),
        };
        if let Some(handle) = &self.handle {
            handle.set_menu(muri_menu);
        }
    }

    /// The tray icon's on-screen rect, or `None` when it is unavailable — no
    /// live tray was spawned, the query timed out, or (on **Linux**) tray
    /// anchoring is unsupported (matching tray-icon's own "unsupported on
    /// Linux", spec `02` §8). Wired to the native best-effort cross-thread
    /// accessor, [`TrayHandle::anchor_rect`](crate::TrayHandle::anchor_rect)
    /// (issue #48); muri reports absence as `None` rather than an error, since
    /// `tray-icon`'s own signature is `Option<Rect>`.
    pub fn rect(&self) -> Option<Rect> {
        self.handle
            .as_ref()
            .and_then(|h| h.anchor_rect())
            .map(Rect::from)
    }

    /// **muri extension (issue #45):** swap the live theme source on the
    /// running tray. The next popup open uses it, and any currently-open popup
    /// is repainted with it — the primitive an in-menu "Preview theme"
    /// switcher is built on. Not muda/tray-icon's API; a no-op if no live tray
    /// was spawned.
    pub fn set_theme(&self, theme: ThemeSource) {
        if let Some(handle) = &self.handle {
            handle.set_theme(theme);
        }
    }

    /// **muri extension (issue #45):** swap the live [`MenuOptions`] wholesale
    /// (theme + width bounds + gutter policy) on the running tray. Not
    /// muda/tray-icon's API; a no-op if no live tray was spawned.
    pub fn set_options(&self, options: MenuOptions) {
        if let Some(handle) = &self.handle {
            handle.set_options(options);
        }
    }
}

impl Drop for TrayIcon {
    /// Remove the OS tray icon and stop its backend when the facade handle is
    /// dropped — matching `tray-icon`'s `TrayIcon`, which deletes its icon on
    /// `Drop`. Without this, the spawned `muri-tray` thread and its live
    /// `Shell_NotifyIcon` / SNI item would leak until process exit. Best-effort
    /// and asynchronous (the removal runs on the backend's UI thread); a no-op
    /// when no live tray was spawned (headless / off the macOS main thread).
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::muda::{MenuItem, SurfaceMode};
    use super::*;

    #[test]
    fn menu_submodule_reexports_muda_menu_item_types() {
        // Real tray-icon does `pub use muda as menu`; apps reach menu-item types
        // via `tray_icon::menu::*`. The facade must resolve those same paths so a
        // pure import swap compiles (issue #5).
        use super::super::tray_icon::menu::{
            CheckMenuItem, Menu as MenuViaTrayIcon, MenuItem, PredefinedMenuItem, Submenu,
        };

        let menu = MenuViaTrayIcon::new();
        let item = MenuItem::with_id("open", "Open", true, None);
        let check = CheckMenuItem::with_id("toggle", "Toggle", true, true, None);
        let sep = PredefinedMenuItem::separator();
        let sub = Submenu::new("More", true);
        menu.append(&item).unwrap();
        menu.append(&check).unwrap();
        menu.append(&sep).unwrap();
        menu.append(&sub).unwrap();

        // `tray_icon::menu::Menu` is the same type as the directly imported `Menu`.
        let _same_type: MenuViaTrayIcon = Menu::new();
        assert_eq!(item.id().as_str(), "open");
    }

    #[test]
    fn with_menu_routes_to_a_custom_surface() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("open", "Open", true, None))
            .unwrap();
        // The facade Menu is Rc-shared, so a clone observes the mode change.
        let probe = menu.clone();

        let tray = TrayIconBuilder::new()
            .with_tooltip("MyApp")
            .with_menu(Box::new(menu))
            .build()
            .expect("tray builds without a loop");

        assert_eq!(
            probe.mode(),
            SurfaceMode::Custom,
            "TrayIconBuilder routes its menu to a muri custom surface"
        );
        assert!(tray.id().0.parse::<u32>().is_ok());
    }

    #[test]
    fn tray_event_channel_round_trips() {
        // Serialize on the shared global-event lock: the tray channel is
        // process-global, so without this a concurrent draining test (e.g. the
        // reentrant test below) could steal the event before `try_recv` sees it.
        let _guard = crate::event::test_lock();
        let rx = TrayIconEvent::receiver();
        // Clear any straggler before emitting our own.
        while rx.try_recv().is_ok() {}

        let ev = TrayIconEvent::Enter {
            id: TrayIconId("t1".into()),
            position: PhysicalPosition::default(),
            rect: Rect::default(),
        };
        TrayIconEvent::emit(ev);
        // The event is deliverable on the global receiver.
        let got = rx.try_recv().expect("an emitted tray event is received");
        assert!(matches!(got, TrayIconEvent::Enter { .. }));
    }

    /// A tray handler that re-enters `set_event_handler` from inside its own body
    /// must run to completion, not deadlock. Serialized on the shared global-event
    /// test lock so it doesn't race other handler-installing tests. Before the
    /// `Arc`-clone-then-drop-lock fix this hung: `emit` held the slot lock across
    /// the callback and the callback's `set_event_handler(None)` re-acquired it.
    #[test]
    fn tray_reentrant_set_event_handler_from_handler_does_not_deadlock() {
        let _guard = crate::event::test_lock();
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = Arc::clone(&hits);
        TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
            if let TrayIconEvent::Enter { id, .. } = &ev {
                if id.0 == "tray_reentrant_probe" {
                    hits2.fetch_add(1, Ordering::SeqCst);
                    // Re-enter the same mutex from inside the callback.
                    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
                }
            }
        }));

        // Would never return if the lock were held across the callback.
        TrayIconEvent::emit(TrayIconEvent::Enter {
            id: TrayIconId("tray_reentrant_probe".into()),
            position: PhysicalPosition::default(),
            rect: Rect::default(),
        });

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
        // Drain the channel so the emitted event doesn't leak to other tests.
        let rx = TrayIconEvent::receiver();
        while rx.try_recv().is_ok() {}
    }

    #[test]
    fn set_icon_records_the_new_icon_instead_of_discarding_it() {
        let tray = TrayIconBuilder::new()
            .build()
            .expect("tray builds without a loop");
        assert!(tray.icon.borrow().is_none(), "no icon configured at build");

        // A valid 1x1 RGBA icon.
        let icon = Icon::from_rgba(vec![10, 20, 30, 40], 1, 1).expect("valid RGBA");
        tray.set_icon(Some(icon)).expect("set_icon succeeds");

        // The facade now actually holds the requested icon (previously discarded).
        {
            let guard = tray.icon.borrow();
            let stored = guard.as_ref().expect("icon was recorded");
            assert_eq!(stored.width, 1);
            assert_eq!(stored.height, 1);
            assert_eq!(stored.rgba, vec![10, 20, 30, 40]);
        }

        // Clearing is honored too.
        tray.set_icon(None).expect("clearing succeeds");
        assert!(
            tray.icon.borrow().is_none(),
            "set_icon(None) clears the icon"
        );
    }

    #[test]
    fn icon_to_muri_encodes_rgba_as_a_decodable_png() {
        // A 2x2 opaque RGBA icon → muri `Icon::Png` whose bytes round-trip back
        // to the same pixels. This encoded icon feeds the Linux SNI
        // `icon_pixmap` (#6) and the Windows `HICON` (#7); before the fix the
        // facade left the tray on the placeholder Symbol and the item was dropped.
        let rgba = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 255, 255, // white
        ];
        let icon = Some(Icon::from_rgba(rgba.clone(), 2, 2).expect("valid RGBA"));

        match icon_to_muri(&icon) {
            MuriIcon::Png(bytes) => {
                let (decoded, w, h) =
                    crate::render::decode_png(&bytes).expect("encoded tray icon PNG decodes");
                assert_eq!((w, h), (2, 2));
                assert_eq!(
                    decoded, rgba,
                    "the pixels round-trip through the encoded icon"
                );
            }
            other => panic!("expected an encoded PNG icon, got {other:?}"),
        }
    }

    #[test]
    fn icon_to_muri_falls_back_to_the_placeholder_symbol() {
        // No configured icon → the placeholder symbol (never an empty pixmap that
        // a host would drop).
        assert!(matches!(icon_to_muri(&None), MuriIcon::Symbol("tray")));
    }

    #[test]
    fn title_is_recorded_from_the_builder_and_setter() {
        // The macOS menu-bar text (#8 part 3): with_title records it, set_title
        // replaces and clears it on the facade. (The live NSStatusItem render is
        // exercised on-device; here we assert the facade-side contract.)
        let tray = TrayIconBuilder::new()
            .with_title("45%")
            .build()
            .expect("tray builds");
        assert_eq!(tray.title.borrow().as_deref(), Some("45%"));

        tray.set_title(Some("12%"));
        assert_eq!(tray.title.borrow().as_deref(), Some("12%"));

        tray.set_title(None::<String>);
        assert!(tray.title.borrow().is_none());
    }

    #[test]
    fn facade_setters_and_drop_post_the_matching_commands_to_the_live_handle() {
        // The facade setters only reach a live tray through `self.handle`, but
        // build() spawns best-effort and leaves `handle: None` headlessly — so
        // the other facade tests never exercise that branch. Build a TrayIcon
        // around a known handle directly and assert each setter (and Drop) posts
        // the *right* TrayCommand. A swap (set_title -> SetTooltip, or Drop not
        // posting Shutdown) would fail here.
        use crate::TrayCommand;
        let native = crate::Tray::new(MuriIcon::Symbol("tray"));
        let handle = native.handle();
        let tray = TrayIcon {
            id: TrayIconId("t".into()),
            icon: RefCell::new(None),
            tooltip: RefCell::new(None),
            title: RefCell::new(None),
            handle: Some(handle.clone()),
        };

        tray.set_icon(Some(Icon::from_rgba(vec![1, 2, 3, 4], 1, 1).unwrap()))
            .unwrap();
        tray.set_title(Some("9%"));
        tray.set_tooltip(Some("tip")).unwrap();
        tray.set_menu(None);
        tray.set_visible(false).unwrap();
        drop(tray); // Drop must post Shutdown.

        let posted = handle.take_posted();
        assert!(matches!(&posted[0], TrayCommand::SetIcon(_)));
        assert!(matches!(&posted[1], TrayCommand::SetTitle(Some(s)) if s == "9%"));
        assert!(matches!(&posted[2], TrayCommand::SetTooltip(Some(s)) if s == "tip"));
        assert!(matches!(&posted[3], TrayCommand::SetMenu(_)));
        assert!(matches!(&posted[4], TrayCommand::SetVisible(false)));
        assert!(
            matches!(&posted[5], TrayCommand::Shutdown),
            "Drop must post Shutdown so the OS icon is removed; got {:?}",
            posted.get(5)
        );
    }

    #[test]
    fn with_options_and_with_theme_are_threaded_into_the_built_tray() {
        // #45: the builder's staged `MenuOptions` (and the `with_theme` sugar)
        // must actually reach the `Tray` `build()` constructs — not be left on
        // `MenuOptions::default()`. Observe the configured Tray's `menu_options()`
        // directly; this fails if `with_options`/`with_theme` were no-ops (the old
        // test only checked the tray had a numeric id, which is tautological).
        let tray = TrayIconBuilder::new()
            .with_options(MenuOptions::default().min_width(200.0).max_width(400.0))
            .configured_tray();
        let mo = tray.menu_options();
        assert_eq!(
            mo.min_width,
            Some(200.0),
            "with_options width must reach the tray"
        );
        assert_eq!(mo.max_width, Some(400.0));

        // `with_theme` sets `options.theme`, and applied after `with_options` must
        // not clobber the previously-staged width bound.
        let themed = TrayIconBuilder::new()
            .with_options(MenuOptions::default().min_width(50.0))
            .with_theme(ThemeSource::Windows(crate::ThemeMode::Dark))
            .configured_tray();
        let tm = themed.menu_options();
        assert_eq!(
            tm.min_width,
            Some(50.0),
            "with_theme must not clobber a prior width bound"
        );
        assert!(
            matches!(tm.theme, ThemeSource::Windows(_)),
            "with_theme must set options.theme, got {:?}",
            tm.theme
        );
    }

    #[test]
    fn rect_maps_the_native_anchor_rect_through_the_live_handle() {
        // #48: `rect()` must consult the live `TrayHandle::anchor_rect()`
        // rather than unconditionally returning `None`. There is no running
        // platform backend in a headless unit test to answer the query, so the
        // *query path* is exercised (it must not panic and must time out to
        // `None` rather than hang); the `From<LogicalRect>` conversion itself
        // is asserted directly below.
        let native = crate::Tray::new(MuriIcon::Symbol("tray"));
        let handle = native.handle();
        let tray = TrayIcon {
            id: TrayIconId("t".into()),
            icon: RefCell::new(None),
            tooltip: RefCell::new(None),
            title: RefCell::new(None),
            handle: Some(handle),
        };
        // No backend is installed to answer `QueryAnchorRect`, so this must
        // resolve to `None` (via the query's own timeout) rather than block.
        assert_eq!(tray.rect(), None);

        // Without a live handle at all, `rect()` is `None` unconditionally.
        let headless = TrayIcon {
            id: TrayIconId("h".into()),
            icon: RefCell::new(None),
            tooltip: RefCell::new(None),
            title: RefCell::new(None),
            handle: None,
        };
        assert_eq!(headless.rect(), None);
    }

    #[test]
    fn logical_rect_converts_into_the_facade_physical_rect() {
        // #48: the `From<crate::LogicalRect> for Rect` bridge used by `rect()`.
        use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
        let logical = LogicalRect::new(LogicalPoint::new(10.0, 20.0), LogicalSize::new(30.0, 40.0));
        let rect: Rect = logical.into();
        assert_eq!(rect.position, PhysicalPosition { x: 10.0, y: 20.0 });
        assert_eq!(rect.size, (30.0, 40.0));
    }

    #[test]
    fn build_and_build_result_agree_on_liveness() {
        // #3: `build()` degrades a spawn failure to a passive facade (infallible,
        // `is_live() == false`); `build_result()` surfaces it as `Err`. Both go
        // through the same spawn path, so they must agree: `build_result().is_ok()`
        // iff `build().is_live()`. Portable across environments (whether or not a
        // tray can actually spawn in the test harness).
        let live = TrayIconBuilder::new()
            .build()
            .expect("build() is infallible")
            .is_live();
        let ok = TrayIconBuilder::new().build_result().is_ok();
        assert_eq!(
            ok, live,
            "build_result() must succeed exactly when build() yields a live tray"
        );
    }

    #[test]
    fn build_result_reports_a_structured_error_when_the_tray_is_not_live() {
        // EH-1/EH-2: when the tray cannot install, `build()` must degrade to a
        // non-live facade without panicking, and `build_result()` must surface a
        // *structured* error kind (MainThread off the main thread; TrayInstall on a
        // real Windows/Linux install failure) rather than the old stringly
        // `Platform` catch-all — so a consumer can `match` on the failure.
        // DEVICE-VERIFY(0.10.8): the Windows/Linux TrayInstall path (a genuine
        // Shell_NotifyIcon(NIM_ADD)/SNI failure on a real session), exercised at
        // the seam by platform::handshake_tests here.
        let facade = TrayIconBuilder::new()
            .build()
            .expect("build() is infallible");
        if facade.is_live() {
            // A live tray means the environment could install it; then
            // build_result() must succeed and there is no error to classify.
            assert!(TrayIconBuilder::new().build_result().is_ok());
            return;
        }
        match TrayIconBuilder::new().build_result() {
            Ok(_) => panic!("build_result() must be Err when build() is not live"),
            Err(e) => assert!(
                matches!(
                    e,
                    super::super::muda::Error::MainThread
                        | super::super::muda::Error::TrayInstall(_)
                        | super::super::muda::Error::ThreadSpawn(_)
                        | super::super::muda::Error::Unsupported(_)
                ),
                "the non-live build_result error must be a structured kind, got {e:?}"
            ),
        }
    }

    #[test]
    fn set_theme_and_set_options_post_to_the_live_handle() {
        // #45: the runtime-swap escape hatch on the live `TrayIcon`.
        use crate::TrayCommand;
        let native = crate::Tray::new(MuriIcon::Symbol("tray"));
        let handle = native.handle();
        let tray = TrayIcon {
            id: TrayIconId("t".into()),
            icon: RefCell::new(None),
            tooltip: RefCell::new(None),
            title: RefCell::new(None),
            handle: Some(handle.clone()),
        };

        tray.set_theme(ThemeSource::System(crate::ThemeMode::Dark));
        tray.set_options(MenuOptions::default().min_width(10.0));

        let posted = handle.take_posted();
        assert!(matches!(&posted[0], TrayCommand::SetTheme(_)));
        assert!(matches!(&posted[1], TrayCommand::SetOptions(_)));
    }
}
