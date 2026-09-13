//! muda + `tray-icon` **compatibility facade** (spec `02`, `60`) — behind the
//! `muda-compat` feature.
//!
//! This module mirrors muda's and `tray-icon`'s module layout and type *names* so
//! an unchanged app can change only its imports
//! (`use muda::…` → `use muri::compat::muda::…`,
//! `use tray_icon::…` → `use muri::compat::tray_icon::…`) and compile. Every
//! facade type maps **onto muri's native model** ([`crate::menu`],
//! [`crate::Tray`], [`crate::ContextMenu`]); it does **not** depend on the real
//! `muda` / `tray-icon` crates (locked decision #5). Events are emitted by muri
//! itself and projected onto the process-global channel in [`crate::event`].
//!
//! ## Scope of this (M3) implementation
//!
//! The routing decision, item-type translation onto muri's `Item`/`Row` tree,
//! muda's `MenuId` auto-generation semantics, and the unified `MenuEvent`
//! channel are implemented and unit-tested here (spec `50` §2.6). Actual native
//! menu-bar materialization lands per the milestone ladder; the facade is
//! honest about the documented divergences (D1–D9, spec `02` §9).

pub mod muda;
pub mod tray_icon;

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

/// Max distinct icons retained in the encode cache. Menus/trays carry only a
/// handful of icons; this bounds the cache so it can never grow without limit.
const ENCODE_CACHE_CAP: usize = 32;

#[allow(clippy::type_complexity)]
fn encode_cache() -> &'static Mutex<Vec<(u32, u32, u64, Arc<[u8]>)>> {
    static CACHE: OnceLock<Mutex<Vec<(u32, u32, u64, Arc<[u8]>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Encode raw straight-alpha RGBA to PNG, returning a **stable** `Arc` for
/// identical `(width, height, bytes)` from a small bounded cache.
///
/// The muda/tray-icon translation re-runs on every `set_menu` / `set_icon`, so
/// encoding an unchanged logo each time wastes CPU and defeats the render
/// layer's `Arc`-pointer decode cache (a fresh `Vec` makes a fresh `Arc` that
/// never matches the previous frame's key). Returning the same `Arc` for
/// identical pixels fixes both. Returns `None` for a zero dimension or a length
/// mismatch (delegating to [`crate::render::encode_rgba_png`]).
pub(crate) fn encode_rgba_cached(rgba: &[u8], width: u32, height: u32) -> Option<Arc<[u8]>> {
    let key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        rgba.hash(&mut hasher);
        hasher.finish()
    };
    let Ok(mut cache) = encode_cache().lock() else {
        // Poisoned lock: fall back to a one-off uncached encode rather than panic.
        return crate::render::encode_rgba_png(rgba, width, height).map(Arc::from);
    };
    // Match on dimensions + a 64-bit content hash (collision on identical-length
    // content is negligible for the small icons this carries).
    if let Some((_, _, _, arc)) = cache
        .iter()
        .find(|(w, h, k, _)| *w == width && *h == height && *k == key)
    {
        return Some(Arc::clone(arc));
    }
    let arc: Arc<[u8]> = crate::render::encode_rgba_png(rgba, width, height)?.into();
    if cache.len() >= ENCODE_CACHE_CAP {
        cache.remove(0); // FIFO eviction of the oldest entry.
    }
    cache.push((width, height, key, Arc::clone(&arc)));
    Some(arc)
}

#[cfg(test)]
mod encode_cache_tests {
    use super::*;

    #[test]
    fn identical_rgba_returns_the_same_arc() {
        let rgba = vec![1, 2, 3, 255, 4, 5, 6, 128];
        let a = encode_rgba_cached(&rgba, 2, 1).expect("encodes");
        let b = encode_rgba_cached(&rgba.clone(), 2, 1).expect("encodes again");
        // Same content -> same cached Arc, so the render decode cache can hit and
        // the PNG encoder runs only once.
        assert!(
            Arc::ptr_eq(&a, &b),
            "identical RGBA must reuse the cached Arc"
        );
        // And it decodes back to the source pixels.
        let (decoded, w, h) = crate::render::decode_png(&a).expect("decodes");
        assert_eq!((w, h), (2, 1));
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn zero_dimension_returns_none() {
        assert!(encode_rgba_cached(&[], 0, 1).is_none());
    }
}
