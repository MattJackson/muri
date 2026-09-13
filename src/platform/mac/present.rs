//! Present path: blit the shared [`Framebuffer`] to a panel's content view
//! via a `CALayer`, replacing the old `softbuffer` framebuffer.
//!
//! The `RasterDrawer` framebuffer is premultiplied straight-alpha RGBA (byte
//! order R, G, B, A). We wrap its bytes in a `CGImage` and set that as the
//! content view's `layer.contents`. Because the image carries per-pixel alpha,
//! the rounded-corner transparency and any reduced-alpha panel body composite
//! over the `NSVisualEffectView` backdrop — the OS supplies the vibrancy blur
//! behind the raster, exactly as spec 20 §2 requires. No opaque flatten happens
//! here (that was the shipped bug that hid vibrancy).

use core::ptr;

use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};
use objc2_quartz_core::CATransaction;

use crate::render::Framebuffer;

/// The device-RGB color space, created once per thread and cheaply retained on
/// each present. The color space is constant, so recreating it on every frame
/// (the present runs on every hover row-change) was pure waste; a `CFRetained`
/// clone is a refcount bump, not a rebuild. The present path is main-thread only,
/// so a `thread_local` sidesteps `Send`/`Sync` on the CF wrapper.
fn device_rgb_color_space() -> Option<CFRetained<CGColorSpace>> {
    thread_local! {
        static DEVICE_RGB: Option<CFRetained<CGColorSpace>> = CGColorSpace::new_device_rgb();
    }
    DEVICE_RGB.with(|cs| cs.clone())
}

/// Build a `CGImage` from a premultiplied-RGBA framebuffer. The returned image
/// retains a copy of the pixel bytes (via the `CFData` its data provider
/// holds), so it stays valid after the framebuffer is dropped; keep it alive for
/// as long as it is the layer's contents.
pub(super) fn framebuffer_to_cgimage(fb: &Framebuffer) -> Option<CFRetained<CGImage>> {
    let w = fb.width() as usize;
    let h = fb.height() as usize;
    if w == 0 || h == 0 {
        return None;
    }
    let data = fb.pixels();
    // `CFDataCreate` copies the bytes, so the framebuffer may be freed afterwards.
    let cfdata = unsafe { CFData::new(None, data.as_ptr(), data.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&cfdata))?;
    let color_space = device_rgb_color_space()?;
    // Alpha last (RGBA), premultiplied — matches the framebuffer's pixel format.
    let bitmap_info = CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0);
    unsafe {
        CGImage::new(
            w,
            h,
            8,
            32,
            w * 4,
            Some(&color_space),
            bitmap_info,
            Some(&provider),
            ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

/// Set `image` as the layer contents of a layer-backed content view and pin the
/// contents scale to the device scale so device pixels map back to points.
pub(super) fn set_layer_contents(view: &NSView, image: &CGImage, scale: f32) {
    let Some(layer) = view.layer() else {
        return;
    };
    // A `CGImageRef` is toll-free acceptable as `CALayer.contents`.
    let obj: *const AnyObject = (image as *const CGImage).cast();
    // `contents` is an animatable CALayer property: assigning it outside a
    // `drawRect:` cycle triggers Core Animation's DEFAULT implicit action — a
    // ~0.25s cross-fade — on every swap. On hover that makes each highlight fade
    // in mushily and feel laggy (the native menu highlights instantly). Wrap the
    // update in a `CATransaction` with actions disabled so the contents swap is
    // immediate, matching the OEM menu's instant highlight.
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    unsafe {
        layer.setContents(Some(&*obj));
    }
    layer.setContentsScale(scale as f64);
    CATransaction::commit();
}
