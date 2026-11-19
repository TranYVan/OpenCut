use opencut_editor::commands::{
    AddMediaAsset, InsertElement, Placement, SetElementParam, SetElementRetime,
    ToggleSourceAudioSeparation,
};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{
    Element, ElementRef, MediaAsset, MediaType, ParamValue, Project, RetimeConfig,
};
use opencut_editor::{EditError, Editor};
use pretty_assertions::assert_eq;

fn secs(seconds: f64) -> MediaTime {
    MediaTime::from_seconds_f64(seconds).unwrap()
}

/// An editor with one 10 s video (with audio) on the main track.
fn editor_with_video(has_audio: bool) -> (Editor, ElementRef) {
    let mut editor = Editor::new(Project::new("Test", chrono::Utc::now())).unwrap();
    let asset = MediaAsset {
        id: "m".to_owned(),
        name: "clip".to_owned(),
        media_type: MediaType::Video,
        size: 1,
        last_modified: 0,
        width: Some(1280),
        height: Some(720),
        duration: Some(10.0),
        fps: Some(24.0),
        has_audio: Some(has_audio),
        ephemeral: None,
        thumbnail_url: None,
    };
    editor
        .execute(&mut AddMediaAsset {
            asset: asset.clone(),
        })
        .unwrap();
    let mut insert = InsertElement::from_media(&asset, MediaTime::ZERO, Placement::Auto);
    editor.execute(&mut insert).unwrap();
    let clip = ElementRef::new(insert.track_id().unwrap(), insert.element_id());
    (editor, clip)
}

fn param(editor: &Editor, clip: &ElementRef, key: &str) -> Option<ParamValue> {
    editor
        .tracks()
        .element(clip)
        .unwrap()
        .base()
        .params
        .get(key)
        .cloned()
}

#[test]
fn param_edits_are_validated_and_undoable() {
    let (mut editor, clip) = editor_with_video(true);
    let before = editor.undo_label();

    editor
        .execute(&mut SetElementParam::new(clip.clone(), "opacity", 4.0))
        .unwrap();
    // Clamped to the 0-1 range.
    assert_eq!(param(&editor, &clip, "opacity"), Some(1.0.into()));

    editor
        .execute(&mut SetElementParam::new(
            clip.clone(),
            "blendMode",
            "multiply",
        ))
        .unwrap();
    assert_eq!(param(&editor, &clip, "blendMode"), Some("multiply".into()));

    for (key, value) in [
        ("opacity", ParamValue::from("x")),
        ("blendMode", "nope".into()),
    ] {
        assert!(matches!(
            editor.execute(&mut SetElementParam::new(clip.clone(), key, value)),
            Err(EditError::InvalidParamValue { .. })
        ));
    }
    assert!(matches!(
        editor.execute(&mut SetElementParam::new(clip.clone(), "nonsense", 1.0)),
        Err(EditError::UnknownParam { .. })
    ));

    assert!(editor.undo());
    assert_eq!(param(&editor, &clip, "blendMode"), None);
    assert!(editor.undo());
    assert_eq!(param(&editor, &clip, "opacity"), None);
    assert_eq!(editor.undo_label(), before);
}

#[test]
fn a_drag_is_one_undo_step() {
    let (mut editor, clip) = editor_with_video(true);
    for tenths in 1..=9 {
        let mut command = SetElementParam::new(clip.clone(), "opacity", f64::from(tenths) / 10.0);
        let key = command.coalesce_key();
        editor.execute_coalescing(&mut command, &key).unwrap();
    }
    assert_eq!(param(&editor, &clip, "opacity"), Some(0.9.into()));

    // The gesture ends; the next drag is a new step.
    editor.end_coalescing();
    let mut command = SetElementParam::new(clip.clone(), "opacity", 0.1);
    let key = command.coalesce_key();
    editor.execute_coalescing(&mut command, &key).unwrap();

    assert!(editor.undo());
    assert_eq!(param(&editor, &clip, "opacity"), Some(0.9.into()));
    assert!(editor.undo());
    assert_eq!(param(&editor, &clip, "opacity"), None);

    let mut remaining = 0;
    while editor.undo() {
        remaining += 1;
    }
    // Adding the media and inserting the clip.
    assert_eq!(remaining, 2);

    // Redo replays the folded step in one go.
    assert!(editor.redo());
}

