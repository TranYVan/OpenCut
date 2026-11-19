//! Ported from the classic `timeline/placement/__tests__/resolve.test.ts`.

use opencut_editor::drop_target::{
    DragDirection, InsertSide, NewTrackPosition, PlacementStrategy, TimeSpan, TrackPlacement,
    resolve_track_placement,
};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{
    AudioElement, AudioSource, Element, ElementBase, SceneTracks, TextElement, Track, TrackKind,
    VideoElement,
};
use pretty_assertions::assert_eq;

fn t(ticks: i64) -> MediaTime {
    MediaTime::from_ticks(ticks)
}

fn element(kind: TrackKind, id: &str, start: i64, duration: i64) -> Element {
    let base = ElementBase::new(id, id, t(start), t(duration));
    match kind {
        TrackKind::Audio => Element::Audio(AudioElement {
            base,
            source: AudioSource::Upload {
                media_id: format!("media-{id}"),
            },
            retime: None,
        }),
        TrackKind::Video => Element::Video(VideoElement {
            base,
            media_id: format!("media-{id}"),
            is_source_audio_enabled: None,
            hidden: None,
            retime: None,
            effects: None,
            masks: None,
        }),
        _ => Element::Text(TextElement {
            base,
            hidden: None,
            effects: None,
        }),
    }
}

fn track(id: &str, kind: TrackKind, elements: &[(&str, i64, i64)]) -> Track {
    let mut track = Track::new(id, kind);
    track.elements = elements
        .iter()
        .map(|(element_id, start, duration)| element(kind, element_id, *start, *duration))
        .collect();
    track
}

fn scene(overlay: Vec<Track>, main: Option<Track>, audio: Vec<Track>) -> SceneTracks {
    SceneTracks {
        overlay,
        main: main.unwrap_or_else(|| track("video-main", TrackKind::Video, &[])),
        audio,
    }
}

fn span(start: i64, duration: i64) -> TimeSpan {
    TimeSpan::new(t(start), t(duration))
}

fn existing(id: &str, index: usize, kind: TrackKind) -> TrackPlacement {
    TrackPlacement::Existing {
        track_id: id.into(),
        track_index: index,
        kind,
        adjusted_start: None,
    }
}

fn new_track(kind: TrackKind, index: usize, position: Option<InsertSide>) -> TrackPlacement {
    TrackPlacement::New {
        kind,
        insert_index: index,
        position,
    }
}

fn prefer(index: usize, hover: InsertSide) -> PlacementStrategy {
    PlacementStrategy::PreferIndex {
        track_index: index,
        hover,
        vertical_drag: None,
        create_new_track_only: false,
    }
}

#[test]
fn explicit_returns_the_requested_compatible_track() {
    let tracks = scene(vec![track("text-1", TrackKind::Text, &[])], None, vec![]);
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Text,
            &[span(2, 3)],
            &PlacementStrategy::Explicit("text-1".into())
        ),
        Some(existing("text-1", 0, TrackKind::Text))
    );
}

#[test]
fn explicit_rejects_missing_and_incompatible_tracks() {
    let tracks = scene(
        vec![],
        Some(track("video-1", TrackKind::Video, &[])),
        vec![],
    );
    for id in ["missing", "video-1"] {
        assert_eq!(
            resolve_track_placement(
                &tracks,
                TrackKind::Text,
                &[span(0, 1)],
                &PlacementStrategy::Explicit(id.into())
            ),
            None
        );
    }
}

#[test]
fn first_available_picks_the_first_compatible_track_without_overlap() {
    let tracks = scene(
        vec![
            track("text-1", TrackKind::Text, &[("a", 0, 5)]),
            track("text-2", TrackKind::Text, &[]),
        ],
        None,
        vec![],
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Text,
            &[span(2, 1)],
            &PlacementStrategy::FirstAvailable
        ),
        Some(existing("text-2", 1, TrackKind::Text))
    );
}

#[test]
fn first_available_creates_a_new_track_when_all_compatible_tracks_are_full() {
    let tracks = scene(
        vec![track("graphic-1", TrackKind::Graphic, &[("a", 0, 5)])],
        None,
        vec![track("audio-1", TrackKind::Audio, &[])],
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Graphic,
            &[span(1, 1)],
            &PlacementStrategy::FirstAvailable
        ),
        Some(new_track(TrackKind::Graphic, 0, None))
    );
}

#[test]
fn prefer_index_uses_the_preferred_track_when_it_fits() {
    let tracks = scene(vec![], None, vec![track("audio-1", TrackKind::Audio, &[])]);
    // Hovering the (video) main track: audio gets a new track below it.
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Audio,
            &[span(3, 2)],
            &prefer(0, InsertSide::Below)
        ),
        Some(new_track(TrackKind::Audio, 1, Some(InsertSide::Below)))
    );
    // Hovering the audio track itself, which has room, uses it.
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Audio,
            &[span(3, 2)],
            &prefer(1, InsertSide::Below)
        ),
        Some(existing("audio-1", 1, TrackKind::Audio))
    );
}

#[test]
fn prefer_index_creates_a_new_overlay_track_above_the_main_track() {
    let tracks = scene(vec![], None, vec![track("audio-1", TrackKind::Audio, &[])]);
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Graphic,
            &[span(1, 2)],
            &prefer(1, InsertSide::Below)
        ),
        Some(new_track(TrackKind::Graphic, 0, Some(InsertSide::Above)))
    );
}

