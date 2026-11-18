//! Probes the fixtures in `tests/fixtures/` (see `make-video.swift` there).

use std::path::PathBuf;

use opencut_media::{MediaError, MediaInfo, probe, probe_asset};
use opencut_model::MediaType;
use opencut_time::FrameRate;
use pretty_assertions::assert_eq;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn assert_close(actual: Option<f64>, expected: f64, tolerance: f64) {
    let actual = actual.expect("a duration");
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected} ± {tolerance}, got {actual}"
    );
}

#[test]
fn links_the_pinned_ffmpeg() {
    let pinned: serde_json::Value =
        serde_json::from_str(include_str!("../setup/ffmpeg.json")).unwrap();
    // Builds report "8.1.3"; BtbN's prebuilts "n8.1.3-…".
    let version = opencut_media::ffmpeg_version();
    assert!(
        version
            .trim_start_matches('n')
            .starts_with(pinned["version"].as_str().unwrap()),
        "linked FFmpeg {version}"
    );
}

#[test]
fn probes_h264_aac_mp4() {
    let info = probe(fixture("video.mp4")).unwrap();
    assert_close(info.duration, 2.0, 0.05);
    assert_eq!(
        MediaInfo {
            duration: None,
            ..info
        },
        MediaInfo {
            media_type: MediaType::Video,
            duration: None,
            width: Some(64),
            height: Some(48),
            frame_rate: Some(FrameRate::FPS_30),
            has_audio: true,
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            container: "mov,mp4,m4a,3gp,3g2,mj2".into(),
        }
    );
}

#[test]
fn probes_png() {
    let info = probe(fixture("image.png")).unwrap();
    assert_eq!(info.media_type, MediaType::Image);
    assert_eq!((info.width, info.height), (Some(40), Some(30)));
    assert_eq!(info.video_codec.as_deref(), Some("png"));
    assert_eq!(
        (info.duration, info.frame_rate, info.has_audio),
        (None, None, false)
    );
}

#[test]
fn probes_jpg() {
    let info = probe(fixture("image.jpg")).unwrap();
    assert_eq!(info.media_type, MediaType::Image);
    assert_eq!((info.width, info.height), (Some(40), Some(30)));
    assert_eq!(info.video_codec.as_deref(), Some("mjpeg"));
    assert_eq!(info.duration, None);
}

#[test]
fn probes_wav() {
    let info = probe(fixture("audio.wav")).unwrap();
    assert_eq!(info.media_type, MediaType::Audio);
    assert_close(info.duration, 1.0, 0.001);
    assert_eq!(info.audio_codec.as_deref(), Some("pcm_s16le"));
    assert_eq!(
        (info.width, info.video_codec, info.has_audio),
        (None, None, true)
    );
}

#[test]
fn probes_aac_m4a() {
    let info = probe(fixture("audio.m4a")).unwrap();
    assert_eq!(info.media_type, MediaType::Audio);
    // The container counts AAC's encoder delay and last-frame padding: up
    // to a few 1024-sample frames past the 1 s of source audio.
    assert_close(info.duration, 1.0, 0.1);
    assert_eq!(info.audio_codec.as_deref(), Some("aac"));
}

#[test]
fn maps_into_a_media_asset() {
    let path = fixture("video.mp4");
    let asset = probe_asset(&path, "asset-1".into()).unwrap();
    assert_eq!(asset.id, "asset-1");
    assert_eq!(asset.name, "video.mp4");
    assert_eq!(asset.media_type, MediaType::Video);
    assert_eq!(asset.size, std::fs::metadata(&path).unwrap().len());
    assert!(asset.last_modified > 0);
    assert_eq!((asset.width, asset.height), (Some(64), Some(48)));
    assert_eq!(asset.fps, Some(30.0));
    assert_eq!(asset.has_audio, Some(true));

    let image = probe_asset(fixture("image.png"), "asset-2".into()).unwrap();
    assert_eq!(
        (image.has_audio, image.fps, image.duration),
        (None, None, None)
    );
}

#[test]
fn rejects_a_file_that_is_not_media() {
    let error = probe(fixture("make-video.swift")).unwrap_err();
    assert!(matches!(error, MediaError::Open { .. }), "{error}");
}

#[test]
fn reports_a_missing_file() {
    let error = probe_asset(fixture("missing.mp4"), "id".into()).unwrap_err();
    assert!(matches!(error, MediaError::Io { .. }), "{error}");
}
