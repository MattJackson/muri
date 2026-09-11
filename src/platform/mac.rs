//! macOS backend: `NSStatusItem` tray anchor plus a native, non-activating
//! `NSPanel` popup driven directly by `NSApplication` (no winit / softbuffer).
//!
//! The status item's button is both the click target and the anchor rect. On
//! click the popup opens as a borderless `NSPanel` (see the `window` submodule)
//! whose content view hosts an `NSVisualEffectView` vibrancy backdrop under a
//! `CALayer` that the shared raster [`Framebuffer`](crate::render::Framebuffer)
//! is blitted to (see the
//! `present` submodule). Submenu rows open a second such panel (a flyout).
//!
//! ## Event model
//!
//! The app runs a native `NSApplication` run loop. Every AppKit callback (tray
//! click, mouse, keyboard, key-window changes, AccessKit actions) is tiny: it
//! translates the event and *enqueues* a `UiEvent`, then asks for a main-thread
//! drain via GCD. A single drain is the only place that mutates state, so
//! windows are never created or destroyed re-entrantly inside an AppKit event
//! dispatch. A [`TrayHandle`](crate::TrayHandle) posts its commands through the
//! same drain from any thread.
//!
//! ## Device-verified behaviors
//!
//! Focus-driven dismissal, keyboard nav on the key panel, VoiceOver traversal,
//! and vibrancy appearance require a real display + assistive tech; those spots
//! are marked `DEVICE-VERIFY(0.9.0)`. The architecture (native non-activating
//! panel, CALayer present, per-window a11y adapter) is complete and compiles.

#![allow(unsafe_code)]

mod input;
mod present;
mod window;

#[cfg(feature = "a11y")]
mod a11y;

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDidResignActiveNotification,
    NSColor, NSColorSpace, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSImage,
    NSMenuDidBeginTrackingNotification, NSScreen, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSData, NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString,
};

use crate::anchor::place_popup;
use crate::error::{Error, Result};
use crate::flyout::{next_flyout, place_flyout, HoverTarget};
use crate::geometry::{Edge, LogicalPoint, LogicalRect, LogicalSize};
use crate::keynav::{handle_key, FlyoutFocus, MenuFocus, NavAction, NavKey};
use crate::menu::{Icon, Item, Menu, MenuId};
use crate::platform::{Appearance, Platform};
use crate::render::paint::{render_menu, LaidMenu};
use crate::render::RasterDrawer;
use crate::style::Color;
use crate::theme::{MenuOptions, OsFamily, Theme};
use crate::{Tray, TrayCommand};

use window::{make_panel, MuriView, MuriWindowDelegate};

// =============================================================================
// GCD main-queue dispatch (replaces the winit event-loop proxy)
// =============================================================================

extern "C" {
    /// The libdispatch main queue (`dispatch_get_main_queue()` is a macro over
    /// the address of this global).
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

// CoreText FFI for resolving the real on-disk file behind the system UI font.
// `NSFont*` is toll-free bridged to `CTFontRef`, and the `CFURL` returned for
// `kCTFontURLAttribute` is toll-free bridged to `NSURL` — so we can hand fontdb
// the actual SF NS file (`/System/Library/Fonts/…`) instead of a family name it
// cannot load (SF Pro lives in a protected file), which otherwise falls back to
// the bundled UI face. CoreText is linked explicitly so the symbols resolve.
#[link(name = "CoreText", kind = "framework")]
extern "C" {
    /// `CTFontCopyAttribute(font, attribute)` → a `+1` `CFTypeRef` (or NULL).
    fn CTFontCopyAttribute(font: *const c_void, attribute: *const c_void) -> *const c_void;
    /// The `kCTFontURLAttribute` `CFStringRef` key (value is the font's file URL).
    static kCTFontURLAttribute: *const c_void;
}

/// The filesystem path of the file backing `font`, via CoreText's URL attribute.
/// `None` when the font has no on-disk URL (e.g. a synthesized/in-memory face).
fn system_ui_font_path(font: &objc2_app_kit::NSFont) -> Option<String> {
    // `NSFont*` is toll-free bridged to `CTFontRef`: the object pointer *is* the
    // `CTFontRef`. `CTFontCopyAttribute` follows the Copy rule (+1 on the result).
    let ct_font: *const c_void = (font as *const objc2_app_kit::NSFont).cast();
    let url: *const c_void = unsafe { CTFontCopyAttribute(ct_font, kCTFontURLAttribute) };
    if url.is_null() {
        return None;
    }
    // The `CFURLRef` is toll-free bridged to `NSURL`; take ownership of the +1.
    let nsurl: Retained<objc2_foundation::NSURL> =
        unsafe { Retained::from_raw(url as *mut objc2_foundation::NSURL)? };
    nsurl.path().map(|p| p.to_string())
}

/// The GCD callback: drain the queued UI events + tray commands on the main
/// thread. Never re-entrant (GCD serializes main-queue blocks), and it skips if
/// a drain is somehow already in flight — the queued work is picked up next.
extern "C" fn drain_trampoline(_ctx: *mut c_void) {
    let app = MAIN_APP.with(|slot| slot.borrow().clone());
    if let Some(app) = app {
        if let Ok(mut state) = app.try_borrow_mut() {
            state.drain();
        }
    }
}

/// Schedule a main-thread drain. Thread-safe: GCD accepts a main-queue dispatch
/// from any thread, which is exactly how a [`TrayHandle`](crate::TrayHandle) on
/// a worker thread pokes the run loop.
pub(super) fn defer_drain() {
    unsafe {
        dispatch_async_f(
            (&_dispatch_main_q as *const c_void).cast(),
            std::ptr::null_mut(),
            drain_trampoline,
        );
    }
}

// =============================================================================
// Thread-local run-loop state + event inbox
// =============================================================================

thread_local! {
    /// The single running [`AppState`], reachable from every main-thread AppKit
    /// callback and the GCD drain. Set once in [`run_tray`].
    static MAIN_APP: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };

    /// Pending high-level UI events, pushed by AppKit callbacks and applied by
    /// the drain. Kept separate from [`AppState`] so callbacks never take an
    /// `AppState` borrow (which could alias the drain's).
    static EVENTS: RefCell<Vec<UiEvent>> = const { RefCell::new(Vec::new()) };
}

/// Enqueue a UI event and request a drain. Safe to call from any AppKit
/// callback; performs no [`AppState`] borrow.
pub(super) fn push_event(event: UiEvent) {
    EVENTS.with(|e| e.borrow_mut().push(event));
    defer_drain();
}

/// Which muri panel an event came from. Flyouts carry their **depth** on the open
/// stack (`0` = the first flyout, opened from the popup; `1` = its child; …) so a
/// callback can tag its events with the exact level without consulting shared
/// state (decision #8, N-level submenus).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WindowKind {
    /// The top-level popup (anchored to the tray icon, a point, or a rect).
    Popup,
    /// An open submenu flyout at the given stack depth (`0` = first flyout).
    Flyout(usize),
}

impl WindowKind {
    /// The menu level this panel renders: `0` is the top-level menu, `k` the
    /// submenu reached by descending `k` open flyouts.
    fn menu_level(self) -> usize {
        match self {
            WindowKind::Popup => 0,
            WindowKind::Flyout(depth) => depth + 1,
        }
    }
}

/// A translated, backend-neutral UI event awaiting application on the drain.
pub(super) enum UiEvent {
    /// The tray icon button was clicked — toggle the popup.
    TrayClicked,
    /// The pointer moved over a panel (view-local top-left points).
    MouseMoved {
        /// Which panel.
        kind: WindowKind,
        /// X in the panel's logical points.
        x: f64,
        /// Y in the panel's logical points.
        y: f64,
    },
    /// A left mouse-down landed on a panel.
    MouseDown {
        /// Which panel.
        kind: WindowKind,
        /// X in the panel's logical points.
        x: f64,
        /// Y in the panel's logical points.
        y: f64,
    },
    /// The pointer left a panel's tracking area. Drives submenu collapse + hover
    /// clear when the pointer leaves the menu onto the desktop (the native menu
    /// closes its open submenu when you move away).
    MouseExited {
        /// Which panel the pointer left.
        kind: WindowKind,
    },
    /// A navigation key was pressed on the key panel.
    Key(NavKey),
    /// A panel gained or lost key-window status.
    FocusChanged {
        /// Which panel.
        kind: WindowKind,
        /// `true` on become-key, `false` on resign-key.
        key: bool,
    },
    /// A global/local `NSEvent` monitor saw a mouse-down at a global screen point
    /// (AppKit bottom-left coordinates). The drain dismisses the whole stack when
    /// the point falls outside every open panel — the click-away path a
    /// non-activating panel's `resignKey` never delivers (#11).
    OutsideClick {
        /// Global screen x (AppKit bottom-left origin).
        x: f64,
        /// Global screen y (AppKit bottom-left origin).
        y: f64,
    },
    /// Another menu began tracking, or the app resigned active — dismiss the whole
    /// stack so at most one menu is ever open (OS menu-tracking parity, #11).
    Dismiss,
    /// An AccessKit action request (VoiceOver focus/activate) for a panel.
    #[cfg(feature = "a11y")]
    A11yAction {
        /// Which panel's adapter raised it.
        kind: WindowKind,
        /// The requested action.
        request: accesskit::ActionRequest,
    },
}

// =============================================================================
// Tray click target
// =============================================================================

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MuriTrayTarget"]
    #[thread_kind = MainThreadOnly]
    struct TrayTarget;

    impl TrayTarget {
        #[unsafe(method(trayClicked:))]
        fn tray_clicked(&self, _sender: Option<&AnyObject>) {
            push_event(UiEvent::TrayClicked);
        }
    }
);

impl TrayTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![super(mtm.alloc::<Self>().set_ivars(())), init] }
    }
}

// =============================================================================
// Native-menu-parity dismissal (#11)
// =============================================================================

