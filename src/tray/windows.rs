//! Windows tray anchoring via `Shell_NotifyIcon` + `Shell_NotifyIconGetRect`.
//!
//! `Shell_NotifyIcon(NIM_ADD, …)` registers a notification-area icon owned by a
//! window we create; `Shell_NotifyIconGetRect` then returns that icon's on-screen
//! rectangle given a `NOTIFYICONIDENTIFIER`. muri converts the rect to logical
//! coordinates (dividing by the window's DPI) and feeds it to the shared,
//! unit-tested [`place_popup`](crate::anchor::place_popup) math, which will
//! position a `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` layered popup toward screen
//! center. Outside-click dismiss will use a `WH_MOUSE_LL` hook plus
//! `WM_ACTIVATEAPP`.
//!
//! Because the icon must belong to a window, [`install`](TrayAnchor::install)
//! creates a **message-only** window (parented to `HWND_MESSAGE`) purely to own
//! the icon and receive its callback messages. The popup event loop itself and
//! decoding a muri PNG [`Icon`](crate::menu::Icon) into an `HICON` are still
//! `todo!()`; installation currently shows the stock application icon.
//!
//! Built with `windows-sys` and compile-verified for `x86_64-pc-windows-msvc`.

#![allow(unsafe_code)]

use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, RECT, S_OK};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, LoadIconW, RegisterClassW, HWND_MESSAGE,
    IDI_APPLICATION, WM_APP, WNDCLASSW,
};

use crate::anchor::place_popup;
use crate::error::{Error, Result};
use crate::geometry::{Edge, LogicalPoint, LogicalRect, LogicalSize};
use crate::tray::TrayAnchor;

/// The private window message the tray icon posts back to its owner window.
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
/// The notification icon's id within its owner window.
const TRAY_ICON_UID: u32 = 0x0001;

/// Whether the message-only window class has been registered this process.
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Encode a Rust string as a NUL-terminated UTF-16 buffer for the Win32 `*W`
/// APIs.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The message-only window procedure. It only needs to hand every message to the
/// default handler; the tray callback is polled elsewhere once the event loop
/// lands.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// The Windows notification-area anchor. Owns the message-only window and the
/// registered icon until dropped.
pub struct WindowsAnchor {
    /// The message-only window that owns the notification icon.
    hwnd: HWND,
    /// Whether the icon is currently registered (so `Drop` can remove it).
    installed: bool,
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
        }
    }

    /// Create the message-only window that owns the notification icon.
    unsafe fn create_message_window(&mut self) -> Result<()> {
        let hinstance = GetModuleHandleW(null_mut());
        let class_name = wide("muri_tray_msgwnd");

        if !CLASS_REGISTERED.swap(true, Ordering::SeqCst) {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wnd_proc);
            wc.hInstance = hinstance;
            wc.lpszClassName = class_name.as_ptr();
            // A zero atom means the class already existed (benign) or failed; the
            // subsequent CreateWindowExW is the real success signal.
            RegisterClassW(&wc);
        }

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
}

impl TrayAnchor for WindowsAnchor {
    fn install(&mut self, tooltip: Option<&str>) -> Result<()> {
        unsafe {
            self.create_message_window()?;

            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = TRAY_ICON_UID;
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY_CALLBACK;
            // TODO: decode the muri PNG `Icon` into an HICON; use the stock
            // application icon until then so the icon is visible.
            nid.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);

            if let Some(tip) = tooltip {
                let tip = wide(tip);
                let n = tip.len().min(nid.szTip.len());
                nid.szTip[..n].copy_from_slice(&tip[..n]);
            }

            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                let _ = DestroyWindow(self.hwnd);
                self.hwnd = null_mut();
                return Err(Error::Platform("Shell_NotifyIcon(NIM_ADD) failed".into()));
            }
            self.installed = true;
        }
        Ok(())
    }

    fn anchor_rect(&self) -> Result<LogicalRect> {
        if !self.installed {
            return Err(Error::Platform("tray icon not installed".into()));
        }
        let id = self.identifier();
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let hr = unsafe { Shell_NotifyIconGetRect(&id, &mut rect) };
        if hr != S_OK {
            return Err(Error::Platform("Shell_NotifyIconGetRect failed".into()));
        }
        // Convert the physical-pixel rect to logical coordinates via the owner
        // window's DPI (96 dpi == scale 1.0).
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let origin = LogicalPoint::new(rect.left as f32 / scale, rect.top as f32 / scale);
        let size = LogicalSize::new(
            (rect.right - rect.left) as f32 / scale,
            (rect.bottom - rect.top) as f32 / scale,
        );
        Ok(LogicalRect::new(origin, size))
    }

    fn supports_tray_anchor(&self) -> bool {
        true
    }
}

impl WindowsAnchor {
    /// Where a popup of `popup` size should open, anchored to the tray icon and
    /// clamped into `work_area`. A convenience wrapper over the shared
    /// [`place_popup`] so the (future) Windows event loop shares the exact
    /// placement logic macOS uses. Taskbars usually sit at the bottom, so the
    /// popup grows up from the icon ([`Edge::Top`]).
    pub fn popup_origin(&self, popup: LogicalSize, work_area: LogicalRect) -> Result<LogicalPoint> {
        let anchor = self.anchor_rect()?;
        Ok(place_popup(anchor, popup, work_area, Edge::Top, 2.0))
    }
}

impl Drop for WindowsAnchor {
    fn drop(&mut self) {
        unsafe {
            if self.installed {
                let id = self.identifier();
                let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
                nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                nid.hWnd = id.hWnd;
                nid.uID = id.uID;
                Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            if !self.hwnd.is_null() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
