#![allow(dead_code, reason = "each test binary uses a different part")]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use opencut_compositor::gpu::GpuContext;
use opencut_compositor::{FrameSource, RgbaImage, SourceFrame};
use opencut_model::time::{MediaTime, TICKS_PER_SECOND};
use opencut_model::{
    Background, CanvasSize, Element, ElementBase, ImageElement, MediaAsset, MediaType, ParamValue,
    Project, Track, TrackKind, VideoElement,
};

pub const CANVAS: CanvasSize = CanvasSize {
    width: 160,
    height: 90,
};
pub const VIDEO_FPS: i64 = 30;

pub fn seconds(value: f64) -> MediaTime {
    MediaTime::from_seconds_f64(value).unwrap()
}

/// A context on any adapter, or `None` (after saying why) when the machine
/// has none. `OPENCUT_REQUIRE_GPU=1` turns a missing adapter into a failure.
pub fn gpu() -> Option<GpuContext> {
    match GpuContext::new_blocking() {
        Ok(context) => {
            let info = context.adapter().get_info();
            eprintln!(
                "rendering on {} ({:?}, {:?})",
                info.name, info.backend, info.device_type
            );
            Some(context)
        }
        Err(error) if std::env::var_os("OPENCUT_REQUIRE_GPU").is_some() => {
            panic!("OPENCUT_REQUIRE_GPU is set but no adapter is available: {error}")
        }
        Err(error) => {
            eprintln!(
                "SKIPPED: {error}. Install a software rasterizer (mesa-vulkan-drivers on \
                 Linux) or set OPENCUT_REQUIRE_GPU=1 to make this a failure."
            );
            None
        }
    }
}

fn asset(id: &str, media_type: MediaType, width: u32, height: u32) -> MediaAsset {
    MediaAsset {
        id: id.to_owned(),
        name: id.to_owned(),
        media_type,
        size: 0,
        last_modified: 0,
        width: Some(width),
        height: Some(height),
        duration: (media_type == MediaType::Video).then_some(10.0),
        fps: (media_type == MediaType::Video).then_some(30.0),
        has_audio: None,
        ephemeral: None,
        thumbnail_url: None,
    }
}

fn base(id: &str, start: f64, duration: f64, params: &[(&str, ParamValue)]) -> ElementBase {
    let mut base = ElementBase::new(id, id, seconds(start), seconds(duration));
    for (key, value) in params {
        base.params.insert((*key).to_owned(), value.clone());
    }
    base
}

fn video(base: ElementBase, media_id: &str) -> Element {
    Element::Video(VideoElement {
        base,
        media_id: media_id.to_owned(),
        is_source_audio_enabled: None,
        hidden: None,
        retime: None,
        effects: None,
        masks: None,
    })
}

fn image(base: ElementBase, media_id: &str, hidden: bool) -> Element {
    Element::Image(ImageElement {
        base,
        media_id: media_id.to_owned(),
        hidden: hidden.then_some(true),
        effects: None,
        masks: None,
    })
}

fn num(value: f64) -> ParamValue {
    ParamValue::Number(value)
}

