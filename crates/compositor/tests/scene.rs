//! Scene builder: which layers a project shows at a time. No GPU needed.

mod common;

use common::{golden_project, seconds};
use opencut_compositor::{BlendMode, LayerSource, ScenePlan, build_scene};
use opencut_model::{Background, Element, RetimeConfig};
use pretty_assertions::assert_eq;

fn layer_ids(plan: &ScenePlan) -> Vec<&str> {
    plan.layers.iter().map(|layer| layer.id.as_str()).collect()
}

#[test]
fn stacks_main_track_under_overlays_and_skips_hidden() {
    let project = golden_project();
    let at = |time| layer_ids(&build_scene(&project, seconds(time))).join(",");

    assert_eq!(at(0.25), "background,main-video");
    assert_eq!(at(1.0), "background,main-video,overlay-video");
    assert_eq!(at(1.75), "background,main-video,overlay-checker");
    assert_eq!(at(2.5), "background,main-card,overlay-checker");
    // The hidden element (3.5–5 s) and the hidden track (0–5 s) never show.
    assert_eq!(at(3.75), "background,main-card");
    assert_eq!(at(4.5), "background");
}

#[test]
fn element_spans_are_half_open() {
    let project = golden_project();
    let ids = |time| layer_ids(&build_scene(&project, time)).join(",");
    assert_eq!(ids(seconds(2.0)), "background,main-card,overlay-checker");
    assert_eq!(
        ids(seconds(2.0) - opencut_model::time::MediaTime::ONE_TICK),
        "background,main-video,overlay-checker"
    );
}

#[test]
fn reads_transform_opacity_and_blend_mode() {
    let project = golden_project();
    let plan = build_scene(&project, seconds(2.5));
    let checker = &plan.layers[2];
    assert_eq!(checker.blend_mode, BlendMode::Multiply);
    assert_eq!(checker.opacity, 0.75);
    assert_eq!(checker.transform.rotate, 30.0);
    assert_eq!(
        (checker.transform.position_x, checker.transform.position_y),
        (30.0, -10.0)
    );
    let card = &plan.layers[1];
    assert_eq!(card.blend_mode, BlendMode::Normal);
    assert_eq!(card.transform.scale_x, -1.0);
    assert_eq!(
        plan.layers[0].source,
        LayerSource::Color([32.0 / 255.0, 48.0 / 255.0, 64.0 / 255.0, 1.0])
    );
}

#[test]
fn video_source_time_applies_trim_and_rate() {
    let mut project = golden_project();
    let source_time =
        |project: &opencut_model::Project, time| match &build_scene(project, seconds(time)).layers
            [1]
        .source
        {
            LayerSource::Video { source_time, .. } => *source_time,
            other => panic!("not a video: {other:?}"),
        };
    // Trimmed 0.5 s in, so 0.25 s into the clip is 0.75 s into the source.
    assert_eq!(source_time(&project, 0.25), seconds(0.75));

    let scene = project.current_scene_mut().unwrap();
    let Element::Video(video) = &mut scene.tracks.main.elements[0] else {
        panic!("main track starts with a video");
    };
    video.retime = Some(RetimeConfig {
        rate: 2.0,
        maintain_pitch: None,
    });
    assert_eq!(source_time(&project, 0.25), seconds(1.0));
}

#[test]
fn transparent_and_unsupported_backgrounds_draw_nothing() {
    let mut project = golden_project();
    for background in [
        Background::Color {
            color: "transparent".to_owned(),
        },
        Background::Color {
            color: "linear-gradient(red, blue)".to_owned(),
        },
        Background::Blur {
            blur_intensity: 8.0,
        },
    ] {
        project.settings.background = background;
        assert_eq!(
            layer_ids(&build_scene(&project, seconds(4.5))),
            Vec::<&str>::new()
        );
    }
}

#[test]
fn missing_or_mismatched_media_is_left_out() {
    let mut project = golden_project();
    project.media_assets.retain(|asset| asset.id != "vid");
    assert_eq!(
        layer_ids(&build_scene(&project, seconds(1.0))),
        vec!["background"]
    );

    let mut project = golden_project();
    let checker = project
        .media_assets
        .iter_mut()
        .find(|asset| asset.id == "checker")
        .unwrap();
    checker.media_type = opencut_model::MediaType::Video;
    assert_eq!(
        layer_ids(&build_scene(&project, seconds(1.75))),
        vec!["background", "main-video"]
    );
}