define_class!(
    // Notification sink for the two "some other menu is taking over" signals:
    // `NSMenuDidBeginTrackingNotification` (any other menu-bar / context menu
    // began tracking) and `NSApplicationDidResignActiveNotification` (focus went
    // to another app). Either one enqueues a `Dismiss`, mirroring how AppKit's
    // menu-tracking guarantees exactly one open menu at a time.
    //
    // This gives the FORWARD half of OEM mutual-exclusion: when a native (OEM)
    // menu opens, muri's popup dismisses — so the two are never both up because a
    // native one appeared. The REVERSE (muri force-closing an *already-open,
    // foreign-app* native menu when muri's popup opens) is an inherent macOS
    // limitation, NOT a muri bug: a foreign app's `NSMenu` tracking session can
    // only be ended by clicking outside it or by that app deactivating, and muri's
    // popup is a **non-activating** `NSPanel` by design (it must not steal the
    // user's keyboard focus), so it does neither. There is no public API to cancel
    // another process's menu tracking. Documented as a known limitation.
    #[unsafe(super(NSObject))]
    #[name = "MuriDismissObserver"]
    #[thread_kind = MainThreadOnly]
    struct DismissObserver;

    impl DismissObserver {
        #[unsafe(method(muriDismiss:))]
        fn muri_dismiss(&self, _n: &NSNotification) {
            push_event(UiEvent::Dismiss);
        }
    }
);

impl DismissObserver {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![super(mtm.alloc::<Self>().set_ivars(())), init] }
    }
}

/// The AppKit-level dismissal watchers armed while a popup is open: the two
/// `NSEvent` monitors that catch click-away (which a non-activating panel's
/// `resignKey` misses) and the notification observer for mutual exclusion. Owned
/// by the [`PopupSession`] for the popup's lifetime and dropped on close, which
/// unregisters everything exactly once — no monitors/observers leak across
/// open/close cycles (#11).
struct DismissWatchers {
    global_monitor: Option<Retained<AnyObject>>,
    local_monitor: Option<Retained<AnyObject>>,
    observer: Retained<DismissObserver>,
}

impl DismissWatchers {
    /// Install the monitors + observer. Each callback only enqueues a `UiEvent` +
    /// asks for a drain (no `AppState` borrow), exactly like every other AppKit
    /// callback here.
    fn install(mtm: MainThreadMarker) -> Self {
        let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown;

        // Global monitor: fires for mouse-downs delivered to OTHER apps / the
        // desktop — the clicks `resignKey` never reports. Cannot consume the
        // event (nor should it); it just reports the point.
        let global_block = RcBlock::new(|_event: NonNull<NSEvent>| {
            let p = NSEvent::mouseLocation();
            push_event(UiEvent::OutsideClick { x: p.x, y: p.y });
        });
        let global_monitor =
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block);

        // Local monitor: fires for mouse-downs headed into our own process. A
        // click on a panel is hit-tested away by the drain; a click elsewhere
        // in-app dismisses. The event is returned unchanged so normal delivery
        // still happens.
        let local_block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            let p = NSEvent::mouseLocation();
            push_event(UiEvent::OutsideClick { x: p.x, y: p.y });
            event.as_ptr()
        });
        // SAFETY: the block returns the `NSEvent*` it was handed, satisfying
        // `addLocalMonitor...`'s contract.
        let local_monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) };

        let observer = DismissObserver::new(mtm);
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: `observer` responds to `muriDismiss:`; both names are valid
        // `&'static NSNotificationName`s.
        unsafe {
            center.addObserver_selector_name_object(
                &observer,
                sel!(muriDismiss:),
                Some(NSMenuDidBeginTrackingNotification),
                None,
            );
            center.addObserver_selector_name_object(
                &observer,
                sel!(muriDismiss:),
                Some(NSApplicationDidResignActiveNotification),
                None,
            );
        }

        DismissWatchers {
            global_monitor,
            local_monitor,
            observer,
        }
    }
}

impl Drop for DismissWatchers {
    fn drop(&mut self) {
        // SAFETY: each token came from an `NSEvent` monitor add and is removed at
        // most once (taken out of its `Option`); the observer is the one we added.
        unsafe {
            if let Some(m) = self.global_monitor.take() {
                NSEvent::removeMonitor(&m);
            }
            if let Some(m) = self.local_monitor.take() {
                NSEvent::removeMonitor(&m);
            }
            NSNotificationCenter::defaultCenter().removeObserver(&self.observer);
        }
    }
}

/// Whether `(x, y)` lies outside every rectangle in `frames`, each given as
/// `(min_x, min_y, max_x, max_y)`. The click-away decision: a mouse-down outside
/// all open panels (and the tray button) dismisses the stack. Pure + testable so
/// the hit-test is covered without a real display (the monitors are
/// DEVICE-VERIFY).
fn point_outside_all(frames: &[(f64, f64, f64, f64)], x: f64, y: f64) -> bool {
    !frames
        .iter()
        .any(|&(min_x, min_y, max_x, max_y)| x >= min_x && x <= max_x && y >= min_y && y <= max_y)
}

// =============================================================================
// NSStatusItem anchor
// =============================================================================

/// The macOS `NSStatusItem`-based anchor. Owns the status item and its click
/// target once installed, and removes the status item explicitly on drop.
pub struct MacosAnchor {
    mtm: MainThreadMarker,
    status_item: Option<Retained<NSStatusItem>>,
    // Kept alive so the button's (non-retaining) target isn't deallocated.
    _target: Option<Retained<TrayTarget>>,
}

impl std::fmt::Debug for MacosAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacosAnchor")
            .field("installed", &self.status_item.is_some())
            .finish()
    }
}

/// The status item's on-screen geometry, resolved against the *button's own*
/// screen so multi-monitor / notch-overflow placement is correct (spec 20 §2).
/// All the derived rectangles live in that screen's local, top-left logical
/// space; [`AnchorGeometry::to_screen`] converts back to AppKit's global,
/// bottom-left screen coordinates for window placement.
#[derive(Clone, Copy)]
struct AnchorGeometry {
    button_frame: NSRect,
    screen_frame: NSRect,
    visible_frame: NSRect,
    scale: f32,
}

impl AnchorGeometry {
    /// The status-item rect in the screen's local top-left space.
    fn anchor_rect_local(&self) -> LogicalRect {
        let sf = self.screen_frame;
        let bf = self.button_frame;
        let x = (bf.origin.x - sf.origin.x) as f32;
        let top = (sf.origin.y + sf.size.height - (bf.origin.y + bf.size.height)) as f32;
        LogicalRect::new(
            LogicalPoint::new(x, top),
            LogicalSize::new(bf.size.width as f32, bf.size.height as f32),
        )
    }

    /// The usable work area (excludes menu bar + Dock) in the same local space.
    fn work_area_local(&self) -> LogicalRect {
        let sf = self.screen_frame;
        let vf = self.visible_frame;
        let x = (vf.origin.x - sf.origin.x) as f32;
        let top = (sf.origin.y + sf.size.height - (vf.origin.y + vf.size.height)) as f32;
        LogicalRect::new(
            LogicalPoint::new(x, top),
            LogicalSize::new(vf.size.width as f32, vf.size.height as f32),
        )
    }

    /// Convert a local top-left origin (+ panel size) back to an AppKit global
    /// bottom-left screen point for `initWithContentRect:`.
    fn to_screen(self, origin: LogicalPoint, size: LogicalSize) -> NSPoint {
        let sf = self.screen_frame;
        let x = sf.origin.x + origin.x as f64;
        let y = sf.origin.y + sf.size.height - (origin.y as f64 + size.height as f64);
        NSPoint::new(x, y)
    }

    /// A fixed anchor `rect` (top-left logical, e.g. a zero-size rect at a
    /// pointer, or a toolbar-button rect), resolved against the main screen — the
    /// anchor geometry for a pointer/rect-anchored
    /// [`ContextMenu`](crate::ContextMenu) / [`Popup`](crate::Popup) that has no
    /// tray icon (spec 20 §3).
    ///
    // DEVICE-VERIFY(0.9.0): multi-monitor point resolution — this resolves the
    // rect against the *main* screen; a rect on a secondary display needs the
    // screen that actually contains it (same flip fragility as the tray anchor).
    fn for_rect(mtm: MainThreadMarker, rect: LogicalRect) -> Option<Self> {
        let screen = NSScreen::screens(mtm).firstObject()?;
        let sf = screen.frame();
        // Invert `anchor_rect_local`: a local top-left rect maps to a bottom-left
        // button frame on the main screen.
        let bf = NSRect::new(
            NSPoint::new(
                sf.origin.x + rect.origin.x as f64,
                sf.origin.y + sf.size.height - (rect.origin.y + rect.size.height) as f64,
            ),
            NSSize::new(rect.size.width as f64, rect.size.height as f64),
        );
        Some(AnchorGeometry {
            button_frame: bf,
            screen_frame: sf,
            visible_frame: screen.visibleFrame(),
            scale: screen.backingScaleFactor() as f32,
        })
    }
}

impl MacosAnchor {
    /// Create the (not-yet-installed) macOS anchor. Must be called on the main
    /// thread.
    pub fn new(mtm: MainThreadMarker) -> Self {
        MacosAnchor {
            mtm,
            status_item: None,
            _target: None,
        }
    }

