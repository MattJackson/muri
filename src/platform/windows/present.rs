//! Present path: blit the shared [`Framebuffer`] to a layered popup window
//! with per-pixel alpha via `UpdateLayeredWindow`.
//!
//! The `RasterDrawer` framebuffer is premultiplied RGBA (byte order R, G, B, A).
//! A layered window with `ULW_ALPHA` + an `AC_SRC_ALPHA` blend wants a **top-down
//! 32-bit premultiplied BGRA** DIB, so the only conversion is the R/B channel
//! swap — the premultiplication the raster blitter already did is exactly what
//! the layered blit expects, so the rounded-corner transparency and any
//! reduced-alpha panel body composite over whatever is behind the window (the
//! acrylic backdrop, spec 21 §2). No opaque flatten happens here.
//!
//! `UpdateLayeredWindow` also *positions* the window (via `pptdst`), so one call
//! both moves and paints it — there is no separate `SetWindowPos` for geometry.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use crate::render::Framebuffer;

/// Blit `fb` (premultiplied RGBA, device pixels) to `hwnd` as a layered window,
/// positioning its top-left at the physical screen point `(px, py)`.
///
/// Returns `false` if any GDI object could not be created (the caller keeps the
/// window hidden rather than showing an unpainted frame).
///
/// # Safety
/// `hwnd` must be a live `WS_EX_LAYERED` window owned by the calling thread.
pub(super) unsafe fn present_layered(hwnd: HWND, fb: &Framebuffer, px: i32, py: i32) -> bool {
    let w = fb.width() as i32;
    let h = fb.height() as i32;
    if w == 0 || h == 0 {
        return false;
    }

    let screen_dc = GetDC(null_mut());
    if screen_dc.is_null() {
        return false;
    }
    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.is_null() {
        ReleaseDC(null_mut(), screen_dc);
        return false;
    }

    // Top-down (negative height) 32-bit DIB so row 0 is the top scanline, matching
    // the pixmap's row-major layout.
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
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
    let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if dib.is_null() || bits.is_null() {
        DeleteDC(mem_dc);
        ReleaseDC(null_mut(), screen_dc);
        return false;
    }

    // R,G,B,A (premultiplied) → B,G,R,A (premultiplied): the only conversion the
    // layered blit needs.
    {
        let src = fb.pixels();
        let n = (w * h) as usize;
        let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), n * 4);
        for i in 0..n {
            let s = i * 4;
            dst[s] = src[s + 2]; // B
            dst[s + 1] = src[s + 1]; // G
            dst[s + 2] = src[s]; // R
            dst[s + 3] = src[s + 3]; // A
        }
    }

    let old = SelectObject(mem_dc, dib as HGDIOBJ);

    let dst_pt = POINT { x: px, y: py };
    let src_pt = POINT { x: 0, y: 0 };
    let size = SIZE { cx: w, cy: h };
    let blend = windows_sys::Win32::Graphics::Gdi::BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };

    let ok = UpdateLayeredWindow(
        hwnd, screen_dc, &dst_pt, &size, mem_dc, &src_pt, 0, &blend, ULW_ALPHA,
    ) != 0;

    // Restore + release everything (the DIB's pixels were copied into the window).
    SelectObject(mem_dc, old);
    DeleteObject(dib as HGDIOBJ);
    DeleteDC(mem_dc);
    ReleaseDC(null_mut(), screen_dc);
    ok
}