/// The golden project: 160×90, two visible tracks over a colour background,
/// plus a hidden track and a hidden element that must not show.
///
/// - main: video `vid` (0–2 s, trimmed 0.5 s in), then image `card`
///   (2–4 s, flipped horizontally, 80 % opacity).
/// - overlay (muted, which must not matter): video `vid` (0.5–1.5 s,
///   half size, bottom left, screen), then image `checker` (1.5–3.5 s,
///   rotated 30°, 75 % opacity, multiply), then a hidden full-size
///   `checker` (3.5–5 s).
/// - a hidden overlay covering everything (0–5 s).
pub fn golden_project() -> Project {
    let mut project = Project::new("golden", Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
    project.settings.canvas_size = CANVAS;
    project.settings.background = Background::Color {
        color: "#203040".to_owned(),
    };
    project.media_assets = vec![
        asset("vid", MediaType::Video, 64, 36),
        asset("checker", MediaType::Image, 32, 32),
        asset("card", MediaType::Image, 48, 24),
    ];

    let scene = project.current_scene_mut().unwrap();

    let main = &mut scene.tracks.main;
    let mut main_video = base("main-video", 0.0, 2.0, &[]);
    main_video.trim_start = seconds(0.5);
    main.elements.push(video(main_video, "vid"));
    main.elements.push(image(
        base(
            "main-card",
            2.0,
            2.0,
            &[("transform.scaleX", num(-1.0)), ("opacity", num(0.8))],
        ),
        "card",
        false,
    ));

    let mut overlay = Track::new("overlay", TrackKind::Video);
    overlay.muted = Some(true);
    overlay.elements.push(video(
        base(
            "overlay-video",
            0.5,
            1.0,
            &[
                ("transform.scaleX", num(0.5)),
                ("transform.scaleY", num(0.5)),
                ("transform.positionX", num(-35.0)),
                ("transform.positionY", num(20.25)),
                ("blendMode", ParamValue::from("screen")),
            ],
        ),
        "vid",
    ));
    overlay.elements.push(image(
        base(
            "overlay-checker",
            1.5,
            2.0,
            &[
                ("transform.scaleX", num(0.6)),
                ("transform.scaleY", num(0.6)),
                ("transform.positionX", num(30.0)),
                ("transform.positionY", num(-10.0)),
                ("transform.rotate", num(30.0)),
                ("opacity", num(0.75)),
                ("blendMode", ParamValue::from("multiply")),
            ],
        ),
        "checker",
        false,
    ));
    overlay.elements.push(image(
        base("hidden-element", 3.5, 1.5, &[]),
        "checker",
        true,
    ));

    let mut hidden = Track::new("hidden-track", TrackKind::Video);
    hidden.hidden = Some(true);
    hidden.elements.push(image(
        base("hidden-track-card", 0.0, 5.0, &[]),
        "card",
        false,
    ));

    scene.tracks.overlay = vec![overlay, hidden];
    project
}

/// Synthetic pictures for the golden project's assets, so the tests need no
/// decoder.
#[derive(Default)]
pub struct SyntheticSources {
    /// Every video request, as (asset id, source time).
    pub video_requests: Vec<(String, MediaTime)>,
}

impl FrameSource for SyntheticSources {
    fn video_frame(&mut self, asset: &MediaAsset, source_time: MediaTime) -> Option<SourceFrame> {
        self.video_requests.push((asset.id.clone(), source_time));
        let index = source_time.as_ticks() * VIDEO_FPS / TICKS_PER_SECOND;
        Some(SourceFrame {
            key: format!("{}#{index}", asset.id),
            image: Arc::new(video_frame(index)),
        })
    }

    fn image(&mut self, asset: &MediaAsset) -> Option<SourceFrame> {
        let image = match asset.id.as_str() {
            "checker" => checker(),
            "card" => card(),
            _ => return None,
        };
        Some(SourceFrame {
            key: asset.id.clone(),
            image: Arc::new(image),
        })
    }
}

/// 64×36 gradient whose blue channel and a moving bar follow the frame index.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn video_frame(index: i64) -> RgbaImage {
    let bar = (index * 2).rem_euclid(64) as u32;
    let blue = (index * 9).rem_euclid(256) as u8;
    RgbaImage::from_fn(64, 36, |x, y| {
        if x / 4 == bar / 4 {
            [255, 255, 255, 255]
        } else {
            [(x * 4) as u8, (y * 7) as u8, blue, 255]
        }
    })
}

/// 32×32, four 8×8-cell checker of orange and white.
pub fn checker() -> RgbaImage {
    RgbaImage::from_fn(32, 32, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            [240, 120, 20, 255]
        } else {
            [255, 255, 255, 255]
        }
    })
}

/// 48×24 green card with a transparent hole and a half-transparent left
/// third.
pub fn card() -> RgbaImage {
    RgbaImage::from_fn(48, 24, |x, y| {
        if (18..30).contains(&x) && (6..18).contains(&y) {
            [0, 0, 0, 0]
        } else if x < 16 {
            [30, 200, 90, 128]
        } else {
            [30, 200, 90, 255]
        }
    })
}
