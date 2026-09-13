//! Native popup / flyout window construction (spec 21 §2).
//!
//! Each popup and flyout is a borderless `WS_POPUP` window with the extended
//! styles that make it behave like a native menu surface:
//!
//! - `WS_EX_NOACTIVATE` — shown/clicked without stealing foreground activation
//!   (macOS's non-activating panel), so edit actions keep targeting the
//!   previously focused window.
//! - `WS_EX_TOOLWINDOW` — kept out of the Alt-Tab list and the taskbar.
//! - `WS_EX_LAYERED` — enables the per-pixel-alpha [`present`](super::present)
//!   blit so rounded corners and the acrylic body work.
//! - `WS_EX_TOPMOST` — floats above the foreground app like a real menu.
//!
//! On Windows 11 the window also requests the transient-window acrylic system
//! backdrop (decision #6); where unavailable the OS ignores it and the layered
//! blit still shows the drawer's fallback fill.

use std::ptr::null_mut;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{HINSTANCE, HWND};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetWindowLongPtrW, SetWindowPos, GWLP_USERDATA, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

/// Create a hidden, non-activating, layered, top-most popup window of the popup
/// class, tagged with `kind_tag` in `GWLP_USERDATA` so the shared window
/// procedure can route its events. The window is sized/positioned later by the
/// [`present`](super::present) blit; return `null` on failure.
///
/// # Safety
/// `class_name` must name a class registered with the shared window procedure on
/// this thread, and `hinstance` must be the owning module handle.
pub(super) unsafe fn create_popup(
    class_name: PCWSTR,
    hinstance: HINSTANCE,
    kind_tag: isize,
) -> HWND {
    let ex_style = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TOPMOST;
    let hwnd = CreateWindowExW(
        ex_style,
        class_name,
        null_mut(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        hinstance,
        null_mut(),
    );
    if hwnd.is_null() {
        return hwnd;
    }
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, kind_tag);
    apply_backdrop(hwnd);
    hwnd
}

/// Request the transient-window acrylic system backdrop. Best-effort: the call is
/// a no-op on OS builds that don't support it, and the layered blit remains the
/// source of truth for the panel pixels.
///
// DEVICE-VERIFY(0.9.0): the acrylic blur only appears on a real Windows 11
// compositor; on older builds / disabled-transparency the panel falls back to
// the drawer's (tinted or opaque) fill, which cannot be observed off-device.
unsafe fn apply_backdrop(hwnd: HWND) {
    let backdrop: i32 = DWMSBT_TRANSIENTWINDOW;
    // Ignore the HRESULT: an unsupported attribute must not fail popup creation.
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE as u32,
        (&backdrop as *const i32).cast(),
        std::mem::size_of::<i32>() as u32,
    );
}

/// Reveal `hwnd` at the top of the z-order without activating it (so the user's
/// foreground app keeps focus). The pixels + position are already set by the
/// preceding [`present`](super::present) blit.
///
/// # Safety
/// `hwnd` must be a live window owned by the calling thread.
pub(super) unsafe fn raise_topmost(hwnd: HWND) {
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
}
