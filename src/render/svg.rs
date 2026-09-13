//! A minimal, dependency-light SVG rasterizer (ADR-0001: own the glue, use the
//! engine). muri already carries the pure-Rust [`zeno`] path rasterizer
//! transitively (via `swash`), so `Icon::Svg` is lit up by depending on it
//! *directly* — no `tiny-skia`/`resvg`, no second PNG codec, no XML crate.
//!
//! This module owns a hand-rolled XML/attribute reader and the SVG glue
//! (color/transform/shape parsing, a straight-alpha compositor); [`zeno`] owns
//! the anti-aliased fill/stroke rasterization. Output is the **same** shape as
//! [`raster::decode_png`](super::raster::decode_png) — `(rgba, width, height)`
//! straight-alpha RGBA — so it slots into the existing icon path.
//!
//! ## Supported subset
//!
//! `<svg viewBox|width|height>`, `<g>`; `<path d>`, `<rect>` (+`rx`/`ry`),
//! `<circle>`, `<ellipse>`, `<line>`, `<polygon>`, `<polyline>`; `fill`,
//! `fill-opacity`, `opacity`, `stroke`, `stroke-width`, `stroke-opacity`,
//! `fill-rule`, and `transform`, all inherited through `<g>`. Anything else
//! (gradients, `<use>`, `<text>`, filters, clip paths, CSS) is skipped
//! gracefully — the SVG bytes are untrusted input, so malformed constructs
//! never panic and just return `None`.
//!
//! ## Sizing
//!
//! Rasterized at a fixed device size ([`TARGET_MAX`], aspect preserved from the
//! viewBox): produced once and scaled by the blitter rather than re-rasterized
//! per DPI.

use zeno::{Command, Fill, Format, Mask, PathBuilder, PathData, Stroke, Style, Transform};

/// The fixed maximum device dimension the vector is rasterized at (the other
/// dimension is derived to preserve the viewBox aspect ratio). Small because
/// muri only draws menu/tray icons; the blitter scales this to the destination.
const TARGET_MAX: u32 = 64;

/// Hard cap on either viewBox dimension, mirroring
/// [`raster`](super::raster)'s `MAX_ICON_DIM`: a crafted `<svg viewBox>` with an
/// absurd extent is rejected up front (the device buffer is already bounded by
/// [`TARGET_MAX`], but this refuses nonsense coordinate spaces cheaply too).
const MAX_VIEWBOX_DIM: f32 = 4096.0;

/// Hard cap on total viewBox area (`w * h`), mirroring `raster`'s
/// `MAX_ICON_PIXELS` (2048×2048): a large-but-thin box that clears the
/// per-dimension cap is still refused.
const MAX_VIEWBOX_AREA: f64 = 2048.0 * 2048.0;

/// Rasterize SVG bytes to straight-alpha RGBA `(rgba, width, height)`, the same
/// output shape as [`decode_png`](super::raster::decode_png). Returns `None` for
/// non-SVG / unparseable bytes or an out-of-range viewBox, so callers fall back
/// cleanly. Never panics on malformed/malicious input.
pub fn rasterize_svg(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let text = std::str::from_utf8(bytes).ok()?;
    // Cheap reject: must look like SVG at all before we scan it.
    if !text.contains("<svg") {
        return None;
    }
    let tags = parse_tags(text);

    let mut acc: Option<Vec<u8>> = None;
    let mut tw: u32 = 0;
    let mut th: u32 = 0;
    let mut stack: Vec<State> = Vec::new();

    for tag in &tags {
        match tag.kind {
            TagKind::Close => {
                stack.pop();
            }
            TagKind::Open | TagKind::Empty => {
                let empty = matches!(tag.kind, TagKind::Empty);
                if tag.name == "svg" {
                    if acc.is_some() {
                        // A second/nested <svg>: keep the stack balanced but
                        // don't reinitialize the canvas.
                        if !empty {
                            let parent = stack.last().cloned().unwrap_or_default();
                            stack.push(parent);
                        }
                        continue;
                    }
                    let (device, w, h) = setup_root(&tag.attrs)?;
                    tw = w;
                    th = h;
                    acc = Some(vec![0u8; (w as usize) * (h as usize) * 4]);
                    let base = State::root(device);
                    let st = base.merge(&tag.name, &tag.attrs);
                    if !empty {
                        stack.push(st);
                    }
                    continue;
                }
                let Some(parent) = stack.last() else {
                    continue;
                };
                let st = parent.merge(&tag.name, &tag.attrs);
                if !st.suppressed {
                    if let Some(buf) = acc.as_mut() {
                        paint_shape(buf, tw, th, &st, &tag.name, &tag.attrs);
                    }
                }
                if !empty {
                    stack.push(st);
                }
            }
        }
    }

    let buf = acc?;
    if tw == 0 || th == 0 {
        return None;
    }
    Some((buf, tw, th))
}

