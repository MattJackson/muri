//! macOS tray anchoring via `NSStatusItem`, plus the live tray + popup event
//! loop.
//!
//! The status item's `button` is both the click target and the anchor rect;
//! AppKit reports its on-screen frame. The shared raster surface is painted into
//! a borderless `winit` window (via `softbuffer`) positioned under that rect, and
//! transient dismiss uses window focus-loss. AppKit is the *anchor*, not the
//! renderer.
//!
//! A small Objective-C target subclass (defined with `objc2`) receives the
//! status-item button click and forwards it to the `winit` event loop through an
//! [`EventLoopProxy`].

#![allow(unsafe_code)]

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSImage, NSScreen, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSSize, NSString};

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize as WinitLogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId, WindowLevel};

use crate::error::{Error, Result};
use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use crate::menu::Icon;
use crate::render::paint::{render_menu, LaidMenu};
use crate::render::RasterDrawer;
use crate::theme::Theme;
use crate::tray::TrayAnchor;
use crate::Tray;

/// User events posted to the winit loop from the AppKit status-item click.
#[derive(Debug, Clone)]
enum UserEvent {
    /// The tray icon was clicked — toggle the popup.
    ToggleTray,
}

static TRAY_PROXY: OnceLock<EventLoopProxy<UserEvent>> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MuriTrayTarget"]
    #[thread_kind = MainThreadOnly]
    struct TrayTarget;

    impl TrayTarget {
        #[unsafe(method(trayClicked:))]
        fn tray_clicked(&self, _sender: Option<&AnyObject>) {
            if let Some(proxy) = TRAY_PROXY.get() {
                let _ = proxy.send_event(UserEvent::ToggleTray);
            }
        }
    }
);

impl TrayTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![super(mtm.alloc::<Self>().set_ivars(())), init] }
    }
}

/// The macOS `NSStatusItem`-based anchor. Owns the status item and the click
/// target once [`install`](TrayAnchor::install)ed.
pub struct MacosAnchor {
    mtm: MainThreadMarker,
    status_item: Option<Retained<NSStatusItem>>,
    // Kept alive so the button's target isn't deallocated.
    _target: Option<Retained<TrayTarget>>,
}

impl std::fmt::Debug for MacosAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacosAnchor")
            .field("installed", &self.status_item.is_some())
            .finish()
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

    /// Set the status-item button's image from a muri [`Icon`]. PNG icons are
    /// decoded by AppKit; other kinds fall back to a text title.
    fn set_icon(&self, icon: &Icon, tooltip: Option<&str>) {
        let Some(item) = &self.status_item else { return };
        let Some(button) = item.button(self.mtm) else {
            return;
        };
        match icon {
            Icon::Png(bytes) | Icon::Svg(bytes) => {
                let data = NSData::with_bytes(bytes);
                if let Some(image) =
                    NSImage::initWithData(NSImage::alloc(), &data).filter(|i| i.isValid())
                {
                    image.setSize(NSSize::new(18.0, 18.0));
                    button.setImage(Some(&image));
                } else {
                    button.setTitle(&NSString::from_str("●"));
                }
            }
            _ => button.setTitle(&NSString::from_str("●")),
        }
        if let Some(tip) = tooltip {
            button.setToolTip(Some(&NSString::from_str(tip)));
        }
    }
}

impl TrayAnchor for MacosAnchor {
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
        let item = self
            .status_item
            .as_ref()
            .ok_or_else(|| Error::Platform("status item not installed".into()))?;
        let button = item
            .button(self.mtm)
            .ok_or_else(|| Error::Platform("status item has no button".into()))?;
        let window = button
            .window()
            .ok_or_else(|| Error::Platform("status button has no window".into()))?;
        let frame = window.frame(); // screen coords, bottom-left origin
                                    // The menu bar lives on the primary screen; use its height to flip Y into
                                    // winit's top-left coordinate space.
        let screen_h = NSScreen::screens(self.mtm)
            .firstObject()
            .map(|s| s.frame().size.height)
            .unwrap_or(0.0);
        let top = screen_h - (frame.origin.y + frame.size.height);
        Ok(LogicalRect::new(
            LogicalPoint::new(frame.origin.x as f32, top as f32),
            LogicalSize::new(frame.size.width as f32, frame.size.height as f32),
        ))
    }

    fn supports_tray_anchor(&self) -> bool {
        true
    }
}

/// Install the tray icon and run the winit event loop, opening the styled popup
/// on click and dispatching row clicks to the tray's handler. Consumes the
/// [`Tray`]; returns when the loop exits.
pub fn run_tray(tray: Tray) -> Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| Error::Platform(format!("event loop: {e}")))?;
    let _ = TRAY_PROXY.set(event_loop.create_proxy());

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| Error::Platform("Tray::run must be called on the main thread".into()))?;

    // A status-bar app shouldn't own a Dock icon or a main menu.
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let mut anchor = MacosAnchor::new(mtm);
    anchor.install(tray.tooltip.as_deref())?;
    anchor.set_icon(&tray.icon, tray.tooltip.as_deref());

    let mut app_state = App {
        tray,
        anchor,
        window: None,
        surface: None,
        context: None,
        drawer: RasterDrawer::new(2.0),
        laid: None,
        hovered: None,
        cursor: LogicalPoint::default(),
    };

    event_loop
        .run_app(&mut app_state)
        .map_err(|e| Error::Platform(format!("run loop: {e}")))
}

