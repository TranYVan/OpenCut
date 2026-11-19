use opencut_editor::animation::EASING_PRESETS;
use opencut_editor::commands::{
    AddMediaAsset, InsertElement, PasteKeyframes, Placement, RemoveKeyframe, RetimeKeyframe,
    SetKeyframeEasing, SplitElements, UpsertKeyframe, copy_keyframes,
};
use opencut_editor::model::animation::{Channel, ChannelData, ScalarChannel, SegmentType};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{Element, ElementRef, MediaAsset, MediaType, ParamValue, Project};
use opencut_editor::{EditError, Editor};
use pretty_assertions::assert_eq;

fn secs(seconds: f64) -> MediaTime {
    MediaTime::from_seconds_f64(seconds).unwrap()
}

/// An editor with one 10 s video on the main track.
fn editor_with_video() -> (Editor, ElementRef) {
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
        has_audio: Some(false),
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

fn element<'a>(editor: &'a Editor, clip: &ElementRef) -> &'a Element {
    editor.tracks().element(clip).unwrap()
}

fn opacity_at(element: &Element, local: MediaTime) -> f64 {
    element.base().params_at(local)["opacity"].as_f64().unwrap()
}

fn opacity_channel(element: &Element) -> &ScalarChannel {
    match &element.base().animations.as_ref().unwrap()["opacity"] {
        ChannelData::Leaf(Channel::Scalar(channel)) => channel,
        other => panic!("{other:?}"),
    }
}

/// Fades opacity 0 → 1 over the first 4 s; returns the two key ids.
fn fade_in(editor: &mut Editor, clip: &ElementRef) -> (String, String) {
    let mut ids = Vec::new();
    for (time, value) in [(0.0, 0.0), (4.0, 1.0)] {
        let mut upsert = UpsertKeyframe::new(clip.clone(), "opacity", secs(time), value);
        editor.execute(&mut upsert).unwrap();
        ids.push(upsert.keyframe_id.unwrap());
    }
    (ids.remove(0), ids.remove(0))
}

#[test]
fn keyframes_animate_params_and_are_undoable() {
    let (mut editor, clip) = editor_with_video();
    fade_in(&mut editor, &clip);
    let video = element(&editor, &clip);
    assert_eq!(opacity_at(video, secs(2.0)), 0.5);
    assert_eq!(opacity_at(video, secs(8.0)), 1.0);

    // Values are coerced like params; times clamp to the element.
    let mut upsert = UpsertKeyframe::new(clip.clone(), "opacity", secs(60.0), 7.0);
    editor.execute(&mut upsert).unwrap();
    let keys = &opacity_channel(element(&editor, &clip)).keys;
    assert_eq!((keys[2].time, keys[2].value), (secs(10.0), 1.0));
    assert!(matches!(
        editor.execute(&mut UpsertKeyframe::new(
            clip.clone(),
            "nope",
            secs(1.0),
            1.0
        )),
        Err(EditError::UnknownParam { .. })
    ));

    editor.undo();
    editor.undo();
    editor.undo();
    assert_eq!(element(&editor, &clip).base().animations, None);
}

#[test]
fn removing_the_last_key_keeps_the_value_on_screen() {
    let (mut editor, clip) = editor_with_video();
    let (first, second) = fade_in(&mut editor, &clip);
    for id in [first, second] {
        editor
            .execute(&mut RemoveKeyframe {
                element: clip.clone(),
                path: "opacity".to_owned(),
                keyframe_id: id,
                value_at_playhead: Some(0.25.into()),
            })
            .unwrap();
    }
    let base = element(&editor, &clip).base();
    assert_eq!(base.animations, None);
    assert_eq!(base.params["opacity"], ParamValue::Number(0.25));
}

#[test]
fn retimed_keys_move_the_curve() {
    let (mut editor, clip) = editor_with_video();
    let (_, second) = fade_in(&mut editor, &clip);
    editor
        .execute(&mut RetimeKeyframe {
            element: clip.clone(),
            path: "opacity".to_owned(),
            keyframe_id: second,
            time: secs(2.0),
        })
        .unwrap();
    assert_eq!(opacity_at(element(&editor, &clip), secs(1.0)), 0.5);
}

#[test]
fn easing_presets_reshape_a_segment() {
    let (mut editor, clip) = editor_with_video();
    let (first, _) = fade_in(&mut editor, &clip);
    let (_, ease_in) = EASING_PRESETS[2];
    editor
        .execute(&mut SetKeyframeEasing {
            element: clip.clone(),
            path: "opacity".to_owned(),
            component: "value".to_owned(),
            keyframe_id: first,
            easing: ease_in,
        })
        .unwrap();
    let video = element(&editor, &clip);
    assert_eq!(
        opacity_channel(video).keys[0].segment_to_next,
        SegmentType::Bezier
    );
    assert!(opacity_at(video, secs(2.0)) < 0.5);
}

#[test]
fn copied_keys_paste_at_the_playhead() {
    let (mut editor, clip) = editor_with_video();
    let (first, second) = fade_in(&mut editor, &clip);
    let items = copy_keyframes(
        element(&editor, &clip),
        &[
            ("opacity".to_owned(), first),
            ("opacity".to_owned(), second),
        ],
    );
    editor
        .execute(&mut PasteKeyframes {
            element: clip.clone(),
            time: secs(5.0),
            items,
        })
        .unwrap();
    let video = element(&editor, &clip);
    assert_eq!(opacity_channel(video).keys.len(), 4);
    assert_eq!(opacity_at(video, secs(7.0)), 0.5);
}

#[test]
fn split_keeps_curve_values_at_the_cut() {
    let (mut editor, clip) = editor_with_video();
    fade_in(&mut editor, &clip);
    let before = element(&editor, &clip).clone();
    editor
        .execute(&mut SplitElements::new(vec![clip.clone()], secs(1.0)))
        .unwrap();
    let elements = &editor.tracks().main.elements;
    let (left, right) = (&elements[0], &elements[1]);
    assert_eq!(opacity_at(left, secs(1.0)), opacity_at(&before, secs(1.0)));
    assert_eq!(
        opacity_at(right, MediaTime::ZERO),
        opacity_at(&before, secs(1.0))
    );
    assert_eq!(opacity_at(right, secs(1.0)), opacity_at(&before, secs(2.0)));
}