// =============================================================================
// Root / viewBox setup
// =============================================================================

/// Compute the viewBox→device transform and device size from the `<svg>`
/// attributes. Returns `None` if the coordinate space is empty or exceeds the
/// dimension caps.
fn setup_root(attrs: &[(String, String)]) -> Option<(Mat, u32, u32)> {
    // Coordinate space: prefer viewBox, else width/height, else a 24-unit box.
    let (min_x, min_y, vb_w, vb_h) = if let Some(vb) = attr(attrs, "viewBox") {
        let n = scan_numbers(vb);
        if n.len() == 4 {
            (n[0], n[1], n[2], n[3])
        } else {
            return None;
        }
    } else {
        let w = attr(attrs, "width").and_then(parse_len).unwrap_or(24.0);
        let h = attr(attrs, "height").and_then(parse_len).unwrap_or(24.0);
        (0.0, 0.0, w, h)
    };

    if !(vb_w.is_finite() && vb_h.is_finite()) || vb_w <= 0.0 || vb_h <= 0.0 {
        return None;
    }
    if vb_w > MAX_VIEWBOX_DIM || vb_h > MAX_VIEWBOX_DIM {
        return None;
    }
    if (vb_w as f64) * (vb_h as f64) > MAX_VIEWBOX_AREA {
        return None;
    }

    // Fit the viewBox into a TARGET_MAX-bounded box, aspect preserved (the
    // default xMidYMid-meet behavior, but with the target sized to the content
    // so there is no letterboxing).
    let (tw, th) = if vb_w >= vb_h {
        let tw = TARGET_MAX;
        let th = ((TARGET_MAX as f32) * vb_h / vb_w).round().max(1.0) as u32;
        (tw, th)
    } else {
        let th = TARGET_MAX;
        let tw = ((TARGET_MAX as f32) * vb_w / vb_h).round().max(1.0) as u32;
        (tw, th)
    };
    let s = (tw as f32 / vb_w).min(th as f32 / vb_h);
    // device = scale(s) then translate(-min * s): a=d=s, e=-min_x*s, f=-min_y*s.
    let device = Mat([s, 0.0, 0.0, s, -min_x * s, -min_y * s]);
    Some((device, tw, th))
}

// =============================================================================
// Graphics state (inherited presentation attributes)
// =============================================================================

/// The inherited drawing state at a point in the SVG tree.
#[derive(Clone)]
struct State {
    /// Current transform matrix (viewBox→device composed with element transforms).
    ctm: Mat,
    /// Fill color, or `None` for `fill="none"`. Default is opaque black.
    fill: Option<[u8; 3]>,
    fill_opacity: f32,
    /// Stroke color, or `None` for no stroke (the default).
    stroke: Option<[u8; 3]>,
    stroke_width: f32,
    stroke_opacity: f32,
    /// Group/element opacity, accumulated down the tree (an approximation of
    /// true layer compositing — adequate for flat icons).
    opacity: f32,
    /// `true` for `fill-rule:evenodd`.
    evenodd: bool,
    /// `true` inside a non-rendered subtree (`<defs>`, gradients, clip paths…).
    suppressed: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            ctm: Mat::IDENTITY,
            fill: Some([0, 0, 0]),
            fill_opacity: 1.0,
            stroke: None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            opacity: 1.0,
            evenodd: false,
            suppressed: false,
        }
    }
}

impl State {
    fn root(device: Mat) -> Self {
        State {
            ctm: device,
            ..State::default()
        }
    }

