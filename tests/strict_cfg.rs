//! Guards the "no scattered `#[cfg(target_os)]` outside `src/platform/*`" rule
//! at `cargo test` time, so an ADR-0002 violation is caught locally rather than
//! only in review. All host-OS-specific behavior must be routed through the
//! `Platform` trait (see `src/platform/mod.rs`) and implemented in
//! `src/platform/{mac,windows,linux}.rs`.
//!
//! The equivalent one-liner is:
//!
//! ```sh
//! grep -rn 'cfg(target_os' src/ | grep -v '^src/platform/'
//! ```

use std::fs;
use std::path::{Path, PathBuf};

/// Patterns that indicate a direct OS check. Covers the attribute form
/// (`#[cfg(target_os = "...")]`), nested forms (`#[cfg(all(target_os = ...`,
/// `#[cfg(any(target_os = ...`, `#[cfg(not(target_os = ...`), and the
/// expression-macro form (`cfg!(target_os = "...")`). Each nested wrapper needs
/// its own needle — "cfg(not(target_os" does NOT contain "cfg(target_os" as a
/// contiguous substring (there's a `not(` in between), so a single
/// "cfg(target_os" needle would silently miss it. (Plain `cfg(unix)` /
/// `cfg(windows)` — no `target_os` — are NOT covered here and don't need to be:
/// those are the sanctioned per-OS-family escape hatch for code that merely
/// varies by a std-lib-recognized family, not a `Platform`-trait-sized branch.)
const NEEDLES: &[&str] = &[
    "cfg(target_os",
    "cfg!(target_os",
    "cfg(not(target_os",
    "cfg(all(target_os",
    "cfg(any(target_os",
];

/// Documented exceptions. Every entry is `(relative path, line number)`.
/// Additions require a comment explaining WHY the site can't route through the
/// `Platform` trait yet, and either a linked issue or a note explaining what
/// would need to happen for the exception to go away.
///
/// muri currently has **no** exceptions: every `target_os` gate lives inside
/// `src/platform/` (the one place ADR-0002 permits it), so this list is empty.
const ALLOWLIST: &[(&str, u32)] = &[];

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Recursively collect every `.rs` file under `dir`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_scattered_target_os_cfg_outside_platform_module() {
    let root = src_root();
    let platform_dir = root.join("platform");

    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allowlist: std::collections::HashSet<(PathBuf, u32)> = ALLOWLIST
        .iter()
        .map(|(rel, line)| (manifest_dir.join(rel), *line))
        .collect();

    let mut violations = Vec::new();
    for path in &files {
        if path.starts_with(&platform_dir) {
            continue;
        }
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        for (lineno, line) in contents.lines().enumerate() {
            // Skip comment lines — a doc-comment example / rationale that
            // MENTIONS `#[cfg(target_os = ...)]` is not itself a violation.
            let stripped = line.trim_start();
            if stripped.starts_with("//") {
                continue;
            }
            if NEEDLES.iter().any(|needle| line.contains(needle)) {
                let line_1based = (lineno + 1) as u32;
                if allowlist.contains(&(path.clone(), line_1based)) {
                    continue;
                }
                violations.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_1based,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "found #[cfg(target_os)] / cfg!(target_os) outside src/platform/ — \
         route platform-specific behavior through the Platform trait \
         (src/platform/mod.rs) instead:\n{}",
        violations.join("\n")
    );
}
