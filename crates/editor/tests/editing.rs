use opencut_editor::commands::{
    AddTrack, DeleteElements, DuplicateElements, Edge, ElementMove, InsertElement, MoveElements,
    Placement, RemoveTrack, SetElementTiming, SplitElements, SplitRetain, ToggleElementsHidden,
    ToggleTrackMute, UpdateElement, UpdateProjectSettings,
};
use opencut_editor::model::time::{FrameRate, MediaTime};
use opencut_editor::model::{
    AudioElement, AudioSource, Element, ElementBase, ElementRef, ParamValue, ParamValues, Project,
    RetimeConfig, TextElement, TrackKind, VideoElement,
};
use opencut_editor::{Batch, Command, EditError, Editor};
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

/// Inserts with auto placement and returns where it landed.
fn insert(editor: &mut Editor, element: Element) -> ElementRef {
    let mut command = InsertElement::new(element, Placement::Auto);
    editor.execute(&mut command).unwrap();
    ElementRef::new(command.track_id().unwrap(), command.element_id())
}

fn main_id(editor: &Editor) -> String {
    editor.tracks().main.id.clone()
}

fn element<'a>(editor: &'a Editor, element_ref: &ElementRef) -> &'a Element {
    editor.tracks().element(element_ref).unwrap()
}

#[test]
fn auto_placement_follows_classic_track_rules() {
    let mut editor = editor();

    // First video goes on the main track, pulled to zero (no leading gap).
    let first = insert(&mut editor, video("a", 3.0, 5.0));
    assert_eq!(first.track_id, main_id(&editor));
    assert_eq!(element(&editor, &first).start_time(), MediaTime::ZERO);

    // A video that fits after it shares the main track.
    let second = insert(&mut editor, video("b", 5.0, 2.0));
    assert_eq!(second.track_id, main_id(&editor));

    // An overlapping video gets a new overlay video track.
    let third = insert(&mut editor, video("c", 1.0, 2.0));
    assert_eq!(editor.tracks().overlay[0].id, third.track_id);
    assert_eq!(editor.tracks().overlay[0].kind, TrackKind::Video);

    // Text and audio get tracks of their own kind in the right group.
    let title = insert(&mut editor, text("t", 0.0, 1.0));
    let song = insert(&mut editor, audio("s", 0.0, 10.0));
    assert_eq!(
        editor.tracks().track(&title.track_id).unwrap().kind,
        TrackKind::Text
    );
    assert_eq!(editor.tracks().audio[0].id, song.track_id);

    // The newest insert is selected, and the duration follows the content.
    assert_eq!(editor.selection().elements(), &[song]);
    assert_eq!(editor.duration(), secs(10.0));
    assert_eq!(editor.project().metadata.duration, secs(10.0));
}

#[test]
fn rejected_commands_leave_no_trace() {
    let mut editor = editor();
    let before = editor.project().clone();
    let revision = editor.revision();

    let mut onto_main =
        InsertElement::new(audio("s", 0.0, 1.0), Placement::Track(main_id(&editor)));
    let error = editor.execute(&mut onto_main).unwrap_err();

    assert_eq!(
        error,
        EditError::IncompatibleTrack {
            element: opencut_editor::model::ElementKind::Audio,
            track: TrackKind::Video
        }
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.revision(), revision);
    assert!(!editor.can_undo());
}

#[test]
fn batch_is_atomic() {
    let mut editor = editor();
    let mut batch = Batch::new(
        "Two inserts",
        vec![
            Box::new(InsertElement::new(video("a", 0.0, 2.0), Placement::Auto)),
            Box::new(InsertElement::new(
                audio("s", 0.0, 1.0),
                Placement::Track(main_id(&editor)),
            )),
        ],
    );
    assert!(editor.execute(&mut batch).is_err());
    assert_eq!(editor.tracks().element_count(), 0);
    assert!(!editor.can_undo());
}