    /// Derive the child state for an element by applying its presentation
    /// attributes on top of `self` (which is the parent's inherited state).
    fn merge(&self, name: &str, attrs: &[(String, String)]) -> State {
        let mut s = self.clone();
        s.suppressed = self.suppressed || is_nonrendered(name);

        if let Some(v) = attr(attrs, "transform") {
            if let Some(local) = parse_transform(v) {
                s.ctm = self.ctm.mul(local);
            }
        }
        if let Some(v) = attr(attrs, "fill") {
            s.fill = parse_paint(v);
        }
        if let Some(v) = attr(attrs, "fill-opacity") {
            if let Some(o) = parse_opacity(v) {
                s.fill_opacity = o;
            }
        }
        if let Some(v) = attr(attrs, "stroke") {
            s.stroke = parse_paint(v);
        }
        if let Some(v) = attr(attrs, "stroke-width") {
            if let Some(w) = parse_len(v) {
                s.stroke_width = w.max(0.0);
            }
        }
        if let Some(v) = attr(attrs, "stroke-opacity") {
            if let Some(o) = parse_opacity(v) {
                s.stroke_opacity = o;
            }
        }
        if let Some(v) = attr(attrs, "opacity") {
            if let Some(o) = parse_opacity(v) {
                s.opacity *= o;
            }
        }
        if let Some(v) = attr(attrs, "fill-rule") {
            s.evenodd = v.trim().eq_ignore_ascii_case("evenodd");
        }
        // Also honor the common `style="fill:…;stroke:…"` inline form for the
        // handful of properties we support.
        if let Some(style) = attr(attrs, "style") {
            apply_inline_style(&mut s, style);
        }
        s
    }
}

/// Elements whose subtree we never rasterize (definitions, gradients, masks,
/// text, etc.). Their descendants inherit `suppressed = true`.
fn is_nonrendered(name: &str) -> bool {
    matches!(
        name,
        "defs"
            | "clipPath"
            | "mask"
            | "symbol"
            | "marker"
            | "pattern"
            | "linearGradient"
            | "radialGradient"
            | "filter"
            | "style"
            | "text"
            | "metadata"
            | "title"
            | "desc"
    )
}

/// Apply the subset of properties we support from an inline `style="…"` string.
fn apply_inline_style(s: &mut State, style: &str) {
    for decl in style.split(';') {
        let mut kv = decl.splitn(2, ':');
        let (Some(k), Some(v)) = (kv.next(), kv.next()) else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "fill" => s.fill = parse_paint(v),
            "stroke" => s.stroke = parse_paint(v),
            "fill-opacity" => {
                if let Some(o) = parse_opacity(v) {
                    s.fill_opacity = o;
                }
            }
            "stroke-opacity" => {
                if let Some(o) = parse_opacity(v) {
                    s.stroke_opacity = o;
                }
            }
            "opacity" => {
                if let Some(o) = parse_opacity(v) {
                    s.opacity *= o;
                }
            }
            "stroke-width" => {
                if let Some(w) = parse_len(v) {
                    s.stroke_width = w.max(0.0);
                }
            }
            "fill-rule" => s.evenodd = v.eq_ignore_ascii_case("evenodd"),
            _ => {}
        }
    }
}

// =============================================================================
// Shape rasterization
// =============================================================================

