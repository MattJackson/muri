//! DPI-independent ("logical") geometry primitives shared by the layout engine,
//! the scene drawer, and the per-OS anchoring shims.

/// A logical (DPI-independent) screen point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalPoint {
    /// Horizontal position in logical pixels.
    pub x: f32,
    /// Vertical position in logical pixels.
    pub y: f32,
}

impl LogicalPoint {
    /// A point from its components.
    pub fn new(x: f32, y: f32) -> Self {
        LogicalPoint { x, y }
    }
}

/// A logical size in DPI-independent pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalSize {
    /// Width in logical pixels.
    pub width: f32,
    /// Height in logical pixels.
    pub height: f32,
}

impl LogicalSize {
    /// A size from its components.
    pub fn new(width: f32, height: f32) -> Self {
        LogicalSize { width, height }
    }
}

/// A logical rectangle (origin at its top-left corner).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalRect {
    /// The top-left corner.
    pub origin: LogicalPoint,
    /// The rectangle's size.
    pub size: LogicalSize,
}

impl LogicalRect {
    /// A rectangle from an origin and size.
    pub fn new(origin: LogicalPoint, size: LogicalSize) -> Self {
        LogicalRect { origin, size }
    }

    /// The x coordinate of the right edge.
    pub fn max_x(&self) -> f32 {
        self.origin.x + self.size.width
    }

    /// The y coordinate of the bottom edge.
    pub fn max_y(&self) -> f32 {
        self.origin.y + self.size.height
    }

    /// Whether the rectangle contains the given point (half-open on max edges).
    pub fn contains(&self, p: LogicalPoint) -> bool {
        p.x >= self.origin.x && p.x < self.max_x() && p.y >= self.origin.y && p.y < self.max_y()
    }
}

/// Which edge of the anchor the popup should grow from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edge {
    /// Open below the anchor (typical for a top menu bar). The default.
    #[default]
    Bottom,
    /// Open above the anchor (typical for a bottom taskbar).
    Top,
    /// Open to the left of the anchor.
    Left,
    /// Open to the right of the anchor.
    Right,
}

/// Per-side insets in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    /// Top inset.
    pub top: f32,
    /// Right inset.
    pub right: f32,
    /// Bottom inset.
    pub bottom: f32,
    /// Left inset.
    pub left: f32,
}

impl Insets {
    /// Uniform insets on all four sides.
    pub fn uniform(v: f32) -> Self {
        Insets {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// Separate horizontal and vertical insets.
    pub fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Insets {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Total horizontal inset (`left + right`).
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Total vertical inset (`top + bottom`).
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}