#[test]
fn split_keeps_source_spans_consistent_under_retime() {
    let mut editor = editor();
    let mut clip = video("a", 0.0, 4.0);
    if let Element::Video(video) = &mut clip {
        video.retime = Some(RetimeConfig {
            rate: 2.0,
            maintain_pitch: None,
        });
        video.base.trim_start = secs(1.0);
        video.base.trim_end = secs(0.5);
    }
    let clip = insert(&mut editor, clip);

    editor
        .execute(&mut SplitElements::new(vec![clip.clone()], secs(1.0)))
        .unwrap();

    let main = &editor.tracks().main;
    assert_eq!(main.elements.len(), 2);
    let (left, right) = (main.elements[0].base(), main.elements[1].base());

    assert_eq!(left.id, clip.element_id);
    assert_eq!(left.name, "a (left)");
    assert_eq!(left.duration, secs(1.0));
    // 3 s of timeline at 2x is 6 s of source cut away on the right.
    assert_eq!(left.trim_end, secs(6.5));

    assert_eq!(right.name, "a (right)");
    assert_eq!(right.start_time, secs(1.0));
    assert_eq!(right.duration, secs(3.0));
    // 1 s of timeline at 2x is 2 s of source consumed on the left.
    assert_eq!(right.trim_start, secs(3.0));

    // The right half is selected.
    assert_eq!(
        editor.selection().elements(),
        &[ElementRef::new(&main.id, &right.id)]
    );
}

#[test]
fn split_left_and_right_keep_one_side() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));

    let mut keep_left = SplitElements::new(vec![clip.clone()], secs(1.0));
    keep_left.retain = SplitRetain::Left;
    editor.execute(&mut keep_left).unwrap();
    let main = &editor.tracks().main;
    assert_eq!(main.elements.len(), 1);
    assert_eq!(main.elements[0].base().duration, secs(1.0));
    assert_eq!(main.elements[0].id(), clip.element_id);

    editor.undo();
    let mut keep_right = SplitElements::new(vec![clip.clone()], secs(1.0));
    keep_right.retain = SplitRetain::Right;
    editor.execute(&mut keep_right).unwrap();
    let main = &editor.tracks().main;
    assert_eq!(main.elements.len(), 1);
    assert_eq!(main.elements[0].start_time(), secs(1.0));
    assert_ne!(main.elements[0].id(), clip.element_id);
}

#[test]
fn split_outside_every_element_fails() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));
    for at in [0.0, 4.0, 9.0] {
        assert_eq!(
            editor.execute(&mut SplitElements::new(vec![clip.clone()], secs(at))),
            Err(EditError::NothingToSplit(secs(at)))
        );
    }
}

#[test]
fn undo_and_redo_restore_project_and_selection() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));
    editor
        .execute(&mut SplitElements::new(vec![clip.clone()], secs(2.0)))
        .unwrap();
    let after_split = editor.project().clone();
    let split_selection = editor.selection().clone();

    assert_eq!(editor.undo_label(), Some("Split"));
    assert!(editor.undo());
    assert_eq!(editor.tracks().main.elements.len(), 1);
    assert_eq!(editor.selection().elements(), std::slice::from_ref(&clip));

    assert!(editor.redo());
    assert_eq!(editor.project(), &after_split);
    assert_eq!(editor.selection(), &split_selection);

    // A new edit clears the redo stack.
    editor.undo();
    editor
        .execute(&mut DeleteElements {
            elements: vec![clip],
        })
        .unwrap();
    assert!(!editor.can_redo());
    assert!(editor.selection().is_empty());
}

#[test]
fn undo_keeps_click_selection_for_commands_that_do_not_select() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));
    let title = insert(&mut editor, text("t", 0.0, 1.0));

    editor.selection_mut().select(clip.clone());
    editor
        .execute(&mut ToggleElementsHidden {
            elements: vec![title.clone()],
        })
        .unwrap();
    editor.undo();
    assert_eq!(editor.selection().elements(), &[clip]);
}

#[test]
fn undo_drops_selection_of_elements_that_no_longer_exist() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));
    assert_eq!(editor.selection().elements(), &[clip]);
    editor.undo();
    assert!(editor.selection().is_empty());
}

