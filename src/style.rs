//! Visual style primitives: colors, fonts, and weights. These are the building
//! blocks a consumer attaches to segments and rows; they are resolved against a
//! [`Theme`](crate::Theme) into concrete pixels by the renderer.

/// A color: either a literal RGBA value, or a **semantic** role that resolves
/// against the active [`Theme`](crate::Theme) (and, on macOS, to the matching
/// system `NSColor`) so dark/light and accent adapt automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// A literal 8-bit-per-channel RGBA color.
    Rgba(u8, u8, u8, u8),
    /// Primary text color (`labelColor`).
    Label,
    /// De-emphasized text color (`secondaryLabelColor`), e.g. a version tail.
    SecondaryLabel,
    /// The user's accent color (`controlAccentColor`).
    Accent,
    /// Separator / hairline color.
    Separator,
    /// System red — used for critical/over-limit severity.
    SystemRed,
    /// System orange — used for warning severity.
    SystemOrange,
    /// System green — used for healthy severity.
    SystemGreen,
    /// System yellow.
    SystemYellow,
}

impl Color {
    /// An opaque literal color from 8-bit channels.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgba(r, g, b, 255)
    }

    /// Whether this is a literal (non-semantic) color.
    pub fn is_literal(&self) -> bool {
        matches!(self, Color::Rgba(..))
    }
}

/// A concrete, fully-resolved 8-bit-per-channel RGBA color. This is what the
/// scene drawer actually rasterizes with; semantic [`Color`]s become `Rgba`
/// after theme resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel (255 = opaque).
    pub a: u8,
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    /// Opaque white.
    pub const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    /// Fully transparent.
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// A color from its four channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }

    /// An opaque color from its three channels.
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }
}

/// A font family selector. `System`/`SystemMono` resolve to the platform UI
/// font so menus match the OS; `Named` looks up an installed family.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontFamily {
    /// The platform UI font (San Francisco, Segoe UI, system default).
    #[default]
    System,
    /// The platform monospace UI font.
    SystemMono,
    /// A specific installed family by name.
    Named(String),
}

/// Font weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Weight {
    /// Regular / normal weight. The default.
    #[default]
    Regular,
    /// Medium weight.
    Medium,
    /// Semibold weight.
    Semibold,
    /// Bold weight.
    Bold,
}

impl Weight {
    /// The OpenType numeric weight (100–900) for this weight, for backends that
    /// select fonts by numeric weight.
    pub fn ot_weight(&self) -> u16 {
        match self {
            Weight::Regular => 400,
            Weight::Medium => 500,
            Weight::Semibold => 600,
            Weight::Bold => 700,
        }
    }
}

/// A resolved font: family, size (in logical points), and weight.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The font family.
    pub family: FontFamily,
    /// Size in logical points.
    pub size: f32,
    /// Weight.
    pub weight: Weight,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            family: FontFamily::System,
            size: 13.0,
            weight: Weight::Regular,
        }
    }
}

impl Font {
    /// A system-font at the given size and weight.
    pub fn system(size: f32, weight: Weight) -> Self {
        Font {
            family: FontFamily::System,
            size,
            weight,
        }
    }

    /// A system monospace font at the given size and weight.
    pub fn mono(size: f32, weight: Weight) -> Self {
        Font {
            family: FontFamily::SystemMono,
            size,
            weight,
        }
    }

    /// Return a copy of this font with a different weight.
    pub fn with_weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }
}
