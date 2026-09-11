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
/// The failure sites that carry actionable meaning are captured as structured
/// variants — [`TrayInstall`](Error::TrayInstall), [`MainThread`](Error::MainThread),
/// [`ThreadSpawn`](Error::ThreadSpawn) — so a consumer can `match` on the *kind*
/// of failure (e.g. distinguish "the OS refused the tray install" from "you called
/// this off the main thread") rather than string-matching a message.
/// [`Platform`](Error::Platform) remains the catch-all for residual per-OS API
/// failures that don't fit a structured kind. A synchronous tray-install handshake
/// surfaces a Windows/Linux install failure as [`TrayInstall`](Error::TrayInstall)
/// (via [`Tray::spawn`](crate::Tray::spawn)) instead of a false `Ok`.
///
/// `#[non_exhaustive]`: future muri versions may add variants, so a consumer's
/// `match` must include a `_` arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A capability that is not available on the current platform.
    Unsupported(Unsupported),
    /// The supplied icon bytes could not be decoded.
    BadIcon(String),
    /// The OS tray/status-item install failed — `Shell_NotifyIcon(NIM_ADD)` on
    /// Windows, or the SNI/`StatusNotifierItem` D-Bus registration on Linux. On
    /// Windows/Linux this is surfaced synchronously through the tray-thread
    /// install handshake, so a caller learns the icon never appeared instead of
    /// seeing a false `Ok`.
    TrayInstall(String),
    /// A tray or surface that must be created on the main thread was requested
    /// off it (AppKit's `NSStatusItem`, and the platform's main-thread-only
    /// install/run paths).
    MainThread,
    /// The background `muri-tray` UI thread could not be spawned.
    ThreadSpawn(String),
    /// A platform API call failed while creating or anchoring the surface and
    /// does not fit a more specific structured variant.
    Platform(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(u) => write!(f, "unsupported on this platform: {u}"),
            Error::BadIcon(m) => write!(f, "bad icon: {m}"),
            Error::TrayInstall(m) => write!(f, "tray install failed: {m}"),
            Error::MainThread => f.write_str("must be created on the main thread"),
            Error::ThreadSpawn(m) => write!(f, "failed to spawn the muri tray thread: {m}"),
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
    fn structured_variants_display_sensibly() {
        assert_eq!(
            Error::TrayInstall("Shell_NotifyIcon(NIM_ADD) failed".into()).to_string(),
            "tray install failed: Shell_NotifyIcon(NIM_ADD) failed"
        );
        assert_eq!(
            Error::MainThread.to_string(),
            "must be created on the main thread"
        );
        assert_eq!(
            Error::ThreadSpawn("resource limit".into()).to_string(),
            "failed to spawn the muri tray thread: resource limit"
        );
        assert_eq!(
            Error::BadIcon("empty".into()).to_string(),
            "bad icon: empty"
        );
        assert_eq!(
            Error::Unsupported(Unsupported::TrayAnchor).to_string(),
            "unsupported on this platform: tray-anchored styled popup"
        );
    }

    #[test]
    fn variants_are_matchable_by_kind() {
        assert!(matches!(
            Error::TrayInstall("x".into()),
            Error::TrayInstall(_)
        ));
        assert!(matches!(Error::MainThread, Error::MainThread));
    }
}
