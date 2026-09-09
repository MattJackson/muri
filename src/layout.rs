//! Pure, platform-independent row layout: distributing a row's available width
//! across its segments according to their [`Flex`] behavior, then placing each
//! segment's text within its box according to its [`Align`].
//!
//! This is the heart of muri's "flush-right value with no chevron column"
//! promise, and it is deliberately free of any text-measurement, GPU, or OS
//! dependency: the caller supplies each segment's already-measured intrinsic
//! width (from `cosmic-text` in the real backend, or a fixed value in tests),
//! and this module returns where every box and every glyph run starts.

use crate::menu::{Align, Flex};

/// The measured inputs for one segment: how wide its text wants to be, and how
/// it should claim and align within space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentMetrics {
    /// The segment's intrinsic (measured) text width in logical pixels.
    pub intrinsic_width: f32,
    /// How the segment claims horizontal space.
    pub flex: Flex,
    /// How the segment aligns its text within its resolved box.
    pub align: Align,
}

impl SegmentMetrics {
    /// Convenience constructor.
    pub fn new(intrinsic_width: f32, flex: Flex, align: Align) -> Self {
        SegmentMetrics {
            intrinsic_width,
            flex,
            align,
        }
    }
}

/// The resolved geometry of one segment after width distribution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentBox {
    /// The leading edge of the segment's box, relative to the content band.
    pub x: f32,
    /// The width of the segment's box.
    pub width: f32,
    /// The x at which the segment's text is actually drawn, accounting for
    /// alignment of the intrinsic text within the (possibly wider) box.
    pub text_x: f32,
}

/// Resolve the segment boxes for a row, given the content-band `available_width`
/// (the width remaining after leading/trailing icon columns and padding).
///
/// Rules:
/// - Every segment gets at least its intrinsic width.
/// - Leftover width (`available_width - sum(intrinsic)`, clamped at zero) is
///   split evenly among [`Flex::Grow`] segments. With no `Grow` segment the row
///   is left-packed and any leftover stays empty on the right.
/// - Within its box, a segment's text is placed per its [`Align`].
///
/// A single `Grow` segment followed by a `Fixed` + `Align::Right` segment
/// therefore yields a value flush with the right edge of the content band, with
/// no reserved column — the core layout muri exists to provide.
pub fn resolve_segments(segments: &[SegmentMetrics], available_width: f32) -> Vec<SegmentBox> {
    let total_intrinsic: f32 = segments.iter().map(|s| s.intrinsic_width).sum();
    let leftover = (available_width - total_intrinsic).max(0.0);
    let grow_count = segments.iter().filter(|s| s.flex == Flex::Grow).count();
    let per_grow = if grow_count > 0 {
        leftover / grow_count as f32
    } else {
        0.0
    };

    let mut boxes = Vec::with_capacity(segments.len());
    let mut cursor = 0.0_f32;
    for seg in segments {
        let width = match seg.flex {
            Flex::Grow => seg.intrinsic_width + per_grow,
            Flex::Fixed => seg.intrinsic_width,
        };
        let slack = (width - seg.intrinsic_width).max(0.0);
        let text_x = match seg.align {
            Align::Left => cursor,
            Align::Center => cursor + slack / 2.0,
            Align::Right => cursor + slack,
        };
        boxes.push(SegmentBox {
            x: cursor,
            width,
            text_x,
        });
        cursor += width;
    }
    boxes
}

/// The total resolved width consumed by a set of segment boxes (the position of
/// the trailing edge of the last box).
pub fn consumed_width(boxes: &[SegmentBox]) -> f32 {
    boxes.last().map(|b| b.x + b.width).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < EPS, "expected {b}, got {a}");
    }

    #[test]
    fn grow_label_then_right_value_is_flush_right() {
        // label Grows, value is Fixed + Right. Available 200, label 50, value 40.
        let segs = [
            SegmentMetrics::new(50.0, Flex::Grow, Align::Left),
            SegmentMetrics::new(40.0, Flex::Fixed, Align::Right),
        ];
        let boxes = resolve_segments(&segs, 200.0);
        // label box grows to absorb the 110px leftover.
        approx(boxes[0].x, 0.0);
        approx(boxes[0].width, 160.0);
        approx(boxes[0].text_x, 0.0); // left aligned
                                      // value box stays 40 wide, positioned right after the label box.
        approx(boxes[1].x, 160.0);
        approx(boxes[1].width, 40.0);
        // value text is flush to the right edge: 200 - 40 = 160.
        approx(boxes[1].text_x, 160.0);
        approx(consumed_width(&boxes), 200.0);
    }

    #[test]
    fn two_grow_segments_split_leftover_evenly() {
        let segs = [
            SegmentMetrics::new(20.0, Flex::Grow, Align::Left),
            SegmentMetrics::new(20.0, Flex::Grow, Align::Left),
        ];
        let boxes = resolve_segments(&segs, 120.0);
        // leftover = 80, split 40/40 → each box 60 wide.
        approx(boxes[0].width, 60.0);
        approx(boxes[1].width, 60.0);
        approx(boxes[1].x, 60.0);
    }

    #[test]
    fn no_grow_segments_are_left_packed() {
        let segs = [
            SegmentMetrics::new(30.0, Flex::Fixed, Align::Left),
            SegmentMetrics::new(30.0, Flex::Fixed, Align::Left),
        ];
        let boxes = resolve_segments(&segs, 200.0);
        approx(boxes[0].x, 0.0);
        approx(boxes[1].x, 30.0);
        approx(consumed_width(&boxes), 60.0); // leftover stays empty
    }

    #[test]
    fn center_alignment_centers_within_box() {
        let segs = [SegmentMetrics::new(40.0, Flex::Grow, Align::Center)];
        let boxes = resolve_segments(&segs, 100.0);
        // box is 100 wide, text 40 → centered at (100-40)/2 = 30.
        approx(boxes[0].width, 100.0);
        approx(boxes[0].text_x, 30.0);
    }

    #[test]
    fn overflow_clamps_leftover_to_zero() {
        // intrinsic totals 150 but only 100 available: no negative widths.
        let segs = [
            SegmentMetrics::new(100.0, Flex::Grow, Align::Left),
            SegmentMetrics::new(50.0, Flex::Fixed, Align::Right),
        ];
        let boxes = resolve_segments(&segs, 100.0);
        approx(boxes[0].width, 100.0); // no extra added
        approx(boxes[1].width, 50.0);
        approx(boxes[1].text_x, 100.0); // right-align with zero slack
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert!(resolve_segments(&[], 100.0).is_empty());
        approx(consumed_width(&[]), 0.0);
    }
}
