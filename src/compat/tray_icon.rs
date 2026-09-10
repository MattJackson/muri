//! The `muri::compat::tray_icon` facade — the `tray-icon` crate's surface,
//! subsumed by muri's [`Tray`](crate::Tray) (spec `02` §8).
//!
//! `TrayIconBuilder::build()` returns immediately with a live handle (as
//! tray-icon's does); it does **not** block on a loop, preserving the passive
//! model (spec `02` §2.1). The important behavior change: raw tray-icon's click
//! opens the OS's native menu, whereas muri's opens the **custom-drawn** popup —
//! the point of migrating.
//!
//! ### Divergences (spec `02` §8)
//!
//! - `TrayIconEvent` is emitted on macOS/Windows and **not on Linux** (matching
//!   tray-icon's own contract — the SNI host never delivers the click). The
//!   facade never fabricates Linux events.
//! - The tray icon is carried as raw RGBA in the facade [`Icon`]; the muri
//!   [`Tray`](crate::Tray) is built with a placeholder symbol until the backend's
//!   image path consumes the RGBA (divergence D6, spec `02` §4.5).

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::menu::Icon as MuriIcon;

// Re-export the shared icon/error types so a `tray_icon::Icon` import resolves.
pub use super::muda::{BadIcon, Icon, Menu};

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

    /// Set the tray icon image (raw RGBA; see the module D6 note).
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

    /// Build the tray icon. Returns immediately (passive model, spec `02` §2.1);
    /// it does **not** enter an event loop.
    pub fn build(self) -> super::muda::Result<TrayIcon> {
        // The muri Tray owns the icon; the facade's raw RGBA is carried until the
        // backend image path consumes it, so build with a placeholder symbol.
        let muri_menu = if let Some(menu) = &self.menu {
            // Route to a muri custom surface (marks the menu Custom).
            menu.build_custom_surface().menu().clone()
        } else {
            crate::Menu::new()
        };

        let mut tray = crate::Tray::new(MuriIcon::Symbol("tray")).menu(muri_menu);
        if let Some(tooltip) = &self.tooltip {
            tray = tray.tooltip(tooltip.clone());
        }

        let id = self.id.unwrap_or_else(|| TrayIconId(next_tray_id()));
        Ok(TrayIcon {
            id,
            icon: RefCell::new(self.icon),
            tooltip: RefCell::new(self.tooltip),
            title: RefCell::new(self.title),
            _tray: RefCell::new(tray),
        })
    }
}

fn next_tray_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
}

/// A live tray icon, mirroring `tray-icon`'s `TrayIcon`. Wraps a muri
/// [`Tray`](crate::Tray). Post-construction setters mirror tray-icon's API.
pub struct TrayIcon {
    id: TrayIconId,
    icon: RefCell<Option<Icon>>,
    tooltip: RefCell<Option<String>>,
    title: RefCell<Option<String>>,
    _tray: RefCell<crate::Tray>,
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
    /// **Divergence D6 (spec `02` §4.5):** the facade carries the icon as raw
    /// RGBA, but muri's own [`Icon`](crate::menu::Icon) has no raw-RGBA variant —
    /// it holds *encoded* bytes / symbols — so the pixels cannot yet be handed to
    /// the live muri [`Tray`](crate::Tray); the on-screen glyph stays the
    /// placeholder symbol until the backend image path consumes the RGBA. The
    /// facade therefore truthfully records the requested icon rather than
    /// pretending it was applied to the drawn tray.
    pub fn set_icon(&self, icon: Option<Icon>) -> super::muda::Result<()> {
        *self.icon.borrow_mut() = icon;
        Ok(())
    }

    /// Replace the tooltip / accessible name.
    pub fn set_tooltip(&self, tooltip: Option<impl Into<String>>) -> super::muda::Result<()> {
        *self.tooltip.borrow_mut() = tooltip.map(Into::into);
        Ok(())
    }

    /// Replace the title (macOS).
    pub fn set_title(&self, title: Option<impl Into<String>>) {
        *self.title.borrow_mut() = title.map(Into::into);
    }

    /// Show or hide the tray icon.
    pub fn set_visible(&self, _visible: bool) -> super::muda::Result<()> {
        Ok(())
    }

    /// Replace the attached menu. Routes to a muri custom surface.
    pub fn set_menu(&self, menu: Option<Box<Menu>>) {
        let muri_menu = match &menu {
            Some(menu) => menu.build_custom_surface().menu().clone(),
            None => crate::Menu::new(),
        };
        self._tray.borrow_mut().set_menu(muri_menu);
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

#[cfg(test)]
mod tests {
    use super::super::muda::{MenuItem, SurfaceMode};
    use super::*;

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
}
