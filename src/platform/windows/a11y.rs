//! Windows accessibility bridge built on `accesskit_windows` (spec 21 §6 / spec
//! 30 §3 Option B: one platform adapter per popup/flyout window, bridged to UI
//! Automation for NVDA + Narrator).
//!
//! `accesskit_windows::SubclassingAdapter` subclasses a popup's `HWND` to answer
//! `WM_GETOBJECT` from the already-built [`crate::a11y`] tree. Each panel owns its
//! own adapter and a small snapshot (the menu it renders + the current
//! selection); the run loop refreshes the snapshot and pushes a `TreeUpdate`
//! whenever focus, the open flyout, or the menu content changes — exactly as the
//! macOS backend does with `accesskit_macos`.
//!
//! The adapter's action handler never touches the shared `AppState`: it enqueues
//! a UIA action so Focus/Click requests are applied on the pump-thread drain like
//! every other input. Unlike `accesskit_macos` (which guarantees its action
//! handler runs on the main thread), `accesskit_windows` may invoke `do_action`
//! on a *foreign* UIA thread, so the enqueue path here is cross-thread-safe: it
//! pushes into a process-wide `Mutex` inbox and wakes the pump with a
//! `PostMessageW` (documented thread-safe) to the owner `HWND` stored in a
//! thread-safe static — never via the pump thread's thread-local inbox, which is
//! empty on a foreign thread (see [`super::push_a11y_action`]).

use std::cell::RefCell;
use std::rc::Rc;

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use accesskit_windows::{SubclassingAdapter, HWND as AkHwnd};
use windows_sys::Win32::Foundation::HWND;

use super::WindowKind;
use crate::keynav::MenuFocus;
use crate::menu::Menu;

/// The per-panel accessibility snapshot: the menu this panel renders and the
/// selection within it. Shared with the adapter's activation handler so it can
/// serve the initial tree without reaching into `AppState`.
pub(super) struct A11ySnapshot {
    pub menu: Menu,
    pub focus: MenuFocus,
}

/// Build an AccessKit `TreeUpdate` from a snapshot (root menu + selection).
pub(super) fn build_update(snap: &A11ySnapshot) -> TreeUpdate {
    let mut tree = crate::a11y::build_tree(&snap.menu);
    // Mark every open flyout level on the stack expanded, descending one nested
    // submenu per frame (decision #8, N-level submenus) so a 2+-deep open submenu
    // is reported expanded — not just the first level.
    let mut path = Vec::with_capacity(snap.focus.flyout.len());
    for frame in &snap.focus.flyout {
        path.push(frame.parent);
        crate::a11y::set_expanded(&mut tree, &path, true);
    }
    let focus = crate::a11y::focused_id(&tree, &snap.focus);
    crate::a11y::accesskit::tree_update(&tree, focus)
}

struct Activation {
    snapshot: Rc<RefCell<A11ySnapshot>>,
}

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(build_update(&self.snapshot.borrow()))
    }
}

struct Actions {
    kind: WindowKind,
}

impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        // `accesskit_windows` 0.24.1 may call this on a foreign UIA thread where
        // the pump thread's thread-local `EVENTS`/`OWNER_HWND` are empty, so we
        // must NOT use `push_event`. Instead enqueue into the cross-thread inbox
        // and wake the pump via a `PostMessageW` to the thread-safe owner `HWND`.
        super::push_a11y_action(self.kind, request);
    }
}

/// Attach a UIA adapter to a popup/flyout window by subclassing its `HWND`. Must
/// be called before the window is shown for the first time.
///
/// # Safety
/// `hwnd` must be a live window owned by the calling thread.
pub(super) unsafe fn make_adapter(
    hwnd: HWND,
    snapshot: Rc<RefCell<A11ySnapshot>>,
    kind: WindowKind,
) -> SubclassingAdapter {
    // `accesskit_windows` names its own (windows-crate) `HWND` newtype; both are
    // `*mut c_void`, so the pointer crosses directly and never leaves this module.
    SubclassingAdapter::new(AkHwnd(hwnd), Activation { snapshot }, Actions { kind })
}

/// Refresh a panel's snapshot and push the resulting tree if the adapter is
/// active (a no-op when no assistive technology is listening).
///
/// The snapshot is refreshed — and `menu` invoked to produce the (cloned) menu —
/// *only* when the adapter is active, since [`SubclassingAdapter::update_if_active`]
/// skips the factory entirely otherwise. This keeps the hot hover/keynav path free
/// of the per-frame `Menu` clone when no assistive technology is attached; the
/// snapshot stays correct whenever an AT is listening (the adapter is active, so
/// the factory runs on every sync).
pub(super) fn sync(
    adapter: &mut SubclassingAdapter,
    snapshot: &Rc<RefCell<A11ySnapshot>>,
    menu: impl FnOnce() -> Menu,
    focus: MenuFocus,
) {
    let snap = Rc::clone(snapshot);
    if let Some(events) = adapter.update_if_active(move || {
        {
            let mut s = snap.borrow_mut();
            s.menu = menu();
            s.focus = focus;
        }
        build_update(&snap.borrow())
    }) {
        events.raise();
    }
}
