use opencut_editor::commands::{AddMediaAsset, InsertElement, Placement, RemoveMediaAssets};
use opencut_editor::model::time::{FrameRate, MediaTime};
use opencut_editor::model::{
    AudioElement, AudioSource, CanvasSize, DEFAULT_CANVAS_SIZE, DEFAULT_FPS, Element, ElementBase,
    ElementRef, ImageElement, MediaAsset, MediaType, Project, Scene, TextElement, VideoElement,
};
use opencut_editor::{Batch, Command, EditError, Editor};
use pretty_assertions::assert_eq;

fn secs(seconds: f64) -> MediaTime {
    MediaTime::from_seconds_f64(seconds).unwrap()
}

fn editor() -> Editor {
    Editor::new(Project::new("Test", chrono::Utc::now())).unwrap()
}

fn asset(id: &str, media_type: MediaType) -> MediaAsset {
    let (width, height, duration, fps) = match media_type {
        MediaType::Video => (Some(1280), Some(720), Some(10.0), Some(24.0)),
        MediaType::Image => (Some(800), Some(1200), None, None),
        MediaType::Audio => (None, None, Some(30.0), None),
    };
    MediaAsset {
        id: id.to_owned(),
        name: format!("{id}.file"),
        media_type,
        size: 1000,
        last_modified: 0,
        width,
        height,
        duration,
        fps,
        has_audio: (media_type != MediaType::Image).then_some(true),
        ephemeral: None,
        thumbnail_url: None,
    }
}

fn video(id: &str, media_id: &str, start: f64) -> Element {
    Element::Video(VideoElement {
        base: ElementBase::new(id, id, secs(start), secs(2.0)),
        media_id: media_id.to_owned(),
        is_source_audio_enabled: None,
        hidden: None,
        retime: None,
        effects: None,
        masks: None,
    })
}

fn image(id: &str, media_id: &str, start: f64) -> Element {
    Element::Image(ImageElement {
        base: ElementBase::new(id, id, secs(start), secs(2.0)),
        media_id: media_id.to_owned(),
        hidden: None,
        effects: None,
        masks: None,
    })
}

fn audio(id: &str, media_id: &str) -> Element {
    Element::Audio(AudioElement {
        base: ElementBase::new(id, id, MediaTime::ZERO, secs(2.0)),
        source: AudioSource::Upload {
            media_id: media_id.to_owned(),
        },
        retime: None,
    })
}

fn text(id: &str) -> Element {
    Element::Text(TextElement {
        base: ElementBase::new(id, id, MediaTime::ZERO, secs(1.0)),
        hidden: None,
        effects: None,
    })
}

fn add(editor: &mut Editor, asset: MediaAsset) {
    editor.execute(&mut AddMediaAsset { asset }).unwrap();
}

fn insert(editor: &mut Editor, element: Element) -> ElementRef {
    let mut command = InsertElement::new(element, Placement::Auto);
    editor.execute(&mut command).unwrap();
    ElementRef::new(command.track_id().unwrap(), command.element_id())
}

fn asset_ids(editor: &Editor) -> Vec<&str> {
    editor
        .project()
        .media_assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect()
}

#[test]
fn adding_assets_is_undoable_and_rejects_duplicate_ids() {
    let mut editor = editor();
    add(&mut editor, asset("v", MediaType::Video));
    add(&mut editor, asset("i", MediaType::Image));
    add(&mut editor, asset("a", MediaType::Audio));
    assert_eq!(asset_ids(&editor), ["v", "i", "a"]);
    assert_eq!(editor.undo_label(), Some("Import media"));

    let before = editor.project().clone();
    let error = editor
        .execute(&mut AddMediaAsset {
            asset: asset("v", MediaType::Image),
        })
        .unwrap_err();
    assert_eq!(error, EditError::DuplicateMediaAsset("v".to_owned()));
    assert_eq!(editor.project(), &before);

    assert!(editor.undo());
    assert_eq!(asset_ids(&editor), ["v", "i"]);
    assert!(editor.redo());
    assert_eq!(asset_ids(&editor), ["v", "i", "a"]);
}

#[test]
fn importing_a_faster_video_raises_the_project_frame_rate() {
    let mut editor = editor();
    assert_eq!(editor.project().settings.fps, FrameRate::FPS_30);

    // Slower than the project: unchanged.
    add(&mut editor, asset("slow", MediaType::Video));
    assert_eq!(editor.project().settings.fps, FrameRate::FPS_30);

    let mut fast = asset("fast", MediaType::Video);
    fast.fps = Some(59.94);
    add(&mut editor, fast);
    assert_eq!(editor.project().settings.fps, FrameRate::FPS_59_94);

    editor.undo();
    assert_eq!(editor.project().settings.fps, FrameRate::FPS_30);
}

