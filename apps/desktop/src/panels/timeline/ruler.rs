//! Where the ruler puts its ticks and labels, and what the labels say.
//! Ported from the classic app's `timeline/ruler-utils.ts` (tag
//! `pre-rewrite`), but in whole ticks instead of float seconds, so "does
//! this tick get a label" is an exact remainder check.

use opencut_editor::model::time::{FrameRate, MediaTime, TICKS_PER_SECOND};

use super::scale::pixels_per_second;

/// Frame steps between labels. Starts at 2 so there's always a tick
/// between two labels, even at max zoom.
const LABEL_FRAME_INTERVALS: [i64; 5] = [2, 3, 5, 10, 15];
/// Frame steps between ticks.
const TICK_FRAME_INTERVALS: [i64; 6] = [1, 2, 3, 5, 10, 15];
/// Second steps, once a frame is too narrow to matter.
const SECOND_INTERVALS: [i64; 14] = [1, 2, 3, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];
/// Narrowest gap between labels that keeps them readable.
pub(crate) const MIN_LABEL_SPACING_PX: f64 = 120.0;
/// Narrowest gap between ticks.
pub(crate) const MIN_TICK_SPACING_PX: f64 = 18.0;
/// Used when even an hour is narrower than the minimum spacing.
const FALLBACK_INTERVAL_SECONDS: i64 = 3600;

/// Most ticks [`ruler_ticks`] yields, whatever the range.
const MAX_TICKS: usize = 2_000;

/// Gaps between the ruler's ticks and between its labels. The tick
/// interval always divides the label interval, so every label sits on a
/// tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RulerConfig {
    pub(crate) label_interval: MediaTime,
    pub(crate) tick_interval: MediaTime,
}

/// The ruler's intervals at `zoom` for a project at `fps`. Classic
/// `getRulerConfig`.
pub(crate) fn ruler_config(zoom: f64, fps: FrameRate) -> RulerConfig {
    let pixels_per_second = pixels_per_second(zoom);
    let ticks_per_frame = fps.ticks_per_frame();
    let label = optimal_interval(
        pixels_per_second,
        ticks_per_frame,
        MIN_LABEL_SPACING_PX,
        &LABEL_FRAME_INTERVALS,
    );
    let tick = optimal_interval(
        pixels_per_second,
        ticks_per_frame,
        MIN_TICK_SPACING_PX,
        &TICK_FRAME_INTERVALS,
    );
    RulerConfig {
        label_interval: MediaTime::from_ticks(label),
        tick_interval: MediaTime::from_ticks(tick_dividing_label(
            tick,
            label,
            pixels_per_second,
            ticks_per_frame,
        )),
    }
}

/// Pixel width of `ticks` at `pixels_per_second`.
#[allow(clippy::cast_precision_loss)]
fn spacing(ticks: i64, pixels_per_second: f64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND as f64 * pixels_per_second
}

/// Candidate intervals in ticks, finest first: whole frames (when the rate
/// has whole ticks per frame), then whole seconds.
fn candidates(
    ticks_per_frame: Option<i64>,
    frame_intervals: &[i64],
) -> impl Iterator<Item = i64> + '_ {
    let frames = frame_intervals
        .iter()
        .filter_map(move |frames| ticks_per_frame.map(|per_frame| per_frame * frames));
    let seconds = SECOND_INTERVALS
        .iter()
        .map(|seconds| seconds * TICKS_PER_SECOND);
    frames.chain(seconds)
}

/// The finest interval, in ticks, at least `min_spacing` pixels wide.
fn optimal_interval(
    pixels_per_second: f64,
    ticks_per_frame: Option<i64>,
    min_spacing: f64,
    frame_intervals: &[i64],
) -> i64 {
    candidates(ticks_per_frame, frame_intervals)
        .find(|ticks| spacing(*ticks, pixels_per_second) >= min_spacing)
        .unwrap_or(FALLBACK_INTERVAL_SECONDS * TICKS_PER_SECOND)
}

