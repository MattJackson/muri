//! Native panel construction and the AppKit responder subclasses.
//!
//! Each popup/flyout is a borderless, non-activating [`NSPanel`] (spec 20 §2):
//! `Borderless | NonactivatingPanel` style, floating + `becomesKeyOnlyIfNeeded`
//! so it can take keyboard focus for menu navigation **without deactivating the
//! user's foreground app**, at the pop-up-menu window level so it floats above
//! everything. Its content view is an [`NSVisualEffectView`] backdrop (the OS
//! vibrancy blur) hosting a layer-backed [`MuriView`] that the raster pixmap is
//! blitted into (see [`super::present`]).
//!
//! The responder subclasses ([`MuriView`], [`MuriWindowDelegate`]) never touch
//! the shared [`super::AppState`] directly: every callback enqueues a
//! [`super::UiEvent`] and asks for a main-thread drain, so all mutation happens
//! in one place, never re-entrantly inside an AppKit event dispatch.

#![allow(non_snake_case)]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSCursor, NSPanel, NSPopUpMenuWindowLevel, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize};

use super::input::translate_ns_key;
use super::{UiEvent, WindowKind};

define_class!(
    // The layer-backed content view muri paints into and that receives mouse
    // and keyboard events for its panel. Its ivar records whether it belongs to
    // the popup or a flyout so callbacks can tag their events without consulting
    // shared state. Flipped so its coordinate origin is top-left, matching the
    // logical layout the renderer produced.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "MuriContentView"]
    #[ivars = WindowKind]
    pub(super) struct MuriView;

    impl MuriView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // The popup is a `NonactivatingPanel` with `becomesKeyOnlyIfNeeded(true)`,
        // so it becomes key only if its first responder says it's needed. Return
        // true so `makeKeyAndOrderFront` actually makes the panel key **without
        // activating the app** — which revives `windowDidResignKey`, the signal
        // that dismisses the menu when focus is lost to Spotlight, an in-app
        // search field, another app, or another (OEM) menu (#17). Flyouts use
        // `orderFrontRegardless` (they never take key), so opening a submenu does
        // not resign the popup — no self-dismiss race. DEVICE-VERIFY.
        #[unsafe(method(needsPanelToBecomeKey))]
        fn needs_panel_to_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&objc2_app_kit::NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &objc2_app_kit::NSEvent) {
            // Force the arrow cursor: a borderless popup otherwise inherits the
            // I-beam from whatever view last set it, so the pointer shows a text
            // caret over the menu. `resetCursorRects` covers the static case;
            // setting it here guarantees the arrow while the pointer is moving.
            NSCursor::arrowCursor().set();
            let (x, y) = view_point(self, event);
            super::push_event(UiEvent::MouseMoved {
                kind: *self.ivars(),
                x,
                y,
            });
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            // Establish the arrow cursor over the whole view so the menu never
            // shows the text I-beam.
            let bounds = self.bounds();
            self.addCursorRect_cursor(bounds, &NSCursor::arrowCursor());
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &objc2_app_kit::NSEvent) {
            let (x, y) = view_point(self, event);
            super::push_event(UiEvent::MouseMoved {
                kind: *self.ivars(),
                x,
                y,
            });
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &objc2_app_kit::NSEvent) {
            // The pointer left this panel; the drain decides (by global cursor
            // geometry) whether to collapse submenus + clear the highlight.
            super::push_event(UiEvent::MouseExited {
                kind: *self.ivars(),
            });
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            let (x, y) = view_point(self, event);
            super::push_event(UiEvent::MouseDown {
                kind: *self.ivars(),
                x,
                y,
            });
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &objc2_app_kit::NSEvent) {
            if let Some(key) = translate_ns_key(event) {
                super::push_event(UiEvent::Key(key));
            }
        }
    }
);

impl MuriView {
    fn new(mtm: MainThreadMarker, frame: NSRect, kind: WindowKind) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(kind);
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    // The panel's delegate: forwards key-window transitions into the focus set
    // that drives dismissal. Its ivar records which window it serves.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MuriWindowDelegate"]
    #[ivars = WindowKind]
    pub(super) struct MuriWindowDelegate;

