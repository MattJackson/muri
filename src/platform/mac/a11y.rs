//! macOS accessibility bridge built directly on `accesskit_macos` (spec 30 §3
//! Option B: one platform adapter per popup/flyout window).
//!
//! `accesskit_macos::SubclassingAdapter` dynamically subclasses a panel's
//! content [`NSView`](super::window::MuriView) to answer the NSAccessibility
//! protocol from the already-built [`crate::a11y`] tree. Each panel owns its own
//! adapter and a small snapshot (the menu it renders + the current selection);
//! the run loop refreshes the snapshot and pushes a `TreeUpdate` whenever focus,
//! the open flyout, or the menu content changes.
//!
//! The adapter's action handler never touches the shared `AppState`: it enqueues
//! a [`UiEvent::A11yAction`](super::UiEvent) so VoiceOver's Focus/Click requests
//! are applied on the main-thread drain like every other input.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use accesskit_macos::SubclassingAdapter;

use super::window::MuriView;
use super::{UiEvent, WindowKind};
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
    // submenu per frame (decision #8, N-level submenus).
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
        // Runs on the main thread; defer application to the single drain.
        super::push_event(UiEvent::A11yAction {
            kind: self.kind,
            request,
        });
    }
}

/// Attach a platform adapter to a panel's content view. Must be called before
/// the panel is shown/focused for the first time.
pub(super) fn make_adapter(
    view: &MuriView,
    snapshot: Rc<RefCell<A11ySnapshot>>,
    kind: WindowKind,
) -> SubclassingAdapter {
    let ptr = (view as *const MuriView) as *mut c_void;
    // SAFETY: `ptr` is a valid, retained `NSView` (a live `MuriView`); the
    // adapter retains it for its own lifetime.
    unsafe { SubclassingAdapter::new(ptr, Activation { snapshot }, Actions { kind }) }
}

/// Refresh a panel's snapshot and push the resulting tree if the adapter is
/// active (a no-op when no assistive technology is listening).
///
/// The snapshot is refreshed — and `menu` invoked to produce the (cloned) menu —
/// *only* when the adapter is active, since [`SubclassingAdapter::update_if_active`]
/// skips the factory entirely otherwise. This keeps the hot hover/keynav path
/// free of the per-frame `Menu` clone when no assistive technology is listening;
/// the snapshot stays correct whenever an AT is attached (the adapter is active,
/// so the factory runs on every sync).
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