    /// Render the status-item button from the current [`Icon`] plus an optional
    /// menu-bar text `title`. A valid PNG/SVG becomes the button image; a
    /// non-empty `title` is drawn as the button's text (the macOS menu-bar text,
    /// e.g. a live "45%"), composing with the image when both are present. When
    /// both a drawable image and a non-empty title are present they render
    /// together **icon leading, title trailing** (`NSImageLeft`), matching native
    /// menu-bar extras (#58). When neither a drawable image nor a title is
    /// available, a bullet placeholder keeps the item visible and clickable (an
    /// empty status item is invisible).
    fn set_status(&self, icon: &Icon, title: Option<&str>, tooltip: Option<&str>) {
        let Some(item) = &self.status_item else {
            return;
        };
        let Some(button) = item.button(self.mtm) else {
            return;
        };
        let mut has_image = false;
        // PNG bytes go straight to AppKit; SVG bytes are rasterized (via the
        // zeno-backed subset rasterizer) and re-encoded to PNG first, since
        // NSImage decodes PNG but not muri's SVG subset.
        let png_bytes: Option<std::borrow::Cow<'_, [u8]>> = match icon {
            Icon::Png(bytes) => Some(std::borrow::Cow::Borrowed(&bytes[..])),
            Icon::Svg(bytes) => svg_to_png(&bytes[..]).map(std::borrow::Cow::Owned),
            _ => None,
        };
        if let Some(bytes) = png_bytes {
            let data = NSData::with_bytes(&bytes);
            if let Some(image) =
                NSImage::initWithData(NSImage::alloc(), &data).filter(|i| i.isValid())
            {
                image.setSize(NSSize::new(18.0, 18.0));
                button.setImage(Some(&image));
                has_image = true;
            }
        }
        if !has_image {
            button.setImage(None);
        }
        // A non-empty title wins as the visible text; else show the image alone
        // (empty title), else the bullet placeholder so the item stays visible.
        let title = title.filter(|t| !t.is_empty());
        let text = match title {
            Some(t) => t,
            None if has_image => "",
            None => "●",
        };
        button.setTitle(&NSString::from_str(text));
        // With both a glyph and a label, lay them out like a native menu-bar
        // extra: icon leading, text trailing (#58). AppKit's default image
        // position can otherwise let one dominate; pin it explicitly. When only
        // the image is present the title is empty, so the position is moot.
        // DEVICE-VERIFY(0.10.8): confirm the icon+title side-by-side layout on a
        // real menu bar.
        if has_image && title.is_some() {
            button.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeft);
        }
        if let Some(tip) = tooltip {
            button.setToolTip(Some(&NSString::from_str(tip)));
        }
    }

    /// Update the tooltip / accessible name of the status-item button.
    fn set_tooltip(&self, tooltip: Option<&str>) {
        if let Some(item) = &self.status_item {
            if let Some(button) = item.button(self.mtm) {
                button.setToolTip(tooltip.map(NSString::from_str).as_deref());
            }
        }
    }

    /// Show or hide the status item.
    fn set_visible(&self, visible: bool) {
        if let Some(item) = &self.status_item {
            item.setVisible(visible);
        }
    }

    /// Remove the status item from the menu bar now (the `Shutdown` command).
    /// `Drop` alone can't be relied on here: the spawned-tray path parks the
    /// owning `AppState` in the `MAIN_APP` thread-local for the life of the
    /// process, so its `Drop` never fires. Takes the item so `Drop` won't try to
    /// remove it a second time.
    fn remove(&mut self) {
        if let Some(item) = self.status_item.take() {
            NSStatusBar::systemStatusBar().removeStatusItem(&item);
        }
    }

    /// Resolve the status item's geometry against the button's own screen.
    fn geometry(&self) -> Option<AnchorGeometry> {
        let item = self.status_item.as_ref()?;
        let button = item.button(self.mtm)?;
        let window = button.window()?;
        let screen = window
            .screen()
            .or_else(|| NSScreen::screens(self.mtm).firstObject())?;
        Some(AnchorGeometry {
            button_frame: window.frame(),
            screen_frame: screen.frame(),
            visible_frame: screen.visibleFrame(),
            scale: screen.backingScaleFactor() as f32,
        })
    }
}

impl MacosAnchor {
    fn install(&mut self, tooltip: Option<&str>) -> Result<()> {
        let status_bar = NSStatusBar::systemStatusBar();
        let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let target = TrayTarget::new(self.mtm);
        if let Some(button) = item.button(self.mtm) {
            unsafe {
                button.setTarget(Some(&target));
                button.setAction(Some(sel!(trayClicked:)));
            }
            if let Some(tip) = tooltip {
                button.setToolTip(Some(&NSString::from_str(tip)));
            }
        }
        self.status_item = Some(item);
        self._target = Some(target);
        Ok(())
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        self.geometry()
            .map(|g| g.anchor_rect_local())
            .ok_or_else(|| Error::Platform("status item not installed".into()))
    }

    /// The logical work area of the monitor the tray anchor lives on.
    fn work_area(&self) -> Result<LogicalRect> {
        self.geometry()
            .map(|g| g.work_area_local())
            .ok_or_else(|| Error::Platform("status item not installed".into()))
    }
}

impl Drop for MacosAnchor {
    fn drop(&mut self) {
        // Explicit teardown (spec 20 §1): don't rely on ARC release alone. Same
        // operation as the `Shutdown` command, so delegate to keep the two in sync.
        self.remove();
    }
}

// =============================================================================
// Panels + app state
// =============================================================================

/// A live popup or flyout panel: the native objects, the raster drawer that
/// paints it, and its current layout / hover / selection state.
struct Panel {
    panel: Retained<objc2_app_kit::NSPanel>,
    view: Retained<MuriView>,
    /// The window delegate is *not* retained by the panel, so we must keep it.
    #[allow(dead_code)]
    delegate: Retained<MuriWindowDelegate>,
    drawer: RasterDrawer,
    laid: Option<LaidMenu>,
    cursor: LogicalPoint,
    hovered: Option<usize>,
    /// Top-left origin in the anchor screen's local logical space.
    origin: LogicalPoint,
    scale: f32,
    #[cfg(feature = "a11y")]
    adapter: accesskit_macos::SubclassingAdapter,
    #[cfg(feature = "a11y")]
    snapshot: Rc<RefCell<a11y::A11ySnapshot>>,
}

impl Panel {
    fn order_out(&self) {
        self.panel.orderOut(None);
    }
}

/// One open flyout level: the row (within its parent level's menu) it opened
/// from, and its native panel. Level `k` of [`PopupSession::flyouts`] is panel
/// depth `k + 1` ([`WindowKind::Flyout`]) and its menu is
/// [`PopupSession::menu_at_level`]`(k + 1)`.
struct Flyout {
    /// Item index (within the parent level's menu) this flyout opened from.
    parent: usize,
    /// The flyout's native panel.
    panel: Panel,
}

/// Where a session's popup anchors: the live tray icon (which also owns icon /
/// tooltip / visibility) or a fixed rectangle for a pointer-anchored
/// [`ContextMenu`](crate::ContextMenu) / [`Popup`](crate::Popup) (spec 20 §3).
enum Anchor {
    /// The `NSStatusItem` tray anchor (its geometry follows the icon).
    Tray(MacosAnchor),
    /// A fixed anchor rectangle, resolved once against the main screen.
    Fixed(AnchorGeometry),
}

impl Anchor {
    /// The current anchor geometry (screen frames + scale + anchor rect).
    fn geometry(&self) -> Option<AnchorGeometry> {
        match self {
            Anchor::Tray(a) => a.geometry(),
            Anchor::Fixed(g) => Some(*g),
        }
    }
}

/// The shared popup machinery: the top-level popup, the **stack** of open flyout
/// panels (decision #8), the focus set that drives dismissal, and everything
/// needed to render/anchor them. [`Tray`], [`ContextMenu`](crate::ContextMenu),
/// and [`Popup`](crate::Popup) all drive one of these; they differ only in how the
/// anchor rectangle is obtained ([`Anchor`]) — spec 20 §3.
///
/// The click handler is stored as a `Box<dyn Fn + 'a>`: the tray uses a `'static`
/// handler owned for the whole run loop; a context menu borrows the caller's
/// handler for the duration of its blocking `open_at`/`anchored_to` call.
struct PopupSession<'a> {
    mtm: MainThreadMarker,
    /// The top-level menu; source of truth for rendering + a11y.
    menu: Menu,
    options: MenuOptions,
    /// Row-activation sink; dispatches the activated [`MenuId`].
    dispatch: Box<dyn Fn(&MenuId) + 'a>,
    /// How the popup anchors (tray icon or a fixed rect).
    anchor: Anchor,
    /// Which edge the popup grows from relative to its anchor.
    edge: Edge,
    popup: Option<Panel>,
    /// The open flyout window stack, shallowest first (decision #8).
    flyouts: Vec<Flyout>,
    /// Which muri panels currently hold key-window focus. The whole stack is
    /// dismissed only when this empties (and a resign armed it) — so opening a
    /// flyout, or focus transferring between our own panels, never self-closes.
    focused: HashSet<WindowKind>,
    /// Set when a resign-key left [`PopupSession::focused`] empty; checked after
    /// the drain so a paired become-key (a within-stack transfer) cancels it.
    dismiss_armed: bool,
    /// AppKit click-away + mutual-exclusion watchers, armed while a popup is open
    /// and dropped (unregistered) on close. `None` when no popup is showing so no
    /// monitors/observers ever leak across open/close cycles (#11).
    watchers: Option<DismissWatchers>,
}

