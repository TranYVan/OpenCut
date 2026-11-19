//! Timeline snapping: candidate points (element edges, playhead, bookmarks)
//! and picking the closest one within a pixel threshold. Ported from the
//! classic `timeline/snapping/` and `timeline/group-move/snap.ts`. Keyframe
//! snap points wait for animations to be ported.

use opencut_model::time::MediaTime;
use opencut_model::{Bookmark, ElementRef, SceneTracks};

use crate::editor::Editor;

/// Classic `DEFAULT_TIMELINE_SNAP_THRESHOLD_PX`.
pub const DEFAULT_SNAP_THRESHOLD_PX: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapPointKind {
    ElementStart,
    ElementEnd,
    Playhead,
    Bookmark,
}

/// Something a dragged edge can snap to. The UI draws its snap indicator at
/// `time`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapPoint {
    pub time: MediaTime,
    pub kind: SnapPointKind,
    /// The element whose edge this is, for element edges.
    pub element: Option<ElementRef>,
}

impl SnapPoint {
    fn at(time: MediaTime, kind: SnapPointKind) -> Self {
        Self {
            time,
            kind,
            element: None,
        }
    }
}

/// The snap threshold in time at a zoom level (classic
/// `getTimelineSnapThresholdInTicks`).
pub fn snap_threshold(threshold_px: f64, pixels_per_second: f64) -> MediaTime {
    if pixels_per_second <= 0.0 {
        return MediaTime::ZERO;
    }
    MediaTime::from_seconds_f64(threshold_px / pixels_per_second).unwrap_or(MediaTime::ZERO)
}

/// Every element start and end in the scene except those of `exclude`d
/// element ids (the ones being dragged).
pub fn element_snap_points(tracks: &SceneTracks, exclude: &[&str]) -> Vec<SnapPoint> {
    let mut points = Vec::new();
    for track in tracks.iter() {
        for element in track
            .elements
            .iter()
            .filter(|element| !exclude.contains(&element.id()))
        {
            let element_ref = Some(ElementRef::new(&track.id, element.id()));
            points.push(SnapPoint {
                time: element.start_time(),
                kind: SnapPointKind::ElementStart,
                element: element_ref.clone(),
            });
            points.push(SnapPoint {
                time: element.end_time(),
                kind: SnapPointKind::ElementEnd,
                element: element_ref,
            });
        }
    }
    points
}

/// Element edges, the playhead and bookmarks, in that order.
pub fn snap_points(
    tracks: &SceneTracks,
    bookmarks: &[Bookmark],
    playhead: MediaTime,
    exclude: &[&str],
) -> Vec<SnapPoint> {
    let mut points = element_snap_points(tracks, exclude);
    points.push(SnapPoint::at(playhead, SnapPointKind::Playhead));
    points.extend(
        bookmarks
            .iter()
            .map(|bookmark| SnapPoint::at(bookmark.time, SnapPointKind::Bookmark)),
    );
    points
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapResult {
    pub time: MediaTime,
    /// The point snapped to; `None` when nothing was within range.
    pub point: Option<SnapPoint>,
    /// Distance to `point`; zero when there is none.
    pub distance: MediaTime,
}

fn distance(a: MediaTime, b: MediaTime) -> MediaTime {
    (a - b).max(b - a)
}

/// The closest point within `max_distance` of `target` (the first of equals).
pub fn resolve_snap(
    target: MediaTime,
    points: &[SnapPoint],
    max_distance: MediaTime,
) -> SnapResult {
    let mut best: Option<(&SnapPoint, MediaTime)> = None;
    for point in points {
        let gap = distance(target, point.time);
        if gap <= max_distance && best.is_none_or(|(_, closest)| gap < closest) {
            best = Some((point, gap));
        }
    }
    match best {
        Some((point, gap)) => SnapResult {
            time: point.time,
            point: Some(point.clone()),
            distance: gap,
        },
        None => SnapResult {
            time: target,
            point: None,
            distance: MediaTime::ZERO,
        },
    }
}

/// How far to shift a dragged group so one of its edges lands on a snap
/// point (classic `snapGroupEdges`). `spans` are the members' `(start, end)`
/// at their unsnapped positions; the closest start or end within
/// `max_distance` wins. `None` when nothing is in range.
pub fn snap_group_shift(
    spans: &[(MediaTime, MediaTime)],
    points: &[SnapPoint],
    max_distance: MediaTime,
) -> Option<(MediaTime, SnapPoint)> {
    let mut best: Option<(MediaTime, MediaTime, SnapPoint)> = None;
    for &(start, end) in spans {
        for edge in [start, end] {
            let snap = resolve_snap(edge, points, max_distance);
            if let Some(point) = snap.point
                && best
                    .as_ref()
                    .is_none_or(|(_, closest, _)| snap.distance < *closest)
            {
                best = Some((snap.time - edge, snap.distance, point));
            }
        }
    }
    best.map(|(shift, _, point)| (shift, point))
}

impl Editor {
    /// Snap candidates for dragging `exclude`d elements: the scene's element
    /// edges, the playhead and the scene's bookmarks.
    pub fn snap_points(&self, exclude: &[&str]) -> Vec<SnapPoint> {
        let bookmarks = self
            .project()
            .current_scene()
            .map_or(&[][..], |scene| &scene.bookmarks);
        snap_points(self.tracks(), bookmarks, self.playback().position, exclude)
    }

    /// Classic `toggle-snapping`. Returns the new state.
    pub fn toggle_snapping(&mut self) -> bool {
        self.snapping_enabled = !self.snapping_enabled;
        self.snapping_enabled
    }

    /// Classic `toggle-ripple-editing`. Returns the new state.
    pub fn toggle_ripple_editing(&mut self) -> bool {
        self.ripple_enabled = !self.ripple_enabled;
        self.ripple_enabled
    }
}
