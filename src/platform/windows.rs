//! Windows backend: a `Shell_NotifyIcon` tray anchor plus a native, borderless,
//! non-activating layered popup + flyout driven directly by a Win32 message pump
//! (no winit / softbuffer).
//!
//! The message-only owner window hosts the notification icon and receives its
//! `WM_TRAY_CALLBACK`. On a left-click the popup opens as a
//! `WS_POPUP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED` window (see
//! the `window` submodule) whose per-pixel-alpha content is blitted with
//! `UpdateLayeredWindow` over an acrylic system backdrop (see `present`).
//! Submenu rows open a second such window (a flyout).
//!
//! ## Event model
//!
//! One thread owns the message pump ([`GetMessageW`]/[`TranslateMessage`]/
//! [`DispatchMessageW`]). Every callback — the tray `wnd_proc`, the global
//! `WH_MOUSE_LL` / `WH_KEYBOARD_LL` hooks, and a [`TrayHandle`](crate::TrayHandle)
//! from another thread — is tiny: it *enqueues* a `UiEvent` and posts a
//! `WM_MURI_DRAIN` to the owner window. A single drain (dispatched by the pump)
//! is the only place that mutates `AppState`, so windows are never created or
//! destroyed re-entrantly inside a hook or a synchronous message send. This is
//! the exact shape the macOS backend runs, with GCD's main-queue drain replaced
//! by a posted window message.
//!
//! ## Outside-click dismiss (spec 21 §2)
//!
//! A `WS_EX_NOACTIVATE` popup never holds focus, so `Focused(false)` is not a
//! usable dismiss signal. Instead a global `WH_MOUSE_LL` hook — installed *only*
//! while a popup is open, removed the instant it closes — marshals every
//! system-wide mouse-down point to the drain, which dismisses the stack when the
//! point falls outside **every** open muri window (the point-in-any-window rule).
//! `WM_ACTIVATEAPP` deactivation dismisses too, catching keyboard app-switches.
//!
//! ## Device-verified behaviors
//!
//! Outside-click dismissal, keyboard nav into the non-activating popup, the
//! acrylic backdrop, per-monitor DPI, and NVDA/Narrator traversal need a real
//! Windows display + assistive tech; those spots are marked
//! `DEVICE-VERIFY(0.9.0)`. The architecture (native layered no-activate window,
//! layered blit present, per-window UIA adapter, hook-driven dismiss) is complete
//! and compiles for `x86_64-pc-windows-msvc`.

#![allow(unsafe_code)]

mod input;
mod present;
mod window;

#[cfg(feature = "a11y")]
mod a11y;

use std::cell::{Cell, RefCell};
use std::ptr::null_mut;
use std::rc::Rc;
#[cfg(feature = "a11y")]
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Once;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, S_OK, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, GetMonitorInfoW, MonitorFromRect,
    ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForSystem, GetDpiForWindow, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIS_HIDDEN, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyWindow, DispatchMessageW, GetMessageW, GetWindowLongPtrW, GetWindowRect, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, GWLP_USERDATA, HC_ACTION, HHOOK, HICON, HWND_MESSAGE, ICONINFO,
    KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_ACTIVATEAPP, WM_APP,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WNDCLASSW,
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
use crate::theme::{MenuOptions, Theme};
use crate::{Tray, TrayCommand};

/// The private window message the tray icon posts back to its owner window.
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
/// The private message that asks the pump to drain the event + command inbox.
const WM_MURI_DRAIN: u32 = WM_APP + 2;
/// The notification icon's id within its owner window.
const TRAY_ICON_UID: u32 = 0x0001;
/// `GWLP_USERDATA` tag stored on the popup window.
const POPUP_TAG: isize = 1;
/// Base `GWLP_USERDATA` tag for flyout windows; a flyout at stack depth `d` is
/// tagged `FLYOUT_TAG + d` so the shared window procedure can recover its level.
const FLYOUT_TAG: isize = 2;

/// Registers the message-only owner window class exactly once per process.
/// A [`Once`] (not a swapped flag) so a second thread blocks until the first has
/// finished `RegisterClassW` — otherwise it could observe the class as
/// "registered" and `CreateWindowExW` before registration actually completed.
static TRAY_CLASS_ONCE: Once = Once::new();
/// Registers the layered-popup window class exactly once per process (same
/// register-before-observe guarantee as [`TRAY_CLASS_ONCE`]).
static POPUP_CLASS_ONCE: Once = Once::new();
/// The `RegisterWindowMessageW("TaskbarCreated")` broadcast id, resolved once at
/// install so `wnd_proc` can re-add the icon after an Explorer restart.
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

/// The owner `HWND` (as an `isize`) the pump listens on, stored in a *thread-safe*
/// static so a producer on ANY thread can wake the pump with `PostMessageW`. Set
/// in the run loops alongside the thread-local [`OWNER_HWND`]. Unlike that
/// thread-local, this is readable from a foreign UIA thread — the one guarantee
/// `accesskit_windows` does not give us about `do_action` (it may fire off-thread,
/// unlike `accesskit_macos`, which is main-thread-only). `PostMessageW` is
/// documented thread-safe, so waking across the thread hop is sound.
#[cfg(feature = "a11y")]
static A11Y_OWNER: AtomicIsize = AtomicIsize::new(0);

/// Cross-thread inbox for UIA action requests raised on a foreign thread. The
/// action payload can't ride the thread-local [`EVENTS`] queue (empty on the
/// foreign thread), so it is pushed here and drained on the pump thread. Kept
/// separate from [`EVENTS`] precisely because it must be `Send`/lockable.
#[cfg(feature = "a11y")]
static A11Y_ACTIONS: std::sync::Mutex<Vec<(WindowKind, accesskit::ActionRequest)>> =
    std::sync::Mutex::new(Vec::new());

thread_local! {
    /// The single running [`AppState`], reachable from every callback and the
    /// drain. Set once in [`run_event_loop`].
    static MAIN_APP: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };

    /// Pending high-level UI events, pushed by callbacks and applied by the drain.
    static EVENTS: RefCell<Vec<UiEvent>> = const { RefCell::new(Vec::new()) };

    /// The owner window, so on-thread callbacks can post a drain to it.
    static OWNER_HWND: Cell<isize> = const { Cell::new(0) };
}

/// Encode a Rust string as a NUL-terminated UTF-16 buffer for the Win32 `*W`
/// APIs.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Enqueue a UI event and ask the pump to drain. Safe from any on-thread callback
/// (it takes no [`AppState`] borrow).
pub(super) fn push_event(event: UiEvent) {
    EVENTS.with(|e| e.borrow_mut().push(event));
    let owner = OWNER_HWND.with(|h| h.get());
    if owner != 0 {
        unsafe {
            PostMessageW(owner as HWND, WM_MURI_DRAIN, 0, 0);
        }
    }
}

/// Enqueue a UIA action request from *any* thread and wake the pump.
///
/// `accesskit_windows` may invoke its action handler on a foreign UIA thread, so
/// this deliberately avoids the thread-local [`push_event`] path: it pushes the
/// action into the thread-safe [`A11Y_ACTIONS`] inbox and posts `WM_MURI_DRAIN`
/// to the [`A11Y_OWNER`] `HWND` (thread-safe static), which the pump drains via
/// [`PopupSession::drain_a11y_actions`]. Without this, every NVDA/Narrator
/// focus/activate raised off-thread would be silently dropped.
#[cfg(feature = "a11y")]
pub(super) fn push_a11y_action(kind: WindowKind, request: accesskit::ActionRequest) {
    if let Ok(mut q) = A11Y_ACTIONS.lock() {
        q.push((kind, request));
    }
    let owner = A11Y_OWNER.load(Ordering::SeqCst);
    if owner != 0 {
        unsafe {
            PostMessageW(owner as HWND, WM_MURI_DRAIN, 0, 0);
        }
    }
}

/// Which muri window an event came from. Flyouts carry their **depth** on the open
/// stack (`0` = the first flyout, opened from the popup; `1` = its child; …) so a
/// callback can tag its events with the exact level without consulting shared
/// state (decision #8, N-level submenus) — mirroring the macOS backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WindowKind {
    /// The top-level popup anchored to the tray icon.
    Popup,
    /// An open submenu flyout at the given stack depth (`0` = first flyout).
    Flyout(usize),
}

impl WindowKind {
    /// The menu level this window renders: `0` is the top-level menu, `k` the
    /// submenu reached by descending `k` open flyouts.
    fn menu_level(self) -> usize {
        match self {
            WindowKind::Popup => 0,
            WindowKind::Flyout(depth) => depth + 1,
        }
    }

    /// The `GWLP_USERDATA` tag identifying this window's kind + depth, so the
    /// shared `wnd_proc` can recover the exact stack level from an `HWND`.
    fn tag(self) -> isize {
        match self {
            WindowKind::Popup => POPUP_TAG,
            WindowKind::Flyout(depth) => FLYOUT_TAG + depth as isize,
        }
    }
}

