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
#[derive(Debug)]
pub enum Error {
    /// A capability that is not available on the current platform.
    Unsupported(Unsupported),
    /// The supplied icon bytes could not be decoded.
    BadIcon(String),
    /// A platform API call failed while creating or anchoring the surface.
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