/// `tick` if it divides `label`, else the finest interval that does and
/// is still wide enough, else `label` itself (no ticks between labels).
/// Classic `ensureTickDividesLabel`.
fn tick_dividing_label(
    tick: i64,
    label: i64,
    pixels_per_second: f64,
    ticks_per_frame: Option<i64>,
) -> i64 {
    if tick > 0 && label % tick == 0 {
        return tick;
    }
    candidates(ticks_per_frame, &TICK_FRAME_INTERVALS)
        .find(|candidate| {
            label % candidate == 0 && spacing(*candidate, pixels_per_second) >= MIN_TICK_SPACING_PX
        })
        .unwrap_or(label)
}

/// One mark on the ruler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RulerTick {
    pub(crate) time: MediaTime,
    /// Whether this tick carries a label.
    pub(crate) labeled: bool,
}

/// The ticks from `start` to `end` (inclusive), at most [`MAX_TICKS`].
pub(crate) fn ruler_ticks(start: MediaTime, end: MediaTime, config: RulerConfig) -> Vec<RulerTick> {
    let step = config.tick_interval.as_ticks();
    let label = config.label_interval.as_ticks();
    if step <= 0 || label <= 0 || end < start {
        return Vec::new();
    }
    let start = start.as_ticks().max(0);
    let first = (start + step - 1) / step * step;
    (0..MAX_TICKS)
        .map_while(|index| {
            let ticks = first.checked_add(step.checked_mul(index as i64)?)?;
            (ticks <= end.as_ticks()).then_some(RulerTick {
                time: MediaTime::from_ticks(ticks),
                labeled: ticks % label == 0,
            })
        })
        .collect()
}

/// A ruler label: `MM:SS` (or `H:MM:SS`) on whole seconds, `Nf` for the
/// frames in between. Classic `formatRulerLabel`.
///
/// Frames count against the rate's nominal frames per second (30 for
/// 29.97), like a non-drop-frame timecode, so labels between seconds read
/// `5f`, `10f`, … and the 30th frame reads as the next second.
pub(crate) fn format_label(time: MediaTime, fps: FrameRate) -> String {
    let ticks = time.as_ticks().max(0);
    if ticks % TICKS_PER_SECOND == 0 {
        return format_seconds(ticks / TICKS_PER_SECOND);
    }
    let (Some(per_frame), Some(nominal)) = (fps.ticks_per_frame(), fps.frame_number_upper_bound())
    else {
        return format_seconds(ticks / TICKS_PER_SECOND);
    };
    let nominal = i64::from(nominal.max(1));
    let frame = (ticks + per_frame / 2) / per_frame;
    if frame % nominal == 0 {
        format_seconds(frame / nominal)
    } else {
        format!("{}f", frame % nominal)
    }
}

