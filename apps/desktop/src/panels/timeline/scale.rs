//! Zoom and the mapping between timeline time and pixels. Pure math, ported
//! from the classic app's `timeline/scale.ts`, `pixel-utils.ts`,
//! `zoom-utils.ts` and `controllers/zoom-controller.ts` (tag `pre-rewrite`).
//!
//! Pixels here are "content" pixels: `0` is the start of the timeline, not
//! the left edge of the viewport. Subtract the horizontal scroll offset to
//! get a viewport position.

use opencut_editor::model::time::{MediaTime, TICKS_PER_SECOND};

/// Pixels per second at zoom `1.0`.
pub(crate) const BASE_PIXELS_PER_SECOND: f64 = 50.0;
/// The most zoomed-in level: 5,000 px per second.
pub(crate) const ZOOM_MAX: f64 = 100.0;
/// Zoom when nothing else says otherwise.
pub(crate) const DEFAULT_ZOOM: f64 = 1.0;
/// How far one toolbar zoom step moves along the zoom slider (`0..=1`).
pub(crate) const ZOOM_STEP: f64 = 0.1;
/// Share of the viewport the scene fills after "Fit".
pub(crate) const FIT_SHARE: f64 = 0.9;

/// Empty space after the content, as a share of the viewport width: most of
/// the viewport when zoomed all the way out, less once zoomed in.
const PADDING_MAX_RATIO: f64 = 0.75;
const PADDING_MIN_RATIO: f64 = 0.15;
/// Zoom percent (`0..=1` between min and max zoom) at which the padding has
/// shrunk to its minimum.
const PADDING_MIN_AT_ZOOM_PERCENT: f64 = 0.2;
/// Largest wheel delta (in pixels) that counts towards one zoom step, so a
/// fast flick doesn't jump to the end of the range.
const WHEEL_DELTA_CAP: f64 = 30.0;
/// Wheel pixels per e-fold of zoom.
const WHEEL_ZOOM_SCALE: f64 = 300.0;

#[allow(clippy::cast_precision_loss)]
const TICKS_PER_SECOND_F64: f64 = TICKS_PER_SECOND as f64;

pub(crate) fn pixels_per_second(zoom: f64) -> f64 {
    BASE_PIXELS_PER_SECOND * zoom
}

/// Where `time` sits, in content pixels.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn time_to_px(time: MediaTime, zoom: f64) -> f64 {
    time.as_ticks() as f64 / TICKS_PER_SECOND_F64 * pixels_per_second(zoom)
}

/// The time at content pixel `px`, never negative.
pub(crate) fn px_to_time(px: f64, zoom: f64) -> MediaTime {
    let pixels_per_second = pixels_per_second(zoom);
    if !(px.is_finite() && pixels_per_second > 0.0) || px <= 0.0 {
        return MediaTime::ZERO;
    }
    MediaTime::from_seconds_f64(px / pixels_per_second).unwrap_or(MediaTime::ZERO)
}

/// The most zoomed-out level: the whole scene (at least a second) fits in a
/// quarter of the viewport, the rest being padding. Classic
/// `getTimelineZoomMin`.
pub(crate) fn min_zoom(duration: MediaTime, viewport_width: f64) -> f64 {
    let seconds = duration.to_seconds_f64().max(1.0);
    let width = if viewport_width > 0.0 {
        viewport_width
    } else {
        1000.0
    };
    let zoom_to_fit = width * (1.0 - PADDING_MAX_RATIO) / (seconds * BASE_PIXELS_PER_SECOND);
    zoom_to_fit.min(ZOOM_MAX)
}

pub(crate) fn clamp_zoom(zoom: f64, min_zoom: f64) -> f64 {
    if !zoom.is_finite() {
        return min_zoom;
    }
    zoom.clamp(min_zoom.min(ZOOM_MAX), ZOOM_MAX)
}

/// How far `zoom` is between `min_zoom` and [`ZOOM_MAX`], `0..=1`, on a
/// log scale so each step of a slider feels the same. Classic
/// `zoomToSlider`.
pub(crate) fn zoom_to_slider(zoom: f64, min_zoom: f64) -> f64 {
    if min_zoom >= ZOOM_MAX || min_zoom <= 0.0 {
        return 1.0;
    }
    let zoom = zoom.clamp(min_zoom, ZOOM_MAX);
    (zoom / min_zoom).ln() / (ZOOM_MAX / min_zoom).ln()
}