struct App {
    tray: Tray,
    anchor: MacosAnchor,
    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    context: Option<Context<Rc<Window>>>,
    drawer: RasterDrawer,
    laid: Option<LaidMenu>,
    hovered: Option<usize>,
    cursor: LogicalPoint,
}

impl App {
    fn theme(&self) -> Theme {
        // FollowSystem: query the live menu-bar appearance.
        let dark = self.tray.options.theme.wants_dark(system_is_dark);
        self.tray.options.theme.resolve_theme(dark)
    }

    fn is_open(&self) -> bool {
        self.window.is_some()
    }

    fn close_popup(&mut self) {
        self.surface = None;
        self.context = None;
        self.window = None;
        self.laid = None;
        self.hovered = None;
    }

    fn open_popup(&mut self, event_loop: &ActiveEventLoop) {
        if self.is_open() {
            return;
        }
        let theme = self.theme();
        // Measure the menu offscreen to size the window before creating it.
        let scale = 2.0;
        let mut probe = RasterDrawer::new(scale);
        let laid = render_menu(
            &mut probe,
            &self.tray.menu,
            &theme,
            &self.tray.options,
            None,
        );

        let anchor = self.anchor.anchor_rect().unwrap_or_default();
        // Open below the status item, right edge roughly aligned to the icon.
        let mut px = anchor.origin.x;
        let py = anchor.origin.y + anchor.size.height + 2.0;
        // Clamp to the primary screen work area so it never spills off-screen.
        if let Some(screen) = NSScreen::screens(self.anchor.mtm).firstObject() {
            let sw = screen.frame().size.width as f32;
            if px + laid.size.width > sw {
                px = (sw - laid.size.width - 4.0).max(4.0);
            }
        }

        let attrs = Window::default_attributes()
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(WinitLogicalSize::new(laid.size.width, laid.size.height))
            .with_position(LogicalPosition::new(px, py));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(_) => return,
        };
        let context = match Context::new(window.clone()) {
            Ok(c) => c,
            Err(_) => return,
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(_) => return,
        };

        self.drawer = RasterDrawer::new(window.scale_factor() as f32);
        self.window = Some(window.clone());
        self.context = Some(context);
        self.surface = Some(surface);
        self.laid = Some(laid);
        self.hovered = None;
        window.request_redraw();
    }

    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface.as_mut()) else {
            return;
        };
        let theme = self.tray.options.theme.resolve_theme(
            self.tray
                .options
                .theme
                .wants_dark(system_is_dark),
        );
        let laid = render_menu(
            &mut self.drawer,
            &self.tray.menu,
            &theme,
            &self.tray.options,
            self.hovered,
        );

        let (dw, dh) = self.drawer.device_size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(dw), NonZeroU32::new(dh)) else {
            return;
        };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        // Blit the tiny-skia pixmap (premultiplied RGBA) into softbuffer's
        // 0RGB u32 words over an opaque theme background (softbuffer is opaque).
        let bg = theme.resolve(theme.background);
        for (dst, px) in buffer.iter_mut().zip(self.drawer.pixmap().pixels().iter()) {
            let a = px.alpha() as u32;
            let inv = 255 - a;
            // Un-premultiply the source over the opaque background.
            let (sr, sg, sb) = (px.red() as u32, px.green() as u32, px.blue() as u32);
            let r = (sr + bg.r as u32 * inv / 255).min(255);
            let g = (sg + bg.g as u32 * inv / 255).min(255);
            let b = (sb + bg.b as u32 * inv / 255).min(255);
            *dst = (r << 16) | (g << 8) | b;
        }
        let _ = buffer.present();
        self.laid = Some(laid);
        window.request_redraw();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ToggleTray => {
                if self.is_open() {
                    self.close_popup();
                } else {
                    self.open_popup(event_loop);
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                // Dismiss on click-outside / focus loss.
                self.close_popup();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let logical = self.to_logical(position);
                self.cursor = logical;
                let hovered = self.laid.as_ref().and_then(|l| l.hit(logical));
                if hovered != self.hovered {
                    self.hovered = hovered;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(id) = self.laid.as_ref().and_then(|l| l.id_at(self.cursor)) {
                    if !id.is_none() {
                        self.tray.dispatch(&id);
                    }
                    self.close_popup();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

impl App {
    fn to_logical(&self, position: PhysicalPosition<f64>) -> LogicalPoint {
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor())
            .unwrap_or(1.0);
        LogicalPoint::new((position.x / scale) as f32, (position.y / scale) as f32)
    }
}

/// Query whether the system (menu-bar) appearance is currently dark.
fn system_is_dark() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    let appearance = app.effectiveAppearance();
    let name = appearance.name();
    name.to_string().to_lowercase().contains("dark")
}
