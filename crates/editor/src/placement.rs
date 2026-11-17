//! Where elements and new tracks go. A subset of the classic
//! `timeline/placement` rules; drop-target resolution, snapping and group
//! moves come with the timeline UI (roadmap P3).

use opencut_model::time::MediaTime;
use opencut_model::{Element, SceneTracks, Track, TrackKind};

use crate::error::EditError;

/// Display index for a new track of this kind when the caller gives none:
/// effect tracks on top, other overlays just above the main track, audio at
/// the bottom.
pub fn default_insert_index(tracks: &SceneTracks, kind: TrackKind) -> usize {
    match kind {
        TrackKind::Audio => tracks.overlay.len() + 1 + tracks.audio.len(),
        TrackKind::Effect => 0,
        _ => tracks.overlay.len(),
    }
}

/// Display index of the top-most slot a track of this kind may take.
pub fn highest_insert_index(tracks: &SceneTracks, kind: TrackKind) -> usize {
    match kind {
        TrackKind::Audio => tracks.overlay.len() + 1,
        _ => 0,
    }
}

/// Inserts `track` at a display index (overlays, then main, then audio).
/// Audio tracks always land in the audio group and other kinds in the overlay
/// group, whatever the index says.
pub fn insert_track(tracks: &mut SceneTracks, track: Track, display_index: usize) {
    if track.kind == TrackKind::Audio {
        let index = display_index
            .saturating_sub(tracks.overlay.len() + 1)
            .min(tracks.audio.len());
        tracks.audio.insert(index, track);
    } else {
        let index = display_index.min(tracks.overlay.len());
        tracks.overlay.insert(index, track);
    }
}

/// The main track has no leading gap: an element placed at or before the
/// earliest main-track element snaps to zero.
pub fn enforce_main_track_start(
    tracks: &SceneTracks,
    track_id: &str,
    requested: MediaTime,
    exclude: &[&str],
) -> MediaTime {
    if !tracks.is_main(track_id) {
        return requested;
    }
    let earliest = tracks
        .main
        .elements
        .iter()
        .filter(|element| !exclude.contains(&element.id()))
        .map(Element::start_time)
        .min();
    match earliest {
        Some(earliest) if requested > earliest => requested,
        _ => MediaTime::ZERO,
    }
}

pub fn check_compatible(element: &Element, track: &Track) -> Result<(), EditError> {
    let kind = element.kind();
    if kind.track_kind() == track.kind {
        Ok(())
    } else {
        Err(EditError::IncompatibleTrack {
            element: kind,
            track: track.kind,
        })
    }
}

pub fn check_timing(element: &Element) -> Result<(), EditError> {
    let base = element.base();
    if base.start_time < MediaTime::ZERO
        || base.duration <= MediaTime::ZERO
        || base.trim_start < MediaTime::ZERO
        || base.trim_end < MediaTime::ZERO
    {
        return Err(EditError::InvalidTiming);
    }
    Ok(())
}

/// Errors when any two elements on the track overlap.
pub fn check_no_overlap(track: &Track) -> Result<(), EditError> {
    let mut spans: Vec<(MediaTime, MediaTime, &str)> = track
        .elements
        .iter()
        .map(|element| (element.start_time(), element.end_time(), element.id()))
        .collect();
    spans.sort_by_key(|&(start, end, _)| (start, end));
    for pair in spans.windows(2) {
        let (_, previous_end, _) = pair[0];
        let (next_start, _, next_id) = pair[1];
        if next_start < previous_end {
            return Err(EditError::Overlap {
                track_id: track.id.clone(),
                element_id: next_id.to_owned(),
            });
        }
    }
    Ok(())
}

/// The first existing track of `kind` with room for `start..end`. Video looks
/// at the main track first, then overlays from the main track upwards; other
/// kinds search top to bottom.
pub fn find_free_track(
    tracks: &SceneTracks,
    kind: TrackKind,
    start: MediaTime,
    end: MediaTime,
) -> Option<String> {
    let fits = |track: &&Track| track.kind == kind && track.is_span_free(start, end, &[]);
    let found = match kind {
        TrackKind::Video => std::iter::once(&tracks.main)
            .chain(tracks.overlay.iter().rev())
            .find(fits),
        _ => tracks.iter().find(fits),
    };
    found.map(|track| track.id.clone())
}