/// A translated, backend-neutral UI event awaiting application on the drain.
pub(super) enum UiEvent {
    /// The tray icon was left-clicked — toggle the popup.
    TrayClicked,
    /// The pointer moved over a window (window-local logical points).
    MouseMoved {
        /// Which window.
        kind: WindowKind,
        /// X in the window's logical points.
        x: f32,
        /// Y in the window's logical points.
        y: f32,
    },
    /// A left mouse-up (a click) landed on a window.
    MouseClick {
        /// Which window.
        kind: WindowKind,
        /// X in the window's logical points.
        x: f32,
        /// Y in the window's logical points.
        y: f32,
    },
    /// A navigation key was pressed (observed by the global keyboard hook).
    Key(NavKey),
    /// A system-wide mouse-down at a physical screen point (from `WH_MOUSE_LL`).
    /// The drain dismisses the stack if it lands outside every muri window.
    GlobalMouseDown {
        /// Physical screen x.
        x: i32,
        /// Physical screen y.
        y: i32,
    },
    /// The application lost activation (`WM_ACTIVATEAPP` false) — dismiss.
    AppDeactivated,
    // UIA action requests do NOT ride this (thread-local) queue: `do_action` may
    // fire on a foreign UIA thread, so they travel through the thread-safe
    // [`A11Y_ACTIONS`] inbox and [`push_a11y_action`] instead.
}

/// Extract the signed `(x, y)` from an `LPARAM`-packed point (client pixels).
fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam & 0xFFFF) as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

/// The `WindowKind` tagged on a window via `GWLP_USERDATA`, or `None` for the
/// message-only owner window.
fn window_kind(hwnd: HWND) -> Option<WindowKind> {
    match unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } {
        POPUP_TAG => Some(WindowKind::Popup),
        n if n >= FLYOUT_TAG => Some(WindowKind::Flyout((n - FLYOUT_TAG) as usize)),
        _ => None,
    }
}

