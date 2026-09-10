//! The process-global [`MenuEvent`] channel — muri's single, unified event
//! source (spec `03` §3).
//!
//! muri emits its **own** activation events for every surface it draws (the
//! custom tray/context menus *and* any native menu bar it installs), so a
//! migrated muda app's single `MenuEvent::receiver()` loop sees every activation
//! with no cross-channel bridge. This module owns that global channel and the
//! optional `set_event_handler` escape hatch.
//!
//! The [`MenuEvent`] struct itself lives in [`crate::menu`] (it is the same type
//! the native `.on_click` path already carries); this module adds the
//! process-global [`MenuEvent::receiver`] / [`MenuEvent::set_event_handler`]
//! projection on top of it. Keeping one `MenuEvent` type — rather than a second,
//! facade-only copy — is what lets a native consumer and a muda-compat consumer
//! observe the *same* value. The channel and its `receiver()` API are part of the
//! **native** crate surface (always compiled, no crate feature required); the
//! muda-compat facade only re-exports them.

use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::menu::{MenuEvent, MenuId};

/// The receive half of the process-global [`MenuEvent`] channel. Cloneable and
/// readable from any thread (the same `crossbeam-channel` receiver muda hands
/// out, for behavioral parity).
pub type MenuEventReceiver = Receiver<MenuEvent>;

/// The optional global handler installed via [`MenuEvent::set_event_handler`].
///
/// Stored behind an [`Arc`] (not a bare `Box`) so [`emit`] can clone the handler
/// out and **drop the slot lock before invoking it**. A handler that re-enters
/// [`MenuEvent::set_event_handler`] from inside its own body (e.g. a one-shot
/// handler that unregisters itself) would otherwise deadlock against the same
/// mutex.
type EventHandler = Arc<dyn Fn(MenuEvent) + Send + Sync + 'static>;

struct GlobalChannel {
    sender: Sender<MenuEvent>,
    receiver: MenuEventReceiver,
}

fn channel() -> &'static GlobalChannel {
    static CHANNEL: OnceLock<GlobalChannel> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = unbounded();
        GlobalChannel { sender, receiver }
    })
}

fn handler_slot() -> &'static Mutex<Option<EventHandler>> {
    static HANDLER: OnceLock<Mutex<Option<EventHandler>>> = OnceLock::new();
    HANDLER.get_or_init(|| Mutex::new(None))
}

impl MenuEvent {
    /// The process-global receiver for menu activations, created lazily on first
    /// use. A migrated muda app polls this exactly as it polled
    /// `muda::MenuEvent::receiver()`.
    pub fn receiver() -> &'static MenuEventReceiver {
        &channel().receiver
    }

    /// Install (or clear, with `None`) a global handler invoked for every
    /// activation. This mirrors muda's escape hatch for apps that forward events
    /// into their own loop (tao/tauri) rather than polling the receiver. The
    /// handler runs *after* the event has been placed on the channel.
    ///
    /// The signature is generic over the closure type (`Option<F>`), exactly like
    /// `muda::MenuEvent::set_event_handler`, so a migrated app's bare-closure call
    /// — `set_event_handler(Some(|event| ...))` — compiles unchanged. Clear with a
    /// type-annotated `None`, e.g. `set_event_handler(None::<fn(MenuEvent)>)`.
    pub fn set_event_handler<F>(handler: Option<F>)
    where
        F: Fn(MenuEvent) + Send + Sync + 'static,
    {
        // Wrap into the `Arc` the slot holds so `emit` can clone-and-drop the lock
        // before dispatching.
        let handler: Option<EventHandler> = handler.map(|f| Arc::new(f) as EventHandler);
        if let Ok(mut slot) = handler_slot().lock() {
            *slot = handler;
        }
    }
}