#[test]
fn moves_between_tracks_with_validation() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    let b = insert(&mut editor, video("b", 2.0, 2.0));
    let title = insert(&mut editor, text("t", 0.0, 1.0));
    let overlay = AddTrack::new(TrackKind::Video);
    let overlay_id = overlay.track_id().to_owned();
    editor.execute(&mut { overlay }).unwrap();

    // Onto a track of the wrong kind.
    assert!(matches!(
        editor.execute(&mut MoveElements {
            moves: vec![ElementMove {
                element: title,
                target_track_id: main_id(&editor),
                start_time: secs(5.0),
            }],
        }),
        Err(EditError::IncompatibleTrack { .. })
    ));

    // Onto an occupied span.
    assert!(matches!(
        editor.execute(&mut MoveElements {
            moves: vec![ElementMove {
                element: b.clone(),
                target_track_id: main_id(&editor),
                start_time: secs(1.0),
            }],
        }),
        Err(EditError::Overlap { .. })
    ));

    // Swapping two clips works because moves apply together.
    editor
        .execute(&mut MoveElements {
            moves: vec![
                ElementMove {
                    element: a.clone(),
                    target_track_id: main_id(&editor),
                    start_time: secs(2.0),
                },
                ElementMove {
                    element: b.clone(),
                    target_track_id: main_id(&editor),
                    start_time: secs(0.0),
                },
            ],
        })
        .unwrap();
    assert_eq!(element(&editor, &a).start_time(), secs(2.0));
    assert_eq!(element(&editor, &b).start_time(), secs(0.0));

    // To another track; selection follows the element.
    editor
        .execute(&mut MoveElements {
            moves: vec![ElementMove {
                element: a.clone(),
                target_track_id: overlay_id.clone(),
                start_time: secs(7.0),
            }],
        })
        .unwrap();
    let moved = ElementRef::new(&overlay_id, &a.element_id);
    assert_eq!(element(&editor, &moved).start_time(), secs(7.0));
    assert_eq!(editor.selection().elements(), &[moved]);
}

#[test]
fn trimming_validates_timing_and_overlap() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    insert(&mut editor, video("b", 2.0, 2.0));

    let trim = |duration: f64, trim_end: f64| SetElementTiming {
        element: a.clone(),
        start_time: MediaTime::ZERO,
        duration: secs(duration),
        trim_start: MediaTime::ZERO,
        trim_end: secs(trim_end),
    };

    editor.execute(&mut trim(1.5, 0.5)).unwrap();
    assert_eq!(element(&editor, &a).base().duration, secs(1.5));
    assert!(matches!(
        editor.execute(&mut trim(2.5, 0.0)),
        Err(EditError::Overlap { .. })
    ));
    assert_eq!(
        editor.execute(&mut trim(0.0, 0.0)),
        Err(EditError::InvalidTiming)
    );
}

#[test]
fn duplicate_puts_copies_on_a_new_top_track() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 2.0));
    let song = insert(&mut editor, audio("s", 0.0, 2.0));

    editor
        .execute(&mut DuplicateElements {
            elements: vec![clip, song.clone()],
        })
        .unwrap();

    let tracks = editor.tracks();
    assert_eq!(tracks.overlay.len(), 1);
    assert_eq!(tracks.overlay[0].elements[0].base().name, "a (copy)");
    assert_eq!(tracks.audio.len(), 2);
    assert_eq!(tracks.audio[0].elements[0].base().name, "s (copy)");
    assert_eq!(tracks.audio[1].id, song.track_id);
    assert_eq!(editor.selection().elements().len(), 2);
}

#[test]
fn hide_toggles_the_whole_selection_together() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    let t = insert(&mut editor, text("t", 0.0, 2.0));
    let s = insert(&mut editor, audio("s", 0.0, 2.0));
    let all = vec![a.clone(), t.clone(), s.clone()];

    editor
        .execute(&mut ToggleElementsHidden {
            elements: all.clone(),
        })
        .unwrap();
    assert_eq!(element(&editor, &a).is_hidden(), Some(true));
    assert_eq!(element(&editor, &t).is_hidden(), Some(true));
    assert_eq!(element(&editor, &s).is_hidden(), None);

    editor
        .execute(&mut ToggleElementsHidden { elements: all })
        .unwrap();
    assert_eq!(element(&editor, &a).is_hidden(), Some(false));

    assert_eq!(
        editor.execute(&mut ToggleElementsHidden { elements: vec![s] }),
        Err(EditError::Empty)
    );
}

