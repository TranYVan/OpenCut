//! The keyboard-action set: what each [`EditorAction`] does and that it undoes.

use opencut_editor::commands::AddMediaAsset;
use opencut_editor::commands::{InsertElement, Placement};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{
    Element, ElementBase, ElementRef, MediaAsset, MediaType, ParamValue, Project, TextElement,
    TrackKind, VideoElement,
};
use opencut_editor::{Editor, EditorAction};
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
        media_id: "clip".to_owned(),
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

fn insert(editor: &mut Editor, element: Element) -> ElementRef {
    let mut command = InsertElement::new(element, Placement::Auto);
    editor.execute(&mut command).unwrap();
    ElementRef::new(command.track_id().unwrap(), command.element_id())
}

fn run(editor: &mut Editor, action: EditorAction) {
    let mut command = editor.command_for(action).expect("action applies");
    editor.execute(command.as_mut()).unwrap();
}

fn names(editor: &Editor) -> Vec<String> {
    let mut names: Vec<String> = editor
        .tracks()
        .iter()
        .flat_map(|track| track.elements.iter().map(|e| e.base().name.clone()))
        .collect();
    names.sort();
    names
}

#[test]
fn actions_without_a_target_do_nothing() {
    let mut editor = editor();
    insert(&mut editor, video("a", 0.0, 4.0));
    editor.deselect_all();
    for action in [
        EditorAction::DeleteSelected,
        EditorAction::DuplicateSelected,
        EditorAction::ToggleElementsMutedSelected,
        EditorAction::ToggleSourceAudio,
        EditorAction::PasteCopied,
    ] {
        assert!(editor.command_for(action).is_none(), "{action:?}");
    }
    assert!(!editor.copy_selection());
}

#[test]
fn split_uses_selection_else_elements_under_the_playhead() {
    let mut editor = editor();
    insert(&mut editor, video("a", 0.0, 4.0));
    let title = insert(&mut editor, text("t", 0.0, 4.0));
    editor.seek(secs(1.0));

    // Nothing selected: both elements under the playhead are cut.
    editor.deselect_all();
    run(&mut editor, EditorAction::Split);
    assert_eq!(editor.tracks().element_count(), 4);

    // Playhead outside every element: nothing to do.
    editor.seek(secs(10.0));
    assert!(editor.command_for(EditorAction::Split).is_none());

    // With a selection, only it is cut.
    editor.seek(secs(0.5));
    editor.selection_mut().select(title.clone());
    run(&mut editor, EditorAction::Split);
    assert_eq!(editor.tracks().element_count(), 5);

    assert!(editor.undo());
    assert_eq!(editor.tracks().element_count(), 4);
}

#[test]
fn split_left_and_right_drop_one_side() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));
    editor.seek(secs(1.0));
    run(&mut editor, EditorAction::SplitLeft);
    let only = &editor.tracks().main.elements[0];
    assert_eq!((only.start_time(), only.end_time()), (secs(1.0), secs(4.0)));
    assert!(editor.tracks().element(&clip).is_none());

    editor.seek(secs(3.0));
    editor.deselect_all();
    run(&mut editor, EditorAction::SplitRight);
    let only = &editor.tracks().main.elements[0];
    assert_eq!((only.start_time(), only.end_time()), (secs(1.0), secs(3.0)));
}

#[test]
fn delete_and_duplicate_selected() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));

    run(&mut editor, EditorAction::DuplicateSelected);
    assert_eq!(names(&editor), ["a", "a (copy)"]);

    editor.select_all();
    assert_eq!(editor.selection().elements().len(), 2);
    run(&mut editor, EditorAction::DeleteSelected);
    assert_eq!(editor.tracks().element_count(), 0);
    assert!(editor.selection().is_empty());

    assert!(editor.undo());
    assert_eq!(editor.tracks().element_count(), 2);
    assert!(editor.tracks().element(&clip).is_some());
}

#[test]
fn copy_paste_keeps_spacing_and_selects_the_copies() {
    let mut editor = editor();
    let a = insert(&mut editor, video("a", 0.0, 2.0));
    let b = insert(&mut editor, video("b", 3.0, 2.0));
    let title = insert(&mut editor, text("t", 1.0, 1.0));
    editor.selection_mut().set(vec![b.clone(), title.clone()]);
    assert!(editor.copy_selection());
    assert_eq!(editor.clipboard().items().len(), 2);

    // The playhead can't pass the end of the timeline (5 s).
    editor.seek(secs(5.0));
    run(&mut editor, EditorAction::PasteCopied);

    // Copies start at the playhead with their original spacing (b is 2 s
    // after the title) and have new ids.
    let pasted: Vec<&ElementRef> = editor.selection().elements().iter().collect();
    assert_eq!(pasted.len(), 2);
    let starts: Vec<MediaTime> = pasted
        .iter()
        .map(|r| editor.tracks().element(r).unwrap().start_time())
        .collect();
    assert_eq!(starts.iter().min(), Some(&secs(5.0)));
    assert_eq!(starts.iter().max(), Some(&secs(7.0)));
    assert!(pasted.iter().all(|r| **r != a && **r != b && **r != title));
    assert_eq!(editor.tracks().element_count(), 5);

    // Pasting again at the same spot can't reuse the tracks, so it adds new
    // ones; the clipboard survives, and undo removes just the paste.
    run(&mut editor, EditorAction::PasteCopied);
    assert_eq!(editor.tracks().element_count(), 7);
    assert!(editor.undo());
    assert_eq!(editor.tracks().element_count(), 5);
    assert!(!editor.clipboard().is_empty());
}