/// Inverse of [`zoom_to_slider`]. Classic `sliderToZoom`.
pub(crate) fn slider_to_zoom(position: f64, min_zoom: f64) -> f64 {
    let position = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    min_zoom * (ZOOM_MAX / min_zoom).powf(position)
}

/// The zoom multiplier for one wheel event with vertical delta `delta_y`
/// pixels (positive = wheel towards the user, which zooms out).
pub(crate) fn wheel_zoom_factor(delta_y: f64) -> f64 {
    if !delta_y.is_finite() {
        return 1.0;
    }
    let capped = delta_y.signum() * delta_y.abs().min(WHEEL_DELTA_CAP);
    (-capped / WHEEL_ZOOM_SCALE).exp()
}

/// Empty space after the scene's end, in pixels. Classic
/// `getTimelinePaddingPx`.
pub(crate) fn padding_px(viewport_width: f64, zoom: f64, min_zoom: f64) -> f64 {
    let zoom_percent = if ZOOM_MAX > min_zoom {
        ((zoom - min_zoom) / (ZOOM_MAX - min_zoom)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let transition = (zoom_percent / PADDING_MIN_AT_ZOOM_PERCENT).min(1.0);
    let ratio = PADDING_MAX_RATIO - (PADDING_MAX_RATIO - PADDING_MIN_RATIO) * transition;
    viewport_width * ratio
}

/// Width of the scrollable content: the scene plus padding, at least the
/// viewport.
pub(crate) fn content_width(
    duration: MediaTime,
    viewport_width: f64,
    zoom: f64,
    min_zoom: f64,
) -> f64 {
    let width = time_to_px(duration, zoom) + padding_px(viewport_width, zoom, min_zoom);
    width.max(viewport_width)
}

/// Clamps a scroll offset into `0..=content - viewport`.
pub(crate) fn clamp_scroll(scroll: f64, content: f64, viewport: f64) -> f64 {
    let max = (content - viewport).max(0.0);
    if !scroll.is_finite() {
        return 0.0;
    }
    scroll.clamp(0.0, max)
}

/// The scroll offset that keeps the time under viewport pixel `anchor_x`
/// in place when the zoom changes from `old_zoom` to `new_zoom` (not yet
/// clamped to the content).
pub(crate) fn anchored_scroll(anchor_x: f64, scroll: f64, old_zoom: f64, new_zoom: f64) -> f64 {
    if old_zoom <= 0.0 {
        return scroll;
    }
    let content_x = scroll + anchor_x;
    content_x * (new_zoom / old_zoom) - anchor_x
}

/// The zoom at which `duration` fills `share` of `viewport_width`.
pub(crate) fn zoom_to_fit(duration: MediaTime, viewport_width: f64, share: f64) -> f64 {
    let seconds = duration.to_seconds_f64();
    if seconds <= 0.0 || viewport_width <= 0.0 {
        return DEFAULT_ZOOM;
    }
    viewport_width * share / (seconds * BASE_PIXELS_PER_SECOND)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: f64) -> MediaTime {
        MediaTime::from_seconds_f64(value).expect("finite")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn time_and_pixels_round_trip() {
        assert!(close(time_to_px(seconds(2.0), 1.0), 100.0));
        assert!(close(time_to_px(seconds(2.0), 1.5), 150.0));
        assert!(close(time_to_px(MediaTime::ZERO, 3.0), 0.0));
        assert_eq!(px_to_time(100.0, 1.0), seconds(2.0));
        assert_eq!(px_to_time(150.0, 1.5), seconds(2.0));
        for zoom in [0.1, 1.0, 7.3, ZOOM_MAX] {
            let time = MediaTime::from_ticks(123_456);
            let back = px_to_time(time_to_px(time, zoom), zoom);
            assert!(
                (back.as_ticks() - time.as_ticks()).abs() <= 1,
                "zoom {zoom}"
            );
        }
    }

    #[test]
    fn pixels_left_of_the_start_are_time_zero() {
        assert_eq!(px_to_time(-20.0, 1.0), MediaTime::ZERO);
        assert_eq!(px_to_time(f64::NAN, 1.0), MediaTime::ZERO);
        assert_eq!(px_to_time(10.0, 0.0), MediaTime::ZERO);
    }

    #[test]
    fn min_zoom_fits_the_scene_in_a_quarter_of_the_viewport() {
        // 10 s in 1000 px: 250 px of content, 25 px/s, zoom 0.5.
        assert!(close(min_zoom(seconds(10.0), 1000.0), 0.5));
        // Short scenes count as one second.
        assert!(close(min_zoom(MediaTime::ZERO, 1000.0), 5.0));
        // Never above the max.
        assert!(close(min_zoom(MediaTime::ZERO, 1e9), ZOOM_MAX));
        // Unknown width falls back to 1000 px.
        assert!(close(min_zoom(seconds(10.0), 0.0), 0.5));
    }

    #[test]
    fn zoom_is_clamped_to_its_range() {
        assert!(close(clamp_zoom(0.01, 0.5), 0.5));
        assert!(close(clamp_zoom(1_000.0, 0.5), ZOOM_MAX));
        assert!(close(clamp_zoom(2.0, 0.5), 2.0));
        assert!(close(clamp_zoom(f64::NAN, 0.5), 0.5));
    }

    #[test]
    fn slider_is_logarithmic_and_inverts() {
        let min = 0.1;
        assert!(close(zoom_to_slider(min, min), 0.0));
        assert!(close(zoom_to_slider(ZOOM_MAX, min), 1.0));
        // sqrt(0.1 * 100) is halfway on a log scale.
        assert!(close(zoom_to_slider((min * ZOOM_MAX).sqrt(), min), 0.5));
        for position in [0.0, 0.25, 0.5, 0.9, 1.0] {
            assert!(close(
                zoom_to_slider(slider_to_zoom(position, min), min),
                position
            ));
        }
        assert!(close(slider_to_zoom(2.0, min), ZOOM_MAX));
    }

    #[test]
    fn wheel_zoom_is_capped_and_symmetric() {
        assert!(close(wheel_zoom_factor(0.0), 1.0));
        assert!(wheel_zoom_factor(10.0) < 1.0, "wheel down zooms out");
        assert!(wheel_zoom_factor(-10.0) > 1.0, "wheel up zooms in");
        assert!(close(wheel_zoom_factor(30.0), wheel_zoom_factor(500.0)));
        assert!(close(
            wheel_zoom_factor(-12.0) * wheel_zoom_factor(12.0),
            1.0
        ));
        assert!(close(wheel_zoom_factor(f64::NAN), 1.0));
    }

    #[test]
    fn padding_shrinks_as_zoom_grows() {
        let min = 0.5;
        assert!(close(padding_px(1000.0, min, min), 750.0));
        let at_twenty_percent = min + (ZOOM_MAX - min) * 0.2;
        assert!(close(padding_px(1000.0, at_twenty_percent, min), 150.0));
        assert!(close(padding_px(1000.0, ZOOM_MAX, min), 150.0));
    }

    #[test]
    fn content_is_scene_plus_padding_but_never_narrower_than_the_viewport() {
        // 10 s at zoom 1 is 500 px, plus 150 px padding at max-ish zoom.
        let width = content_width(seconds(10.0), 1000.0, 1.0, 0.5);
        assert!(close(width, 500.0 + padding_px(1000.0, 1.0, 0.5)));
        assert!(close(
            content_width(MediaTime::ZERO, 1000.0, 1.0, 1.0),
            1000.0
        ));
    }

    #[test]
    fn scroll_is_clamped_to_the_content() {
        assert!(close(clamp_scroll(-5.0, 2000.0, 500.0), 0.0));
        assert!(close(clamp_scroll(1800.0, 2000.0, 500.0), 1500.0));
        assert!(close(clamp_scroll(100.0, 400.0, 500.0), 0.0));
        assert!(close(clamp_scroll(f64::INFINITY, 2000.0, 500.0), 0.0));
    }

    #[test]
    fn anchored_zoom_keeps_the_time_under_the_anchor() {
        let (scroll, anchor) = (240.0, 100.0);
        let time = px_to_time(scroll + anchor, 1.5);
        let next = anchored_scroll(anchor, scroll, 1.5, 3.0);
        let after = px_to_time(next + anchor, 3.0);
        assert!((after.as_ticks() - time.as_ticks()).abs() <= 1);
        // Anchored at the left edge with no scroll stays at zero.
        assert!(close(anchored_scroll(0.0, 0.0, 1.0, 4.0), 0.0));
    }

    #[test]
    fn fit_fills_the_requested_share() {
        let zoom = zoom_to_fit(seconds(20.0), 1000.0, 0.8);
        assert!(close(time_to_px(seconds(20.0), zoom), 800.0));
        assert!(close(
            zoom_to_fit(MediaTime::ZERO, 1000.0, 0.8),
            DEFAULT_ZOOM
        ));
    }
}