#[test]
fn track_commands() {
    let mut editor = editor();
    let mut add_text = AddTrack::new(TrackKind::Text);
    let mut add_effect = AddTrack::new(TrackKind::Effect);
    let mut add_audio = AddTrack::new(TrackKind::Audio);
    editor.execute(&mut add_text).unwrap();
    editor.execute(&mut add_effect).unwrap();
    editor.execute(&mut add_audio).unwrap();

    // Effect tracks go on top, other overlays above the main track.
    let order: Vec<&str> = editor
        .tracks()
        .iter()
        .map(|track| track.id.as_str())
        .collect();
    let main = main_id(&editor);
    assert_eq!(
        order,
        [
            add_effect.track_id(),
            add_text.track_id(),
            &main,
            add_audio.track_id()
        ]
    );

    assert_eq!(
        editor.execute(&mut ToggleTrackMute {
            track_id: add_text.track_id().to_owned()
        }),
        Err(EditError::CannotMute(TrackKind::Text))
    );
    editor
        .execute(&mut ToggleTrackMute {
            track_id: add_audio.track_id().to_owned(),
        })
        .unwrap();
    assert!(editor.tracks().audio[0].is_muted());

    assert_eq!(
        editor.execute(&mut RemoveTrack { track_id: main }),
        Err(EditError::CannotRemoveMainTrack)
    );
    editor
        .execute(&mut RemoveTrack {
            track_id: add_text.track_id().to_owned(),
        })
        .unwrap();
    assert_eq!(editor.tracks().overlay.len(), 1);
}

#[test]
fn update_element_merges_params() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 2.0));
    let mut params = ParamValues::new();
    params.insert("opacity".to_owned(), ParamValue::Number(0.5));
    editor
        .execute(&mut UpdateElement {
            element: clip.clone(),
            name: Some("Intro".to_owned()),
            params,
        })
        .unwrap();
    let base = element(&editor, &clip).base();
    assert_eq!(base.name, "Intro");
    assert_eq!(base.params.get("opacity"), Some(&ParamValue::Number(0.5)));
}

#[test]
fn settings_reject_frame_rates_without_whole_ticks() {
    let mut editor = editor();
    let mut bad = UpdateProjectSettings {
        fps: Some(FrameRate::new(7, 3)),
        ..Default::default()
    };
    assert!(matches!(
        editor.execute(&mut bad),
        Err(EditError::InvalidSettings(_))
    ));

    let mut good = UpdateProjectSettings {
        fps: Some(FrameRate::FPS_25),
        ..Default::default()
    };
    editor.execute(&mut good).unwrap();
    assert_eq!(editor.project().settings.fps, FrameRate::FPS_25);
}

#[test]
fn no_op_commands_do_not_enter_history() {
    let mut editor = editor();
    let mut nothing = UpdateProjectSettings::default();
    editor.execute(&mut nothing).unwrap();
    assert!(!editor.can_undo());
}

#[test]
fn seeking_snaps_to_frames_and_stays_in_bounds() {
    let mut editor = editor();
    insert(&mut editor, video("a", 0.0, 2.0));

    editor.seek(secs(1.01));
    assert_eq!(editor.playback().position, secs(1.0));
    editor.step_frames(3);
    assert_eq!(editor.playback().position, secs(1.1));
    editor.step_frames(-1_000);
    assert_eq!(editor.playback().position, MediaTime::ZERO);
    editor.go_to_end();
    assert_eq!(editor.playback().position, secs(2.0));
    editor.seek(secs(99.0));
    assert_eq!(editor.playback().position, secs(2.0));

    editor.toggle_playing();
    assert!(editor.playback().playing);
}