    unsafe impl NSObjectProtocol for MuriWindowDelegate {}

    unsafe impl NSWindowDelegate for MuriWindowDelegate {
        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _n: &NSNotification) {
            super::push_event(UiEvent::FocusChanged {
                kind: *self.ivars(),
                key: true,
            });
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _n: &NSNotification) {
            super::push_event(UiEvent::FocusChanged {
                kind: *self.ivars(),
                key: false,
            });
        }
    }
);

impl MuriWindowDelegate {
    fn new(mtm: MainThreadMarker, kind: WindowKind) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(kind);
        unsafe { msg_send![super(this), init] }
    }
}

/// Convert an event's window-space location into the flipped view's top-left
/// logical coordinates (points).
pub(super) fn view_point(view: &NSView, event: &objc2_app_kit::NSEvent) -> (f64, f64) {
    let p = view.convertPoint_fromView(event.locationInWindow(), None);
    (p.x, p.y)
}

/// A freshly built native panel and the objects that must be kept alive with
/// it. The vibrancy backdrop is retained by the panel (as its content view),
/// so it isn't returned; the delegate is *not* retained by the panel and must
/// be kept by the caller.
pub(super) struct NativePanel {
    pub panel: Retained<NSPanel>,
    pub view: Retained<MuriView>,
    pub delegate: Retained<MuriWindowDelegate>,
}

/// Create a non-activating vibrant panel at `content_rect` (screen coordinates,
/// AppKit bottom-left origin) sized in points, its content view rounded to
/// `corner_radius`. The panel is *not* shown; the caller orders it front.
pub(super) fn make_panel(
    mtm: MainThreadMarker,
    content_rect: NSRect,
    corner_radius: f32,
    kind: WindowKind,
) -> NativePanel {
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel: Retained<NSPanel> = unsafe {
        msg_send![
            mtm.alloc::<NSPanel>(),
            initWithContentRect: content_rect,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };

    // A status-menu panel must float above everything, cast a menu shadow, and
    // never deactivate the app when it takes key focus.
    unsafe {
        panel.setReleasedWhenClosed(false);
    }
    panel.setLevel(NSPopUpMenuWindowLevel);
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    panel.setOpaque(false);
    panel.setHasShadow(true);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setAcceptsMouseMovedEvents(true);

    let bounds = NSRect::new(NSPoint::new(0.0, 0.0), content_rect.size);

    // Vibrancy backdrop: the OS composites the real behind-window menu blur
    // here; the raster layer sits above it. Rounded via a corner-radius mask so
    // the blur takes muri's panel shape, not a square.
    let effect: Retained<NSVisualEffectView> =
        NSVisualEffectView::initWithFrame(mtm.alloc(), bounds);
    effect.setMaterial(NSVisualEffectMaterial::Menu);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setWantsLayer(true);
    if let Some(layer) = effect.layer() {
        layer.setCornerRadius(corner_radius as f64);
        layer.setMasksToBounds(true);
    }
    panel.setContentView(Some(&effect));

    // The layer-backed raster surface, on top of the backdrop.
    let view = MuriView::new(mtm, bounds, kind);
    view.setWantsLayer(true);
    effect.addSubview(&view);

    // Deliver `mouseMoved:` to the view regardless of key/active state so hover
    // highlighting works on the non-activating panel.
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    let tracking: Retained<NSTrackingArea> = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            options,
            Some(&view),
            None,
        )
    };
    view.addTrackingArea(&tracking);

    let delegate = MuriWindowDelegate::new(mtm, kind);
    let proto: &ProtocolObject<dyn NSWindowDelegate> = ProtocolObject::from_ref(&*delegate);
    panel.setDelegate(Some(proto));
    panel.setInitialFirstResponder(Some(&view));

    let _ = effect; // retained by the panel as its content view
    NativePanel {
        panel,
        view,
        delegate,
    }
}