#[test]
fn paste_onto_the_main_track_closes_the_leading_gap() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 2.0));
    editor.selection_mut().select(clip);
    assert!(editor.copy_selection());

    // Remove the original, then paste late: the main track starts at zero.
    run(&mut editor, EditorAction::DeleteSelected);
    editor.seek(MediaTime::ZERO);
    run(&mut editor, EditorAction::PasteCopied);
    assert_eq!(
        editor.tracks().main.elements[0].start_time(),
        MediaTime::ZERO
    );
}

#[test]
fn toggle_muted_and_visibility() {
    let mut editor = editor();
    let clip = insert(&mut editor, video("a", 0.0, 2.0));
    let title = insert(&mut editor, text("t", 0.0, 2.0));
    let muted = |editor: &Editor| {
        editor
            .tracks()
            .element(&clip)
            .unwrap()
            .base()
            .params
            .get("muted")
            .cloned()
    };

    // Text can't be muted, so only the video changes.
    editor
        .selection_mut()
        .set(vec![clip.clone(), title.clone()]);
    run(&mut editor, EditorAction::ToggleElementsMutedSelected);
    assert_eq!(muted(&editor), Some(ParamValue::Bool(true)));
    run(&mut editor, EditorAction::ToggleElementsMutedSelected);
    assert_eq!(muted(&editor), Some(ParamValue::Bool(false)));
    assert!(editor.undo());
    assert_eq!(muted(&editor), Some(ParamValue::Bool(true)));

    run(&mut editor, EditorAction::ToggleElementsVisibilitySelected);
    assert_eq!(
        editor.tracks().element(&title).unwrap().is_hidden(),
        Some(true)
    );
}

#[test]
fn toggle_bookmark_adds_and_removes_at_the_playhead() {
    fn bookmarks(editor: &Editor) -> Vec<MediaTime> {
        let scene = editor.project().current_scene().unwrap();
        scene.bookmarks.iter().map(|b| b.time).collect()
    }
    let mut editor = editor();
    // The playhead can't leave an empty timeline, so add content first.
    insert(&mut editor, video("a", 0.0, 4.0));
    editor.seek(secs(1.0));

    run(&mut editor, EditorAction::ToggleBookmark);
    assert_eq!(bookmarks(&editor), [secs(1.0)]);
    run(&mut editor, EditorAction::ToggleBookmark);
    assert!(bookmarks(&editor).is_empty());
    assert!(editor.undo());
    assert_eq!(bookmarks(&editor), [secs(1.0)]);
}

#[test]
fn toggle_source_audio_extracts_then_recovers() {
    let mut editor = editor();
    let asset = MediaAsset {
        id: "clip".to_owned(),
        name: "clip".to_owned(),
        media_type: MediaType::Video,
        size: 0,
        last_modified: 0,
        width: None,
        height: None,
        duration: Some(4.0),
        fps: None,
        has_audio: Some(true),
        ephemeral: None,
        thumbnail_url: None,
    };
    editor.execute(&mut AddMediaAsset { asset }).unwrap();
    let clip = insert(&mut editor, video("a", 0.0, 4.0));
    let enabled = |editor: &Editor| match editor.tracks().element(&clip).unwrap() {
        Element::Video(video) => video.is_source_audio_enabled,
        _ => unreachable!(),
    };

    run(&mut editor, EditorAction::ToggleSourceAudio);
    assert_eq!(enabled(&editor), Some(false));
    let audio = &editor.tracks().audio;
    assert_eq!(audio.len(), 1);
    assert_eq!(audio[0].kind, TrackKind::Audio);
    assert_eq!(audio[0].elements.len(), 1);
    assert_eq!(audio[0].elements[0].media_id(), Some("clip"));

    // Recovering only turns the video's own sound back on.
    run(&mut editor, EditorAction::ToggleSourceAudio);
    assert_eq!(enabled(&editor), Some(true));
    assert_eq!(editor.tracks().audio[0].elements.len(), 1);

    assert!(editor.undo());
    assert_eq!(enabled(&editor), Some(false));
}

#[test]
fn source_audio_needs_media_with_sound() {
    let mut editor = editor();
    // No asset in the project: nothing to extract.
    insert(&mut editor, video("a", 0.0, 4.0));
    let mut command = editor.command_for(EditorAction::ToggleSourceAudio).unwrap();
    assert!(editor.execute(command.as_mut()).is_err());
    assert!(editor.tracks().audio.is_empty());
}