/// The shared window procedure for the owner, popup, and flyout windows. Every
/// arm is tiny: it translates a native message into a [`UiEvent`] (or drains),
/// never touching [`AppState`] except through the serialized drain.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MURI_DRAIN => {
            let app = MAIN_APP.with(|slot| slot.borrow().clone());
            if let Some(app) = app {
                if let Ok(mut state) = app.try_borrow_mut() {
                    state.drain();
                }
            }
            0
        }
        WM_TRAY_CALLBACK => {
            // The low word of lParam carries the actual mouse message.
            if (lparam & 0xFFFF) as u32 == WM_LBUTTONUP {
                push_event(UiEvent::TrayClicked);
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(kind) = window_kind(hwnd) {
                let (x, y) = to_logical_client(hwnd, lparam_xy(lparam));
                push_event(UiEvent::MouseMoved { kind, x, y });
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(kind) = window_kind(hwnd) {
                let (x, y) = to_logical_client(hwnd, lparam_xy(lparam));
                push_event(UiEvent::MouseClick { kind, x, y });
            }
            0
        }
        WM_ACTIVATEAPP => {
            if wparam == 0 {
                push_event(UiEvent::AppDeactivated);
            }
            0
        }
        _ if msg != 0 && msg == TASKBAR_CREATED_MSG.load(Ordering::SeqCst) => {
            // Explorer restarted; re-add our icon (spec 21 §1).
            let app = MAIN_APP.with(|slot| slot.borrow().clone());
            if let Some(app) = app {
                if let Ok(mut state) = app.try_borrow_mut() {
                    if let Anchor::Tray(a) = &mut state.session.anchor {
                        // Re-add is best-effort in this broadcast handler: a
                        // `wnd_proc` cannot surface an error to a caller, and a
                        // failed re-add simply leaves the icon absent until the
                        // next `TaskbarCreated` broadcast retries it. Hence the
                        // `Result` is intentionally discarded (not silently
                        // swallowed).
                        let _ = a.readd();
                    }
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Convert a window-local physical-pixel client point to the window's logical
/// coordinates (points), using the window's own DPI.
fn to_logical_client(hwnd: HWND, (x, y): (i32, i32)) -> (f32, f32) {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
    (x as f32 / scale, y as f32 / scale)
}

/// Global low-level mouse hook: marshal every system-wide button-down point to
/// the drain, which decides dismissal (point-in-any-window). Does *zero* work
/// beyond posting, so it can never exceed the low-level-hook timeout.
///
// DEVICE-VERIFY(0.9.0): the hook only fires against real system input; the
// point-in-any-window decision it feeds is unit-tested here, but the hook
// installation/latency itself needs a Windows box.
unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let msg = wparam as u32;
        if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN {
            let data = &*(lparam as *const MSLLHOOKSTRUCT);
            push_event(UiEvent::GlobalMouseDown {
                x: data.pt.x,
                y: data.pt.y,
            });
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// Global low-level keyboard hook: translate menu-nav key-downs to [`NavKey`]s
/// while a popup is open (a `WS_EX_NOACTIVATE` popup does not receive `WM_KEYDOWN`
/// itself). Observes but never consumes keys.
///
// DEVICE-VERIFY(0.9.0): keyboard delivery to a non-activating popup is the
// same "needs keys but never focused" bind macOS hits; the low-level hook is the
// specified resolution but can only be exercised on a device.
unsafe extern "system" fn kbd_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wparam as u32 == WM_KEYDOWN {
        let data = &*(lparam as *const KBDLLHOOKSTRUCT);
        if let Some(key) = input::translate_vk(data.vkCode as u16) {
            push_event(UiEvent::Key(key));
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

// =============================================================================
// Icon decode: muri PNG `Icon` → HICON
// =============================================================================

/// Decode a muri PNG [`Icon`] into an `HICON` at `size`×`size` device pixels
/// (scaled with nearest-neighbour), or `None` for non-PNG icons / bad bytes.
///
/// Builds a 32-bit top-down straight-alpha BGRA color bitmap plus a monochrome
/// mask, then `CreateIconIndirect`; the returned `HICON` owns its own copy, so
/// both source bitmaps are deleted before returning (spec 21 §1).
///
/// # Safety
/// Calls raw GDI; the returned handle (if any) must be freed with `DestroyIcon`.
unsafe fn decode_hicon(icon: &Icon, size: i32) -> Option<HICON> {
    let Icon::Png(bytes) = icon else {
        return None;
    };
    let (rgba, src_w, src_h) = crate::render::decode_png(bytes)?;
    let (sw, sh) = (src_w as i32, src_h as i32);
    if sw == 0 || sh == 0 || size <= 0 {
        return None;
    }
    let src = &rgba[..];

    let screen_dc = GetDC(null_mut());
    if screen_dc.is_null() {
        return None;
    }
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size,
        biHeight: -size, // top-down
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut bits: *mut core::ffi::c_void = null_mut();
    let color = CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    ReleaseDC(null_mut(), screen_dc);
    if color.is_null() || bits.is_null() {
        if !color.is_null() {
            DeleteObject(color as HGDIOBJ);
        }
        return None;
    }

    // Nearest-neighbour scale into straight-alpha BGRA. `decode_png` already
    // yields straight-alpha RGBA, so this is just the R/B channel swap.
    let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (size * size * 4) as usize);
    for row in 0..size {
        for col in 0..size {
            let sx = (col * sw / size).clamp(0, sw - 1);
            let sy = (row * sh / size).clamp(0, sh - 1);
            let s = ((sy * sw + sx) * 4) as usize;
            let d = ((row * size + col) * 4) as usize;
            let a = src[s + 3];
            let (r, g, b) = if a == 0 {
                (0, 0, 0)
            } else {
                (src[s], src[s + 1], src[s + 2])
            };
            dst[d] = b;
            dst[d + 1] = g;
            dst[d + 2] = r;
            dst[d + 3] = a;
        }
    }

    // A same-size monochrome mask (zeros): 32-bit alpha carries transparency, so
    // the AND mask is inert, but `CreateIconIndirect` still requires one.
    let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
    if mask.is_null() {
        DeleteObject(color as HGDIOBJ);
        return None;
    }

    let info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let hicon = CreateIconIndirect(&info);
    DeleteObject(color as HGDIOBJ);
    DeleteObject(mask as HGDIOBJ);
    if hicon.is_null() {
        None
    } else {
        Some(hicon)
    }
}

// =============================================================================
// Anchor geometry
// =============================================================================

/// The tray icon's physical geometry resolved against the monitor it sits on, in
/// one shared logical space (physical pixels ÷ DPI scale) so [`place_popup`] and
/// [`place_flyout`] can work in the exact same coordinates the macOS backend
/// uses. [`WinGeometry::to_physical`] converts a logical origin back to physical
/// pixels for window placement.
#[derive(Clone, Copy)]
struct WinGeometry {
    /// Anchor rect (physical pixels, virtual-screen space).
    anchor: RECT,
    /// The anchor monitor's work area (physical pixels).
    work: RECT,
    /// Logical pixels per physical pixel's inverse (physical / scale = logical).
    scale: f32,
}

impl WinGeometry {
    fn anchor_rect_local(&self) -> LogicalRect {
        self.rect_local(&self.anchor)
    }

    fn work_area_local(&self) -> LogicalRect {
        self.rect_local(&self.work)
    }

    fn rect_local(&self, r: &RECT) -> LogicalRect {
        LogicalRect::new(
            LogicalPoint::new(r.left as f32 / self.scale, r.top as f32 / self.scale),
            LogicalSize::new(
                (r.right - r.left) as f32 / self.scale,
                (r.bottom - r.top) as f32 / self.scale,
            ),
        )
    }

    /// A logical top-left origin back to a physical screen point.
    fn to_physical(self, origin: LogicalPoint) -> (i32, i32) {
        (
            (origin.x * self.scale).round() as i32,
            (origin.y * self.scale).round() as i32,
        )
    }

    /// Resolve a caller-supplied logical anchor `rect` against the monitor it lands
    /// on, producing the same shared-logical geometry the tray path uses — the
    /// anchor geometry for a pointer/rect-anchored [`ContextMenu`](crate::ContextMenu)
    /// / [`Popup`](crate::Popup) that has no tray icon (spec 21 §3).
    ///
    // DEVICE-VERIFY(0.9.0): multi-monitor point resolution. The first
    // logical->physical conversion uses the *system* DPI to find the monitor, then
    // snaps to that monitor's effective DPI; a secondary display with a different
    // scale needs a real multi-monitor box to confirm the anchor lands on (and
    // flips against) the right monitor — the same fragility the macOS `for_rect`
    // carries.
    fn for_rect(rect: LogicalRect) -> Option<WinGeometry> {
        let sys_dpi = unsafe { GetDpiForSystem() };
        let sys_scale = if sys_dpi == 0 {
            1.0
        } else {
            sys_dpi as f32 / 96.0
        };
        let to_phys = |scale: f32| RECT {
            left: (rect.origin.x * scale).round() as i32,
            top: (rect.origin.y * scale).round() as i32,
            right: ((rect.origin.x + rect.size.width) * scale).round() as i32,
            bottom: ((rect.origin.y + rect.size.height) * scale).round() as i32,
        };

        let probe = to_phys(sys_scale);
        let hmon = unsafe { MonitorFromRect(&probe, MONITOR_DEFAULTTONEAREST) };
        if hmon.is_null() {
            return None;
        }

        // Snap to the resolved monitor's effective DPI so the stored physical
        // anchor and `to_physical`/`rect_local` round-trip stay self-consistent.
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        let scale = if unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
            == S_OK
            && dpi_x != 0
        {
            dpi_x as f32 / 96.0
        } else {
            sys_scale
        };
        let anchor = to_phys(scale);

        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work = if unsafe { GetMonitorInfoW(hmon, &mut mi) } != 0 {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: (1440.0 * scale) as i32,
                bottom: (900.0 * scale) as i32,
            }
        };

        Some(WinGeometry {
            anchor,
            work,
            scale,
        })
    }
}

// =============================================================================
// The notification-area anchor
// =============================================================================

/// The Windows notification-area anchor. Owns the message-only window, the
/// registered icon, and the current `HICON` until dropped.
pub struct WindowsAnchor {
    /// The message-only window that owns the notification icon.
    hwnd: HWND,
    /// Whether the icon is currently registered (so `Drop` can remove it).
    installed: bool,
    /// The current tooltip, retained so a `TaskbarCreated` re-add keeps it.
    tooltip: Option<String>,
    /// The live `HICON` (destroyed on replacement / drop), and the `Arc` bytes it
    /// was decoded from so `set_icon` on a tick can skip re-decoding. The retained
    /// `Arc` (not a bare pointer value) is what makes the skip sound: it keeps the
    /// source allocation alive so a freed-and-reused address can't be mistaken for
    /// the same icon (ABA), and it is compared with [`Arc::ptr_eq`].
    hicon: HICON,
    icon_bytes: Option<std::sync::Arc<[u8]>>,
}

impl std::fmt::Debug for WindowsAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsAnchor")
            .field("installed", &self.installed)
            .finish()
    }
}

impl Default for WindowsAnchor {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsAnchor {
    /// Create the (not-yet-installed) Windows anchor.
    pub fn new() -> Self {
        WindowsAnchor {
            hwnd: null_mut(),
            installed: false,
            tooltip: None,
            hicon: null_mut(),
            icon_bytes: None,
        }
    }

    /// Create the message-only window that owns the notification icon.
    unsafe fn create_message_window(&mut self) -> Result<()> {
        let hinstance = GetModuleHandleW(null_mut());
        let class_name = wide("muri_tray_msgwnd");

        TRAY_CLASS_ONCE.call_once(|| unsafe {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wnd_proc);
            wc.hInstance = hinstance;
            wc.lpszClassName = class_name.as_ptr();
            RegisterClassW(&wc);
        });

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            null_mut(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            hinstance,
            null_mut(),
        );
        if hwnd.is_null() {
            return Err(Error::Platform(
                "failed to create tray message window".into(),
            ));
        }
        self.hwnd = hwnd;
        Ok(())
    }

    /// The `NOTIFYICONIDENTIFIER` naming this anchor's icon.
    fn identifier(&self) -> NOTIFYICONIDENTIFIER {
        let mut id: NOTIFYICONIDENTIFIER = unsafe { std::mem::zeroed() };
        id.cbSize = std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32;
        id.hWnd = self.hwnd;
        id.uID = TRAY_ICON_UID;
        id
    }

    /// A zeroed `NOTIFYICONDATAW` pre-filled with this anchor's identity.
    fn base_nid(&self) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ICON_UID;
        nid
    }

    /// Copy a tooltip string into a `szTip` array (NUL-terminated, clamped).
    ///
    /// Reserve the last slot so a NUL terminator always remains: [`base_nid`]
    /// zero-initializes the array, so leaving `dst[n..]` untouched keeps it `0`.
    /// Without this, a tooltip encoding to ≥128 UTF-16 units would fill every
    /// slot and Win32 would read past `szTip`.
    fn fill_tip(tip: &str, dst: &mut [u16; 128]) {
        let src = wide(tip);
        let n = src.len().min(dst.len() - 1);
        dst[..n].copy_from_slice(&src[..n]);
    }

    /// Replace the live `HICON` from an [`Icon`] (skips re-decode when the same
    /// `Arc` bytes are passed again), returning the handle to store in `hIcon`.
    ///
    /// Surfaces decode failure honestly: undecodable `Icon::Png` bytes yield
    /// `Err(Error::BadIcon(..))` rather than silently substituting a stock/blank
    /// icon while the caller believes the requested icon was set. Non-raster kinds
    /// (`Svg`/`Checkmark`/`Symbol`) have no tray `HICON`; they resolve to a null
    /// handle (no tray image), which is not a decode failure.
    unsafe fn resolve_hicon(&mut self, icon: &Icon) -> Result<HICON> {
        // Fast path: the same PNG bytes passed again (e.g. an unchanged icon on a
        // ~0.75s menu tick) skip the re-decode. Guarded by `Arc::ptr_eq` on the
        // *retained* Arc, never a bare pointer value: a freed-and-reused
        // allocation could otherwise land different bytes at the same address and
        // return a stale HICON for the previous icon (ABA).
        if let Icon::Png(bytes) = icon {
            if !self.hicon.is_null()
                && self
                    .icon_bytes
                    .as_ref()
                    .is_some_and(|b| std::sync::Arc::ptr_eq(b, bytes))
            {
                return Ok(self.hicon);
            }
        }
        let new = match icon {
            Icon::Png(_) => decode_hicon(icon, 16)
                .ok_or_else(|| Error::BadIcon("could not decode PNG tray icon bytes".into()))?,
            _ => null_mut(),
        };
        if !self.hicon.is_null() {
            DestroyIcon(self.hicon);
        }
        self.hicon = new;
        self.icon_bytes = match icon {
            Icon::Png(bytes) => Some(std::sync::Arc::clone(bytes)),
            _ => None,
        };
        Ok(new)
    }

    fn install(&mut self, icon: &Icon, tooltip: Option<&str>) -> Result<()> {
        unsafe {
            // Decode the icon *before* creating any window or registering the
            // notification icon, so an undecodable icon fails cleanly (with no
            // window to tear down) and the caller learns the icon was rejected
            // rather than seeing a silent blank-icon "success".
            let hicon = self.resolve_hicon(icon)?;

            self.create_message_window()?;
            self.tooltip = tooltip.map(str::to_owned);

            let mut nid = self.base_nid();
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY_CALLBACK;
            nid.hIcon = hicon;
            if let Some(tip) = tooltip {
                Self::fill_tip(tip, &mut nid.szTip);
            }

            // `Shell_NotifyIconW` returns a `BOOL`: nonzero on success. A zero
            // return means `NIM_ADD` did not register the icon, so undo the window
            // and surface the failure instead of leaving `installed` false while
            // reporting success.
            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                let _ = DestroyWindow(self.hwnd);
                self.hwnd = null_mut();
                return Err(Error::Platform("Shell_NotifyIcon(NIM_ADD) failed".into()));
            }
            self.installed = true;
        }
        Ok(())
    }

    /// Re-add the icon after an Explorer (`TaskbarCreated`) restart.
    fn readd(&mut self) -> Result<()> {
        if self.hwnd.is_null() {
            return Ok(());
        }
        unsafe {
            let mut nid = self.base_nid();
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY_CALLBACK;
            nid.hIcon = self.hicon;
            if let Some(tip) = self.tooltip.clone() {
                Self::fill_tip(&tip, &mut nid.szTip);
            }
            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                return Err(Error::Platform("Shell_NotifyIcon re-add failed".into()));
            }
            self.installed = true;
        }
        Ok(())
    }

    /// Replace the tray icon (`NIM_MODIFY`).
    fn set_icon(&mut self, icon: &Icon) {
        if !self.installed {
            return;
        }
        unsafe {
            // On undecodable bytes, keep the current icon rather than clearing it
            // to a blank handle: the async `SetIcon` command has no channel to
            // report an error, and silently substituting a wrong/blank icon would
            // be the very bug FIX 4 removes from the `install` path.
            let Ok(hicon) = self.resolve_hicon(icon) else {
                return;
            };
            let mut nid = self.base_nid();
            nid.uFlags = NIF_ICON;
            nid.hIcon = hicon;
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Replace the tooltip / accessible name (`NIM_MODIFY`).
    fn set_tooltip(&mut self, tooltip: Option<&str>) {
        self.tooltip = tooltip.map(str::to_owned);
        if !self.installed {
            return;
        }
        unsafe {
            let mut nid = self.base_nid();
            nid.uFlags = NIF_TIP;
            if let Some(tip) = tooltip {
                Self::fill_tip(tip, &mut nid.szTip);
            }
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Show or hide the status item (`NIM_MODIFY` with `NIF_STATE`).
    fn set_visible(&self, visible: bool) {
        if !self.installed {
            return;
        }
        unsafe {
            let mut nid = self.base_nid();
            nid.uFlags = NIF_STATE;
            nid.dwStateMask = NIS_HIDDEN;
            nid.dwState = if visible { 0 } else { NIS_HIDDEN };
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Resolve the anchor + its monitor's work area into one logical space.
    fn geometry(&self) -> Option<WinGeometry> {
        if !self.installed {
            return None;
        }
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };

        let id = self.identifier();
        let mut anchor: RECT = unsafe { std::mem::zeroed() };
        let ok = unsafe { Shell_NotifyIconGetRect(&id, &mut anchor) } == S_OK;
        if !ok {
            // DEVICE-VERIFY(0.9.0): under the overflow ("hidden icons") flyout,
            // `Shell_NotifyIconGetRect` returns the chevron rect or fails; fall
            // back to a cursor-anchored open (spec 21 §2, risk #4).
            let mut pt: POINT = unsafe { std::mem::zeroed() };
            if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
                return None;
            }
            anchor = RECT {
                left: pt.x,
                top: pt.y,
                right: pt.x + 1,
                bottom: pt.y + 1,
            };
        }

        let hmon = unsafe { MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST) };
        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work = if !hmon.is_null() && unsafe { GetMonitorInfoW(hmon, &mut mi) } != 0 {
            mi.rcWork
        } else {
            // Fall back to a generous default rect so placement still clamps.
            RECT {
                left: 0,
                top: 0,
                right: (1440.0 * scale) as i32,
                bottom: (900.0 * scale) as i32,
            }
        };

        Some(WinGeometry {
            anchor,
            work,
            scale,
        })
    }

    /// The tray icon's physical screen rect straight from
    /// `Shell_NotifyIconGetRect`, or `None` when it can't be resolved (e.g. the
    /// icon lives in the overflow "hidden icons" flyout). Unlike [`geometry`],
    /// this never falls back to the cursor — a caller hit-testing a global mouse
    /// click against the icon needs the icon's *actual* location or nothing.
    ///
    /// [`geometry`]: WindowsAnchor::geometry
    fn icon_rect_physical(&self) -> Option<RECT> {
        if !self.installed {
            return None;
        }
        let id = self.identifier();
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { Shell_NotifyIconGetRect(&id, &mut rect) } == S_OK {
            Some(rect)
        } else {
            None
        }
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        self.geometry()
            .map(|g| g.anchor_rect_local())
            .ok_or_else(|| Error::Platform("tray icon not installed".into()))
    }

    fn work_area(&self) -> Result<LogicalRect> {
        self.geometry()
            .map(|g| g.work_area_local())
            .ok_or_else(|| Error::Platform("tray icon not installed".into()))
    }

    fn supports_tray_anchor(&self) -> bool {
        true
    }

    /// Where a popup of `popup` size should open, anchored to the tray icon and
    /// clamped into `work_area`. Retained for API parity with the shared
    /// [`place_popup`]; the live loop resolves placement through
    /// `WindowsAnchor::geometry` so it shares the exact math macOS uses. Taskbars
    /// usually sit at the bottom, so the popup grows up from the icon
    /// ([`Edge::Top`]).
    pub fn popup_origin(&self, popup: LogicalSize, work_area: LogicalRect) -> Result<LogicalPoint> {
        let anchor = self.anchor_rect()?;
        Ok(place_popup(anchor, popup, work_area, Edge::Top, 2.0))
    }
}

impl Drop for WindowsAnchor {
    fn drop(&mut self) {
        unsafe {
            if self.installed {
                let nid = self.base_nid();
                Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            if !self.hicon.is_null() {
                DestroyIcon(self.hicon);
            }
            if !self.hwnd.is_null() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

// =============================================================================
// Anchor
// =============================================================================

/// Where a session's popup anchors: the live tray icon (which also owns icon /
/// tooltip / visibility) or a fixed rectangle for a pointer-anchored
/// [`ContextMenu`](crate::ContextMenu) / [`Popup`](crate::Popup) (spec 21 §3).
/// [`Tray`], `ContextMenu`, and `Popup` all drive one [`PopupSession`]; they differ
/// only in how the anchor rectangle is obtained.
enum Anchor {
    /// The `Shell_NotifyIcon` tray anchor (its geometry follows the icon).
    Tray(WindowsAnchor),
    /// A fixed anchor rectangle, resolved once against its monitor.
    Fixed(WinGeometry),
}

impl Anchor {
    /// The current anchor geometry (anchor + work rects + scale).
    fn geometry(&self) -> Option<WinGeometry> {
        match self {
            Anchor::Tray(a) => a.geometry(),
            Anchor::Fixed(g) => Some(*g),
        }
    }
}

// =============================================================================
// Panels + app state
// =============================================================================

/// A live popup or flyout window: its `HWND`, the raster drawer that paints it,
/// its layout / hover state, and its physical top-left.
struct Panel {
    hwnd: HWND,
    drawer: RasterDrawer,
    laid: Option<LaidMenu>,
    cursor: LogicalPoint,
    hovered: Option<usize>,
    /// Top-left origin in the shared logical space (for `place_flyout`).
    origin: LogicalPoint,
    /// Physical top-left, where the layered blit positions the window.
    px: i32,
    py: i32,
    #[cfg(feature = "a11y")]
    adapter: accesskit_windows::SubclassingAdapter,
    #[cfg(feature = "a11y")]
    snapshot: Rc<RefCell<a11y::A11ySnapshot>>,
}

impl Panel {
    fn destroy(&self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }

    /// The window's on-screen rectangle in physical pixels, for hit-testing a
    /// global mouse-down.
    fn phys_rect(&self) -> (i32, i32, i32, i32) {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(self.hwnd, &mut r) } != 0 {
            (r.left, r.top, r.right, r.bottom)
        } else {
            (0, 0, 0, 0)
        }
    }
}

/// One open flyout level: the row (within its parent level's menu) it opened from,
/// and its native window. Level `k` of [`PopupSession::flyouts`] is window depth
/// `k` ([`WindowKind::Flyout`]) and its menu is
/// [`PopupSession::menu_at_level`]`(k + 1)`. Mirrors the macOS `Flyout`.
struct Flyout {
    /// Item index (within the parent level's menu) this flyout opened from.
    parent: usize,
    /// The flyout's native window.
    panel: Panel,
}

/// The shared popup machinery: the top-level popup, the **stack** of open flyout
/// windows (decision #8), the global hooks that drive dismissal + keyboard nav,
/// and everything needed to render/anchor them. [`Tray`],
/// [`ContextMenu`](crate::ContextMenu), and [`Popup`](crate::Popup) all drive one
/// of these; they differ only in how the anchor rectangle is obtained
/// ([`Anchor`]) — spec 21 §3. This is the Windows analogue of the macOS
/// `PopupSession`.
///
/// The click handler is stored as a `Box<dyn Fn + 'a>`: the tray uses a `'static`
/// handler owned for the whole run loop; a context menu borrows the caller's
/// handler for the duration of its blocking `open_at` / `anchored_to` call.
struct PopupSession<'a> {
    /// The top-level menu; source of truth for rendering + a11y.
    menu: Menu,
    options: MenuOptions,
    /// Row-activation sink; dispatches the activated [`MenuId`].
    dispatch: Box<dyn Fn(&MenuId) + 'a>,
    /// How the popup anchors (tray icon or a fixed rect).
    anchor: Anchor,
    /// Which edge the popup grows from relative to its anchor.
    edge: Edge,
    hinstance: windows_sys::Win32::Foundation::HINSTANCE,
    popup: Option<Panel>,
    /// The open flyout window stack, shallowest first (decision #8).
    flyouts: Vec<Flyout>,
    /// The global hooks, live only while a popup is open.
    mouse_hook: HHOOK,
    kbd_hook: HHOOK,
}

impl PopupSession<'_> {
    /// The resolved theme for the current appearance.
    fn theme(&self) -> Theme {
        let dark = self.options.theme.wants_dark(system_is_dark);
        self.options.theme.resolve_theme(dark)
    }

    /// The menu shown at the given level: `0` is the top-level menu, `k` the
    /// submenu reached by descending the first `k` open flyouts' parents. Returns
    /// `None` if a parent along the way is no longer a submenu (e.g. the menu was
    /// swapped underneath an open flyout). Borrows straight out of [`self.menu`]
    /// (no clone), mirroring the macOS `menu_at_level`.
    ///
    /// [`self.menu`]: PopupSession::menu
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

    /// Rebuild the current keyboard/mouse selection from the windows' hovered rows.
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

    // -- hooks ---------------------------------------------------------------

    fn install_hooks(&mut self) {
        if self.mouse_hook.is_null() {
            self.mouse_hook =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), self.hinstance, 0) };
        }
        if self.kbd_hook.is_null() {
            self.kbd_hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_hook_proc), self.hinstance, 0)
            };
        }
    }

    fn remove_hooks(&mut self) {
        if !self.mouse_hook.is_null() {
            unsafe { UnhookWindowsHookEx(self.mouse_hook) };
            self.mouse_hook = null_mut();
        }
        if !self.kbd_hook.is_null() {
            unsafe { UnhookWindowsHookEx(self.kbd_hook) };
            self.kbd_hook = null_mut();
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

        let mut probe = RasterDrawer::new(scale);
        let laid = render_menu(&mut probe, &self.menu, &theme, &self.options, None);

        let origin = place_popup(
            geom.anchor_rect_local(),
            laid.size,
            geom.work_area_local(),
            self.edge,
            2.0,
        );
        let (px, py) = geom.to_physical(origin);

        let class = wide("muri_popup_wnd");
        register_popup_class(self.hinstance, &class);
        let hwnd = unsafe {
            window::create_popup(class.as_ptr(), self.hinstance, WindowKind::Popup.tag())
        };
        if hwnd.is_null() {
            return;
        }

        #[cfg(feature = "a11y")]
        let snapshot = Rc::new(RefCell::new(a11y::A11ySnapshot {
            menu: self.menu.clone(),
            focus: MenuFocus {
                top: None,
                flyout: Vec::new(),
            },
        }));
        #[cfg(feature = "a11y")]
        let adapter = unsafe { a11y::make_adapter(hwnd, Rc::clone(&snapshot), WindowKind::Popup) };

        self.popup = Some(Panel {
            hwnd,
            drawer: RasterDrawer::new(scale),
            laid: Some(laid),
            cursor: LogicalPoint::default(),
            hovered: None,
            origin,
            px,
            py,
            #[cfg(feature = "a11y")]
            adapter,
            #[cfg(feature = "a11y")]
            snapshot,
        });

        self.redraw(WindowKind::Popup);
        unsafe { window::raise_topmost(hwnd) };
        self.install_hooks();
        self.sync_a11y();
    }

    /// Push a flyout for row `parent_index` of the currently deepest open level
    /// (the popup when no flyout is open), placed beside its parent window by
    /// [`place_flyout`]. Mirrors the macOS `push_flyout`: `parent_index` indexes
    /// the deepest level's menu, so descending nested submenus resolves correctly.
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
                pp.drawer.scale(),
                rect,
            )
        };

        let theme = self.theme();
        let Some(geom) = self.anchor.geometry() else {
            return;
        };
        let mut probe = RasterDrawer::new(scale);
        let child_laid = render_menu(&mut probe, &child, &theme, &self.options, None);

        let parent_rect = LogicalRect::new(parent_origin, parent_size);
        let placement = place_flyout(
            parent_rect,
            row_rect,
            child_laid.size,
            geom.work_area_local(),
        );
        let (px, py) = geom.to_physical(placement.origin);

        let kind = WindowKind::Flyout(depth);
        let class = wide("muri_popup_wnd");
        register_popup_class(self.hinstance, &class);
        let hwnd = unsafe { window::create_popup(class.as_ptr(), self.hinstance, kind.tag()) };
        if hwnd.is_null() {
            return;
        }

        #[cfg(feature = "a11y")]
        let snapshot = Rc::new(RefCell::new(a11y::A11ySnapshot {
            menu: child.clone(),
            focus: MenuFocus {
                top: None,
                flyout: Vec::new(),
            },
        }));
        #[cfg(feature = "a11y")]
        let adapter = unsafe { a11y::make_adapter(hwnd, Rc::clone(&snapshot), kind) };

        self.flyouts.push(Flyout {
            parent: parent_index,
            panel: Panel {
                hwnd,
                drawer: RasterDrawer::new(scale),
                laid: None,
                cursor: LogicalPoint::default(),
                hovered: None,
                origin: placement.origin,
                px,
                py,
                #[cfg(feature = "a11y")]
                adapter,
                #[cfg(feature = "a11y")]
                snapshot,
            },
        });

        self.redraw(kind);
        unsafe { window::raise_topmost(hwnd) };
        self.sync_a11y();
    }

    /// Close every flyout deeper than `len`, destroying their windows.
    fn truncate_flyouts(&mut self, len: usize) {
        while self.flyouts.len() > len {
            if let Some(f) = self.flyouts.pop() {
                f.panel.destroy();
            }
        }
    }

    /// Reconcile the open flyout window stack to `target` (per-level parent
    /// indices, as produced by [`next_flyout`]): keep the common prefix, close
    /// anything deeper, then push the remaining levels. Mirrors macOS.
    fn apply_flyout_stack(&mut self, target: &[usize]) {
        let common = common_flyout_prefix(&self.flyout_stack(), target);
        self.truncate_flyouts(common);
        for &parent in &target[common..] {
            self.push_flyout(parent);
        }
    }

    fn close_popup(&mut self) {
        self.remove_hooks();
        self.truncate_flyouts(0);
        if let Some(popup) = self.popup.take() {
            popup.destroy();
        }
    }

    // -- present -------------------------------------------------------------

    /// Re-render one window from its level's menu + hovered row. Mirrors macOS's
    /// unified `redraw`.
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
        unsafe {
            present::present_layered(panel.hwnd, panel.drawer.framebuffer(), panel.px, panel.py)
        };
        panel.laid = Some(laid);
    }

    fn redraw_all(&mut self) {
        self.redraw(WindowKind::Popup);
        for d in 0..self.flyouts.len() {
            self.redraw(WindowKind::Flyout(d));
        }
    }

    // -- pointer -------------------------------------------------------------

    fn set_cursor(&mut self, kind: WindowKind, pt: LogicalPoint) {
        if let Some(p) = self.panel_mut(kind) {
            p.cursor = pt;
        }
    }

    /// Handle a cursor move over `kind`: update its hovered row (repaint on
    /// change), then drive the flyout stack from the pure hover-stack rule.
    /// Mirrors the macOS `on_cursor` — one path for the popup and every flyout.
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

    /// Handle a click on `kind`: a submenu row opens (or switches to) its nested
    /// flyout, closing anything deeper; a leaf row dispatches its id and dismisses.
    /// Mirrors the macOS `on_click` — the flyout branch now honors submenu rows so
    /// clicking a nested submenu opens the next level instead of dispatching.
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
            // `i` indexes the deepest open level's menu (the level `handle_key`
            // just descended into), which is exactly what `push_flyout` opens
            // from — so nested submenus descend correctly.
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
        // flyouts that still exist.
        for (k, f) in self.flyouts.iter_mut().enumerate() {
            if let Some(ff) = focus.flyout.get(k) {
                f.panel.hovered = ff.child;
            }
        }
        self.redraw_all();
        self.sync_a11y();
    }

    // -- dismiss -------------------------------------------------------------

    /// A physical mouse-down: dismiss the stack if it fell outside every open
    /// muri window (spec 21 §2, the point-in-any-window rule).
    fn on_global_mouse_down(&mut self, x: i32, y: i32) {
        if self.popup.is_none() {
            return;
        }
        let mut rects = Vec::with_capacity(1 + self.flyouts.len());
        if let Some(p) = self.popup.as_ref() {
            rects.push(p.phys_rect());
        }
        for f in &self.flyouts {
            rects.push(f.panel.phys_rect());
        }
        if point_in_any(&rects, x, y) {
            return;
        }
        // A mouse-down on the tray icon itself is a toggle handled by the paired
        // `WM_TRAY_CALLBACK` (button-up): dismissing here would close-then-reopen
        // (flicker) so a tray click could never close an open popup. Hit-test the
        // icon's *reliable* rect (never the cursor fallback), so a spurious rect
        // can't swallow a genuine outside-click dismiss. When the rect is
        // unavailable (icon in the overflow flyout) we fall through and dismiss —
        // in that layout the icon isn't directly clickable while a popup is open
        // anyway (opening the overflow flyout is itself the dismissing click).
        if let Anchor::Tray(a) = &self.anchor {
            if let Some(r) = a.icon_rect_physical() {
                if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                    return;
                }
            }
        }
        self.close_popup();
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

    /// Push a fresh `TreeUpdate` to every open window's adapter (Option B: one
    /// per-window adapter per stack level — spec 30 §3). Each window's tree is its
    /// own level's menu; its focus is that window's selection, and any deeper open
    /// levels are carried as its expanded sub-stack. Mirrors the macOS `sync_a11y`.
    #[cfg(feature = "a11y")]
    fn sync_a11y(&mut self) {
        let full = self.current_focus();
        // The `menu` closures below are only invoked when the panel's adapter is
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
                self.on_cursor(kind, LogicalPoint::new(x, y));
            }
            UiEvent::MouseClick { kind, x, y } => {
                self.set_cursor(kind, LogicalPoint::new(x, y));
                self.on_click(kind);
            }
            UiEvent::Key(key) => self.on_key_nav(key),
            UiEvent::GlobalMouseDown { x, y } => self.on_global_mouse_down(x, y),
            UiEvent::AppDeactivated => {
                // DEVICE-VERIFY(0.9.0): whether a non-activating popup lets the
                // owning app "deactivate" here (vs. the mouse hook being the sole
                // dismiss path) can only be confirmed on a device.
                if self.popup.is_some() {
                    self.close_popup();
                }
            }
        }
    }

    /// Drain and apply every UIA action queued by a (possibly foreign) UIA thread
    /// into the pump-thread state. Returns whether any action was applied, so the
    /// drain loops keep spinning until the cross-thread inbox is empty too.
    #[cfg(feature = "a11y")]
    fn drain_a11y_actions(&mut self) -> bool {
        let actions = A11Y_ACTIONS
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        let any = !actions.is_empty();
        for (kind, request) in actions {
            self.on_a11y_action(kind, request);
        }
        any
    }

    /// Apply all pending UI events, looping until the inbox is empty (applying one
    /// can enqueue more). The standalone `open_at` / `anchored_to` pump uses this
    /// on the caller's stack; the tray drains through [`AppState::drain`] instead so
    /// it also applies [`TrayHandle`](crate::TrayHandle) commands.
    fn drain_events(&mut self) {
        loop {
            let events = EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
            let had_events = !events.is_empty();
            for event in events {
                self.apply_event(event);
            }
            #[cfg(feature = "a11y")]
            let had_actions = self.drain_a11y_actions();
            #[cfg(not(feature = "a11y"))]
            let had_actions = false;
            if !had_events && !had_actions {
                break;
            }
        }
    }
}