impl PopupSession<'_> {
    /// The resolved theme for the current appearance, with the live OS accent
    /// injected (spec 20 §4c) so `Color::Accent` follows the system.
    fn theme(&self) -> Theme {
        // The source resolves against the host family (macOS) + live appearance;
        // only a `System(..)` source gets the live OS accent, menu colors, SF
        // face/size, and opaque-when-transparency-disabled treatment. Explicit
        // family / preset / custom themes render exactly as authored.
        let mut theme = self
            .options
            .theme
            .resolve(OsFamily::MacOs, system_is_dark());
        if self.options.theme.injects_system() {
            if let Some((r, g, b, a)) = system_accent(self.mtm) {
                theme.accent = Color::Rgba(r, g, b, a);
            }
            read_system_palette().apply_to(&mut theme);
            if let Some(font) = read_system_menu_font() {
                font.apply_size_to(&mut theme);
            }
            // Native SF tracking (#42/#57): CoreText applies a small size-dependent
            // tracking to San Francisco that a bare shaper does not, so muri's menu
            // text otherwise reads slightly *looser* than a real NSMenu. The macOS
            // preset already bakes tracking in for its 13pt base; here we OVERWRITE
            // (assign, not `+=`) with the value recomputed for the *live* menu size
            // just applied by `apply_size_to` above — so the System theme tracks for
            // the live size with no double application on top of the preset. Only
            // System themes get this — forced/preset/custom faces aren't SF, but a
            // forced macOS preset still carries its baked-in tracking on any host.
            theme.row_font.letter_spacing = sf_ui_tracking(theme.row_font.size);
            theme.header_font.letter_spacing = sf_ui_tracking(theme.header_font.size);
            if !transparency_enabled() {
                theme.make_opaque();
            }
        }
        theme
    }

    /// The menu shown at the given level: `0` is the top-level menu, `k` the
    /// submenu reached by descending the first `k` open flyouts' parents. Returns
    /// `None` if a parent along the way is no longer a submenu.
    fn menu_at_level(&self, level: usize) -> Option<&Menu> {
        crate::menu::descend(
            &self.menu,
            self.flyouts.iter().take(level).map(|f| f.parent),
        )
    }

    fn panel(&self, kind: WindowKind) -> Option<&Panel> {
        match kind {
            WindowKind::Popup => self.popup.as_ref(),
            WindowKind::Flyout(d) => self.flyouts.get(d).map(|f| &f.panel),
        }
    }

    fn panel_mut(&mut self, kind: WindowKind) -> Option<&mut Panel> {
        match kind {
            WindowKind::Popup => self.popup.as_mut(),
            WindowKind::Flyout(d) => self.flyouts.get_mut(d).map(|f| &mut f.panel),
        }
    }

    /// The open-flyout parent stack ([`next_flyout`]'s representation).
    fn flyout_stack(&self) -> Vec<usize> {
        self.flyouts.iter().map(|f| f.parent).collect()
    }

    /// Rebuild the current keyboard/mouse selection from the panels' hovered rows.
    fn current_focus(&self) -> MenuFocus {
        MenuFocus {
            top: self.popup.as_ref().and_then(|p| p.hovered),
            flyout: self
                .flyouts
                .iter()
                .map(|f| FlyoutFocus {
                    parent: f.parent,
                    child: f.panel.hovered,
                })
                .collect(),
        }
    }

    // -- open / close --------------------------------------------------------

    fn open_popup(&mut self) {
        if self.popup.is_some() {
            return;
        }
        let theme = self.theme();
        let Some(geom) = self.anchor.geometry() else {
            return;
        };
        let scale = geom.scale.max(1.0);

        // Measure offscreen to size the panel before it exists (no resize flash).
        // This same drawer becomes the panel's drawer (below) so its warm
        // shaping/glyph caches carry into the first paint — the menu is not shaped
        // a second time with a cold drawer (#23).
        // Forced-OS theme -> target OS font; else host-native (#54).
        let mut drawer = RasterDrawer::for_menu_options(scale, &self.options);
        let laid = render_menu(&mut drawer, &self.menu, &theme, &self.options, None);

        let origin = place_popup(
            geom.anchor_rect_local(),
            laid.size,
            geom.work_area_local(),
            self.edge,
            2.0,
        );
        let screen_origin = geom.to_screen(origin, laid.size);
        let content_rect = NSRect::new(
            screen_origin,
            NSSize::new(laid.size.width as f64, laid.size.height as f64),
        );

        let native = make_panel(
            self.mtm,
            content_rect,
            theme.corner_radius,
            WindowKind::Popup,
        );

        #[cfg(feature = "a11y")]
        let snapshot = Rc::new(RefCell::new(a11y::A11ySnapshot {
            menu: self.menu.clone(),
            focus: MenuFocus {
                top: None,
                flyout: Vec::new(),
            },
        }));
        #[cfg(feature = "a11y")]
        let adapter = a11y::make_adapter(&native.view, Rc::clone(&snapshot), WindowKind::Popup);

        self.popup = Some(Panel {
            panel: native.panel,
            view: native.view,
            delegate: native.delegate,
            drawer,
            laid: Some(laid),
            cursor: LogicalPoint::default(),
            hovered: None,
            origin,
            scale,
            #[cfg(feature = "a11y")]
            adapter,
            #[cfg(feature = "a11y")]
            snapshot,
        });

        // Paint the first frame, then reveal + take key. A NonactivatingPanel
        // becoming key does not deactivate the user's foreground app.
        self.redraw(WindowKind::Popup);
        if let Some(popup) = self.popup.as_ref() {
            popup.panel.makeKeyAndOrderFront(None);
        }
        // Arm the click-away + mutual-exclusion watchers now that a panel is up.
        self.watchers = Some(DismissWatchers::install(self.mtm));
        self.sync_a11y();
    }

    /// Push a flyout for row `parent_index` of the currently deepest open level
    /// (the popup when no flyout is open), placed beside its parent panel by
    /// [`place_flyout`] (right by default, flipped left on spill — doc 10 §12).
    fn push_flyout(&mut self, parent_index: usize) {
        let depth = self.flyouts.len();
        // The level whose row we're opening from == the current deepest level.
        let Some(parent_menu) = self.menu_at_level(depth) else {
            return;
        };
        let Some(child) = (match parent_menu.items.get(parent_index) {
            Some(Item::Submenu { menu, .. }) => Some(menu.clone()),
            _ => None,
        }) else {
            return;
        };

        let (parent_origin, parent_size, scale, row_rect) = {
            let parent_panel = if depth == 0 {
                self.popup.as_ref()
            } else {
                self.flyouts.get(depth - 1).map(|f| &f.panel)
            };
            let Some(pp) = parent_panel else {
                return;
            };
            let Some(rect) = pp
                .laid
                .as_ref()
                .and_then(|l| l.rows.iter().find(|r| r.index == parent_index))
                .map(|r| r.rect)
            else {
                return;
            };
            (
                pp.origin,
                pp.laid.as_ref().map(|l| l.size).unwrap_or_default(),
                pp.scale,
                rect,
            )
        };

        let theme = self.theme();
        let Some(geom) = self.anchor.geometry() else {
            return;
        };
        // Reuse this measuring drawer as the flyout's drawer (#23).
        // Forced-OS theme -> target OS font; else host-native (#54).
        let mut drawer = RasterDrawer::for_menu_options(scale, &self.options);
        let child_laid = render_menu(&mut drawer, &child, &theme, &self.options, None);

        let parent_rect = LogicalRect::new(parent_origin, parent_size);
        let placement = place_flyout(
            parent_rect,
            row_rect,
            child_laid.size,
            geom.work_area_local(),
        );
        let screen_origin = geom.to_screen(placement.origin, child_laid.size);
        let content_rect = NSRect::new(
            screen_origin,
            NSSize::new(child_laid.size.width as f64, child_laid.size.height as f64),
        );

        let kind = WindowKind::Flyout(depth);
        let native = make_panel(self.mtm, content_rect, theme.corner_radius, kind);

        #[cfg(feature = "a11y")]
        let snapshot = Rc::new(RefCell::new(a11y::A11ySnapshot {
            menu: child.clone(),
            focus: MenuFocus {
                top: None,
                flyout: Vec::new(),
            },
        }));
        #[cfg(feature = "a11y")]
        let adapter = a11y::make_adapter(&native.view, Rc::clone(&snapshot), kind);

        self.flyouts.push(Flyout {
            parent: parent_index,
            panel: Panel {
                panel: native.panel,
                view: native.view,
                delegate: native.delegate,
                drawer,
                laid: None,
                cursor: LogicalPoint::default(),
                hovered: None,
                origin: placement.origin,
                scale,
                #[cfg(feature = "a11y")]
                adapter,
                #[cfg(feature = "a11y")]
                snapshot,
            },
        });
        // The focus set tracks only REAL key windows — i.e. the popup (which
        // becomes key, #17). Flyouts are `orderFrontRegardless` and never take
        // key, so they must NOT be added here: a synthetic entry would never be
        // cleared by a resign and would keep `focused` non-empty after the popup
        // resigns, defeating focus-loss dismissal while a submenu is open (#37).
        self.redraw(kind);
        // Order in front but do NOT take key from the popup: keyboard nav keeps
        // running through the key panel and the stack does not self-dismiss.
        if let Some(f) = self.flyouts.last() {
            f.panel.panel.orderFrontRegardless();
        }
        self.sync_a11y();
    }

    /// Close every flyout deeper than `len`, ordering their panels out.
    fn truncate_flyouts(&mut self, len: usize) {
        while self.flyouts.len() > len {
            let depth = self.flyouts.len() - 1;
            if let Some(f) = self.flyouts.pop() {
                f.panel.order_out();
            }
            let _ = depth;
        }
    }

    fn close_popup(&mut self) {
        self.truncate_flyouts(0);
        if let Some(popup) = self.popup.take() {
            popup.order_out();
        }
        self.focused.clear();
        self.dismiss_armed = false;
        // Dropping unregisters both `NSEvent` monitors and the observer exactly
        // once; a no-op when already `None` (idempotent close).
        self.watchers = None;
    }

    /// Reconcile the open flyout window stack to `target` (per-level parent
    /// indices, as produced by [`next_flyout`]): keep the common prefix, close
    /// anything deeper, then push the remaining levels.
    fn apply_flyout_stack(&mut self, target: &[usize]) {
        let mut common = 0;
        while common < target.len()
            && common < self.flyouts.len()
            && self.flyouts[common].parent == target[common]
        {
            common += 1;
        }
        self.truncate_flyouts(common);
        for &parent in &target[common..] {
            self.push_flyout(parent);
        }
    }

    // -- present -------------------------------------------------------------

    /// Re-render one panel from its level's menu + hovered row.
    fn redraw(&mut self, kind: WindowKind) {
        let theme = self.theme();
        let options = self.options.clone();
        // Borrow the level's menu from `self.menu`; the panel below is taken from
        // the disjoint `self.popup`/`self.flyouts` fields (not via `panel_mut`,
        // which would borrow all of `self`), so no clone is needed on redraw.
        let Some(menu) = crate::menu::descend(
            &self.menu,
            self.flyouts
                .iter()
                .take(kind.menu_level())
                .map(|f| f.parent),
        ) else {
            return;
        };
        let panel = match kind {
            WindowKind::Popup => self.popup.as_mut(),
            WindowKind::Flyout(d) => self.flyouts.get_mut(d).map(|f| &mut f.panel),
        };
        let Some(panel) = panel else {
            return;
        };
        let laid = render_menu(&mut panel.drawer, menu, &theme, &options, panel.hovered);
        if let Some(image) = present::framebuffer_to_cgimage(panel.drawer.framebuffer()) {
            present::set_layer_contents(&panel.view, &image, panel.scale);
        }
        panel.laid = Some(laid);
    }

    fn redraw_all(&mut self) {
        self.redraw(WindowKind::Popup);
        for d in 0..self.flyouts.len() {
            self.redraw(WindowKind::Flyout(d));
        }
    }

    // -- pointer -------------------------------------------------------------

    /// Handle a cursor move over `kind`: update its hovered row (repaint on
    /// change), then drive the flyout stack from the pure hover-stack rule.
    fn on_cursor(&mut self, kind: WindowKind, pt: LogicalPoint) {
        let (hovered, changed) = {
            let Some(p) = self.panel_mut(kind) else {
                return;
            };
            p.cursor = pt;
            let h = p.laid.as_ref().and_then(|l| l.hit(pt));
            let changed = h != p.hovered;
            if changed {
                p.hovered = h;
            }
            (h, changed)
        };
        if changed {
            self.redraw(kind);
        }
        let panel_depth = kind.menu_level();
        let level_menu = self.menu_at_level(panel_depth);
        let target = match hovered {
            Some(i)
                if level_menu
                    .is_some_and(|m| matches!(m.items.get(i), Some(Item::Submenu { .. }))) =>
            {
                HoverTarget::ParentRow {
                    panel: panel_depth,
                    index: i,
                }
            }
            Some(_) => HoverTarget::OtherRow { panel: panel_depth },
            None => HoverTarget::Outside,
        };
        let next = next_flyout(&self.flyout_stack(), target);
        self.apply_flyout_stack(&next);
        self.sync_a11y();
    }

    /// The pointer left a panel's tracking area. If it merely crossed into
    /// another open panel (e.g. into a child flyout), that panel's own tracking
    /// takes over — do nothing, so we never race-close a submenu we're entering.
    /// If it left every panel (onto the desktop), collapse all submenu flyouts
    /// and clear the row highlight; the popup itself stays open (dismissal is the
    /// click-away / focus-loss path), matching native `NSMenu` behavior.
    fn on_exit(&mut self, _kind: WindowKind) {
        if self.popup.is_none() {
            return;
        }
        // Current global pointer location (AppKit bottom-left screen coords, the
        // same space as `panel_screen_frames`).
        let loc = NSEvent::mouseLocation();
        let frames = self.panel_screen_frames();
        if !point_outside_all(&frames, loc.x, loc.y) {
            return;
        }
        self.truncate_flyouts(0);
        let cleared = self
            .popup
            .as_mut()
            .map(|p| p.hovered.take().is_some())
            .unwrap_or(false);
        if cleared {
            self.redraw(WindowKind::Popup);
        }
        self.sync_a11y();
    }

    fn on_click(&mut self, kind: WindowKind) {
        let level = kind.menu_level();
        let Some(menu) = self.menu_at_level(level) else {
            return;
        };
        let (hit, id) = {
            let Some(p) = self.panel(kind) else {
                return;
            };
            (
                p.laid.as_ref().and_then(|l| l.hit(p.cursor)),
                p.laid.as_ref().and_then(|l| l.id_at(p.cursor)),
            )
        };
        if let Some(i) = hit {
            if matches!(menu.items.get(i), Some(Item::Submenu { .. })) {
                // Open (or switch to) this row's flyout, closing anything deeper.
                self.truncate_flyouts(level);
                self.push_flyout(i);
                return;
            }
        }
        if let Some(id) = id {
            if !id.is_none() {
                (self.dispatch)(&id);
            }
            self.close_popup();
        }
    }

    // -- keyboard ------------------------------------------------------------

    fn on_key_nav(&mut self, key: NavKey) {
        let mut focus = self.current_focus();
        let action = handle_key(&self.menu, &mut focus, key);
        if let Some(popup) = self.popup.as_mut() {
            popup.hovered = focus.top;
        }
        match action {
            NavAction::None => return,
            NavAction::Redraw => {}
            NavAction::OpenFlyout(i) => self.push_flyout(i),
            NavAction::CloseFlyout => {
                let keep = self.flyouts.len().saturating_sub(1);
                self.truncate_flyouts(keep);
            }
            NavAction::Activate(id) => {
                if !id.is_none() {
                    (self.dispatch)(&id);
                }
                self.close_popup();
                return;
            }
            NavAction::CloseAll => {
                self.close_popup();
                return;
            }
        }
        // Write the (possibly new) per-level child selections back into the
        // panels that still exist.
        for (k, f) in self.flyouts.iter_mut().enumerate() {
            if let Some(ff) = focus.flyout.get(k) {
                f.panel.hovered = ff.child;
            }
        }
        self.redraw_all();
        self.sync_a11y();
    }

    // -- accessibility -------------------------------------------------------

    #[cfg(feature = "a11y")]
    fn is_submenu_at_path(&self, path: &[usize]) -> bool {
        let mut menu = &self.menu;
        for (k, &idx) in path.iter().enumerate() {
            match menu.items.get(idx) {
                Some(Item::Submenu { menu: child, .. }) => {
                    if k + 1 == path.len() {
                        return true;
                    }
                    menu = child;
                }
                _ => return false,
            }
        }
        false
    }

    #[cfg(feature = "a11y")]
    fn menu_id_at_path(&self, path: &[usize]) -> Option<MenuId> {
        let mut menu = &self.menu;
        for (k, &idx) in path.iter().enumerate() {
            match menu.items.get(idx)? {
                Item::Row(row) if k + 1 == path.len() => return Some(row.id.clone()),
                Item::Submenu { menu: child, .. } => menu = child,
                _ => return None,
            }
        }
        None
    }

    /// Push a fresh `TreeUpdate` to every open panel's adapter (Option B: one
    /// per-window adapter per stack level — spec 30 §3). Each window's tree is
    /// its own level's menu; its focus is that window's selection, and any deeper
    /// open levels are carried as its expanded sub-stack.
    #[cfg(feature = "a11y")]
    fn sync_a11y(&mut self) {
        let full = self.current_focus();
        // `menu` closures below are only invoked when the panel's adapter is
        // active (an AT is listening), so the `Menu` clone is skipped entirely on
        // the hot hover/keynav path when nothing is attached (spec 30 §3).
        let root = &self.menu;
        if let Some(popup) = self.popup.as_mut() {
            a11y::sync(
                &mut popup.adapter,
                &popup.snapshot,
                || root.clone(),
                MenuFocus {
                    top: full.top,
                    flyout: full.flyout.clone(),
                },
            );
        }
        // Pre-collect the open flyouts' parent indices so each level's menu can be
        // borrowed from `self.menu` (via `descend`) while the matching panel in the
        // disjoint `self.flyouts` is mutably borrowed for its adapter.
        let parents: Vec<usize> = self.flyouts.iter().map(|f| f.parent).collect();
        for d in 0..self.flyouts.len() {
            let Some(menu) = crate::menu::descend(&self.menu, parents[..=d].iter().copied()) else {
                continue;
            };
            let top = full.flyout.get(d).and_then(|f| f.child);
            let sub: Vec<FlyoutFocus> = full.flyout.iter().skip(d + 1).copied().collect();
            if let Some(f) = self.flyouts.get_mut(d) {
                a11y::sync(
                    &mut f.panel.adapter,
                    &f.panel.snapshot,
                    || menu.clone(),
                    MenuFocus { top, flyout: sub },
                );
            }
        }
    }

    #[cfg(not(feature = "a11y"))]
    #[inline]
    fn sync_a11y(&mut self) {}

    #[cfg(feature = "a11y")]
    fn on_a11y_action(&mut self, kind: WindowKind, request: accesskit::ActionRequest) {
        let target = crate::a11y::AxId(request.target.0);
        let level = kind.menu_level();
        let Some(menu) = self.menu_at_level(level) else {
            return;
        };
        let tree = crate::a11y::build_tree(menu);
        let Some(rel_path) = crate::a11y::locate_path(&tree, target) else {
            return;
        };
        // Absolute path from the top-level menu = the parents that lead to this
        // window (levels 1..=level) followed by the in-window path.
        let mut abs: Vec<usize> = self.flyouts.iter().take(level).map(|f| f.parent).collect();
        abs.extend_from_slice(&rel_path);
        self.apply_a11y(&abs, request.action);
    }

    #[cfg(feature = "a11y")]
    fn apply_a11y(&mut self, abs: &[usize], action: accesskit::Action) {
        use accesskit::Action;
        if abs.is_empty() {
            return;
        }
        match action {
            Action::Focus => {
                // Open flyouts for every submenu ancestor along the path (all but
                // the final element), then select the final row in its window.
                let target = &abs[..abs.len() - 1];
                self.apply_flyout_stack(target);
                let final_kind = if target.is_empty() {
                    WindowKind::Popup
                } else {
                    WindowKind::Flyout(target.len() - 1)
                };
                if let (Some(&last), Some(p)) = (abs.last(), self.panel_mut(final_kind)) {
                    p.hovered = Some(last);
                }
                if let (Some(&first), Some(p)) = (abs.first(), self.popup.as_mut()) {
                    p.hovered = Some(first);
                }
                self.redraw_all();
                self.sync_a11y();
            }
            Action::Click => {
                if self.is_submenu_at_path(abs) {
                    self.apply_flyout_stack(abs);
                    if let (Some(&first), Some(p)) = (abs.first(), self.popup.as_mut()) {
                        p.hovered = Some(first);
                    }
                    self.sync_a11y();
                    return;
                }
                if let Some(id) = self.menu_id_at_path(abs) {
                    if !id.is_none() {
                        (self.dispatch)(&id);
                    }
                }
                self.close_popup();
            }
            _ => {}
        }
    }

    // -- drain ---------------------------------------------------------------

    fn apply_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::TrayClicked => {
                if self.popup.is_some() {
                    self.close_popup();
                } else {
                    self.open_popup();
                }
            }
            UiEvent::MouseMoved { kind, x, y } => {
                self.on_cursor(kind, LogicalPoint::new(x as f32, y as f32));
            }
            UiEvent::MouseExited { kind } => self.on_exit(kind),
            UiEvent::MouseDown { kind, x, y } => {
                if let Some(p) = self.panel_mut(kind) {
                    p.cursor = LogicalPoint::new(x as f32, y as f32);
                }
                self.on_click(kind);
            }
            UiEvent::Key(key) => self.on_key_nav(key),
            UiEvent::FocusChanged { kind, key } => {
                if key {
                    self.focused.insert(kind);
                    self.dismiss_armed = false;
                } else {
                    self.focused.remove(&kind);
                    if self.focused.is_empty() {
                        self.dismiss_armed = true;
                    }
                }
            }
            UiEvent::OutsideClick { x, y } => self.on_outside_click(x, y),
            UiEvent::Dismiss => {
                if self.popup.is_some() {
                    self.close_popup();
                }
            }
            #[cfg(feature = "a11y")]
            UiEvent::A11yAction { kind, request } => self.on_a11y_action(kind, request),
        }
    }

    /// Every open panel's on-screen frame as `(min_x, min_y, max_x, max_y)` in
    /// AppKit global (bottom-left) screen coordinates — the hit-test set for a
    /// click-away decision.
    fn panel_screen_frames(&self) -> Vec<(f64, f64, f64, f64)> {
        let mut frames = Vec::with_capacity(1 + self.flyouts.len());
        let mut push = |panel: &Panel| {
            let f = panel.panel.frame();
            frames.push((
                f.origin.x,
                f.origin.y,
                f.origin.x + f.size.width,
                f.origin.y + f.size.height,
            ));
        };
        if let Some(p) = self.popup.as_ref() {
            push(p);
        }
        for fl in &self.flyouts {
            push(&fl.panel);
        }
        frames
    }

    /// A monitored mouse-down at a global screen point: dismiss the whole stack
    /// unless it landed inside an open panel (handled as a row/flyout click) or on
    /// the tray button (whose own toggle owns that click — dismissing here would
    /// close-then-reopen). The non-activating panel never gets `resignKey` for
    /// these clicks, so this monitor-driven path is what actually closes it (#11).
    fn on_outside_click(&mut self, x: f64, y: f64) {
        if self.popup.is_none() {
            return;
        }
        let mut frames = self.panel_screen_frames();
        if let Anchor::Tray(a) = &self.anchor {
            if let Some(g) = a.geometry() {
                let bf = g.button_frame;
                frames.push((
                    bf.origin.x,
                    bf.origin.y,
                    bf.origin.x + bf.size.width,
                    bf.origin.y + bf.size.height,
                ));
            }
        }
        if point_outside_all(&frames, x, y) {
            self.close_popup();
        }
    }

    /// After a drain, dismiss the whole stack iff a resign-key armed it and no
    /// muri panel regained focus. Returns whether it dismissed.
    fn finalize_dismiss(&mut self) -> bool {
        // DEVICE-VERIFY(0.9.0): focus-loss dismissal on a real display —
        // requires the non-activating panel's key transitions to fire as
        // expected across outside clicks and within-stack transfers.
        let dismissed = self.dismiss_armed && self.focused.is_empty() && self.popup.is_some();
        if dismissed {
            self.close_popup();
        }
        self.dismiss_armed = false;
        dismissed
    }
}

