//! Sizes and positions of the timeline's rows and of the parts of elements
//! that are on screen. Pure math; sizes from the classic app's
//! `timeline/components/layout.ts` (tag `pre-rewrite`).

use opencut_editor::model::TrackKind;

/// Height of the ruler strip above the tracks.
pub(crate) const RULER_HEIGHT: f64 = 24.0;
/// Width of the track header column.
pub(crate) const HEADER_WIDTH: f64 = 148.0;
/// Space between two track rows.
pub(crate) const TRACK_GAP: f64 = 6.0;
/// Space above the first track.
pub(crate) const CONTENT_TOP_PADDING: f64 = 4.0;
/// Space below the last track, so it can be scrolled clear of the edge.
pub(crate) const CONTENT_BOTTOM_PADDING: f64 = 24.0;

/// Row height of a track. Classic `TIMELINE_TRACK_HEIGHTS_PX`.
pub(crate) const fn track_height(kind: TrackKind) -> f64 {
    match kind {
        TrackKind::Video => 65.0,
        TrackKind::Audio => 50.0,
        TrackKind::Text | TrackKind::Graphic | TrackKind::Effect => 25.0,
    }
}

/// A track row's vertical extent in content pixels (`0` = top of the
/// tracks area, before vertical scroll).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RowLayout {
    pub(crate) top: f64,
    pub(crate) height: f64,
}

impl RowLayout {
    pub(crate) fn bottom(self) -> f64 {
        self.top + self.height
    }
}

/// Rows for tracks of `kinds`, top to bottom, plus the content height
/// (including top and bottom padding).
pub(crate) fn track_rows(kinds: impl IntoIterator<Item = TrackKind>) -> (Vec<RowLayout>, f64) {
    let mut rows = Vec::new();
    let mut top = CONTENT_TOP_PADDING;
    for kind in kinds {
        if !rows.is_empty() {
            top += TRACK_GAP;
        }
        let height = track_height(kind);
        rows.push(RowLayout { top, height });
        top += height;
    }
    (rows, top + CONTENT_BOTTOM_PADDING)
}

/// The on-screen part of a horizontal span. All values are in viewport
/// pixels except `offset`, which is how far into the span (from its start)
/// the visible part begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VisibleSpan {
    pub(crate) left: f64,
    pub(crate) width: f64,
    pub(crate) offset: f64,
}

/// The part of content span `start..end` that falls within the viewport
/// (`0..viewport_width` after subtracting `scroll`), widened by `margin` on
/// both sides so borders just off screen don't show. `None` when none of it
/// is on screen.
///
/// Elements can be millions of pixels wide when zoomed in on a long clip;
/// drawing only this part keeps every element a viewport wide at most.
pub(crate) fn visible_span(
    start: f64,
    end: f64,
    scroll: f64,
    viewport_width: f64,
    margin: f64,
) -> Option<VisibleSpan> {
    let left = start - scroll;
    let right = end - scroll;
    if right <= 0.0 || left >= viewport_width || right <= left {
        return None;
    }
    let clipped_left = left.max(-margin);
    let clipped_right = right.min(viewport_width + margin);
    Some(VisibleSpan {
        left: clipped_left,
        width: clipped_right - clipped_left,
        offset: clipped_left - left,
    })
}

/// Left edges (relative to the visible part of a span) of the thumbnail
/// tiles covering `span`, `tile_width` apart and aligned to the span's
/// start, so tiles stay put while scrolling.
pub(crate) fn tile_offsets(span: VisibleSpan, tile_width: f64) -> Vec<f64> {
    if !(tile_width > 0.0 && tile_width.is_finite()) {
        return Vec::new();
    }
    let first = (span.offset / tile_width).floor() * tile_width;
    let end = span.offset + span.width;
    let mut offsets = Vec::new();
    let mut tile = first;
    while tile < end && offsets.len() < 512 {
        offsets.push(tile - span.offset);
        tile += tile_width;
    }
    offsets
}