impl Drop for PopupSession<'_> {
    /// Tear down any windows + hooks still live if the session is dropped while a
    /// popup is open (spec 21 §2). If the `GetMessageW` pump exits via a foreign
    /// `WM_QUIT` while a popup is open, `close_popup` never runs on the normal
    /// path, which would leak the popup/flyout `HWND`s and leave the
    /// `WH_MOUSE_LL` / `WH_KEYBOARD_LL` hooks installed. `close_popup` nulls its
    /// hooks and takes/clears the windows it destroys, so running it here is
    /// idempotent with the normal path — no double-free / double-unhook.
    fn drop(&mut self) {
        self.close_popup();
    }
}

/// The tray's run-loop state: the shared [`PopupSession`] plus the [`Tray`] whose
/// icon / tooltip / command inbox drive the persistent tray surface.
struct AppState {
    session: PopupSession<'static>,
    tray: Tray,
}

impl AppState {
    fn apply_command(&mut self, command: TrayCommand) {
        match command {
            TrayCommand::SetMenu(menu) => {
                self.tray.menu = menu.clone();
                self.session.menu = menu;
                if self.session.popup.is_some() {
                    // Structure may have changed under an open flyout; drop the
                    // whole flyout stack, then repaint + refresh the a11y tree live.
                    self.session.truncate_flyouts(0);
                    self.session.redraw(WindowKind::Popup);
                    self.session.sync_a11y();
                }
            }
            TrayCommand::SetIcon(icon) => {
                self.tray.icon = icon;
                let icon = self.tray.icon.clone();
                if let Anchor::Tray(a) = &mut self.session.anchor {
                    a.set_icon(&icon);
                }
            }
            TrayCommand::SetTooltip(tooltip) => {
                self.tray.tooltip = tooltip;
                let tip = self.tray.tooltip.clone();
                if let Anchor::Tray(a) = &mut self.session.anchor {
                    a.set_tooltip(tip.as_deref());
                }
            }
            TrayCommand::SetTitle(title) => {
                // The Windows notification area has no text label (the menu-bar
                // title is a macOS concept); retain it, but there is nothing to
                // draw here.
                self.tray.title = title;
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
                // Post WM_QUIT so GetMessageW returns 0 and run_event_loop exits;
                // unwinding drops AppState -> WindowsAnchor::Drop, which removes
                // the notification icon (NIM_DELETE). Ends the 'muri-tray' thread.
                unsafe { PostQuitMessage(0) };
            }
        }
    }