/// `MM:SS`, or `H:MM:SS` from an hour up.
fn format_seconds(total: i64) -> String {
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::timeline::scale::BASE_PIXELS_PER_SECOND;

    fn frames(count: i64, fps: FrameRate) -> MediaTime {
        MediaTime::from_frame(count, fps).expect("whole ticks per frame")
    }

    fn seconds(count: i64) -> MediaTime {
        MediaTime::from_ticks(count * TICKS_PER_SECOND)
    }

    #[test]
    fn zoomed_all_the_way_in_labels_every_other_frame() {
        // Max zoom: 5000 px/s, 166 px a frame at 30 fps.
        let config = ruler_config(100.0, FrameRate::FPS_30);
        assert_eq!(config.label_interval, frames(2, FrameRate::FPS_30));
        assert_eq!(config.tick_interval, frames(1, FrameRate::FPS_30));
    }

    #[test]
    fn default_zoom_labels_every_few_seconds() {
        // 50 px/s: labels need 120 px (3 s = 150 px); ticks need 18 px
        // (15 frames = 25 px), and half a second divides 3 s.
        let config = ruler_config(1.0, FrameRate::FPS_30);
        assert_eq!(config.label_interval, seconds(3));
        assert_eq!(config.tick_interval, frames(15, FrameRate::FPS_30));
    }

    #[test]
    fn frame_level_labels_at_moderate_zoom() {
        // 500 px/s, 16.7 px a frame: labels every 10 frames (167 px),
        // ticks every 2 frames (33 px), and 2 divides 10.
        let config = ruler_config(10.0, FrameRate::FPS_30);
        assert_eq!(config.label_interval, frames(10, FrameRate::FPS_30));
        assert_eq!(config.tick_interval, frames(2, FrameRate::FPS_30));
    }

    #[test]
    fn ticks_always_divide_labels_and_respect_spacing() {
        for fps in [
            FrameRate::FPS_24,
            FrameRate::FPS_25,
            FrameRate::FPS_29_97,
            FrameRate::FPS_30,
            FrameRate::FPS_60,
        ] {
            let mut zoom = 0.01;
            while zoom <= 100.0 {
                let config = ruler_config(zoom, fps);
                let (label, tick) = (
                    config.label_interval.as_ticks(),
                    config.tick_interval.as_ticks(),
                );
                assert_eq!(label % tick, 0, "zoom {zoom} fps {fps:?}");
                let pixels_per_second = BASE_PIXELS_PER_SECOND * zoom;
                let label_px = spacing(label, pixels_per_second);
                let tick_px = spacing(tick, pixels_per_second);
                let is_fallback = label == FALLBACK_INTERVAL_SECONDS * TICKS_PER_SECOND;
                assert!(label_px >= MIN_LABEL_SPACING_PX || is_fallback);
                assert!(tick_px >= MIN_TICK_SPACING_PX || tick == label);
                zoom *= 1.37;
            }
        }
    }

    #[test]
    fn tick_falls_back_to_a_divisor_of_the_label() {
        // 3-frame ticks don't divide 10-frame labels; 5 does.
        let per_frame = FrameRate::FPS_30.ticks_per_frame();
        let pixels_per_second = 30.0 * MIN_TICK_SPACING_PX; // 1 frame = 18 px
        let tick = tick_dividing_label(
            3 * per_frame.unwrap(),
            10 * per_frame.unwrap(),
            pixels_per_second,
            per_frame,
        );
        assert_eq!(tick, per_frame.unwrap());
        // Nothing divides and is wide enough: no ticks between labels.
        let label = 7 * TICKS_PER_SECOND;
        assert_eq!(
            tick_dividing_label(2 * TICKS_PER_SECOND, label, 1.0, None),
            label
        );
    }

    #[test]
    fn ticks_cover_the_range_and_mark_labels() {
        let config = RulerConfig {
            label_interval: seconds(2),
            tick_interval: seconds(1),
        };
        let ticks = ruler_ticks(MediaTime::from_ticks(1), seconds(4), config);
        let times: Vec<_> = ticks.iter().map(|tick| tick.time).collect();
        assert_eq!(times, [seconds(1), seconds(2), seconds(3), seconds(4)]);
        let labeled: Vec<_> = ticks.iter().map(|tick| tick.labeled).collect();
        assert_eq!(labeled, [false, true, false, true]);
        assert!(ruler_ticks(seconds(5), seconds(1), config).is_empty());
        let capped = ruler_ticks(
            MediaTime::ZERO,
            seconds(1_000_000),
            RulerConfig {
                label_interval: seconds(1),
                tick_interval: seconds(1),
            },
        );
        assert_eq!(capped.len(), MAX_TICKS);
    }

    #[test]
    fn labels_read_as_seconds_or_frames() {
        let fps = FrameRate::FPS_30;
        assert_eq!(format_label(MediaTime::ZERO, fps), "00:00");
        assert_eq!(format_label(seconds(90), fps), "01:30");
        assert_eq!(format_label(seconds(3725), fps), "1:02:05");
        assert_eq!(format_label(frames(5, fps), fps), "5f");
        assert_eq!(format_label(frames(35, fps), fps), "5f");
    }

    #[test]
    fn ntsc_frames_count_against_the_nominal_rate() {
        let fps = FrameRate::FPS_29_97;
        assert_eq!(format_label(frames(15, fps), fps), "15f");
        // Frame 30 is 1.001 s in, labeled as the next second.
        assert_eq!(format_label(frames(30, fps), fps), "00:01");
        assert_eq!(format_label(frames(45, fps), fps), "15f");
        // Whole seconds of real time are still seconds.
        assert_eq!(format_label(seconds(60), fps), "01:00");
    }
}
