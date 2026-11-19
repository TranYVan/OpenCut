//! Snapping, ripple editing and group edits (classic `timeline/snapping`,
//! `ripple`, `timeline/group-move` and `group-resize`).

use opencut_editor::commands::{
    DeleteElements, Edge, InsertElement, MoveElements, Placement, SetElementTiming,
};
use opencut_editor::group_edit::{MoveElementsToNewTracks, resize_group};
use opencut_editor::model::time::{FrameRate, MediaTime};
use opencut_editor::model::{
    AudioElement, AudioSource, Bookmark, Element, ElementBase, ElementRef, Project, TextElement,
    TrackKind, VideoElement,
};
use opencut_editor::snapping::{
    SnapPointKind, resolve_snap, snap_group_shift, snap_points, snap_threshold,
};
use opencut_editor::{Batch, Command, Editor};
use pretty_assertions::assert_eq;

fn secs(seconds: f64) -> MediaTime {
    MediaTime::from_seconds_f64(seconds).unwrap()
}

fn editor() -> Editor {
    Editor::new(Project::new("Test", chrono::Utc::now())).unwrap()
}

fn video(id: &str, start: f64, duration: f64) -> Element {
    Element::Video(VideoElement {
        base: ElementBase::new(id, id, secs(start), secs(duration)),
        media_id: format!("media-{id}"),
        is_source_audio_enabled: None,
        hidden: None,
        retime: None,
        effects: None,
        masks: None,
    })
}

fn text(id: &str, start: f64, duration: f64) -> Element {
    Element::Text(TextElement {
        base: ElementBase::new(id, id, secs(start), secs(duration)),
        hidden: None,
        effects: None,
    })
}

fn audio(id: &str, start: f64, duration: f64) -> Element {
    Element::Audio(AudioElement {
        base: ElementBase::new(id, id, secs(start), secs(duration)),
        source: AudioSource::Upload {
            media_id: format!("media-{id}"),
        },
        retime: None,
    })
}

fn insert(editor: &mut Editor, element: Element) -> ElementRef {
    let mut command = InsertElement::new(element, Placement::Auto);
    editor.execute(&mut command).unwrap();
    ElementRef::new(command.track_id().unwrap(), command.element_id())
}

fn start(editor: &Editor, element: &ElementRef) -> MediaTime {
    editor.tracks().element(element).unwrap().start_time()
}

// --- snapping ---

#[test]
fn snap_points_cover_edges_playhead_and_bookmarks() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    insert(&mut editor, video("b", 2.0, 2.0));
    editor.seek(secs(1.0));
    editor.execute(&mut AddBookmark(secs(3.0))).unwrap();

    let points = editor.snap_points(&["b"]);
    let kinds: Vec<_> = points.iter().map(|p| (p.kind, p.time)).collect();
    assert_eq!(
        kinds,
        [
            (SnapPointKind::ElementStart, secs(0.0)),
            (SnapPointKind::ElementEnd, secs(2.0)),
            (SnapPointKind::Playhead, secs(1.0)),
            (SnapPointKind::Bookmark, secs(3.0)),
        ]
    );
    assert_eq!(points[0].element, Some(a));
}

struct AddBookmark(MediaTime);

impl Command for AddBookmark {
    fn label(&self) -> &'static str {
        "Add bookmark"
    }

    fn apply(
        &mut self,
        project: &mut Project,
    ) -> Result<opencut_editor::Outcome, opencut_editor::EditError> {
        project
            .current_scene_mut()
            .unwrap()
            .bookmarks
            .push(Bookmark {
                time: self.0,
                note: None,
                color: None,
                duration: None,
            });
        Ok(opencut_editor::Outcome::unchanged())
    }
}

#[test]
fn resolve_snap_picks_the_closest_point_within_range() {
    let editor = editor();
    let mut points = snap_points(editor.tracks(), &[], secs(5.0), &[]);
    points.extend(snap_points(editor.tracks(), &[], secs(5.2), &[]));
    let hit = resolve_snap(secs(5.15), &points, secs(0.1));
    assert_eq!(hit.time, secs(5.2));
    assert_eq!(hit.point.unwrap().kind, SnapPointKind::Playhead);

    let miss = resolve_snap(secs(6.0), &points, secs(0.1));
    assert_eq!((miss.time, miss.point), (secs(6.0), None));
}