#[test]
fn removing_assets_removes_their_elements_in_every_scene_as_one_undo_step() {
    let mut editor = editor();
    add(&mut editor, asset("v", MediaType::Video));
    add(&mut editor, asset("a", MediaType::Audio));
    add(&mut editor, asset("i", MediaType::Image));
    let clip = insert(&mut editor, video("clip", "v", 0.0));
    let song = insert(&mut editor, audio("song", "a"));
    let still = insert(&mut editor, image("still", "i", 2.0));
    let title = insert(&mut editor, text("title"));

    // An element of another scene that uses the asset is removed too.
    let mut project = editor.project().clone();
    let mut other = Scene::new("Other", false, chrono::Utc::now());
    other
        .tracks
        .main
        .elements
        .push(video("elsewhere", "v", 0.0));
    project.scenes.push(other);
    let mut editor = Editor::new(project).unwrap();
    editor
        .selection_mut()
        .set(vec![clip.clone(), title.clone()]);
    let before = editor.project().clone();

    editor
        .execute(&mut RemoveMediaAssets {
            asset_ids: vec!["v".to_owned(), "a".to_owned()],
        })
        .unwrap();

    assert_eq!(asset_ids(&editor), ["i"]);
    let tracks = editor.tracks();
    assert!(tracks.element(&clip).is_none());
    assert!(tracks.element(&song).is_none());
    assert!(tracks.element(&still).is_some());
    assert!(tracks.element(&title).is_some());
    assert_eq!(editor.project().scenes[1].tracks.element_count(), 0);
    // Removed elements drop out of the selection.
    assert_eq!(editor.selection().elements(), std::slice::from_ref(&title));
    assert_eq!(editor.undo_label(), Some("Remove media"));

    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(asset_ids(&editor), ["i"]);
    assert!(editor.tracks().element(&clip).is_none());
}

#[test]
fn removing_unknown_or_no_assets_fails_without_changes() {
    let mut editor = editor();
    add(&mut editor, asset("v", MediaType::Video));
    let before = editor.project().clone();

    let error = editor
        .execute(&mut RemoveMediaAssets {
            asset_ids: vec!["v".to_owned(), "missing".to_owned()],
        })
        .unwrap_err();
    assert_eq!(error, EditError::MediaAssetNotFound("missing".to_owned()));
    assert_eq!(
        editor
            .execute(&mut RemoveMediaAssets { asset_ids: vec![] })
            .unwrap_err(),
        EditError::Empty
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn first_video_on_an_empty_timeline_sets_canvas_and_fps() {
    let mut editor = editor();
    let mut clip = asset("v", MediaType::Video);
    clip.fps = Some(23.976);
    add(&mut editor, clip);
    // 23.976 < 30: importing alone leaves the rate.
    assert_eq!(editor.project().settings.fps, DEFAULT_FPS);

    insert(&mut editor, video("clip", "v", 0.0));
    let settings = &editor.project().settings;
    let size = CanvasSize {
        width: 1280,
        height: 720,
    };
    assert_eq!(settings.canvas_size, size);
    assert_eq!(settings.original_canvas_size, Some(size));
    assert_eq!(settings.fps, FrameRate::FPS_23_976);

    // One undo step takes back the insert and the settings.
    editor.undo();
    assert_eq!(editor.project().settings.canvas_size, DEFAULT_CANVAS_SIZE);
    assert_eq!(editor.project().settings.fps, DEFAULT_FPS);
    assert_eq!(editor.project().settings.original_canvas_size, None);
}

#[test]
fn first_image_sets_canvas_but_not_fps() {
    let mut editor = editor();
    add(&mut editor, asset("i", MediaType::Image));
    insert(&mut editor, image("still", "i", 0.0));
    let settings = &editor.project().settings;
    assert_eq!(
        settings.canvas_size,
        CanvasSize {
            width: 800,
            height: 1200
        }
    );
    assert_eq!(settings.fps, DEFAULT_FPS);
}

#[test]
fn later_inserts_and_non_visual_firsts_leave_settings_alone() {
    // Not the first element: settings stay.
    let mut editor = editor();
    add(&mut editor, asset("v", MediaType::Video));
    insert(&mut editor, text("title"));
    insert(&mut editor, video("clip", "v", 0.0));
    assert_eq!(editor.project().settings.canvas_size, DEFAULT_CANVAS_SIZE);
    assert_eq!(editor.project().settings.fps, DEFAULT_FPS);

    // An audio first element doesn't count, nor does a later video.
    let mut editor = self::editor();
    add(&mut editor, asset("a", MediaType::Audio));
    add(&mut editor, asset("v", MediaType::Video));
    insert(&mut editor, audio("song", "a"));
    insert(&mut editor, video("clip", "v", 0.0));
    assert_eq!(editor.project().settings.canvas_size, DEFAULT_CANVAS_SIZE);

    // The original canvas size, once set, is kept.
    let mut editor = self::editor();
    let original = CanvasSize {
        width: 640,
        height: 480,
    };
    let mut project = editor.project().clone();
    project.settings.original_canvas_size = Some(original);
    editor = Editor::new(project).unwrap();
    add(&mut editor, asset("v", MediaType::Video));
    insert(&mut editor, video("clip", "v", 0.0));
    assert_eq!(
        editor.project().settings.original_canvas_size,
        Some(original)
    );
    assert_eq!(
        editor.project().settings.canvas_size,
        CanvasSize {
            width: 1280,
            height: 720
        }
    );
}

#[test]
fn import_and_insert_batch_is_one_undo_step() {
    let mut editor = editor();
    let commands: Vec<Box<dyn Command>> = vec![
        Box::new(AddMediaAsset {
            asset: asset("v", MediaType::Video),
        }),
        Box::new(InsertElement::new(video("clip", "v", 0.0), Placement::Auto)),
    ];
    editor
        .execute(&mut Batch::new("Import media", commands))
        .unwrap();
    assert_eq!(editor.project().settings.canvas_size.width, 1280);
    editor.undo();
    assert!(editor.project().media_assets.is_empty());
    assert_eq!(editor.tracks().element_count(), 0);
    assert!(!editor.can_undo());
}
