//! Shared golden-image compare harness used by `tests/golden.rs`.
//!
//! This lives under `tests/support/` (a directory module, not a top-level
//! `tests/*.rs` file) so Cargo does not treat it as its own test binary — it is
//! `mod`-included by the actual test files that need it.

use std::path::PathBuf;

use muri::render::{decode_png, Framebuffer};

/// Per-channel tolerance for a single pixel: anti-aliased glyph edges and
/// rounded-rect corners can differ by a couple of least-significant bits across
/// `swash` patch versions even with byte-identical inputs (see
/// `docs/design/spec/50-testing-verification.md` §3.2). A named constant, not a
/// magic number, so the honesty budget is visible to a reviewer.
///
/// The comparison runs in **straight-alpha** space: the golden PNG is decoded to
/// straight RGBA and the actual [`Framebuffer`] is un-premultiplied the same way
/// it is for PNG encode, so the round-trip is exact for a deterministic render.
pub const CHANNEL_TOLERANCE: u8 = 2;

/// The fraction of pixels allowed to exceed [`CHANNEL_TOLERANCE`] before a
/// golden comparison fails. Kept tiny but non-zero for the same reason.
pub const MAX_MISMATCH_FRACTION: f64 = 0.001; // 0.1%

/// Env var that, when set (to anything), makes [`assert_golden`] (re)write the
/// reference PNG instead of comparing against it. Mirrors `insta`'s
/// `INSTA_UPDATE` convention; never read by a normal `cargo test` run.
pub const UPDATE_ENV_VAR: &str = "MURI_UPDATE_SNAPSHOTS";

fn snapshots_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn target_dir() -> PathBuf {
    // CARGO_TARGET_TMPDIR is a per-test-binary tmp dir under target/; walk up
    // to the shared target/ so a failed run's actual PNG is easy to find.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    if !p.exists() {
        // Workspace-less crate: target/ is always a sibling of Cargo.toml, but
        // fall back defensively in case that ever changes.
        p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    }
    p
}

/// Compare `actual` against the committed golden `tests/snapshots/<name>.png`.
///
/// With `MURI_UPDATE_SNAPSHOTS` set, (re)writes the reference instead of
/// comparing. Otherwise, decodes the reference PNG and asserts every pixel is
/// within [`CHANNEL_TOLERANCE`] per channel, with at most [`MAX_MISMATCH_FRACTION`]
/// of pixels allowed to exceed it. On any failure the actual render is written
/// to `target/<name>-actual.png` for inspection and the panic message says so.
#[allow(dead_code)]
pub fn assert_golden(name: &str, actual: &Framebuffer) {
    let path = snapshots_dir().join(format!("{name}.png"));

    if std::env::var_os(UPDATE_ENV_VAR).is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshots dir"))
            .expect("create tests/snapshots/");
        std::fs::write(&path, actual.encode_png())
            .unwrap_or_else(|e| panic!("write golden {}: {e}", path.display()));
        eprintln!("wrote golden {}", path.display());
        return;
    }

    let reference_bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden reference {} ({e}); run with {UPDATE_ENV_VAR}=1 once to create it",
            path.display()
        )
    });
    let (ref_rgba, ref_w, ref_h) = decode_png(&reference_bytes)
        .unwrap_or_else(|| panic!("decode golden reference {}", path.display()));

    if ref_w != actual.width() || ref_h != actual.height() {
        write_actual(name, actual);
        panic!(
            "golden '{name}' size mismatch: reference {}x{}, actual {}x{} \
             (actual written to target/{name}-actual.png)",
            ref_w,
            ref_h,
            actual.width(),
            actual.height()
        );
    }

    let actual_rgba = actual.to_straight_rgba();
    let total = (actual.width() as usize) * (actual.height() as usize);
    let mismatched = ref_rgba
        .chunks_exact(4)
        .zip(actual_rgba.chunks_exact(4))
        .filter(|(r, a)| {
            r[0].abs_diff(a[0]) > CHANNEL_TOLERANCE
                || r[1].abs_diff(a[1]) > CHANNEL_TOLERANCE
                || r[2].abs_diff(a[2]) > CHANNEL_TOLERANCE
                || r[3].abs_diff(a[3]) > CHANNEL_TOLERANCE
        })
        .count();

    let fraction = mismatched as f64 / total as f64;
    if fraction > MAX_MISMATCH_FRACTION {
        write_actual(name, actual);
        panic!(
            "golden '{name}' mismatch: {mismatched}/{total} pixels ({:.4}%) differ by more than \
             {CHANNEL_TOLERANCE}/255 in some channel (max allowed {:.4}%); actual written to \
             target/{name}-actual.png for inspection",
            fraction * 100.0,
            MAX_MISMATCH_FRACTION * 100.0
        );
    }
}

