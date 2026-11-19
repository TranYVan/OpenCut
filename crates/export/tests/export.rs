//! Exports a short project and checks the MP4 against the compositor:
//! duration, codecs, and sampled frames matching the rendered ones within
//! what a lossy codec changes. `video.mp4` is 2 s at 30 fps with a 440 Hz
//! tone; frame `n` has a white bar in pixel column `n`.

use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use opencut_compositor::gpu::GpuContext;
use opencut_compositor::{MediaSources, Renderer, RgbaImage};
use opencut_export::{VideoCodec, export_project};
use opencut_media::video::VideoDecoder;
use opencut_media::{AudioFormat, decode_audio, probe};
use opencut_model::time::{FrameRate, MediaTime};
use opencut_model::{CanvasSize, Element, ElementBase, MediaAsset, Project, VideoElement};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name)
}

fn resolve(asset: &MediaAsset) -> Option<PathBuf> {
    (asset.id == "clip").then(|| fixture("video.mp4"))
}

/// A context on any adapter, or `None` (after saying why), as in the
/// compositor's tests. `OPENCUT_REQUIRE_GPU=1` makes a missing one fail.
fn gpu() -> Option<GpuContext> {
    match GpuContext::new_blocking() {
        Ok(context) => Some(context),
        Err(error) if std::env::var_os("OPENCUT_REQUIRE_GPU").is_some() => {
            panic!("OPENCUT_REQUIRE_GPU is set but no adapter is available: {error}")
        }
        Err(error) => {
            eprintln!("SKIPPED: {error}");
            None
        }
    }
}

/// 320×180 at 30 fps: `video.mp4` (with its sound) filling the canvas for
/// its whole 2 s.
fn project() -> Project {
    let mut project = Project::new("export", Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
    project.settings.canvas_size = CanvasSize {
        width: 320,
        height: 180,
    };
    project.settings.fps = FrameRate::new(30, 1);
    let info = probe(fixture("video.mp4")).unwrap();
    project.media_assets = vec![info.to_asset("clip".into(), "clip".into(), 0, 0)];
    let base = ElementBase::new("video", "video", MediaTime::ZERO, frame_time(60));
    project
        .current_scene_mut()
        .unwrap()
        .tracks
        .main
        .elements
        .push(Element::Video(VideoElement {
            base,
            media_id: "clip".into(),
            is_source_audio_enabled: None,
            hidden: None,
            retime: None,
            effects: None,
            masks: None,
        }));
    project
}

fn frame_time(frame: i64) -> MediaTime {
    MediaTime::from_frame(frame, FrameRate::new(30, 1)).unwrap()
}

/// Mean absolute difference over the RGB channels.
fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: u64 = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .flat_map(|(a, b)| (0..3).map(move |i| u64::from(a[i].abs_diff(b[i]))))
        .sum();
    sum as f64 / (a.len() / 4 * 3) as f64
}

/// The column of the white bar: the brightest in the middle row.
fn bar_column(data: &[u8], width: u32, height: u32) -> u32 {
    let row = (height / 2) as usize * width as usize * 4;
    (0..width)
        .max_by_key(|&x| {
            let at = row + x as usize * 4;
            data[at..at + 3].iter().map(|&c| u32::from(c)).sum::<u32>()
        })
        .unwrap()
}

#[test]
fn exported_frames_match_the_compositor() {
    let Some(context) = gpu() else { return };
    let project = project();
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("export.mp4");

    let encoder = export_project(&context, &project, resolve, &output, VideoCodec::H264).unwrap();
    eprintln!("encoded with {encoder}");

    let info = probe(&output).unwrap();
    assert!(
        matches!(info.video_codec.as_deref(), Some("h264" | "mpeg4")),
        "video codec {:?}",
        info.video_codec
    );
    assert_eq!(info.audio_codec.as_deref(), Some("aac"));
    assert_eq!((info.width, info.height), (Some(320), Some(180)));
    assert_eq!(info.frame_rate, Some(FrameRate::new(30, 1)));
    let duration = info.duration.unwrap();
    assert!((duration - 2.0).abs() < 0.05, "duration {duration}");

    // The tone made it through.
    let audio = decode_audio(&output, Some(AudioFormat::STEREO_48K)).unwrap();
    assert!(
        (audio.duration() - 2.0).abs() < 0.05,
        "audio duration {}",
        audio.duration()
    );
    let rms =
        (audio.samples.iter().map(|s| s * s).sum::<f32>() / audio.samples.len() as f32).sqrt();
    assert!(rms > 0.05, "audio rms {rms}");

    let mut renderer = Renderer::new(&context);
    let mut sources = MediaSources::new(resolve);
    let mut decoder = VideoDecoder::open(&output).unwrap();
    for frame in [0, 1, 15, 30, 31, 59] {
        let time = frame_time(frame);
        let expected: RgbaImage = renderer
            .render_project(&context, &project, time, &mut sources)
            .unwrap();
        let actual = decoder.frame_at(time).unwrap().unwrap();
        assert_eq!(actual.index, frame);
        let (width, height) = (actual.image.width, actual.image.height);
        assert_eq!((width, height), (expected.width, expected.height));

        let difference = mean_difference(&actual.image.data, &expected.data);
        assert!(
            difference < 4.0,
            "frame {frame}: mean difference {difference}"
        );
        let (actual_bar, expected_bar) = (
            bar_column(&actual.image.data, width, height),
            bar_column(&expected.data, width, height),
        );
        assert!(
            actual_bar.abs_diff(expected_bar) <= 1,
            "frame {frame}: bar at {actual_bar}, rendered at {expected_bar}"
        );
    }
}