    /// Apply all pending commands and UI events, looping until both inboxes are
    /// empty (applying one can enqueue more).
    fn drain(&mut self) {
        loop {
            let events = EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
            let commands: Vec<TrayCommand> = self
                .tray
                .commands
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default();
            let had_work = !events.is_empty() || !commands.is_empty();
            for command in commands {
                self.apply_command(command);
            }
            for event in events {
                self.session.apply_event(event);
            }
            #[cfg(feature = "a11y")]
            let had_actions = self.session.drain_a11y_actions();
            #[cfg(not(feature = "a11y"))]
            let had_actions = false;
            if !had_work && !had_actions {
                break;
            }
        }
    }
}

/// Descend `menu` through the submenu-row indices in `stack` (the open flyout
/// parents, shallowest first), **borrowing** the menu at that depth — the menu
/// whose rows the deepest open flyout selects among. Returns `None` if some index
/// along the way is not a submenu (e.g. the menu was swapped underneath an open
/// flyout).
///
/// A thin `&[usize]` wrapper over [`crate::menu::descend`], so the N-level
/// submenu resolution that [`PopupSession::menu_at_level`] (and hence the click /
/// keyboard dispatch paths) relies on is unit-testable without creating real
/// windows. Borrows rather than clones, so it is free on the redraw hot path.
///
/// The live paths call [`PopupSession::menu_at_level`]/[`crate::menu::descend`]
/// directly (borrowing disjoint fields of `self`); this wrapper exists for the
/// slice-based unit tests, hence `#[cfg(test)]`.
#[cfg(test)]
fn menu_at_stack<'a>(menu: &'a Menu, stack: &[usize]) -> Option<&'a Menu> {
    crate::menu::descend(menu, stack.iter().copied())
}

