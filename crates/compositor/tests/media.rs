//! `MediaSources` over `opencut-media`, with its test fixtures. `video.mp4`
//! is 60 frames at 30 fps; frame `n` has a white bar in pixel column `n`.

#![cfg(feature = "media")]

mod common;

use std::path::PathBuf;

use common::{gpu, seconds};
use opencut_compositor::{FrameSource, MediaSources, Renderer, RgbaImage};
use opencut_model::{MediaAsset, MediaType};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name)
}

fn asset(id: &str, media_type: MediaType) -> MediaAsset {
    let mut project = common::golden_project();
    let mut asset = project.media_assets.remove(0);
    asset.id = id.to_owned();
    asset.media_type = media_type;
    asset
}

fn sources() -> MediaSources<impl FnMut(&MediaAsset) -> Option<PathBuf>> {
    MediaSources::new(|asset: &MediaAsset| match asset.id.as_str() {
        "clip" => Some(fixture("video.mp4")),
        "still" => Some(fixture("image.png")),
        "broken" => Some(fixture("does-not-exist.mp4")),
        _ => None,
    })
}

/// The column of the white bar: the brightest in the middle row.
fn bar_column(image: &RgbaImage) -> u32 {
    let y = image.height / 2;
    (0..image.width)
        .max_by_key(|&x| {
            image.pixel(x, y)[..3]
                .iter()
                .map(|&c| u32::from(c))
                .sum::<u32>()
        })
        .unwrap()
}

#[test]
fn decodes_video_frames_with_per_frame_keys() {
    let mut sources = sources();
    let clip = asset("clip", MediaType::Video);
    let first = sources.video_frame(&clip, seconds(10.0 / 30.0)).unwrap();
    let again = sources.video_frame(&clip, seconds(10.4 / 30.0)).unwrap();
    let later = sources.video_frame(&clip, seconds(25.0 / 30.0)).unwrap();
    assert_eq!(first.key, "clip#10");
    assert_eq!(again.key, first.key);
    assert_eq!(later.key, "clip#25");
    assert_eq!(bar_column(&first.image), 10);
    assert_eq!(bar_column(&later.image), 25);
}

#[test]
fn decodes_images_once_and_reports_failures() {
    let mut sources = sources();
    let still = sources.image(&asset("still", MediaType::Image)).unwrap();
    assert_eq!((still.image.width, still.image.height), (40, 30));

    assert!(sources.image(&asset("unknown", MediaType::Image)).is_none());
    assert!(
        sources.last_error().is_none(),
        "unresolved paths are not errors"
    );
    assert!(
        sources
            .video_frame(&asset("broken", MediaType::Video), seconds(0.0))
            .is_none()
    );
    assert!(sources.last_error().is_some());
}

#[test]
fn renders_decoded_video_into_the_frame() {
    let Some(context) = gpu() else { return };
    let mut project = common::golden_project();
    project.media_assets[0] = asset("clip", MediaType::Video);
    for track in &mut project.current_scene_mut().unwrap().tracks.overlay {
        track.hidden = Some(true);
    }
    let opencut_model::Element::Video(video) =
        &mut project.current_scene_mut().unwrap().tracks.main.elements[0]
    else {
        panic!("main track starts with a video");
    };
    video.media_id = "clip".to_owned();

    let mut renderer = Renderer::new(&context);
    let mut sources = sources();
    // Trimmed 0.5 s in: at 0.25 s the source is at 0.75 s, frame 22.
    let frame = renderer
        .render_project(&context, &project, seconds(0.25), &mut sources)
        .unwrap();
    let source = sources
        .video_frame(&project.media_assets[0], seconds(0.75))
        .unwrap();
    // Contained in the canvas, centred.
    let (canvas_width, canvas_height) = (f64::from(frame.width), f64::from(frame.height));
    let (width, height) = (
        f64::from(source.image.width),
        f64::from(source.image.height),
    );
    let scale = (canvas_width / width).min(canvas_height / height);
    let left = (canvas_width - width * scale) / 2.0;
    let expected = left + (f64::from(bar_column(&source.image)) + 0.5) * scale;
    let actual = f64::from(bar_column(&frame));
    assert!(
        (actual - expected).abs() <= scale + 1.0,
        "bar at column {actual}, expected near {expected}"
    );
}