#[test]
fn playback_controls_follow_the_clock_and_stop_at_the_end() {
    let mut editor = editor();
    // Nothing to play on an empty scene.
    editor.play();
    assert!(!editor.playback().playing);

    insert(&mut editor, video("a", 0.0, 2.0));
    editor.seek_by(secs(1.0));
    assert_eq!(editor.playback().position, secs(1.0));
    editor.seek_by(secs(-5.0));
    assert_eq!(editor.playback().position, MediaTime::ZERO);

    editor.play();
    assert!(editor.playback().playing);
    // The clock reads mid-frame: the frame being shown wins.
    assert!(editor.advance_to(secs(1.01)));
    assert_eq!(editor.playback().position, secs(1.0));
    // Reaching the end pauses on the last position.
    assert!(!editor.advance_to(secs(2.0)));
    assert_eq!(editor.playback().position, secs(2.0));
    assert!(!editor.playback().playing);

    // Playing from the end restarts; stop rewinds.
    editor.play();
    assert_eq!(editor.playback().position, MediaTime::ZERO);
    editor.seek_by(secs(1.0));
    editor.stop();
    assert!(!editor.playback().playing);
    assert_eq!(editor.playback().position, MediaTime::ZERO);
}

#[test]
fn edits_a_classic_project() {
    let json = include_str!("../../model/tests/fixtures/classic-v31.json");
    let project: Project = serde_json::from_str(json).unwrap();
    let mut editor = Editor::new(project).unwrap();

    // The fixture's first clip runs 0..5 s at 2x speed.
    let clip = ElementRef::new("track-main", "el-clip-1");
    editor
        .execute(&mut SplitElements::new(vec![clip], secs(2.5)))
        .unwrap();
    assert_eq!(editor.tracks().main.elements.len(), 3);

    let saved = serde_json::to_string(editor.project()).unwrap();
    let reloaded: Project = serde_json::from_str(&saved).unwrap();
    assert_eq!(&reloaded, editor.project());
}

#[test]
fn commands_are_object_safe() {
    let commands: Vec<Box<dyn Command>> = vec![Box::new(AddTrack::new(TrackKind::Video))];
    assert_eq!(commands[0].label(), "Add track");
}

#[test]
fn main_track_never_starts_with_a_gap() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    let b = insert(&mut editor, video("b", 2.0, 2.0));
    let main = main_id(&editor);
    let to = |element: &ElementRef, start: f64| ElementMove {
        element: element.clone(),
        target_track_id: main.clone(),
        start_time: secs(start),
    };

    // Moving both right keeps their spacing but pulls them back to zero.
    editor
        .execute(&mut MoveElements {
            moves: vec![to(&a, 5.0), to(&b, 7.0)],
        })
        .unwrap();
    assert_eq!(element(&editor, &a).start_time(), MediaTime::ZERO);
    assert_eq!(element(&editor, &b).start_time(), secs(2.0));

    // Moving the second clip later leaves a gap between clips, which is fine.
    editor
        .execute(&mut MoveElements {
            moves: vec![to(&b, 6.0)],
        })
        .unwrap();
    assert_eq!(element(&editor, &b).start_time(), secs(6.0));
}

#[test]
fn timeline_view_state_is_saved_but_not_undoable() {
    use opencut_editor::model::TimelineViewState;

    let mut editor = editor();
    insert(&mut editor, video("a", 0.0, 4.0));
    let revision = editor.revision();
    let view = TimelineViewState {
        zoom_level: 2.0,
        scroll_left: 120.0,
        playhead_time: MediaTime::ZERO,
    };
    assert!(editor.set_timeline_view_state(view.clone()));
    assert!(!editor.set_timeline_view_state(view.clone()));
    assert_eq!(editor.revision(), revision, "view changes aren't edits");

    // Undo and redo swap the document but keep the view.
    assert!(editor.undo());
    assert_eq!(editor.timeline_view_state(), Some(&view));
    assert!(editor.redo());
    assert_eq!(editor.timeline_view_state(), Some(&view));

    // Saving records the playhead; reopening puts it back.
    editor.seek(secs(1.5));
    let saved = editor.project_to_save();
    assert_eq!(
        saved
            .timeline_view_state
            .as_ref()
            .map(|view| view.playhead_time),
        Some(secs(1.5))
    );
    let reopened = Editor::new(saved.clone()).unwrap();
    assert_eq!(reopened.playback().position, secs(1.5));
    assert_eq!(reopened.project(), &saved);
}

fn with_source(mut element: Element, source: f64) -> Element {
    element.base_mut().source_duration = Some(secs(source));
    element
}