/// The tray's run-loop state: the shared [`PopupSession`] plus the tray icon /
/// tooltip / command bits specific to the persistent tray surface.
struct AppState {
    session: PopupSession<'static>,
    tray: Tray,
    /// True only when muri owns the blocking `NSApplication::run` loop (the
    /// [`Tray::run`] path). A spawned tray installs on the host's main-thread run
    /// loop, which muri must never stop, so this stays false there — it gates the
    /// `Shutdown` arm's `app.stop()` so `run()` returns on shutdown without
    /// tearing down a host-owned loop (#47).
    owns_run_loop: bool,
}

impl AppState {
    fn apply_command(&mut self, command: TrayCommand) {
        match command {
            TrayCommand::SetMenu(menu) => {
                self.tray.menu = menu.clone();
                self.session.menu = menu;
                if self.session.popup.is_some() {
                    // The structure may have changed under an open flyout; drop
                    // it, then repaint + refresh the a11y tree live.
                    self.session.truncate_flyouts(0);
                    self.session.redraw(WindowKind::Popup);
                    self.session.sync_a11y();
                }
            }
            TrayCommand::SetIcon(icon) => {
                self.tray.icon = icon;
                let icon = self.tray.icon.clone();
                let title = self.tray.title.clone();
                let tooltip = self.tray.tooltip.clone();
                if let Anchor::Tray(a) = &self.session.anchor {
                    a.set_status(&icon, title.as_deref(), tooltip.as_deref());
                }
            }
            TrayCommand::SetTooltip(tooltip) => {
                self.tray.tooltip = tooltip;
                if let Anchor::Tray(a) = &self.session.anchor {
                    a.set_tooltip(self.tray.tooltip.as_deref());
                }
            }
            TrayCommand::SetTitle(title) => {
                self.tray.title = title;
                let icon = self.tray.icon.clone();
                let title = self.tray.title.clone();
                let tooltip = self.tray.tooltip.clone();
                if let Anchor::Tray(a) = &self.session.anchor {
                    a.set_status(&icon, title.as_deref(), tooltip.as_deref());
                }
            }
            TrayCommand::SetVisible(visible) => {
                if let Anchor::Tray(a) = &self.session.anchor {
                    a.set_visible(visible);
                }
            }
            TrayCommand::Open => {
                if self.session.popup.is_none() {
                    self.session.open_popup();
                }
            }
            TrayCommand::Close => self.session.close_popup(),
            TrayCommand::Shutdown => {
                // Remove the status item explicitly: the spawned-tray AppState is
                // parked in MAIN_APP for the process lifetime, so MacosAnchor::Drop
                // never runs. Also dismiss any open popup.
                self.session.close_popup();
                if let Anchor::Tray(a) = &mut self.session.anchor {
                    a.remove();
                }
                if self.owns_run_loop {
                    // muri owns the blocking `NSApplication::run` (the `Tray::run`
                    // path): stop it so `run()` returns on shutdown — the #47
                    // contract, which the Windows (WM_QUIT) and Linux (run_sni_loop)
                    // backends already honor. A spawned tray runs on the host's
                    // loop (owns_run_loop == false) and is left untouched.
                    // DEVICE-VERIFY(0.10.6): live run-loop teardown on a real NSApp.
                    stop_run_loop(self.session.mtm);
                }
            }
            TrayCommand::SetTheme(theme) => {
                self.session.options.theme = theme;
                self.repaint_open_popup();
            }
            TrayCommand::SetOptions(options) => {
                self.session.options = options;
                self.repaint_open_popup();
            }
            TrayCommand::QueryAnchorRect(reply) => {
                let rect = match &self.session.anchor {
                    Anchor::Tray(a) => a.anchor_rect().ok(),
                    _ => None,
                };
                let _ = reply.send(rect);
            }
        }
    }