#[test]
fn threshold_scales_with_zoom() {
    // 10 px at 100 px/s is 0.1 s; zooming in tightens it.
    assert_eq!(snap_threshold(10.0, 100.0), secs(0.1));
    assert_eq!(snap_threshold(10.0, 200.0), secs(0.05));
    assert_eq!(snap_threshold(10.0, 0.0), MediaTime::ZERO);
}

#[test]
fn group_shift_snaps_the_closest_start_or_end() {
    let mut editor = editor();
    insert(&mut editor, video("a", 0.0, 4.0));
    let points = editor.snap_points(&[]);
    // A 2 s group member dropped at 4.05 s starts 0.05 s after a's end.
    let (shift, point) = snap_group_shift(&[(secs(4.05), secs(6.05))], &points, secs(0.1)).unwrap();
    assert_eq!(shift, secs(-0.05));
    assert_eq!(point.kind, SnapPointKind::ElementEnd);
    // Its end (9.97) is within range of nothing; its start (8.0) too.
    assert_eq!(
        snap_group_shift(&[(secs(8.0), secs(9.97))], &points, secs(0.1)),
        None
    );
}

#[test]
fn toggles_flip_the_editor_modes() {
    let mut editor = editor();
    assert!(editor.snapping_enabled);
    assert!(!editor.toggle_snapping());
    assert!(editor.toggle_ripple_editing());
    assert!(editor.ripple_enabled);
}

// --- ripple ---

fn three_clips(editor: &mut Editor) -> [ElementRef; 3] {
    [
        insert(editor, video("a", 0.0, 2.0)),
        insert(editor, video("b", 2.0, 2.0)),
        insert(editor, video("c", 4.0, 2.0)),
    ]
}

#[test]
fn delete_without_ripple_leaves_a_gap_and_with_ripple_closes_it() {
    let mut editor = editor();
    let [_, b, c] = three_clips(&mut editor);
    editor
        .execute(&mut DeleteElements {
            elements: vec![b.clone()],
        })
        .unwrap();
    assert_eq!(start(&editor, &c), secs(4.0));

    let mut editor = self::editor();
    let [_, b, c] = three_clips(&mut editor);
    editor.toggle_ripple_editing();
    editor
        .execute(&mut DeleteElements { elements: vec![b] })
        .unwrap();
    assert_eq!(start(&editor, &c), secs(2.0));

    // One undo step restores the gap-closing shift too.
    assert!(editor.undo());
    assert_eq!(start(&editor, &c), secs(4.0));
}

#[test]
fn ripple_trim_shifts_later_clips_by_the_trimmed_time() {
    let mut editor = editor();
    let [a, _, c] = three_clips(&mut editor);
    editor.toggle_ripple_editing();
    let mut trim = SetElementTiming::drag_edge(
        editor.tracks(),
        &a,
        Edge::End,
        secs(-1.0),
        FrameRate::FPS_30,
    )
    .unwrap();
    editor.execute(&mut trim).unwrap();
    // a is now 1 s; b and c close the freed second.
    assert_eq!(start(&editor, &c), secs(3.0));
}

#[test]
fn ripple_ignores_elements_that_only_changed_tracks() {
    let mut editor = editor();
    let a = insert(&mut editor, text("a", 0.0, 2.0));
    let b = insert(&mut editor, text("b", 2.0, 2.0));
    let c = insert(&mut editor, text("c", 4.0, 2.0));
    // All three are on one text track; park a on a new text track.
    let (track, a_start) = (a.track_id.clone(), start(&editor, &a));
    editor.toggle_ripple_editing();
    let mut command = MoveElementsToNewTracks::drag(editor.tracks(), &[], &a, 0, a_start).unwrap();
    editor.execute(&mut command).unwrap();
    let moved = command.moves[0].element.element_id.clone();
    assert_ne!(command.moves[0].target_track_id, track);
    assert_eq!(moved, a.element_id);
    // a left its track but vacated nothing: b and c stay put.
    assert_eq!(start(&editor, &b), secs(2.0));
    assert_eq!(start(&editor, &c), secs(4.0));
}

