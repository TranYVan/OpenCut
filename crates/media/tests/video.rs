//! Decodes the fixtures in `tests/fixtures/`. `video.mp4` is 60 frames of
//! H.264 at 30 fps with a keyframe every 15; frame `n` has a white bar in
//! pixel column `n` (see `make-video.swift`), which tells which frame came
//! back. Generated clips with B-frames are tested in `src/video/tests.rs`.

use std::path::PathBuf;

use opencut_media::MediaError;
use opencut_media::video::{RgbaImage, VideoDecoder, poster_frame, thumbnail_strip};
use opencut_time::{FrameRate, MediaTime, TICKS_PER_SECOND};

const FRAMES: i64 = 60;
const FRAME: i64 = TICKS_PER_SECOND / 30;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The column of the white bar: the brightest in the middle row.
fn bar_index(image: &RgbaImage) -> i64 {
    let y = image.height / 2;
    let brightness = |x: u32| {
        image.pixel(x, y)[..3]
            .iter()
            .map(|&c| u32::from(c))
            .sum::<u32>()
    };
    let column = (0..image.width).max_by_key(|&x| brightness(x)).unwrap();
    i64::from(column) * 64 / i64::from(image.width)
}

#[test]
fn reads_h264_stream_info() {
    let decoder = VideoDecoder::open(fixture("video.mp4")).unwrap();
    let info = decoder.info();
    assert_eq!((info.width, info.height), (64, 48));
    assert_eq!((info.display_width, info.display_height), (64, 48));
    assert_eq!(info.frame_rate, Some(FrameRate::FPS_30));
    assert_eq!(
        info.duration,
        Some(MediaTime::from_ticks(2 * TICKS_PER_SECOND))
    );
    assert_eq!(info.codec, "h264");
    assert_eq!(
        info.frame_index_at(MediaTime::from_ticks(10 * FRAME + 3999)),
        10
    );
    assert_eq!(info.frame_time(10), MediaTime::from_ticks(10 * FRAME));
}

#[test]
fn decodes_all_h264_frames_in_order() {
    let mut decoder = VideoDecoder::open(fixture("video.mp4")).unwrap();
    let mut index = 0;
    while let Some(frame) = decoder.next_frame().unwrap() {
        assert_eq!((frame.index, bar_index(&frame.image)), (index, index));
        assert_eq!(frame.time, MediaTime::from_ticks(index * FRAME));
        index += 1;
    }
    assert_eq!(index, FRAMES);
}

#[test]
fn random_h264_seeks_return_the_frame_showing() {
    let mut decoder = VideoDecoder::open(fixture("video.mp4")).unwrap();
    let mut state: u64 = 0x0c0f_fee5;
    for step in 0..200 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let time = MediaTime::from_ticks((state % (FRAMES * FRAME) as u64) as i64);
        let frame = decoder.frame_at(time).unwrap().expect("a frame");
        let expected = time.as_ticks() / FRAME;
        assert_eq!(
            (frame.index, bar_index(&frame.image)),
            (expected, expected),
            "step {step}: frame at {time:?}"
        );
    }
}

#[test]
fn poster_and_strip_of_a_video() {
    let poster = poster_frame(fixture("video.mp4"), 1280, 720).unwrap();
    assert_eq!((poster.width, poster.height), (64, 48));
    assert_eq!(bar_index(&poster), 30);

    let strip = thumbnail_strip(fixture("video.mp4"), 4, 24).unwrap();
    let indices: Vec<_> = strip
        .iter()
        .map(|tile| tile.time.as_ticks() / FRAME)
        .collect();
    assert_eq!(indices, [7, 22, 37, 52]);
    for tile in &strip {
        assert_eq!((tile.image.width, tile.image.height), (32, 24));
    }
}

#[test]
fn poster_of_still_images() {
    for name in ["image.png", "image.jpg"] {
        let poster = poster_frame(fixture(name), 16, 16).unwrap();
        assert!(
            poster.width <= 16 && poster.height <= 16,
            "{name}: {poster:?}"
        );
        assert_eq!(
            poster.byte_len(),
            (poster.width * poster.height * 4) as usize
        );
    }
    let mut decoder = VideoDecoder::open(fixture("image.png")).unwrap();
    let first = decoder.frame_at(MediaTime::ZERO).unwrap().unwrap();
    let later = decoder
        .frame_at(MediaTime::from_ticks(5 * TICKS_PER_SECOND))
        .unwrap()
        .unwrap();
    assert_eq!(first.image, later.image);
}

#[test]
fn audio_has_no_video() {
    for name in ["audio.wav", "audio.m4a"] {
        assert!(matches!(
            VideoDecoder::open(fixture(name)),
            Err(MediaError::NoVideo { .. })
        ));
        assert!(matches!(
            poster_frame(fixture(name), 64, 64),
            Err(MediaError::NoVideo { .. })
        ));
    }
}