/// Build the geometry for one shape element and composite its fill + stroke
/// onto the accumulator.
fn paint_shape(acc: &mut [u8], w: u32, h: u32, st: &State, name: &str, attrs: &[(String, String)]) {
    match name {
        "path" => {
            if let Some(d) = attr(attrs, "d") {
                paint(acc, w, h, st, d);
            }
        }
        "rect" => {
            let x = num(attrs, "x");
            let y = num(attrs, "y");
            let rw = num(attrs, "width");
            let rh = num(attrs, "height");
            if rw <= 0.0 || rh <= 0.0 {
                return;
            }
            // rx/ry with the SVG "auto from the other" fallback.
            let rx_a = attr(attrs, "rx").and_then(parse_len);
            let ry_a = attr(attrs, "ry").and_then(parse_len);
            let rx = rx_a.or(ry_a).unwrap_or(0.0).max(0.0).min(rw / 2.0);
            let ry = ry_a.or(rx_a).unwrap_or(0.0).max(0.0).min(rh / 2.0);
            let mut cmds: Vec<Command> = Vec::new();
            if rx > 0.0 || ry > 0.0 {
                cmds.add_round_rect((x, y), rw, rh, rx, ry);
            } else {
                cmds.add_rect((x, y), rw, rh);
            }
            paint(acc, w, h, st, cmds.as_slice());
        }
        "circle" => {
            let cx = num(attrs, "cx");
            let cy = num(attrs, "cy");
            let r = num(attrs, "r");
            if r <= 0.0 {
                return;
            }
            let mut cmds: Vec<Command> = Vec::new();
            cmds.add_circle((cx, cy), r);
            paint(acc, w, h, st, cmds.as_slice());
        }
        "ellipse" => {
            let cx = num(attrs, "cx");
            let cy = num(attrs, "cy");
            let rx = num(attrs, "rx");
            let ry = num(attrs, "ry");
            if rx <= 0.0 || ry <= 0.0 {
                return;
            }
            let mut cmds: Vec<Command> = Vec::new();
            cmds.add_ellipse((cx, cy), rx, ry);
            paint(acc, w, h, st, cmds.as_slice());
        }
        "line" => {
            let x1 = num(attrs, "x1");
            let y1 = num(attrs, "y1");
            let x2 = num(attrs, "x2");
            let y2 = num(attrs, "y2");
            let mut cmds: Vec<Command> = Vec::new();
            cmds.move_to((x1, y1));
            cmds.line_to((x2, y2));
            // A line has no fill; only stroke it.
            paint_stroke_only(acc, w, h, st, cmds.as_slice());
        }
        "polygon" | "polyline" => {
            let Some(pts) = attr(attrs, "points") else {
                return;
            };
            let n = scan_numbers(pts);
            if n.len() < 4 {
                return;
            }
            let mut cmds: Vec<Command> = Vec::new();
            cmds.move_to((n[0], n[1]));
            let mut i = 2;
            while i + 1 < n.len() {
                cmds.line_to((n[i], n[i + 1]));
                i += 2;
            }
            if name == "polygon" {
                cmds.close();
            }
            paint(acc, w, h, st, cmds.as_slice());
        }
        _ => {}
    }
}

/// Fill (if any) then stroke (if any) `data` with the current state.
fn paint<D: PathData + Copy>(acc: &mut [u8], w: u32, h: u32, st: &State, data: D) {
    if let Some(rgb) = st.fill {
        let rule = if st.evenodd {
            Fill::EvenOdd
        } else {
            Fill::NonZero
        };
        let cov = coverage(data, rule, st.ctm, w, h);
        composite(acc, &cov, rgb, st.fill_opacity * st.opacity);
    }
    paint_stroke_only(acc, w, h, st, data);
}

/// Stroke `data` only (used by fill-less shapes like `<line>` and by [`paint`]).
fn paint_stroke_only<D: PathData + Copy>(acc: &mut [u8], w: u32, h: u32, st: &State, data: D) {
    if let Some(rgb) = st.stroke {
        if st.stroke_width > 0.0 {
            let cov = coverage(data, Stroke::new(st.stroke_width), st.ctm, w, h);
            composite(acc, &cov, rgb, st.stroke_opacity * st.opacity);
        }
    }
}

/// Rasterize `data` (after `ctm`) into an 8-bit alpha coverage mask sized `w×h`.
fn coverage<'a, D, S>(data: D, style: S, ctm: Mat, w: u32, h: u32) -> Vec<u8>
where
    D: PathData,
    S: Into<Style<'a>>,
{
    let mut buf = vec![0u8; (w as usize) * (h as usize)];
    Mask::new(data)
        .style(style)
        .transform(Some(ctm.to_zeno()))
        .format(Format::Alpha)
        .size(w, h)
        .render_into(&mut buf, None);
    buf
}