// --- group move / resize ---

#[test]
fn group_move_to_new_tracks_keeps_order_and_offsets() {
    let mut editor = editor();
    let v = insert(&mut editor, video("v", 0.0, 2.0));
    let t = insert(&mut editor, text("t", 2.0, 2.0));
    let selection = [v.clone(), t.clone()];
    let mut command =
        MoveElementsToNewTracks::drag(editor.tracks(), &selection, &t, 0, secs(5.0)).unwrap();
    editor.execute(&mut command).unwrap();

    // The text anchor's track is at index 0; the video's goes above it,
    // clamped into the overlay section.
    let tracks = editor.tracks();
    let t_now = &command
        .moves
        .iter()
        .find(|m| m.element == t)
        .unwrap()
        .target_track_id;
    let v_now = &command
        .moves
        .iter()
        .find(|m| m.element == v)
        .unwrap()
        .target_track_id;
    assert_eq!(tracks.overlay.len(), 3); // two new, plus the emptied source
    assert_eq!(tracks.track(t_now).unwrap().kind, TrackKind::Text);
    assert_eq!(tracks.track(v_now).unwrap().kind, TrackKind::Video);
    let anchor_start = tracks.track(t_now).unwrap().elements[0].start_time();
    let other_start = tracks.track(v_now).unwrap().elements[0].start_time();
    assert_eq!((anchor_start, other_start), (secs(5.0), secs(3.0)));
}

#[test]
fn group_move_to_new_tracks_refuses_to_mix_audio_with_other_kinds() {
    let mut editor = editor();
    let v = insert(&mut editor, video("v", 0.0, 2.0));
    let a = insert(&mut editor, audio("a", 0.0, 2.0));
    assert!(
        MoveElementsToNewTracks::drag(editor.tracks(), &[v, a.clone()], &a, 0, secs(1.0)).is_none()
    );
}

#[test]
fn group_move_to_new_tracks_puts_audio_below_main_and_clamps_start() {
    let mut editor = editor();
    let a = insert(&mut editor, audio("a", 2.0, 2.0));
    let b = insert(&mut editor, audio("b", 5.0, 2.0));
    let mut command =
        MoveElementsToNewTracks::drag(editor.tracks(), std::slice::from_ref(&b), &a, 0, secs(-3.0))
            .unwrap();
    // Index 0 would be above main; audio is clamped below it.
    assert_eq!(command.create[0].index, 1);
    editor.execute(&mut command).unwrap();
    let tracks = editor.tracks();
    assert_eq!(tracks.audio[0].kind, TrackKind::Audio);
    let times: Vec<_> = tracks.audio[0]
        .elements
        .iter()
        .map(|e| e.start_time())
        .collect();
    // Never before zero; the 3 s spacing is kept.
    assert_eq!(times, [secs(0.0), secs(3.0)]);
}

#[test]
fn group_resize_takes_the_tightest_delta() {
    let mut editor = editor();
    let long = insert(&mut editor, text("long", 0.0, 4.0));
    let short = insert(&mut editor, video("short", 0.0, 1.0));
    let timings = resize_group(
        editor.tracks(),
        &[long.clone(), short.clone()],
        Edge::End,
        secs(-2.0),
        FrameRate::FPS_30,
    )
    .unwrap();
    // `short` can only lose just under 1 s (one frame stays), so both shrink by that.
    let shrink = timings[1].duration - secs(1.0);
    assert_eq!(timings[0].duration - secs(4.0), shrink);
    assert!(shrink < MediaTime::ZERO && shrink > secs(-1.0));

    let mut batch = Batch::new(
        "Trim",
        timings
            .into_iter()
            .map(|timing| Box::new(timing) as Box<dyn Command>)
            .collect(),
    );
    editor.execute(&mut batch).unwrap();
    assert!(editor.can_undo());
}

#[test]
fn moving_onto_an_existing_track_still_rejects_overlap() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    let b = insert(&mut editor, video("b", 2.0, 2.0));
    let main = a.track_id.clone();
    let mut command = MoveElements::drag(editor.tracks(), &[], &b, &main, secs(1.0)).unwrap();
    assert!(editor.execute(&mut command).is_err());
}
