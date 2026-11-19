//! Group edits across tracks: dragging a selection onto new tracks (classic
//! `group-move` `newTracks` target) and trimming a selection's edges
//! together (classic `group-resize`). Moves onto existing tracks are
//! [`MoveElements::drag`](crate::commands::MoveElements::drag).

use opencut_model::time::{FrameRate, MediaTime};
use opencut_model::{ElementRef, Project, SceneTracks, Track, TrackKind, new_id};

use crate::command::{Command, Outcome, current_tracks};
use crate::commands::{Edge, ElementMove, MoveElements, SetElementTiming};
use crate::error::EditError;
use crate::placement::insert_track;

/// A track [`MoveElementsToNewTracks`] creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrack {
    pub id: String,
    pub kind: TrackKind,
    /// Display index (overlays, main, audio) to insert it at.
    pub index: usize,
}

/// Creates tracks and moves elements onto them in one undo step.
pub struct MoveElementsToNewTracks {
    pub create: Vec<NewTrack>,
    pub moves: Vec<ElementMove>,
}

impl MoveElementsToNewTracks {
    /// Drags `selection` (by its `anchor`) onto new tracks, one new track per
    /// source track, keeping their order, with the anchor's track at display
    /// index `anchor_insert_index` (clamped to the section: audio below the
    /// main track, everything else above it). The anchor starts at
    /// `anchor_start`, clamped so the group never starts before zero.
    /// `None` when an element isn't found or the group mixes audio with
    /// other kinds.
    pub fn drag(
        tracks: &SceneTracks,
        selection: &[ElementRef],
        anchor: &ElementRef,
        anchor_insert_index: usize,
        anchor_start: MediaTime,
    ) -> Option<Self> {
        let display: Vec<&Track> = tracks.iter().collect();
        let anchor_start_time = tracks.element(anchor)?.start_time();

        let mut members: Vec<(&ElementRef, MediaTime, usize)> = Vec::new();
        for element_ref in std::iter::once(anchor).chain(selection) {
            if members
                .iter()
                .any(|(seen, _, _)| seen.element_id == element_ref.element_id)
            {
                continue;
            }
            let offset = tracks.element(element_ref)?.start_time() - anchor_start_time;
            let index = display
                .iter()
                .position(|track| track.id == element_ref.track_id)?;
            members.push((element_ref, offset, index));
        }

        let mut sources: Vec<usize> = members.iter().map(|(_, _, index)| *index).collect();
        sources.sort_unstable();
        sources.dedup();
        let kind_of = |source: usize| display[source].kind;
        let audio = |source: usize| kind_of(source) == TrackKind::Audio;
        if sources.iter().any(|s| audio(*s)) && sources.iter().any(|s| !audio(*s)) {
            return None;
        }

        let anchor_slot = display.iter().position(|t| t.id == anchor.track_id)?;
        let anchor_slot = sources.iter().position(|s| *s == anchor_slot)?;
        let desired = anchor_insert_index.saturating_sub(anchor_slot);
        let first_audio = tracks.overlay.len() + 1;
        let block_start = if audio(sources[0]) {
            desired.clamp(first_audio, first_audio + tracks.audio.len())
        } else {
            desired.min(tracks.overlay.len())
        };

        let new_ids: Vec<String> = sources.iter().map(|_| new_id()).collect();
        let create = sources
            .iter()
            .zip(&new_ids)
            .enumerate()
            .map(|(slot, (source, id))| NewTrack {
                id: id.clone(),
                kind: kind_of(*source),
                index: block_start + slot,
            })
            .collect();

        let earliest = members
            .iter()
            .map(|(_, offset, _)| *offset)
            .min()
            .unwrap_or(MediaTime::ZERO);
        let anchor_start = anchor_start.max(MediaTime::ZERO - earliest.min(MediaTime::ZERO));
        let moves = members
            .iter()
            .map(|(element_ref, offset, source)| {
                let slot = sources.iter().position(|s| s == source)?;
                Some(ElementMove {
                    element: (*element_ref).clone(),
                    target_track_id: new_ids[slot].clone(),
                    start_time: anchor_start + *offset,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { create, moves })
    }
}

impl Command for MoveElementsToNewTracks {
    fn label(&self) -> &'static str {
        "Move"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        for new_track in &self.create {
            let track = Track::new(new_track.id.clone(), new_track.kind);
            insert_track(tracks, track, new_track.index);
        }
        MoveElements {
            moves: self.moves.clone(),
        }
        .apply(project)
    }
}

/// The timings for dragging the `edge` of every element in `elements` by
/// `delta` together. Each element is clamped as
/// [`SetElementTiming::drag_edge`] does alone, and the group takes the
/// tightest delta so all edges move by the same amount. Run the result as
/// one undo step with a [`Batch`](crate::Batch). `None` when an element isn't
/// found or `elements` is empty.
pub fn resize_group(
    tracks: &SceneTracks,
    elements: &[ElementRef],
    edge: Edge,
    delta: MediaTime,
    fps: FrameRate,
) -> Option<Vec<SetElementTiming>> {
    let applied = |timing: &SetElementTiming| {
        let element = tracks.element(&timing.element)?;
        Some(match edge {
            Edge::Start => timing.start_time - element.start_time(),
            Edge::End => timing.duration - element.base().duration,
        })
    };
    let mut group_delta: Option<MediaTime> = None;
    for element in elements {
        let alone = applied(&SetElementTiming::drag_edge(
            tracks, element, edge, delta, fps,
        )?)?;
        group_delta = Some(match group_delta {
            None => alone,
            Some(tightest) if delta >= MediaTime::ZERO => tightest.min(alone),
            Some(tightest) => tightest.max(alone),
        });
    }
    let group_delta = group_delta?;
    elements
        .iter()
        .map(|element| SetElementTiming::drag_edge(tracks, element, edge, group_delta, fps))
        .collect()
}