/// The length of the common prefix between the currently open flyout stack
/// (`current`, shallowest first) and a `target` stack — the number of levels
/// [`PopupSession::apply_flyout_stack`] keeps before it truncates the deeper open
/// levels and pushes the target's remaining levels. Pure, so the N-level flyout
/// reconciliation (switch-to-shallower-sibling, re-hover-open-parent) is
/// unit-testable without creating real windows.
fn common_flyout_prefix(current: &[usize], target: &[usize]) -> usize {
    let mut common = 0;
    while common < target.len() && common < current.len() && current[common] == target[common] {
        common += 1;
    }
    common
}

/// Whether `(x, y)` lies inside any `(left, top, right, bottom)` rectangle. Pure,
/// so the outside-click dismiss decision the (device-only) `WH_MOUSE_LL` hook
/// feeds is unit-testable.
fn point_in_any(rects: &[(i32, i32, i32, i32)], x: i32, y: i32) -> bool {
    rects
        .iter()
        .any(|&(l, t, r, b)| x >= l && x < r && y >= t && y < b)
}

/// Register the layered-popup window class once per process.
fn register_popup_class(hinstance: windows_sys::Win32::Foundation::HINSTANCE, class: &[u16]) {
    POPUP_CLASS_ONCE.call_once(|| unsafe {
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
    });
}

// =============================================================================
// System appearance
// =============================================================================

/// Query whether the system uses a dark app theme (`AppsUseLightTheme == 0` under
/// `HKCU`), defaulting to light if the value can't be read.
fn system_is_dark() -> bool {
    unsafe {
        let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let value = wide("AppsUseLightTheme");
        let mut data: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        );
        rc == 0 && data == 0
    }
}

// =============================================================================
// Platform seam (ADR-0002)
// =============================================================================

/// The Windows [`Platform`] implementation: a `Shell_NotifyIcon` tray anchor plus
/// the native layered non-activating popup + flyout message-pump loop, the
/// `UpdateLayeredWindow` present path, the per-window UIA adapter (behind
/// `a11y`), and the appearance / work-area queries. Every Win32-specific
/// dependency (`windows-sys`, `accesskit_windows`) lives in this module behind
/// the [`Platform`] trait — no `HWND` crosses the seam.
#[derive(Debug, Default)]
pub struct WindowsPlatform {
    anchor: WindowsAnchor,
}

impl WindowsPlatform {
    /// Create the (not-yet-installed) Windows platform.
    pub fn new() -> Self {
        WindowsPlatform {
            anchor: WindowsAnchor::new(),
        }
    }
}