/// Project a row activation onto the global channel and the optional global
/// handler. Called by the surface `dispatch` methods **after** the per-surface
/// `on_click` closure has run (spec `03` §3: closure first, then channel). Inert
/// [`MenuId::none`](crate::MenuId::none) ids are filtered out by the callers, so
/// this is only ever reached with an addressable id.
pub(crate) fn emit(id: MenuId, source: crate::SurfaceId) {
    let event = MenuEvent { id, source };
    // Channel first (the muda-compat door), then the optional forwarding handler.
    let _ = channel().sender.send(event.clone());
    // Clone the `Arc` handler out **while holding the lock**, then release the
    // lock before invoking it. Holding the lock across the user callback would
    // self-deadlock any handler that re-enters `set_event_handler` (e.g. a
    // one-shot handler that clears itself).
    let handler = handler_slot().lock().ok().and_then(|slot| slot.clone());
    if let Some(handler) = handler {
        handler(event);
    }
}

/// Serializes tests that touch the process-global `MenuEvent` channel. The
/// channel is shared process-wide, so parallel tests would otherwise steal each
/// other's events (a drain pops *every* pending message, not just its own). Every
/// test that emits to or drains the channel — here and in `lib.rs` — takes this
/// lock first. Poisoning is ignored so one failing test can't cascade.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain the global receiver, returning only ids that start with `prefix` (so
    /// tests emitting on the same global channel in parallel don't interfere).
    fn drain_prefixed(prefix: &str) -> Vec<String> {
        let rx = MenuEvent::receiver();
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if ev.id.0.starts_with(prefix) {
                out.push(ev.id.0);
            }
        }
        out
    }

    #[test]
    fn emit_places_event_on_the_global_channel() {
        let _guard = super::test_lock();
        let _ = drain_prefixed("event_emit_");
        emit(MenuId::from("event_emit_alpha"), crate::SurfaceId::next());
        emit(MenuId::from("event_emit_beta"), crate::SurfaceId::next());
        let seen = drain_prefixed("event_emit_");
        assert!(seen.contains(&"event_emit_alpha".to_string()));
        assert!(seen.contains(&"event_emit_beta".to_string()));
    }

    #[test]
    fn emit_carries_the_source_surface_id() {
        // Per-surface correlation (#51): the event on the global channel carries
        // the SurfaceId of the surface that dispatched it.
        let _guard = super::test_lock();
        let rx = MenuEvent::receiver();
        while rx.try_recv().is_ok() {}
        let source = crate::SurfaceId::next();
        emit(MenuId::from("event_source_probe"), source);
        let mut found = None;
        while let Ok(ev) = rx.try_recv() {
            if ev.id.0 == "event_source_probe" {
                found = Some(ev.source);
            }
        }
        assert_eq!(found, Some(source));
    }

    #[test]
    fn set_event_handler_receives_emitted_events() {
        let _guard = super::test_lock();
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = Arc::clone(&hits);
        // Bare closure (no `Box::new`) — the muda-parity drop-in call form.
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            if ev.id.0 == "event_handler_probe" {
                hits2.fetch_add(1, Ordering::SeqCst);
            }
        }));

        emit(
            MenuId::from("event_handler_probe"),
            crate::SurfaceId::next(),
        );
        // Clear before asserting so a failure can't leave a dangling global.
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let _ = drain_prefixed("event_handler_probe");
    }

    /// A handler that re-enters `set_event_handler` from inside its own body — a
    /// one-shot handler that unregisters itself — must run to completion, not
    /// deadlock. Before the `Arc`-clone-then-drop-lock fix this hung forever
    /// because `emit` held the slot lock across the callback and the callback's
    /// `set_event_handler(None)` tried to re-acquire the same lock.
    #[test]
    fn reentrant_set_event_handler_from_handler_does_not_deadlock() {
        let _guard = super::test_lock();
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = Arc::clone(&hits);
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            if ev.id.0 == "event_reentrant_probe" {
                hits2.fetch_add(1, Ordering::SeqCst);
                // Re-enter the same mutex from inside the callback.
                MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
            }
        }));

        // If the lock were held across the callback this call would never return.
        emit(
            MenuId::from("event_reentrant_probe"),
            crate::SurfaceId::next(),
        );

        // The handler cleared itself; make sure it ran exactly once.
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // Belt-and-suspenders cleanup so a failure can't leak the global.
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        let _ = drain_prefixed("event_reentrant_probe");
    }
}