/// Composite a solid `rgb` color, masked by `cov` and scaled by `ka` (0..=1),
/// over the straight-alpha RGBA accumulator with a Porter-Duff `over`.
fn composite(acc: &mut [u8], cov: &[u8], rgb: [u8; 3], ka: f32) {
    let ka = ka.clamp(0.0, 1.0);
    if ka <= 0.0 {
        return;
    }
    for (i, &c) in cov.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let sa = (c as f32 / 255.0) * ka;
        if sa <= 0.0 {
            continue;
        }
        let idx = i * 4;
        let Some(px) = acc.get_mut(idx..idx + 4) else {
            break;
        };
        let da = px[3] as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        if oa <= 0.0 {
            continue;
        }
        for ch in 0..3 {
            let sc = rgb[ch] as f32 / 255.0;
            let dc = px[ch] as f32 / 255.0;
            let oc = (sc * sa + dc * da * (1.0 - sa)) / oa;
            px[ch] = (oc * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        px[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

// =============================================================================
// 2D affine matrix (SVG `matrix(a,b,c,d,e,f)` convention)
// =============================================================================

/// An affine transform stored in SVG order `[a, b, c, d, e, f]`, mapping
/// `(x, y) → (a·x + c·y + e, b·x + d·y + f)`.
#[derive(Clone, Copy)]
struct Mat([f32; 6]);

impl Mat {
    const IDENTITY: Mat = Mat([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// `self * other` (apply `other` first, then `self`) — i.e. the child CTM
    /// when `self` is the parent CTM and `other` is the element's transform.
    fn mul(self, o: Mat) -> Mat {
        let a = self.0;
        let b = o.0;
        Mat([
            a[0] * b[0] + a[2] * b[1],
            a[1] * b[0] + a[3] * b[1],
            a[0] * b[2] + a[2] * b[3],
            a[1] * b[2] + a[3] * b[3],
            a[0] * b[4] + a[2] * b[5] + a[4],
            a[1] * b[4] + a[3] * b[5] + a[5],
        ])
    }

    /// zeno's `Transform` uses the identical `(xx, xy, yx, yy, x, y)` layout as
    /// SVG's `matrix(a, b, c, d, e, f)`.
    fn to_zeno(self) -> Transform {
        let m = self.0;
        Transform::new(m[0], m[1], m[2], m[3], m[4], m[5])
    }
}

/// Parse an SVG `transform` attribute (a sequence of `matrix/translate/scale/
/// rotate/skewX/skewY` functions) into a single matrix. Unknown functions are
/// skipped; returns `None` only if nothing parsed.
fn parse_transform(s: &str) -> Option<Mat> {
    let mut m = Mat::IDENTITY;
    let mut any = false;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Read a function name (letters).
        while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if start == i {
            break;
        }
        let name = &s[start..i];
        // Read the parenthesized argument list.
        while i < bytes.len() && bytes[i] != b'(' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let open = i + 1;
        while i < bytes.len() && bytes[i] != b')' {
            i += 1;
        }
        let args = s.get(open..i).unwrap_or("");
        i += 1; // past ')'
        let n = scan_numbers(args);
        if let Some(local) = transform_fn(name, &n) {
            m = m.mul(local);
            any = true;
        }
    }
    any.then_some(m)
}

fn transform_fn(name: &str, n: &[f32]) -> Option<Mat> {
    match name {
        "matrix" if n.len() == 6 => Some(Mat([n[0], n[1], n[2], n[3], n[4], n[5]])),
        "translate" if !n.is_empty() => {
            let tx = n[0];
            let ty = n.get(1).copied().unwrap_or(0.0);
            Some(Mat([1.0, 0.0, 0.0, 1.0, tx, ty]))
        }
        "scale" if !n.is_empty() => {
            let sx = n[0];
            let sy = n.get(1).copied().unwrap_or(sx);
            Some(Mat([sx, 0.0, 0.0, sy, 0.0, 0.0]))
        }
        "rotate" if !n.is_empty() => {
            let rad = n[0].to_radians();
            let (sin, cos) = rad.sin_cos();
            let rot = Mat([cos, sin, -sin, cos, 0.0, 0.0]);
            if n.len() >= 3 {
                let (cx, cy) = (n[1], n[2]);
                // translate(cx,cy) * rotate * translate(-cx,-cy)
                let t1 = Mat([1.0, 0.0, 0.0, 1.0, cx, cy]);
                let t2 = Mat([1.0, 0.0, 0.0, 1.0, -cx, -cy]);
                Some(t1.mul(rot).mul(t2))
            } else {
                Some(rot)
            }
        }
        "skewX" if !n.is_empty() => Some(Mat([1.0, 0.0, n[0].to_radians().tan(), 1.0, 0.0, 0.0])),
        "skewY" if !n.is_empty() => Some(Mat([1.0, n[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0])),
        _ => None,
    }
}

// =============================================================================
// Color / number parsing
// =============================================================================

/// Parse a `fill`/`stroke` paint value. `none`/`transparent`/unsupported paint
/// servers (`url(...)`) yield `None` (no paint); everything else resolves to an
/// opaque RGB triple (`currentColor` and unknown names default to black).
fn parse_paint(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if s.starts_with("url(") {
        // A gradient/pattern paint server — unsupported; skip this paint.
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if s.starts_with("rgb") {
        if let Some(open) = s.find('(') {
            let inner = &s[open + 1..s.find(')').unwrap_or(s.len())];
            let n = scan_numbers(inner);
            if n.len() >= 3 {
                return Some([clamp_u8(n[0]), clamp_u8(n[1]), clamp_u8(n[2])]);
            }
        }
        return None;
    }
    if s.eq_ignore_ascii_case("currentColor") {
        return Some([0, 0, 0]);
    }
    named_color(s)
}

fn parse_hex(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.trim();
    let h = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let b = hex.as_bytes();
    match b.len() {
        3 | 4 => {
            // #rgb / #rgba (alpha ignored)
            let r = h(b[0])?;
            let g = h(b[1])?;
            let bl = h(b[2])?;
            Some([r * 17, g * 17, bl * 17])
        }
        6 | 8 => {
            // #rrggbb / #rrggbbaa (alpha ignored)
            let r = h(b[0])? * 16 + h(b[1])?;
            let g = h(b[2])? * 16 + h(b[3])?;
            let bl = h(b[4])? * 16 + h(b[5])?;
            Some([r, g, bl])
        }
        _ => None,
    }
}

/// A small but common subset of the SVG/CSS named colors.
fn named_color(name: &str) -> Option<[u8; 3]> {
    let n = name.to_ascii_lowercase();
    let c = match n.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "lime" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        "gray" | "grey" => [128, 128, 128],
        "silver" => [192, 192, 192],
        "maroon" => [128, 0, 0],
        "olive" => [128, 128, 0],
        "teal" => [0, 128, 128],
        "navy" => [0, 0, 128],
        "purple" => [128, 0, 128],
        "orange" => [255, 165, 0],
        "gold" => [255, 215, 0],
        "pink" => [255, 192, 203],
        "brown" => [165, 42, 42],
        "darkgray" | "darkgrey" => [169, 169, 169],
        "lightgray" | "lightgrey" => [211, 211, 211],
        _ => return None,
    };
    Some(c)
}

fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Parse an opacity value in `0.0..=1.0` (also accepts a trailing `%`).
fn parse_opacity(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        return pct
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| (v / 100.0).clamp(0.0, 1.0));
    }
    s.parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0))
}

/// Parse a length: a leading number, ignoring any unit suffix (`px`, `pt`, …).
/// Percentages are not resolved (returns `None`).
fn parse_len(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.ends_with('%') {
        return None;
    }
    let end = s
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
        })
        .unwrap_or(s.len());
    let num = &s[..end];
    let v = num.parse::<f32>().ok()?;
    v.is_finite().then_some(v)
}

