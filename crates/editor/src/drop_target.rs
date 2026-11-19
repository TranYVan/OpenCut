//! Drop-target resolution: which track (existing or new) a dragged or
//! inserted element lands on. Ported from the classic
//! `timeline/placement/resolve.ts` and `insert-index.ts`.

use opencut_model::time::MediaTime;
use opencut_model::{SceneTracks, Track, TrackKind};

use crate::placement::{default_insert_index, enforce_main_track_start, highest_insert_index};

/// A stretch of time an element will occupy; checked against the target
/// track for overlaps. `exclude` is the element being moved, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeSpan {
    pub start: MediaTime,
    pub duration: MediaTime,
    pub exclude: Option<String>,
}

impl TimeSpan {
    pub fn new(start: MediaTime, duration: MediaTime) -> Self {
        Self {
            start,
            duration,
            exclude: None,
        }
    }
}

/// Which side of the hovered track a new track goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertSide {
    Above,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragDirection {
    Up,
    Down,
}

/// Where a new track goes when the caller has no preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewTrackPosition {
    /// As high as the kind may go.
    Highest,
    /// [`default_insert_index`].
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementStrategy {
    /// This track, or nothing when it is missing or the wrong kind.
    Explicit(String),
    /// The first compatible track with room, else a new track on top.
    FirstAvailable,
    /// The track at a display index when it fits, else a new track beside it.
    PreferIndex {
        track_index: usize,
        hover: InsertSide,
        /// Overrides `hover` when the hovered track is the wrong kind.
        vertical_drag: Option<DragDirection>,
        create_new_track_only: bool,
    },
    /// The track above `source_track_index`, then any track with room, else
    /// a new track on top.
    AboveSource {
        source_track_index: usize,
    },
    AlwaysNew(NewTrackPosition),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackPlacement {
    Existing {
        track_id: String,
        /// Display index (overlays, main, audio).
        track_index: usize,
        kind: TrackKind,
        /// Set when the main track's no-leading-gap rule moved the first span.
        adjusted_start: Option<MediaTime>,
    },
    New {
        kind: TrackKind,
        /// Display index for [`insert_track`](crate::placement::insert_track).
        insert_index: usize,
        position: Option<InsertSide>,
    },
}

/// Display index and side for a new track of `kind` next to
/// `preferred_index`. Audio stays below the main track and other kinds above.
pub fn preferred_new_track_placement(
    tracks: &SceneTracks,
    kind: TrackKind,
    preferred_index: usize,
    side: InsertSide,
) -> (usize, Option<InsertSide>) {
    let track_count = tracks.overlay.len() + 1 + tracks.audio.len();
    let main_index = tracks.overlay.len();
    let preferred = preferred_index.min(track_count - 1);
    let insert_at = |index: usize| {
        if side == InsertSide::Above {
            index
        } else {
            index + 1
        }
    };

    if kind == TrackKind::Audio {
        if preferred <= main_index {
            return (main_index + 1, Some(InsertSide::Below));
        }
        return (insert_at(preferred), Some(side));
    }
    let index = insert_at(preferred);
    if index > main_index {
        return (main_index, Some(InsertSide::Above));
    }
    (index, Some(side))
}

/// Resolves where an element of `kind` occupying `spans` should go. `None`
/// when an explicit track is missing or incompatible.
pub fn resolve_track_placement(
    tracks: &SceneTracks,
    kind: TrackKind,
    spans: &[TimeSpan],
    strategy: &PlacementStrategy,
) -> Option<TrackPlacement> {
    let ordered: Vec<&Track> = tracks.iter().collect();
    let fits = |track: &Track| {
        spans.iter().all(|span| {
            let exclude: Vec<&str> = span.exclude.as_deref().into_iter().collect();
            track.is_span_free(span.start, span.start + span.duration, &exclude)
        })
    };
    let first_available = || {
        ordered
            .iter()
            .position(|track| track.kind == kind && fits(track))
    };
    let existing = |index: usize| {
        let track = ordered[index];
        let first = spans.first();
        let requested = first.map_or(MediaTime::ZERO, |span| span.start);
        let exclude: Vec<&str> = first
            .and_then(|span| span.exclude.as_deref())
            .into_iter()
            .collect();
        let adjusted = enforce_main_track_start(tracks, &track.id, requested, &exclude);
        TrackPlacement::Existing {
            track_id: track.id.clone(),
            track_index: index,
            kind: track.kind,
            adjusted_start: (adjusted != requested).then_some(adjusted),
        }
    };
    let new_track = |insert_index: usize, position: Option<InsertSide>| TrackPlacement::New {
        kind,
        insert_index,
        position,
    };
    let on_top = || new_track(highest_insert_index(tracks, kind), None);

    Some(match strategy {
        PlacementStrategy::Explicit(track_id) => {
            let index = ordered.iter().position(|track| track.id == *track_id)?;
            if ordered[index].kind != kind {
                return None;
            }
            existing(index)
        }
        PlacementStrategy::FirstAvailable => first_available().map_or_else(on_top, existing),
        PlacementStrategy::PreferIndex {
            track_index,
            hover,
            vertical_drag,
            create_new_track_only,
        } => {
            let compatible = ordered
                .get(*track_index)
                .is_some_and(|track| track.kind == kind);
            if !create_new_track_only && compatible && fits(ordered[*track_index]) {
                return Some(existing(*track_index));
            }
            let side = match vertical_drag.filter(|_| !compatible) {
                Some(DragDirection::Up) => InsertSide::Above,
                Some(DragDirection::Down) => InsertSide::Below,
                None => *hover,
            };
            let (index, position) = preferred_new_track_placement(tracks, kind, *track_index, side);
            new_track(index, position)
        }
        PlacementStrategy::AboveSource { source_track_index } => {
            let above = source_track_index.checked_sub(1);
            match above.filter(|index| {
                ordered
                    .get(*index)
                    .is_some_and(|track| track.kind == kind && fits(track))
            }) {
                Some(index) => existing(index),
                None => first_available().map_or_else(on_top, existing),
            }
        }
        PlacementStrategy::AlwaysNew(NewTrackPosition::Highest) => on_top(),
        PlacementStrategy::AlwaysNew(NewTrackPosition::Default) => {
            new_track(default_insert_index(tracks, kind), None)
        }
    })
}
