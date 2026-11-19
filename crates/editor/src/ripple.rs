//! Ripple editing: after a command, time it freed on a track (deleted or
//! shortened elements) is closed by shifting the later elements left.
//! Ported from the classic `ripple/`. [`Editor`](crate::Editor) runs this
//! after every command while `ripple_enabled` is set.

use std::collections::{HashMap, HashSet};

use opencut_model::SceneTracks;
use opencut_model::time::MediaTime;

type Interval = (MediaTime, MediaTime);

/// Shift every element of `track_id` that starts at or after `after` left by
/// `amount`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RippleAdjustment {
    pub track_id: String,
    pub after: MediaTime,
    pub amount: MediaTime,
}

/// The ripple that closes the gaps `after` left compared to `before`: per
/// track, the time elements vacated (removed, or ending earlier) minus the
/// time newly joined elements fill. Elements that moved to another track
/// vacate nothing.
pub fn compute_ripple_adjustments(
    before: &SceneTracks,
    after: &SceneTracks,
) -> Vec<RippleAdjustment> {
    let after_ids: HashSet<&str> = after
        .iter()
        .flat_map(|track| track.elements.iter().map(|element| element.id()))
        .collect();
    let after_spans: HashMap<&str, HashMap<&str, Interval>> = after
        .iter()
        .map(|track| {
            let spans = track
                .elements
                .iter()
                .map(|element| (element.id(), (element.start_time(), element.end_time())))
                .collect();
            (track.id.as_str(), spans)
        })
        .collect();
    let no_spans = HashMap::new();

    let mut adjustments = Vec::new();
    for track in before.iter() {
        let after_track = after_spans.get(track.id.as_str()).unwrap_or(&no_spans);
        let mut vacated = Vec::new();
        for element in &track.elements {
            let end = element.end_time();
            match after_track.get(element.id()) {
                None if !after_ids.contains(element.id()) => {
                    push_interval(&mut vacated, element.start_time(), end);
                }
                Some(&(_, after_end)) if end > after_end => {
                    push_interval(&mut vacated, after_end, end);
                }
                _ => {}
            }
        }
        let before_ids: HashSet<&str> = track.elements.iter().map(|element| element.id()).collect();
        let mut joined = Vec::new();
        for (id, &(start, end)) in after_track {
            if !before_ids.contains(id) {
                push_interval(&mut joined, start, end);
            }
        }

        let joined = normalize(joined);
        for interval in normalize(vacated) {
            for (start, end) in subtract(interval, &joined) {
                adjustments.push(RippleAdjustment {
                    track_id: track.id.clone(),
                    after: end,
                    amount: end - start,
                });
            }
        }
    }
    adjustments
}

/// Applies the adjustments, latest `after` first so earlier ones still see
/// the original times.
pub fn apply_ripple_adjustments(tracks: &mut SceneTracks, adjustments: &[RippleAdjustment]) {
    let mut sorted: Vec<&RippleAdjustment> = adjustments.iter().collect();
    sorted.sort_by_key(|adjustment| std::cmp::Reverse(adjustment.after));
    for adjustment in sorted {
        let Some(track) = tracks.track_mut(&adjustment.track_id) else {
            continue;
        };
        for element in &mut track.elements {
            if element.start_time() >= adjustment.after {
                element.base_mut().start_time -= adjustment.amount;
            }
        }
    }
}

fn push_interval(intervals: &mut Vec<Interval>, start: MediaTime, end: MediaTime) {
    if end > start {
        intervals.push((start, end));
    }
}

/// Sorted, with touching and overlapping intervals merged.
fn normalize(mut intervals: Vec<Interval>) -> Vec<Interval> {
    intervals.sort();
    let mut merged: Vec<Interval> = Vec::new();
    for (start, end) in intervals {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// `source` with every interval of `holes` cut out.
fn subtract(source: Interval, holes: &[Interval]) -> Vec<Interval> {
    let mut remaining = vec![source];
    for &(hole_start, hole_end) in holes {
        remaining = remaining
            .into_iter()
            .flat_map(|(start, end)| {
                if hole_end <= start || hole_start >= end {
                    return vec![(start, end)];
                }
                let mut parts = Vec::new();
                push_interval(&mut parts, start, hole_start);
                push_interval(&mut parts, hole_end, end);
                parts
            })
            .collect();
    }
    remaining
}