/// A shape-coordinate attribute as a finite `f32`, defaulting to `0.0`.
fn num(attrs: &[(String, String)], key: &str) -> f32 {
    attr(attrs, key).and_then(parse_len).unwrap_or(0.0)
}

/// Extract every floating-point number from a string, handling SVG's compact
/// forms (comma/whitespace separators and sign/decimal adjacency like `1-2` or
/// `.5.5`). Non-numeric runs are skipped.
fn scan_numbers(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'+' || c == b'-' || c == b'.' || c.is_ascii_digit() {
            let start = i;
            let mut seen_dot = c == b'.';
            let mut seen_digit = c.is_ascii_digit();
            i += 1;
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() {
                    seen_digit = true;
                    i += 1;
                } else if d == b'.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E')
                    && i + 1 < b.len()
                    && (b[i + 1].is_ascii_digit() || b[i + 1] == b'+' || b[i + 1] == b'-')
                {
                    // exponent: consume 'e' and an optional sign
                    i += 1;
                    if b[i] == b'+' || b[i] == b'-' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if seen_digit {
                if let Ok(v) = s[start..i].parse::<f32>() {
                    if v.is_finite() {
                        out.push(v);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

// =============================================================================
// Tiny XML reader
// =============================================================================

#[derive(Clone, Copy, PartialEq)]
enum TagKind {
    Open,
    Close,
    Empty,
}

struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    kind: TagKind,
}

/// Look up an attribute by exact name.
fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A deliberately tiny XML tag scanner: yields start/end/empty tags with their
/// attributes, skipping comments, processing instructions, DOCTYPE, CDATA, and
/// text content. It is *not* a validating parser — it just extracts the element
/// structure muri's SVG subset needs, and it never panics on malformed input.
fn parse_tags(s: &str) -> Vec<Tag> {
    let mut tags = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        // Dispatch on what follows '<'.
        let next = b.get(i + 1).copied().unwrap_or(0);
        if next == b'!' {
            // Comment, CDATA, or DOCTYPE.
            if s[i..].starts_with("<!--") {
                i = s[i + 4..]
                    .find("-->")
                    .map(|p| i + 4 + p + 3)
                    .unwrap_or(b.len());
            } else if s[i..].starts_with("<![CDATA[") {
                i = s[i + 9..]
                    .find("]]>")
                    .map(|p| i + 9 + p + 3)
                    .unwrap_or(b.len());
            } else {
                i = s[i..].find('>').map(|p| i + p + 1).unwrap_or(b.len());
            }
            continue;
        }
        if next == b'?' {
            // Processing instruction.
            i = s[i..].find("?>").map(|p| i + p + 2).unwrap_or(b.len());
            continue;
        }
        // A start/end/empty element: find its closing '>'.
        let Some(close_rel) = s[i..].find('>') else {
            break;
        };
        let close = i + close_rel;
        let inner = &s[i + 1..close];
        i = close + 1;
        parse_element(inner, &mut tags);
    }
    tags
}

/// Parse the text between `<` and `>` into a [`Tag`].
fn parse_element(inner: &str, tags: &mut Vec<Tag>) {
    let inner = inner.trim();
    if inner.is_empty() {
        return;
    }
    if let Some(rest) = inner.strip_prefix('/') {
        // End tag.
        let name = rest.split_whitespace().next().unwrap_or("");
        if !name.is_empty() {
            tags.push(Tag {
                name: local_name(name),
                attrs: Vec::new(),
                kind: TagKind::Close,
            });
        }
        return;
    }
    let empty = inner.ends_with('/');
    let body = inner.strip_suffix('/').unwrap_or(inner);
    let body = body.trim();

    // Element name is up to the first whitespace.
    let mut chars = body.char_indices();
    let name_end = loop {
        match chars.next() {
            Some((idx, c)) if c.is_whitespace() => break idx,
            Some(_) => continue,
            None => break body.len(),
        }
    };
    let name = &body[..name_end];
    if name.is_empty() {
        return;
    }
    let attrs = parse_attrs(&body[name_end..]);
    tags.push(Tag {
        name: local_name(name),
        attrs,
        kind: if empty { TagKind::Empty } else { TagKind::Open },
    });
}

/// Strip an XML namespace prefix (`svg:rect` → `rect`).
fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// Parse `name="value"` / `name='value'` pairs from an element's attribute
/// section. Attribute names keep no namespace stripping (SVG presentation
/// attributes are unprefixed); values are XML-entity decoded.
fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // Skip whitespace.
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        // Read name (up to '=' or whitespace).
        let start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            break;
        }
        let name = &s[start..i];
        // Skip whitespace + '='.
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            // Valueless attribute; ignore and continue.
            continue;
        }
        i += 1; // past '='
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let quote = b[i];
        if quote != b'"' && quote != b'\'' {
            // Unquoted values aren't valid XML; skip this attribute.
            continue;
        }
        i += 1;
        let vstart = i;
        while i < b.len() && b[i] != quote {
            i += 1;
        }
        let value = &s[vstart..i.min(b.len())];
        i += 1; // past closing quote
        out.push((local_name(name), decode_entities(value)));
    }
    out
}

