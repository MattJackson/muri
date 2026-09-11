//! Error types for muri's fallible operations.

/// Things muri genuinely cannot do on a given platform. Surfaced rather than
/// papered over, so consumers can fall back deliberately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Anchoring a styled popup to a tray icon. Returned on Linux/Wayland, where
    /// the SNI/AppIndicator host owns the icon and no geometry or click
    /// coordinate reaches the app. Use a native-menu fallback or a
    /// pointer-anchored [`ContextMenu`](crate::ContextMenu) instead.
    TrayAnchor,
    /// Positioning a non-activating toplevel, which Wayland forbids by protocol.
    ClientPositioning,
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsupported::TrayAnchor => f.write_str("tray-anchored styled popup"),
            Unsupported::ClientPositioning => f.write_str("client-side toplevel positioning"),
        }
    }
}

/// Errors returned by muri's fallible operations.
///
/// [`Platform`](Error::Platform) is the catch-all for a failed per-OS API call
/// while creating, installing, or anchoring the tray/surface — its message names
/// the concrete failure site (e.g. `"tray install failed: …"`, `"must be created
/// on the main thread"`). A synchronous tray-install handshake means a Windows/
/// Linux install failure is surfaced through this variant (via the compat
/// facade's `build_result`) rather than returning a false `Ok`.
//
// NOTE: this enum is intentionally NOT `#[non_exhaustive]` and its variant set
// matches 0.10.7 — marking it non-exhaustive or adding public variants is a
// semver-major break for a 0.x crate (see cargo-semver-checks). Machine-matchable
// structured variants are deferred to the next intentional minor (0.11).
#[derive(Debug)]
pub enum Error {
    /// A capability that is not available on the current platform.
    Unsupported(Unsupported),
    /// The supplied icon bytes could not be decoded.
    BadIcon(String),
    /// A platform API call failed while creating, installing, or anchoring the
    /// surface. The message names the concrete failure (tray install, main-thread
    /// requirement, thread spawn, or a residual per-OS API error).
    Platform(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(u) => write!(f, "unsupported on this platform: {u}"),
            Error::BadIcon(m) => write!(f, "bad icon: {m}"),
            Error::Platform(m) => write!(f, "platform error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// A convenient result alias for muri operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_display_sensibly() {
        // The tray-install handshake surfaces the concrete failure site in the
        // Platform message, so a caller still learns *what* failed.
        assert_eq!(
            Error::Platform("tray install failed: Shell_NotifyIcon(NIM_ADD) failed".into())
                .to_string(),
            "platform error: tray install failed: Shell_NotifyIcon(NIM_ADD) failed"
        );
        assert_eq!(
            Error::BadIcon("empty".into()).to_string(),
            "bad icon: empty"
        );
        // Unsupported is preserved (not folded into a stringly Platform).
        assert_eq!(
            Error::Unsupported(Unsupported::TrayAnchor).to_string(),
            "unsupported on this platform: tray-anchored styled popup"
        );
    }
}