    /// Repaint an already-open popup after a live theme/options swap (#45), so an
    /// in-menu theme switcher redraws instantly instead of only on next open.
    fn repaint_open_popup(&mut self) {
        if self.session.popup.is_some() {
            self.session.truncate_flyouts(0);
            self.session.redraw(WindowKind::Popup);
            self.session.sync_a11y();
        }
    }

    /// Apply all pending commands and UI events, looping until both inboxes are
    /// empty (applying an event can enqueue more, e.g. a become-key). Then run the
    /// armed focus-loss dismissal check.
    fn drain(&mut self) {
        loop {
            let events = EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
            let commands: Vec<TrayCommand> = self
                .tray
                .commands
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default();
            if events.is_empty() && commands.is_empty() {
                break;
            }
            for command in commands {
                self.apply_command(command);
            }
            for event in events {
                self.session.apply_event(event);
            }
        }
        self.session.finalize_dismiss();
    }
}

// =============================================================================
// Platform seam (ADR-0002)
// =============================================================================

/// The macOS [`Platform`] implementation: an `NSStatusItem` tray anchor plus the
/// native non-activating `NSPanel` popup + flyout event loop, the CALayer
/// present path, the per-window AccessKit adapter (behind `a11y`), and the
/// appearance/work-area queries. Every macOS-specific dependency (objc2,
/// objc2-quartz-core, objc2-core-graphics, accesskit_macos) lives in this module
/// behind the [`Platform`] trait — no AppKit type crosses the seam.
pub struct MacPlatform {
    mtm: Option<MainThreadMarker>,
    anchor: Option<MacosAnchor>,
}

impl Default for MacPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl MacPlatform {
    /// Create the macOS platform handle. Captures a [`MainThreadMarker`] if
    /// called on the main thread; the tray methods and [`Platform::run_tray`]
    /// error out otherwise.
    pub fn new() -> Self {
        MacPlatform {
            mtm: MainThreadMarker::new(),
            anchor: None,
        }
    }

    fn require_mtm(&self) -> Result<MainThreadMarker> {
        self.mtm
            .ok_or(Error::Platform("must be created on the main thread".into()))
    }
}

impl Platform for MacPlatform {
    fn install_tray(&mut self, icon: &Icon, tooltip: Option<&str>) -> Result<()> {
        let mtm = self.require_mtm()?;
        let mut anchor = MacosAnchor::new(mtm);
        anchor.install(tooltip)?;
        anchor.set_status(icon, None, tooltip);
        self.anchor = Some(anchor);
        Ok(())
    }

    fn tray_anchor_rect(&self) -> Result<LogicalRect> {
        self.anchor
            .as_ref()
            .ok_or_else(|| Error::Platform("tray not installed".into()))?
            .anchor_rect()
    }

    fn supports_tray_anchor(&self) -> bool {
        true
    }

    fn cursor_position(&self) -> Option<LogicalPoint> {
        // `NSEvent::mouseLocation` is global screen coords, **bottom-left** origin;
        // invert the exact top-left→bottom-left mapping `AnchorGeometry::for_rect`
        // uses (relative to the main screen) so the result is in the same top-left
        // logical space `ContextMenu::open_at` consumes.
        // DEVICE-VERIFY(0.10.7): main-screen resolution; a pointer on a secondary
        // display shares the tray anchor's multi-monitor flip fragility.
        let mtm = self.require_mtm().ok()?;
        let p = NSEvent::mouseLocation();
        let sf = NSScreen::screens(mtm).firstObject()?.frame();
        Some(LogicalPoint::new(
            (p.x - sf.origin.x) as f32,
            (sf.origin.y + sf.size.height - p.y) as f32,
        ))
    }