#[test]
fn other_edits_break_coalescing() {
    let (mut editor, clip) = editor_with_video(true);
    for (key, value) in [
        ("opacity", 0.5),
        ("transform.rotate", 10.0),
        ("opacity", 0.6),
    ] {
        let mut command = SetElementParam::new(clip.clone(), key, value);
        let merge = command.coalesce_key();
        editor.execute_coalescing(&mut command, &merge).unwrap();
    }
    assert!(editor.undo());
    assert_eq!(param(&editor, &clip, "opacity"), Some(0.5.into()));
    assert!(editor.undo());
    assert_eq!(param(&editor, &clip, "transform.rotate"), None);
}

#[test]
fn speed_retimes_and_resizes_the_clip() {
    let (mut editor, clip) = editor_with_video(true);
    let duration = |editor: &Editor| editor.tracks().element(&clip).unwrap().base().duration;
    assert_eq!(duration(&editor), secs(10.0));

    editor
        .execute(&mut SetElementRetime {
            element: clip.clone(),
            rate: 2.0,
            maintain_pitch: true,
        })
        .unwrap();
    let element = editor.tracks().element(&clip).unwrap();
    assert_eq!(
        element.retime(),
        Some(&RetimeConfig {
            rate: 2.0,
            maintain_pitch: Some(true)
        })
    );
    assert_eq!(duration(&editor), secs(5.0));

    // Out-of-range rates clamp; going slower again doesn't drift.
    editor
        .execute(&mut SetElementRetime {
            element: clip.clone(),
            rate: 0.5,
            maintain_pitch: false,
        })
        .unwrap();
    assert_eq!(duration(&editor), secs(20.0));
    editor
        .execute(&mut SetElementRetime {
            element: clip.clone(),
            rate: 99.0,
            maintain_pitch: false,
        })
        .unwrap();
    assert_eq!(duration(&editor), secs(2.0));

    // Back to 1x without pitch lock drops the retime entirely.
    editor
        .execute(&mut SetElementRetime {
            element: clip.clone(),
            rate: 1.0,
            maintain_pitch: false,
        })
        .unwrap();
    assert_eq!(editor.tracks().element(&clip).unwrap().retime(), None);
    assert_eq!(duration(&editor), secs(10.0));

    assert!(editor.undo());
    assert_eq!(duration(&editor), secs(2.0));
}

#[test]
fn slowing_down_cannot_overlap_a_neighbour() {
    let (mut editor, clip) = editor_with_video(true);
    editor
        .execute(&mut SetElementRetime {
            element: clip.clone(),
            rate: 2.0,
            maintain_pitch: false,
        })
        .unwrap();
    // A second clip right after the first.
    let asset = editor.project().media_assets[0].clone();
    let mut insert = InsertElement::from_media(&asset, secs(5.0), Placement::Auto);
    editor.execute(&mut insert).unwrap();
    assert_eq!(insert.track_id(), Some(clip.track_id.as_str()));

    let before = editor.project().clone();
    assert!(matches!(
        editor.execute(&mut SetElementRetime {
            element: clip.clone(),
            rate: 1.0,
            maintain_pitch: false,
        }),
        Err(EditError::Overlap { .. })
    ));
    assert_eq!(editor.project(), &before);
}

#[test]
fn source_audio_separates_and_recovers() {
    let (mut editor, clip) = editor_with_video(true);
    editor
        .execute(&mut SetElementParam::new(clip.clone(), "volume", -6.0))
        .unwrap();
    editor
        .execute(&mut ToggleSourceAudioSeparation {
            element: clip.clone(),
        })
        .unwrap();

    let Some(Element::Video(video)) = editor.tracks().element(&clip) else {
        panic!("video moved");
    };
    assert_eq!(video.is_source_audio_enabled, Some(false));
    let extracted = &editor.tracks().audio[0].elements[0];
    assert_eq!(extracted.base().duration, secs(10.0));
    assert_eq!(extracted.media_id(), Some("m"));
    assert_eq!(extracted.base().params.get("volume"), Some(&(-6.0).into()));

    // Recovering turns the video's audio back on; one undo step each.
    editor
        .execute(&mut ToggleSourceAudioSeparation {
            element: clip.clone(),
        })
        .unwrap();
    let Some(Element::Video(video)) = editor.tracks().element(&clip) else {
        panic!("video moved");
    };
    assert_eq!(video.is_source_audio_enabled, Some(true));
    assert!(editor.undo());
    assert_eq!(editor.tracks().audio[0].elements.len(), 1);
    assert!(editor.undo());
    assert!(editor.tracks().audio.is_empty());
}

#[test]
fn media_without_audio_cannot_separate() {
    let (mut editor, clip) = editor_with_video(false);
    assert_eq!(
        editor.execute(&mut ToggleSourceAudioSeparation { element: clip }),
        Err(EditError::NoSourceAudio)
    );
}