impl Platform for WindowsPlatform {
    fn install_tray(&mut self, icon: &Icon, tooltip: Option<&str>) -> Result<()> {
        self.anchor.install(icon, tooltip)
    }

    fn tray_anchor_rect(&self) -> Result<LogicalRect> {
        self.anchor.anchor_rect()
    }

    fn supports_tray_anchor(&self) -> bool {
        self.anchor.supports_tray_anchor()
    }

    fn appearance(&self) -> Appearance {
        Appearance::from_is_dark(system_is_dark())
    }

    fn work_area(&self) -> LogicalRect {
        self.anchor.work_area().unwrap_or_else(|_| {
            LogicalRect::new(LogicalPoint::new(0.0, 0.0), LogicalSize::new(1440.0, 900.0))
        })
    }

    fn run_tray(self, tray: Tray) -> Result<()> {
        run_event_loop(tray)
    }

    fn spawn_tray(self, tray: Tray) -> Result<()> {
        // The tray's HWND, message pump and thread-local state all live on the
        // thread that runs `run_event_loop`, so a dedicated background thread is
        // fully self-consistent; a `TrayHandle` drives it cross-thread by
        // `PostMessageW`-ing the (thread-safe) owner window.
        super::spawn_tray_thread(tray, run_event_loop)
    }

    fn open_popup_session(
        &mut self,
        menu: Menu,
        options: MenuOptions,
        on_click: &(dyn Fn(&MenuId) + '_),
        anchor: LogicalRect,
        edge: Edge,
    ) -> Result<()> {
        run_popup_session(menu, options, on_click, anchor, edge)
    }
}

/// Install the tray icon and run the native Win32 message pump, opening the
/// styled popup on click and dispatching row clicks to the tray's handler.
/// Consumes the [`Tray`]; returns when the pump exits (a `WM_QUIT`).
fn run_event_loop(mut tray: Tray) -> Result<()> {
    let hinstance = unsafe { GetModuleHandleW(null_mut()) };

    // Resolve the Explorer-restart broadcast id once.
    let taskbar_msg = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    TASKBAR_CREATED_MSG.store(taskbar_msg, Ordering::SeqCst);

    let mut anchor = WindowsAnchor::new();
    anchor.install(&tray.icon, tray.tooltip.as_deref())?;
    let owner = anchor.hwnd as isize;
    OWNER_HWND.with(|h| h.set(owner));
    // Also publish the owner in the thread-safe static so a UIA action raised on a
    // foreign thread can wake this pump (spec 30 §3, FIX 1).
    #[cfg(feature = "a11y")]
    A11Y_OWNER.store(owner, Ordering::SeqCst);

    // Install the TrayHandle waker so posts from any thread schedule a drain by
    // posting to the (thread-safe) owner window.
    if let Ok(mut waker) = tray.waker.lock() {
        *waker = Some(Box::new(move || unsafe {
            PostMessageW(owner as HWND, WM_MURI_DRAIN, 0, 0);
        }));
    }

    // Move the tray's click handler into the session's dispatch sink (owned for the
    // whole run loop, hence `'static`). Preserve `Tray::dispatch`'s behavior:
    // run the per-surface handler, then project the activation onto the global
    // `MenuEvent` channel (callers only ever invoke this for addressable ids).
    let dispatch: Box<dyn Fn(&MenuId) + 'static> = match tray.on_click.take() {
        Some(handler) => Box::new(move |id| {
            handler(id);
            crate::event::emit(id.clone());
        }),
        None => Box::new(|id| crate::event::emit(id.clone())),
    };

    let session = PopupSession {
        menu: tray.menu.clone(),
        options: tray.options.clone(),
        dispatch,
        anchor: Anchor::Tray(anchor),
        // Taskbars usually sit at the bottom, so the popup grows up from the icon.
        edge: Edge::Top,
        hinstance,
        popup: None,
        flyouts: Vec::new(),
        mouse_hook: null_mut(),
        kbd_hook: null_mut(),
    };

    // Keep an Arc to the waker slot before `tray` moves into `AppState`, so we
    // can clear the stale `WakeFn` once the pump exits — otherwise a surviving
    // `TrayHandle` would `PostMessageW` a destroyed owner HWND.
    let waker_arc = std::sync::Arc::clone(&tray.waker);

    let state = Rc::new(RefCell::new(AppState { session, tray }));
    MAIN_APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&state)));

    // Apply any commands a handle posted before the loop came up.
    push_drain(owner as HWND);

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    MAIN_APP.with(|slot| *slot.borrow_mut() = None);
    OWNER_HWND.with(|h| h.set(0));
    // Clear the waker so a `TrayHandle` outliving the pump can't post to the
    // now-destroyed owner window.
    if let Ok(mut waker) = waker_arc.lock() {
        *waker = None;
    }
    #[cfg(feature = "a11y")]
    A11Y_OWNER.store(0, Ordering::SeqCst);
    Ok(())
}

/// Post a bare drain request to the owner window.
fn push_drain(owner: HWND) {
    unsafe {
        PostMessageW(owner, WM_MURI_DRAIN, 0, 0);
    }
}