    fn appearance(&self) -> Appearance {
        Appearance::from_is_dark(system_is_dark())
    }

    fn system_menu_font(&self) -> Option<crate::platform::SystemFont> {
        self.require_mtm().ok()?;
        read_system_menu_font()
    }

    fn system_palette(&self) -> crate::platform::SystemPalette {
        if self.require_mtm().is_err() {
            return crate::platform::SystemPalette::default();
        }
        read_system_palette()
    }

    fn work_area(&self) -> LogicalRect {
        self.anchor
            .as_ref()
            .and_then(|a| a.work_area().ok())
            .unwrap_or_else(|| {
                LogicalRect::new(LogicalPoint::new(0.0, 0.0), LogicalSize::new(1440.0, 900.0))
            })
    }

    fn run_tray(self, tray: Tray) -> Result<()> {
        run_event_loop(tray)
    }

    fn spawn_tray(self, tray: Tray) -> Result<()> {
        // Best-effort (spec: the `tray-icon` facade contract). AppKit's status
        // item must live on the main thread and be serviced by an
        // `NSApplication` run loop, and a background thread cannot own either —
        // so install on the main thread and hand the loop back to the host
        // (which a real GUI app already runs). Unlike `run_tray` this neither
        // calls `app.run()` nor forces the Accessory activation policy, so it
        // composes with the host's existing app. If called off the main thread
        // there is nothing safe to do, so the main-thread requirement is
        // surfaced as an error.
        let mtm = self.require_mtm()?;
        // Host owns the run loop here, so `owns_run_loop = false` — Shutdown must
        // not stop the host's `NSApplication`.
        install_tray_session(tray, mtm, false)
    }

    fn open_popup_session(
        &mut self,
        menu: Menu,
        options: MenuOptions,
        on_click: &(dyn Fn(&MenuId) + '_),
        anchor: LogicalRect,
        edge: Edge,
    ) -> Result<()> {
        let mtm = self.require_mtm()?;
        run_popup_session(mtm, menu, options, on_click, anchor, edge)
    }
}

/// Stop muri's *owned* `NSApplication::run` loop (the [`Tray::run`] path) so it
/// returns on `Shutdown`. `stop()` only takes effect after the next event is
/// dequeued, so an application-defined no-op event is posted to wake
/// `app.run()`'s internal `nextEventMatchingMask` immediately. Only ever called
/// when muri owns the loop — never for a spawned tray on the host's loop (#47).
fn stop_run_loop(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    // An application-defined no-op event, posted to the front of the main thread's
    // queue so `app.run()`'s `nextEventMatchingMask` wakes and re-checks the stop
    // flag (we are on the main thread — `mtm`).
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(event) = event {
        app.postEvent_atStart(&event, true);
    }
}

/// Install the tray icon and run the native `NSApplication` loop, opening the
/// styled popup on click and dispatching row clicks to the tray's handler.
/// Consumes the [`Tray`]; returns when the loop exits.
fn run_event_loop(tray: Tray) -> Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or(Error::Platform("must be created on the main thread".into()))?;

    // A tray-only native app is an Accessory: no Dock icon, no app menu bar
    // (spec 20 §5). The muda-compat menu-bar path chooses Regular elsewhere.
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    install_tray_session(tray, mtm, true)?;

    app.run();
    Ok(())
}

/// Install the status item, wire the click dispatch + [`TrayHandle`] waker, and
/// publish the retained [`AppState`] into the thread-local `MAIN_APP` slot —
/// everything [`run_event_loop`] does *except* owning the run loop
/// (`NSApplication::run`) and setting the activation policy.
///
/// Split out so the non-blocking [`Platform::spawn_tray`] path can install the
/// tray on the main thread and hand the run loop back to the host (the
/// `tray-icon` facade contract), while [`run_event_loop`] keeps driving the loop
/// itself. The retained `AppState` lives in the `MAIN_APP` thread-local (the main
/// thread lives for the process), so the status item persists after this returns.
fn install_tray_session(mut tray: Tray, mtm: MainThreadMarker, owns_run_loop: bool) -> Result<()> {
    let mut anchor = MacosAnchor::new(mtm);
    anchor.install(tray.tooltip.as_deref())?;
    anchor.set_status(&tray.icon, tray.title.as_deref(), tray.tooltip.as_deref());

    // Install the TrayHandle waker so posts from any thread schedule a drain.
    if let Ok(mut waker) = tray.waker.lock() {
        *waker = Some(Box::new(defer_drain));
    }

    // Move the click handler into the session's dispatch sink (owned for the
    // whole run loop, hence `'static`). Preserve `Tray::dispatch`'s behavior:
    // run the per-surface handler, then project the activation onto the global
    // `MenuEvent` channel — the muda-compat door has no `on_click` and consumes
    // clicks via `MenuEvent::receiver()`, so without the `emit` every facade menu
    // item is inert on macOS (#13). Mirrors the Windows pump (spec 03 §3).
    let surface = tray.surface_id;
    let dispatch: Box<dyn Fn(&MenuId) + 'static> = match tray.on_click.take() {
        Some(handler) => Box::new(move |id| {
            handler(id);
            crate::event::emit(id.clone(), surface);
        }),
        None => Box::new(move |id| crate::event::emit(id.clone(), surface)),
    };
    let session = PopupSession {
        mtm,
        menu: tray.menu.clone(),
        options: tray.options.clone(),
        dispatch,
        anchor: Anchor::Tray(anchor),
        edge: Edge::Bottom,
        popup: None,
        flyouts: Vec::new(),
        focused: HashSet::new(),
        dismiss_armed: false,
        watchers: None,
    };

    let state = Rc::new(RefCell::new(AppState {
        session,
        tray,
        owns_run_loop,
    }));
    MAIN_APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&state)));

    // Apply any commands a handle posted before the loop came up.
    defer_drain();

    Ok(())
}

/// Run a pointer/rect-anchored [`ContextMenu`](crate::ContextMenu) /
/// [`Popup`](crate::Popup) session to completion (spec 20 §3): open the styled
/// popup at `anchor`, then pump AppKit events — draining muri UI events after each
/// — until the whole stack dismisses. Reuses the shared [`PopupSession`]; the only
/// difference from the tray is that the anchor is a fixed rect and the handler is
/// borrowed for the call rather than owned for a run loop.
///
/// Leaves the app's activation policy untouched (a context menu runs inside the
/// host's existing app — spec 20 §5, "neither surface installed").
fn run_popup_session(
    mtm: MainThreadMarker,
    menu: Menu,
    options: MenuOptions,
    on_click: &(dyn Fn(&MenuId) + '_),
    anchor: LogicalRect,
    edge: Edge,
) -> Result<()> {
    let app = NSApplication::sharedApplication(mtm);
    let geom = AnchorGeometry::for_rect(mtm, anchor)
        .ok_or_else(|| Error::Platform("no screen available for the popup".into()))?;

    let mut session = PopupSession {
        mtm,
        menu,
        options,
        dispatch: Box::new(move |id| on_click(id)),
        anchor: Anchor::Fixed(geom),
        edge,
        popup: None,
        flyouts: Vec::new(),
        focused: HashSet::new(),
        dismiss_armed: false,
        watchers: None,
    };
    session.open_popup();
    if session.popup.is_none() {
        return Err(Error::Platform("failed to open the popup window".into()));
    }

    // DEVICE-VERIFY(0.9.0): scoped modal event pump. Unlike the tray, this drives
    // the popup with a bounded `nextEventMatchingMask:` loop (no persistent run
    // loop, no GCD drain) so `open_at`/`anchored_to` can block on the caller's
    // stack and return when the menu dismisses. AppKit callbacks still enqueue
    // into the shared `EVENTS` inbox, which we drain here after each native event.
    loop {
        let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&objc2_foundation::NSDate::distantFuture()),
            &objc2_foundation::NSString::from_str("kCFRunLoopDefaultMode"),
            true,
        );
        if let Some(event) = event {
            app.sendEvent(&event);
        }
        let events = EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
        for e in events {
            session.apply_event(e);
        }
        session.finalize_dismiss();
        if session.popup.is_none() {
            break;
        }
    }
    Ok(())
}

// =============================================================================
// System appearance
// =============================================================================

/// Query whether the system (menu-bar) appearance is currently dark.
/// Rasterize an `Icon::Svg` (muri's restricted SVG subset, via the zeno-backed
/// rasterizer) and re-encode it as PNG so AppKit's `NSImage` — which decodes PNG
/// but not the SVG subset — can consume it. `None` for non-SVG / unparseable
/// bytes.
fn svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let (rgba, w, h) = crate::render::rasterize_svg(bytes)?;
    crate::render::encode_rgba_png(&rgba, w, h)
}

/// Extra tracking (letter-spacing) in **logical points** for the macOS System
/// theme's UI font at `size_pt`, approximating the small size-dependent tracking
/// CoreText applies to San Francisco that a bare shaper does not (#42). muri's
/// menu text otherwise reads slightly *looser* than a native `NSMenu`, so this is
/// a slight **tightening** (negative). Applied only to `System` themes (real SF) —
/// never to forced/preset/custom themes, whose faces aren't SF.
///
/// DEVICE-VERIFY(0.10.8): the exact factor needs a side-by-side capture against a
/// real `NSMenu`. It is deliberately a single, conservative, easily-tuned constant
/// over the narrow menu-size range (11–14pt) rather than a full optical-size table.
///
/// This delegates to [`crate::theme::macos_sf_tracking`] — the single source of
/// truth (#57) shared with the forced `Theme::macos` preset, so the live-system
/// read path and a forced macOS theme compute *identical* tracking for the same
/// size. The forced preset already bakes tracking in for its 13pt base; this path
/// recomputes it for the live menu size and OVERWRITES (assign, not `+=`) so the
/// System theme tracks for the live size without ever double-applying.
fn sf_ui_tracking(size_pt: f32) -> f32 {
    crate::theme::macos_sf_tracking(size_pt)
}