/// Source seconds shown by the visible part of an element: `span_offset`
/// to `span_offset + span_width` pixels into it, at `pixels_per_second`.
/// The element starts `trim_start` seconds into its source and plays it at
/// `rate` (2.0 = twice as fast).
pub(crate) fn source_range(
    trim_start: f64,
    rate: f64,
    span: VisibleSpan,
    pixels_per_second: f64,
) -> (f64, f64) {
    if pixels_per_second <= 0.0 {
        return (trim_start, trim_start);
    }
    let from = span.offset / pixels_per_second;
    let to = (span.offset + span.width) / pixels_per_second;
    (trim_start + from * rate, trim_start + to * rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stack_with_gaps_and_padding() {
        let (rows, height) = track_rows([TrackKind::Text, TrackKind::Video, TrackKind::Audio]);
        assert_eq!(
            rows,
            [
                RowLayout {
                    top: CONTENT_TOP_PADDING,
                    height: 25.0
                },
                RowLayout {
                    top: CONTENT_TOP_PADDING + 25.0 + TRACK_GAP,
                    height: 65.0
                },
                RowLayout {
                    top: CONTENT_TOP_PADDING + 25.0 + 65.0 + 2.0 * TRACK_GAP,
                    height: 50.0
                },
            ]
        );
        assert_eq!(height, rows[2].bottom() + CONTENT_BOTTOM_PADDING);
        let (empty, empty_height) = track_rows([]);
        assert!(empty.is_empty());
        assert_eq!(empty_height, CONTENT_TOP_PADDING + CONTENT_BOTTOM_PADDING);
    }

    #[test]
    fn spans_off_screen_are_skipped() {
        assert_eq!(visible_span(0.0, 100.0, 200.0, 500.0, 2.0), None);
        assert_eq!(visible_span(800.0, 900.0, 200.0, 500.0, 2.0), None);
        assert_eq!(visible_span(10.0, 10.0, 0.0, 500.0, 2.0), None);
    }

    #[test]
    fn spans_inside_are_drawn_whole() {
        assert_eq!(
            visible_span(300.0, 400.0, 200.0, 500.0, 2.0),
            Some(VisibleSpan {
                left: 100.0,
                width: 100.0,
                offset: 0.0
            })
        );
    }

    #[test]
    fn long_spans_are_clipped_to_the_viewport_plus_margin() {
        assert_eq!(
            visible_span(0.0, 1e9, 1000.0, 500.0, 2.0),
            Some(VisibleSpan {
                left: -2.0,
                width: 504.0,
                offset: 998.0
            })
        );
    }

    #[test]
    fn tiles_align_to_the_span_start() {
        let whole = VisibleSpan {
            left: 0.0,
            width: 250.0,
            offset: 0.0,
        };
        assert_eq!(tile_offsets(whole, 100.0), [0.0, 100.0, 200.0]);
        // Scrolled 130 px into the span: the tile that started at 100 is
        // 30 px off the left edge.
        let scrolled = VisibleSpan {
            left: 0.0,
            width: 150.0,
            offset: 130.0,
        };
        assert_eq!(tile_offsets(scrolled, 100.0), [-30.0, 70.0]);
        assert!(tile_offsets(whole, 0.0).is_empty());
    }

    #[test]
    fn source_range_follows_trim_and_rate() {
        let span = VisibleSpan {
            left: 0.0,
            width: 100.0,
            offset: 50.0,
        };
        // 50 px/s: the visible part is 1 s to 3 s into the element.
        assert_eq!(source_range(0.0, 1.0, span, 50.0), (1.0, 3.0));
        assert_eq!(source_range(10.0, 1.0, span, 50.0), (11.0, 13.0));
        assert_eq!(source_range(10.0, 2.0, span, 50.0), (12.0, 16.0));
        assert_eq!(source_range(4.0, 1.0, span, 0.0), (4.0, 4.0));
    }
}