/// Like [`assert_golden`], but compares a straight-alpha RGBA8 **PNG** (as
/// produced by the display-free public API [`muri::render_menu_to_png`]) against
/// the committed golden, rather than a live [`Framebuffer`]. Both sides are
/// decoded to straight RGBA and compared with the same tolerance budget, so this
/// exercises the real public entry point end-to-end. Honors
/// `MURI_UPDATE_SNAPSHOTS` the same way.
#[allow(dead_code)]
pub fn assert_golden_png(name: &str, png: &[u8]) {
    let path = snapshots_dir().join(format!("{name}.png"));

    if std::env::var_os(UPDATE_ENV_VAR).is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshots dir"))
            .expect("create tests/snapshots/");
        std::fs::write(&path, png)
            .unwrap_or_else(|e| panic!("write golden {}: {e}", path.display()));
        eprintln!("wrote golden {}", path.display());
        return;
    }

    let (act_rgba, act_w, act_h) = decode_png(png).expect("decode actual PNG");
    let reference_bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden reference {} ({e}); run with {UPDATE_ENV_VAR}=1 once to create it",
            path.display()
        )
    });
    let (ref_rgba, ref_w, ref_h) = decode_png(&reference_bytes)
        .unwrap_or_else(|| panic!("decode golden reference {}", path.display()));

    if (ref_w, ref_h) != (act_w, act_h) {
        write_actual_png(name, png);
        panic!(
            "golden '{name}' size mismatch: reference {ref_w}x{ref_h}, actual {act_w}x{act_h} \
             (actual written to target/{name}-actual.png)"
        );
    }

    let total = (act_w as usize) * (act_h as usize);
    let mismatched = ref_rgba
        .chunks_exact(4)
        .zip(act_rgba.chunks_exact(4))
        .filter(|(r, a)| {
            r[0].abs_diff(a[0]) > CHANNEL_TOLERANCE
                || r[1].abs_diff(a[1]) > CHANNEL_TOLERANCE
                || r[2].abs_diff(a[2]) > CHANNEL_TOLERANCE
                || r[3].abs_diff(a[3]) > CHANNEL_TOLERANCE
        })
        .count();

    let fraction = mismatched as f64 / total as f64;
    if fraction > MAX_MISMATCH_FRACTION {
        write_actual_png(name, png);
        panic!(
            "golden '{name}' mismatch: {mismatched}/{total} pixels ({:.4}%) differ by more than \
             {CHANNEL_TOLERANCE}/255 in some channel (max allowed {:.4}%); actual written to \
             target/{name}-actual.png for inspection",
            fraction * 100.0,
            MAX_MISMATCH_FRACTION * 100.0
        );
    }
}

#[allow(dead_code)]
fn write_actual_png(name: &str, png: &[u8]) {
    let dir = target_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{name}-actual.png")), png);
}

#[allow(dead_code)]
fn write_actual(name: &str, fb: &Framebuffer) {
    let dir = target_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}-actual.png"));
    let _ = std::fs::write(&path, fb.encode_png());
}