/// Resolve the bold companion of the system menu `font` and, when it is a
/// genuinely distinct on-disk face, pack the regular+bold file bytes into a
/// single [`SystemFontSource`] (issue #56) so the render layer registers a real
/// bold face rather than silently downgrading bold rows to the regular file.
///
/// `NSFontManager::convertFont:toHaveTrait:` returns the *original* font when no
/// bold variant exists, so we only pack when the bold face resolves to a
/// different file than `regular_path`. `None` (→ caller keeps the single regular
/// `Path`) when there is no distinct bold face or either file can't be read.
///
/// DEVICE-VERIFY(0.10.8): confirm bold menu rows render with the real SF bold
/// face on a physical device.
fn dual_face_source(
    mtm: MainThreadMarker,
    font: &objc2_app_kit::NSFont,
    regular_path: &str,
) -> Option<crate::platform::SystemFontSource> {
    let manager = objc2_app_kit::NSFontManager::sharedFontManager(mtm);
    let bold = manager.convertFont_toHaveTrait(font, objc2_app_kit::NSFontTraitMask::BoldFontMask);
    let bold_path = system_ui_font_path(&bold)?;
    if bold_path == regular_path {
        return None;
    }
    let regular_bytes = std::fs::read(regular_path).ok()?;
    let bold_bytes = std::fs::read(&bold_path).ok()?;
    Some(crate::render::pack_dual_face(regular_bytes, bold_bytes))
}

/// Read the system menu font (`+[NSFont menuFontOfSize:0]`) as a [`SystemFont`],
/// resolving the real SF file via CoreText's URL attribute so fontdb loads the
/// actual system face (falling back to the family name if the file has no URL).
/// `None` off the main thread or when neither a file nor a family resolves.
/// Callers must already be on the main thread (AppKit).
fn read_system_menu_font() -> Option<crate::platform::SystemFont> {
    use crate::platform::{SystemFont, SystemFontSource};
    let mtm = MainThreadMarker::new()?;
    let font = objc2_app_kit::NSFont::menuFontOfSize(0.0);
    let point_size = font.pointSize() as f32;
    let source = match system_ui_font_path(&font) {
        // #56: the macOS system menu font resolves to a single regular file, so a
        // bold row would otherwise silently downgrade (fontdb has no bold face to
        // pick). When a distinct bold variant exists on disk, pack regular+bold
        // into one source so the render layer registers a real bold face; fall
        // back to the single regular file when there is no distinct bold.
        Some(path) => dual_face_source(mtm, &font, &path)
            .unwrap_or_else(|| SystemFontSource::Path(std::path::PathBuf::from(path))),
        None => {
            let family = font.familyName()?.to_string();
            if family.is_empty() {
                return None;
            }
            SystemFontSource::Family(family)
        }
    };
    Some(SystemFont { source, point_size })
}

/// Read the live macOS menu text colors (`labelColor`/`secondaryLabelColor`/
/// `separatorColor`) under the current appearance. Background is left unset so
/// the theme's vibrancy translucency is preserved. Empty off the main thread.
fn read_system_palette() -> crate::platform::SystemPalette {
    let mut pal = crate::platform::SystemPalette::default();
    if MainThreadMarker::new().is_none() {
        return pal;
    }
    pal.label = nscolor_srgba(&NSColor::labelColor());
    pal.secondary_label = nscolor_srgba(&NSColor::secondaryLabelColor());
    pal.separator = nscolor_srgba(&NSColor::separatorColor());
    pal
}

/// Convert an `NSColor` to straight-alpha sRGB `(r, g, b, a)`, or `None` if it
/// can't be represented in sRGB. Shared by the palette reads.
fn nscolor_srgba(color: &NSColor) -> Option<(u8, u8, u8, u8)> {
    let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let to_u8 = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some((
        to_u8(srgb.redComponent()),
        to_u8(srgb.greenComponent()),
        to_u8(srgb.blueComponent()),
        to_u8(srgb.alphaComponent()),
    ))
}

fn system_is_dark() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    let name = app.effectiveAppearance().name();
    name.to_string().to_lowercase().contains("dark")
}

/// Whether OS transparency/vibrancy is enabled — i.e. the "Reduce transparency"
/// accessibility setting is **off**. When it's on, macOS menus render opaque, so
/// the theme drops its translucent background to match. `NSWorkspace`'s
/// `accessibilityDisplayShouldReduceTransparency` is the canonical source.
fn transparency_enabled() -> bool {
    use objc2_app_kit::NSWorkspace;
    if MainThreadMarker::new().is_none() {
        return true;
    }
    let ws = NSWorkspace::sharedWorkspace();
    !ws.accessibilityDisplayShouldReduceTransparency()
}

/// Query the live OS accent color as straight-alpha RGBA, or `None` if it can't
/// be resolved. Fed into the theme's `accent` so `Color::Accent` follows the
/// system (spec 20 §4c).
fn system_accent(_mtm: MainThreadMarker) -> Option<(u8, u8, u8, u8)> {
    let accent = NSColor::controlAccentColor();
    let srgb = accent.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let to_u8 = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some((
        to_u8(srgb.redComponent()),
        to_u8(srgb.greenComponent()),
        to_u8(srgb.blueComponent()),
        to_u8(srgb.alphaComponent()),
    ))
}

#[cfg(test)]
mod font_tests {
    use super::system_ui_font_path;

    /// On macOS the system menu font must resolve to a real on-disk file via
    /// CoreText's URL attribute (so fontdb loads the actual SF face instead of
    /// falling back to the bundled UI font). The file is under a system font
    /// directory and is a real font container.
    #[test]
    fn system_menu_font_resolves_to_a_real_sf_file() {
        let font = objc2_app_kit::NSFont::menuFontOfSize(0.0);
        let path =
            system_ui_font_path(&font).expect("the macOS system menu font has an on-disk URL");
        assert!(
            path.starts_with("/System/") || path.starts_with("/Library/"),
            "expected a system font path, got {path}"
        );
        let lower = path.to_ascii_lowercase();
        assert!(
            lower.ends_with(".ttf")
                || lower.ends_with(".ttc")
                || lower.ends_with(".otf")
                || lower.ends_with(".otc"),
            "expected a font-file extension, got {path}"
        );
        assert!(
            std::path::Path::new(&path).exists(),
            "resolved font file must exist on disk: {path}"
        );
    }

    /// The live macOS menu palette read resolves the primary text color on this
    /// host (proving the `NSColor` → sRGB path works). Runs on whatever thread
    /// the test harness uses; if that isn't the main thread the read yields an
    /// empty palette, which we tolerate rather than assert a false negative.
    #[test]
    fn system_palette_reads_label_when_on_main_thread() {
        use objc2::MainThreadMarker;
        let pal = super::read_system_palette();
        if MainThreadMarker::new().is_some() {
            assert!(
                pal.label.is_some(),
                "on the main thread the label color must resolve"
            );
            // A fully-opaque or translucent color, but a real sRGB triple.
            let (_r, _g, _b, a) = pal.label.unwrap();
            assert!(a > 0, "label color must not be fully transparent");
        }
    }
}

#[cfg(test)]
mod dismiss_tests {
    use super::point_outside_all;

    /// The click-away hit-test: a mouse-down inside any open panel (or the tray
    /// button) keeps the menu open; anywhere else dismisses it (#11).
    #[test]
    fn point_outside_all_matches_panel_frames() {
        // A popup frame and a flyout frame to its right, in AppKit screen coords.
        let frames = [(100.0, 100.0, 300.0, 400.0), (300.0, 200.0, 480.0, 380.0)];
        // Inside the popup / the flyout → not outside → keep open.
        assert!(!point_outside_all(&frames, 150.0, 250.0));
        assert!(!point_outside_all(&frames, 400.0, 300.0));
        // A shared edge counts as inside (inclusive bounds).
        assert!(!point_outside_all(&frames, 300.0, 300.0));
        // In the desktop gap above / beside both → dismiss.
        assert!(point_outside_all(&frames, 150.0, 50.0));
        assert!(point_outside_all(&frames, 600.0, 300.0));
        // No open panels → every click is outside.
        assert!(point_outside_all(&[], 150.0, 250.0));
    }
}

#[cfg(test)]
mod descend_tests {
    use crate::menu::{Item, Menu, Row};

    fn first_row_id(menu: &Menu) -> &str {
        match &menu.items[0] {
            Item::Row(row) => row.id.as_str(),
            other => panic!("expected a row, got {other:?}"),
        }
    }

    /// `menu_at_level`/`descend` must borrow nested submenus straight out of the
    /// source `Menu` (it runs on every redraw) rather than cloning the tree.
    #[test]
    fn descend_borrows_nested_submenus_without_cloning() {
        let grandchild = Menu::new().row(Row::new("c").label("C"));
        let child = Menu::new()
            .row(Row::new("b").label("B"))
            .submenu(Row::new("sub2").label("Sub2"), grandchild);
        let top = Menu::new()
            .row(Row::new("a").label("A"))
            .submenu(Row::new("sub1").label("Sub1"), child);

        // Level 0 is the top menu itself.
        assert_eq!(
            first_row_id(crate::menu::descend(&top, [0usize; 0]).unwrap()),
            "a"
        );
        // Descending the submenu at index 1 yields the child menu.
        assert_eq!(first_row_id(crate::menu::descend(&top, [1]).unwrap()), "b");
        // Two levels deep reaches the grandchild.
        assert_eq!(
            first_row_id(crate::menu::descend(&top, [1, 1]).unwrap()),
            "c"
        );
        // A non-submenu row on the path -> None.
        assert!(crate::menu::descend(&top, [0]).is_none());
        // An out-of-range index -> None.
        assert!(crate::menu::descend(&top, [9]).is_none());

        // The returned reference points *into* `top` (no clone).
        let borrowed = crate::menu::descend(&top, [1]).unwrap();
        let Item::Submenu { menu, .. } = &top.items[1] else {
            panic!("index 1 should be a submenu");
        };
        assert!(std::ptr::eq(borrowed, menu));
    }
}