#[test]
fn prefer_index_keeps_audio_tracks_below_the_main_track() {
    let tracks = scene(
        vec![track("text-1", TrackKind::Text, &[])],
        None,
        vec![track("audio-1", TrackKind::Audio, &[])],
    );
    let strategy = PlacementStrategy::PreferIndex {
        track_index: 0,
        hover: InsertSide::Above,
        vertical_drag: None,
        create_new_track_only: true,
    };
    assert_eq!(
        resolve_track_placement(&tracks, TrackKind::Audio, &[span(0, 1)], &strategy),
        Some(new_track(TrackKind::Audio, 2, Some(InsertSide::Below)))
    );
}

#[test]
fn above_source_tries_the_track_above_source_then_any_compatible_track() {
    let tracks = scene(
        vec![
            track("text-top", TrackKind::Text, &[]),
            track("text-middle", TrackKind::Text, &[("a", 0, 5)]),
            track("text-source", TrackKind::Text, &[]),
        ],
        None,
        vec![],
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Text,
            &[span(1, 1)],
            &PlacementStrategy::AboveSource {
                source_track_index: 2
            }
        ),
        Some(existing("text-top", 0, TrackKind::Text))
    );
}

#[test]
fn above_source_creates_a_new_overlay_track_in_the_overlay_zone_when_none_fit() {
    let tracks = scene(
        vec![
            track("text-top", TrackKind::Text, &[("a", 0, 5)]),
            track("text-source", TrackKind::Text, &[("b", 0, 5)]),
        ],
        None,
        vec![],
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Text,
            &[span(1, 1)],
            &PlacementStrategy::AboveSource {
                source_track_index: 1
            }
        ),
        Some(new_track(TrackKind::Text, 0, None))
    );
}

#[test]
fn always_new_honors_highest_and_default_insertion_rules() {
    let tracks = scene(vec![], None, vec![track("audio-1", TrackKind::Audio, &[])]);
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Audio,
            &[],
            &PlacementStrategy::AlwaysNew(NewTrackPosition::Highest)
        ),
        Some(new_track(TrackKind::Audio, 1, None))
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Audio,
            &[],
            &PlacementStrategy::AlwaysNew(NewTrackPosition::Default)
        ),
        Some(new_track(TrackKind::Audio, 2, None))
    );
}

#[test]
fn batch_time_spans_reject_tracks_when_any_span_overlaps() {
    // The classic test uses 2.5 and 5.5 "ticks"; whole ticks keep the shape:
    // the first span fits the gap, the second lands on `b`.
    let tracks = scene(
        vec![],
        None,
        vec![track(
            "audio-1",
            TrackKind::Audio,
            &[("a", 0, 20), ("b", 50, 20)],
        )],
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Audio,
            &[span(25, 10), span(55, 10)],
            &PlacementStrategy::FirstAvailable
        ),
        Some(new_track(TrackKind::Audio, 1, None))
    );
}

#[test]
fn handles_main_only_timelines_and_single_tracks() {
    assert_eq!(
        resolve_track_placement(
            &scene(vec![], None, vec![]),
            TrackKind::Video,
            &[span(0, 3)],
            &PlacementStrategy::PreferIndex {
                track_index: 0,
                hover: InsertSide::Below,
                vertical_drag: None,
                create_new_track_only: true,
            }
        ),
        Some(new_track(TrackKind::Video, 0, Some(InsertSide::Above)))
    );
    assert_eq!(
        resolve_track_placement(
            &scene(vec![], None, vec![track("audio-1", TrackKind::Audio, &[])]),
            TrackKind::Audio,
            &[],
            &PlacementStrategy::AlwaysNew(NewTrackPosition::Default)
        ),
        Some(new_track(TrackKind::Audio, 2, None))
    );
}

#[test]
fn existing_main_track_reports_the_adjusted_start_when_it_snaps() {
    let tracks = scene(
        vec![],
        Some(track("video-main", TrackKind::Video, &[("a", 5, 5)])),
        vec![],
    );
    assert_eq!(
        resolve_track_placement(
            &tracks,
            TrackKind::Video,
            &[span(2, 2)],
            &PlacementStrategy::Explicit("video-main".into())
        ),
        Some(TrackPlacement::Existing {
            track_id: "video-main".into(),
            track_index: 0,
            kind: TrackKind::Video,
            adjusted_start: Some(t(0)),
        })
    );
}

#[test]
fn prefer_index_uses_vertical_drag_direction_when_hovered_track_is_incompatible() {
    let tracks = scene(
        vec![track("text-1", TrackKind::Text, &[])],
        None,
        vec![track("audio-1", TrackKind::Audio, &[])],
    );
    let strategy = PlacementStrategy::PreferIndex {
        track_index: 0,
        hover: InsertSide::Above,
        vertical_drag: Some(DragDirection::Down),
        create_new_track_only: false,
    };
    assert_eq!(
        resolve_track_placement(&tracks, TrackKind::Audio, &[span(0, 1)], &strategy),
        Some(new_track(TrackKind::Audio, 2, Some(InsertSide::Below)))
    );
}
