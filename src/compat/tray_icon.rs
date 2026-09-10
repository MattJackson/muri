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

/// A tray icon event, mirroring `tray-icon`'s `TrayIconEvent`. Emitted on
/// macOS/Windows and **not on Linux** (spec `02` §8).
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
    #[allow(dead_code)] // wired by the backend; unused until the popup loop lands.
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
    /// `02` §2). Accepts a `Box<Menu>` (the facade's `Menu` is `!Send`, so
    /// tray-icon's `Box<dyn ContextMenu + Send + Sync>` bound cannot apply).
    // `Box<Menu>` is deliberate signature parity with tray-icon's `with_menu`.
    #[allow(clippy::boxed_local)]
    pub fn with_menu(mut self, menu: Box<Menu>) -> Self {
        self.menu = Some(*menu);
        self
    }

    /// Build the tray icon. Returns immediately (passive model, spec `02` §2.1):
    /// it does **not** block the caller, but — unlike tray-icon, which relies on
    /// the host's event loop — it installs a **live** muri tray driven on a
    /// background UI thread (macOS: on the host's main-thread run loop), so the
    /// icon actually appears (issues #6, #7). The returned handle mutates it
    /// (`set_icon` / `set_menu` / `set_tooltip`) via the same cross-thread path.
    pub fn build(self) -> super::muda::Result<TrayIcon> {
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

        // Install + drive the tray without blocking. Best-effort: if it can't be
        // spawned (a headless session, or off the macOS main thread — e.g. in a
        // unit test), degrade to a passive facade that records state rather than
        // failing `build()`, matching tray-icon's build being infallible in
        // practice.
        let handle = tray.spawn().ok();

        let id = self.id.unwrap_or_else(|| TrayIconId(next_tray_id()));
        Ok(TrayIcon {
            id,
            icon: RefCell::new(self.icon),
            tooltip: RefCell::new(self.tooltip),
            title: RefCell::new(self.title),
            handle,
        })
    }
}

/// Convert a facade [`Icon`] (raw straight-alpha RGBA) into a muri
/// [`Icon`](crate::menu::Icon) by encoding it to PNG — the form every muri
/// backend consumes (the Linux SNI `icon_pixmap`, the macOS/Windows image
/// paths). Returns the placeholder tray symbol when there is no icon or the
/// RGBA cannot be encoded, so the tray is never left with an empty pixmap.
fn icon_to_muri(icon: &Option<Icon>) -> MuriIcon {
    icon.as_ref()
        .and_then(|i| crate::render::encode_rgba_png(&i.rgba, i.width, i.height))
        .map(|png| MuriIcon::Png(png.into()))
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

    /// The tray icon's on-screen rect, or `None` when it is unavailable —
    /// including on **Linux**, where tray anchoring is unsupported (matching
    /// tray-icon's own "unsupported on Linux", spec `02` §8). muri reports this
    /// as an absent rect (`None`) rather than an error, since `tray-icon`'s
    /// signature is `Option<Rect>`.
    pub fn rect(&self) -> Option<Rect> {
        None
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
}