#[test]
fn drag_move_maps_each_source_track_to_one_target_track() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    let top = AddTrack::new(TrackKind::Video);
    let top_id = top.track_id().to_owned();
    editor.execute(&mut { top }).unwrap();
    let middle = AddTrack::new(TrackKind::Video);
    let middle_id = middle.track_id().to_owned();
    editor.execute(&mut { middle }).unwrap();
    // Display order: top, middle, main.
    let x = ElementRef::new(&middle_id, "x");
    editor
        .execute(&mut InsertElement::new(
            video("x", 1.0, 1.0),
            Placement::Track(middle_id.clone()),
        ))
        .unwrap();
    let group = [a.clone(), x.clone()];
    let tracks = editor.tracks().clone();

    // Up one row: a onto middle, x onto the next video track above.
    let mut up = MoveElements::drag(&tracks, &group, &a, &middle_id, secs(3.0)).unwrap();
    assert_eq!(
        up.moves,
        [
            ElementMove {
                element: a.clone(),
                target_track_id: middle_id.clone(),
                start_time: secs(3.0),
            },
            ElementMove {
                element: x.clone(),
                target_track_id: top_id.clone(),
                start_time: secs(4.0),
            },
        ]
    );
    editor.execute(&mut up).unwrap();
    editor.undo();

    // Nothing above the top track for x.
    assert!(MoveElements::drag(&tracks, &group, &a, &top_id, secs(0.0)).is_none());

    // The group never starts before zero; a text track keeps a on main.
    let title = insert(&mut editor, text("t", 0.0, 1.0));
    let tracks = editor.tracks().clone();
    let moved = MoveElements::drag(
        &tracks,
        std::slice::from_ref(&x),
        &a,
        &title.track_id,
        secs(-5.0),
    )
    .unwrap();
    assert_eq!(moved.moves[0].target_track_id, main_id(&editor));
    assert_eq!(moved.moves[0].start_time, MediaTime::ZERO);
    assert_eq!(moved.moves[1].start_time, secs(1.0));
}

#[test]
fn drag_edge_clamps_to_neighbours_media_and_frames() {
    let mut editor = editor();
    let fps = editor.project().settings.fps;
    let a = insert(&mut editor, with_source(video("a", 0.0, 2.0), 2.0));
    let b = insert(&mut editor, with_source(video("b", 3.0, 2.0), 4.0));
    let tracks = editor.tracks().clone();
    let drag = |element: &ElementRef, edge, delta: f64| {
        SetElementTiming::drag_edge(&tracks, element, edge, secs(delta), fps).unwrap()
    };

    // a's end can't reach past its media; b's can, up to its source.
    assert_eq!(drag(&a, Edge::End, 1.0).duration, secs(2.0));
    assert_eq!(drag(&b, Edge::End, 5.0).duration, secs(4.0));
    // Shortening keeps the trims in step and leaves at least a frame.
    let shorter = drag(&a, Edge::End, -1.0);
    assert_eq!((shorter.duration, shorter.trim_end), (secs(1.0), secs(1.0)));
    let tiny = drag(&a, Edge::End, -10.0);
    assert_eq!(tiny.duration, MediaTime::from_frame(1, fps).unwrap());
    // b's start stops at a's end and can't go before its media's start.
    let left = drag(&b, Edge::Start, -5.0);
    assert_eq!((left.start_time, left.duration), (secs(3.0), secs(2.0)));
    let right = drag(&b, Edge::Start, 0.5);
    assert_eq!(
        (right.start_time, right.duration, right.trim_start),
        (secs(3.5), secs(1.5), secs(0.5))
    );
    // Less than half a frame doesn't move.
    assert_eq!(drag(&b, Edge::End, 0.01).duration, secs(2.0));
}

#[test]
fn trimming_the_first_main_clip_start_keeps_it_at_zero() {
    let mut editor = editor();
    let fps = editor.project().settings.fps;
    let a = insert(&mut editor, with_source(video("a", 0.0, 2.0), 2.0));
    let mut trim =
        SetElementTiming::drag_edge(editor.tracks(), &a, Edge::Start, secs(0.5), fps).unwrap();
    editor.execute(&mut trim).unwrap();
    let base = element(&editor, &a).base();
    assert_eq!(
        (base.start_time, base.duration, base.trim_start),
        (MediaTime::ZERO, secs(1.5), secs(0.5))
    );
}