/// Decode the five predefined XML entities (and leave anything else as-is).
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alpha channel of a pixel in a straight-alpha RGBA buffer.
    fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn rect_fill_hex_color_renders_solid_red() {
        let svg = br##"<svg viewBox="0 0 10 10"><rect x="0" y="0" width="10" height="10" fill="#ff0000"/></svg>"##;
        let (rgba, w, h) = rasterize_svg(svg).expect("rect svg rasterizes");
        assert!(w > 0 && h > 0);
        // The center pixel must be opaque red.
        let c = px(&rgba, w, w / 2, h / 2);
        assert_eq!(c, [255, 0, 0, 255], "center should be opaque red");
    }

    #[test]
    fn path_triangle_fills_interior_and_leaves_a_corner_clear() {
        // A right triangle occupying the lower-left; the top-right corner stays
        // transparent.
        let svg = br##"<svg viewBox="0 0 64 64"><path d="M0,64 L0,0 L64,64 Z" fill="rgb(0,0,255)"/></svg>"##;
        let (rgba, w, h) = rasterize_svg(svg).expect("path svg rasterizes");
        // A point clearly inside the triangle (lower-left) is blue & opaque.
        let inside = px(&rgba, w, 4, h - 4);
        assert!(
            inside[3] > 200,
            "interior should be near-opaque: {inside:?}"
        );
        assert!(inside[2] > 200 && inside[0] < 40, "interior should be blue");
        // The top-right corner is outside the triangle and stays transparent.
        let outside = px(&rgba, w, w - 2, 2);
        assert_eq!(outside[3], 0, "top-right corner should be transparent");
    }

    #[test]
    fn named_color_and_group_transform_compose() {
        let svg = br##"<svg viewBox="0 0 20 20"><g transform="translate(10,0)"><rect width="10" height="20" fill="green"/></g></svg>"##;
        let (rgba, w, h) = rasterize_svg(svg).expect("rasterizes");
        // Left half is empty (rect was translated to the right half).
        assert_eq!(px(&rgba, w, 1, h / 2)[3], 0, "left half transparent");
        // Right half is opaque green (0,128,0).
        let right = px(&rgba, w, w - 2, h / 2);
        assert!(
            right[3] > 200 && right[1] > 100 && right[0] < 40,
            "{right:?}"
        );
    }

    #[test]
    fn transparent_region_stays_transparent() {
        let svg = br##"<svg viewBox="0 0 10 10"><rect x="0" y="0" width="4" height="4" fill="#000"/></svg>"##;
        let (rgba, w, h) = rasterize_svg(svg).expect("rasterizes");
        // Far corner away from the small rect is untouched.
        assert_eq!(px(&rgba, w, w - 1, h - 1)[3], 0);
    }

    #[test]
    fn malformed_and_empty_bytes_return_none() {
        assert!(rasterize_svg(&[]).is_none());
        assert!(rasterize_svg(b"not svg at all").is_none());
        // Invalid UTF-8.
        assert!(rasterize_svg(&[0xff, 0xfe, 0x00]).is_none());
        // A `<svg` with a broken/unterminated tag must not panic.
        assert!(rasterize_svg(b"<svg viewBox=\"0 0 1 1\"><rect ").is_some());
    }

    #[test]
    fn empty_svg_with_default_box_is_fully_transparent() {
        // `<svg></svg>` has no viewBox; it falls back to a 24-unit box and no
        // shapes, so every pixel is transparent (this is intentionally a valid,
        // if blank, raster — not `None`).
        let svg = b"<svg width=\"16\" height=\"16\"></svg>";
        let (rgba, ..) = rasterize_svg(svg).expect("blank but valid");
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 0));
    }

    #[test]
    fn oversized_viewbox_is_rejected() {
        let svg =
            br##"<svg viewBox="0 0 100000 100000"><rect width="1" height="1" fill="red"/></svg>"##;
        assert!(
            rasterize_svg(svg).is_none(),
            "a viewBox past the dimension cap must be refused"
        );
        // A large-but-thin box over the area cap (but under the per-dim cap) too.
        let thin =
            br##"<svg viewBox="0 0 4000 4000"><rect width="1" height="1" fill="red"/></svg>"##;
        assert!(
            rasterize_svg(thin).is_none(),
            "area cap must reject 4000x4000"
        );
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        // A 2:1 viewBox → width TARGET_MAX, height TARGET_MAX/2.
        let svg = br##"<svg viewBox="0 0 40 20"><rect width="40" height="20" fill="#fff"/></svg>"##;
        let (_rgba, w, h) = rasterize_svg(svg).expect("rasterizes");
        assert_eq!(w, TARGET_MAX);
        assert_eq!(h, TARGET_MAX / 2);
    }

    #[test]
    fn stable_pixels_for_a_known_icon() {
        // A golden-ish stability check: the same SVG rasterizes identically
        // twice (deterministic) and the known center pixel is stable.
        let svg =
            br##"<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="7" fill="#3366cc"/></svg>"##;
        let a = rasterize_svg(svg).expect("rasterizes");
        let b = rasterize_svg(svg).expect("rasterizes");
        assert_eq!(a, b, "rasterization must be deterministic");
        let c = px(&a.0, a.1, a.1 / 2, a.2 / 2);
        assert_eq!(
            c,
            [0x33, 0x66, 0xcc, 255],
            "center of the disc is the fill color"
        );
    }
}