/// Run a pointer/rect-anchored [`ContextMenu`](crate::ContextMenu) /
/// [`Popup`](crate::Popup) session to completion (spec 21 §3): open the styled
/// popup at `anchor`, then pump the Win32 message loop — draining muri UI events
/// after each dispatched message — until the whole stack dismisses. Reuses the
/// shared [`PopupSession`]; the only difference from the tray is that the anchor is
/// a fixed rect, the handler is borrowed for the call rather than owned for a run
/// loop, and there is no persistent owner window / [`MAIN_APP`] / command inbox —
/// the pump drains `EVENTS` directly on the caller's stack. This is the Windows
/// analogue of the macOS `run_popup_session`.
fn run_popup_session(
    menu: Menu,
    options: MenuOptions,
    on_click: &(dyn Fn(&MenuId) + '_),
    anchor: LogicalRect,
    edge: Edge,
) -> Result<()> {
    let hinstance = unsafe { GetModuleHandleW(null_mut()) };
    let geom = WinGeometry::for_rect(anchor)
        .ok_or_else(|| Error::Platform("no monitor available for the popup".into()))?;

    let mut session = PopupSession {
        menu,
        options,
        dispatch: Box::new(move |id| on_click(id)),
        anchor: Anchor::Fixed(geom),
        edge,
        hinstance,
        popup: None,
        flyouts: Vec::new(),
        mouse_hook: null_mut(),
        kbd_hook: null_mut(),
    };
    session.open_popup();
    let Some(popup_hwnd) = session.popup.as_ref().map(|p| p.hwnd) else {
        return Err(Error::Platform("failed to open the popup window".into()));
    };

    // Route on-thread callbacks' drain posts at our own popup window so a global
    // hook event (`WH_MOUSE_LL` / `WH_KEYBOARD_LL`) wakes `GetMessageW` promptly.
    // Save and restore any prior owner (e.g. a tray loop) so a context menu opened
    // from within a tray callback doesn't strand the tray's inbox routing.
    let prev_owner = OWNER_HWND.with(|h| h.replace(popup_hwnd as isize));
    // Mirror the thread-safe owner so foreign-thread UIA actions wake this pump;
    // restore the prior value (e.g. a tray loop's) when the session ends.
    #[cfg(feature = "a11y")]
    let prev_a11y_owner = A11Y_OWNER.swap(popup_hwnd as isize, Ordering::SeqCst);

    // DEVICE-VERIFY(0.9.0): scoped modal pump. Unlike the tray, this drives the
    // popup with a bounded `GetMessageW` loop (no persistent owner window, no
    // `MAIN_APP`, no command inbox) so `open_at` / `anchored_to` blocks on the
    // caller's stack and returns when the menu dismisses. Callbacks still enqueue
    // into the shared `EVENTS` inbox, which we drain after each dispatched message;
    // that the `WS_EX_NOACTIVATE` popup + global hooks deliver these events as
    // expected can only be confirmed on a real Windows display.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while session.popup.is_some() {
            let got = GetMessageW(&mut msg, null_mut(), 0, 0);
            if got <= 0 {
                // `WM_QUIT` (0) or an error (-1): stop pumping and tear down below.
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            session.drain_events();
        }
    }

    // Ensure the windows + hooks are gone even if the pump exited on `WM_QUIT`
    // with the popup still open (`close_popup` is a no-op once already closed).
    session.close_popup();
    OWNER_HWND.with(|h| h.set(prev_owner));
    #[cfg(feature = "a11y")]
    A11Y_OWNER.store(prev_a11y_owner, Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_any_matches_windows_rects() {
        // Two windows: a popup and a flyout to its right.
        let rects = [(100, 200, 300, 500), (300, 220, 460, 420)];
        // Inside the popup.
        assert!(point_in_any(&rects, 150, 250));
        // Inside the flyout.
        assert!(point_in_any(&rects, 400, 300));
        // In the gap above / outside both → a dismissing click.
        assert!(!point_in_any(&rects, 150, 100));
        assert!(!point_in_any(&rects, 500, 300));
        // Right/bottom edges are exclusive (a click on the far edge is outside).
        assert!(!point_in_any(&[(0, 0, 10, 10)], 10, 5));
        assert!(!point_in_any(&[(0, 0, 10, 10)], 5, 10));
        assert!(point_in_any(&[(0, 0, 10, 10)], 0, 0));
    }

    // FIX 6: the N-level flyout reconciliation `apply_flyout_stack` performs —
    // keep the common prefix, close everything deeper, push the rest — must hold
    // beyond the single happy path. `common_flyout_prefix` is the pure decision it
    // rests on, so these guard the multi-level cases directly.
    #[test]
    fn flyout_reconcile_switches_to_shallower_sibling() {
        // A 3-deep stack switches to a shallower sibling under the first level:
        // level 0 is kept, levels 1 and 2 are closed, and the sibling is pushed.
        let current = [1, 2, 3];
        let target = [1, 5];
        let common = common_flyout_prefix(&current, &target);
        assert_eq!(common, 1, "only the shared level-0 parent is kept");
        // truncate_flyouts(common) would close depths >= 1 (the old 2 and 3)…
        assert_eq!(&current[common..], &[2, 3]);
        // …and the remaining target levels (the sibling 5) get pushed.
        assert_eq!(&target[common..], &[5]);
    }

    #[test]
    fn flyout_reconcile_rehovering_open_parent_keeps_children() {
        // Re-hovering an already-open parent chain yields an identical target, so
        // the entire stack is the common prefix: nothing is closed, nothing pushed
        // (no flyout flicker on a redundant hover).
        let current = [1, 2];
        let target = [1, 2];
        let common = common_flyout_prefix(&current, &target);
        assert_eq!(common, 2);
        assert!(current[common..].is_empty(), "no deeper level is closed");
        assert!(target[common..].is_empty(), "no level is re-pushed");
    }

    #[test]
    fn flyout_reconcile_switching_top_sibling_and_collapsing() {
        // Switching the top-level sibling shares nothing: the whole old stack is
        // closed and the new one is opened from scratch.
        assert_eq!(common_flyout_prefix(&[1, 2], &[5]), 0);
        // Extending an open parent one level deeper keeps it and pushes the child.
        assert_eq!(common_flyout_prefix(&[1], &[1, 2]), 1);
        // An empty target (outside-hover) collapses everything.
        assert_eq!(common_flyout_prefix(&[1, 2, 3], &[]), 0);
        // An empty current (nothing open) has no common prefix to keep.
        assert_eq!(common_flyout_prefix(&[], &[1, 2]), 0);
    }

    #[test]
    fn geometry_maps_logical_to_physical_round_trip() {
        let geom = WinGeometry {
            anchor: RECT {
                left: 1800,
                top: 1400,
                right: 1832,
                bottom: 1432,
            },
            work: RECT {
                left: 0,
                top: 0,
                right: 3840,
                bottom: 2100,
            },
            scale: 2.0,
        };
        // Work area in logical space is physical / scale.
        let wa = geom.work_area_local();
        assert_eq!(wa.size.width, 1920.0);
        assert_eq!(wa.size.height, 1050.0);
        // A logical origin converts back to physical by * scale.
        let (px, py) = geom.to_physical(LogicalPoint::new(100.0, 50.0));
        assert_eq!((px, py), (200, 100));
        // The anchor is below the work area (taskbar), as expected on Windows.
        let anchor = geom.anchor_rect_local();
        assert_eq!(anchor.origin.y, 700.0);
    }

    // FIX 2 (conformance): `fill_tip` must always leave a NUL terminator, even
    // when the tooltip encodes to ≥128 UTF-16 units — otherwise Win32 reads past
    // `szTip`.
    #[test]
    fn fill_tip_always_nul_terminates_when_overlong() {
        let long = "A".repeat(200);
        let mut buf = [0u16; 128];
        WindowsAnchor::fill_tip(&long, &mut buf);
        // The final slot is a guaranteed NUL terminator.
        assert_eq!(
            buf[buf.len() - 1],
            0,
            "the last szTip slot must remain a NUL terminator"
        );
        // The 127 usable slots are filled with content (no truncation to nothing).
        assert_eq!(buf[0], u16::from(b'A'));
        assert_eq!(buf[126], u16::from(b'A'));
        assert_ne!(buf[126], 0);
    }

    #[test]
    fn fill_tip_terminates_a_short_tooltip_too() {
        let mut buf = [0u16; 128];
        WindowsAnchor::fill_tip("Hi", &mut buf);
        assert_eq!(buf[0], u16::from(b'H'));
        assert_eq!(buf[1], u16::from(b'i'));
        // `wide` appends its own NUL, and the rest stays zero.
        assert_eq!(buf[2], 0);
        assert_eq!(buf[127], 0);
    }

    // FIX 1 (correctness): a 2-level-nested menu must open the nested flyout and
    // dispatch the *deep leaf* id — not the submenu row's id, and not resolved
    // against the top-level menu. We prove the N-level resolution the fixed click
    // and keyboard paths rely on, without creating real windows.
    #[test]
    fn nested_submenu_opens_deep_flyout_and_dispatches_deep_leaf() {
        use crate::menu::Row;

        // top:  0 "a"
        //       1 "outer" ->  0 "inner_leaf"
        //                     1 "inner_leaf2"
        //                     2 "middle" -> 0 "deep_leaf"
        //       2 "quit"
        //
        // `middle` sits at index 2 *within the outer flyout*; index 2 in the
        // *top* menu is the non-submenu "quit" — so a flyout-local index resolved
        // against the top menu (the single-level bug) lands on the wrong item.
        let menu = Menu::new()
            .row(Row::new("a").label("Apple"))
            .submenu(
                Row::new("outer").label("Outer"),
                Menu::new()
                    .row(Row::new("inner_leaf").label("Inner Leaf"))
                    .row(Row::new("inner_leaf2").label("Inner Leaf 2"))
                    .submenu(
                        Row::new("middle").label("Middle"),
                        Menu::new().row(Row::new("deep_leaf").label("Deep Leaf")),
                    ),
            )
            .row(Row::new("quit").label("Quit"));
        let outer = 1; // submenu index in the top-level menu
        let middle = 2; // submenu index *within* the outer flyout

        // Level descent (drives `menu_at_level`, hence every click / key path).
        assert_eq!(menu_at_stack(&menu, &[]).unwrap().items.len(), 3);
        let level1 = menu_at_stack(&menu, &[outer]).expect("outer is a submenu");
        // The nested submenu row is detected as a *submenu* at flyout level 1, so
        // `on_click`'s flyout branch opens the next level instead of dispatching.
        assert!(
            matches!(level1.items.get(middle), Some(Item::Submenu { .. })),
            "clicking the nested submenu row must open a deeper flyout"
        );
        // The single-level bug in the flesh: resolving the flyout-local `middle`
        // index against the *top* menu (as the old `open_flyout(i)` / flyout-click
        // path did) hits "quit", a non-submenu — so it can never open the nested
        // flyout. Only the deepest-level resolution is correct.
        assert!(
            menu_at_stack(&menu, &[middle]).is_none(),
            "top-level resolution of a flyout-local submenu index must fail"
        );

        // The deepest level's leaf carries the deep id — what actually gets
        // dispatched — not the "middle" submenu row's id.
        let deepest = menu_at_stack(&menu, &[outer, middle]).expect("middle is a submenu");
        match deepest.items.first() {
            Some(Item::Row(row)) => assert_eq!(row.id, MenuId::from("deep_leaf")),
            other => panic!("expected the deep leaf row, got {other:?}"),
        }

        // Mouse: hovering the nested submenu row inside flyout level 1 grows the
        // open stack to two levels (decision #8), so a click there opens it.
        let stack_after_hover = next_flyout(
            &[outer],
            HoverTarget::ParentRow {
                panel: 1,
                index: middle,
            },
        );
        assert_eq!(stack_after_hover, vec![outer, middle]);

        // Keyboard: with the outer flyout open + the nested submenu selected,
        // Right descends. `handle_key` returns the *flyout-local* index and pushes
        // the frame, so `push_flyout` (which opens from the deepest open level)
        // resolves it correctly — the exact index the old `open_flyout(i)`, which
        // resolved against the top menu, would have mis-opened.
        let mut focus = MenuFocus {
            top: Some(outer),
            flyout: vec![FlyoutFocus {
                parent: outer,
                child: Some(middle),
            }],
        };
        let action = handle_key(&menu, &mut focus, NavKey::Right);
        assert_eq!(action, NavAction::OpenFlyout(middle));
        let parents: Vec<usize> = focus.flyout.iter().map(|f| f.parent).collect();
        assert_eq!(
            parents,
            vec![outer, middle],
            "keyboard descent must build the same 2-level stack the mouse does"
        );
        // And resolving that keyboard stack lands on the deep leaf.
        let deepest = menu_at_stack(&menu, &parents).expect("keyboard stack resolves");
        match deepest.items.first() {
            Some(Item::Row(row)) => assert_eq!(row.id, MenuId::from("deep_leaf")),
            other => panic!("expected the deep leaf row, got {other:?}"),
        }
    }
}
