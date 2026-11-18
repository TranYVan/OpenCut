//! Decoding generated clips whose frames show their own index.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencut_time::{FrameRate, MediaTime, TICKS_PER_SECOND};

use super::test_clip::{self, Clip, read_index};
use super::*;

const FRAMES: u32 = 90;
/// Ticks per frame at 30 fps.
const FRAME: i64 = TICKS_PER_SECOND / 30;

/// 3 s at 30 fps, a keyframe every 12 frames, two B-frames between
/// references (so decode order differs from presentation order).
fn clip() -> Clip {
    Clip {
        width: 160,
        height: 96,
        frames: FRAMES,
        fps: 30,
        gop: 12,
        b_frames: 2,
    }
}

/// A small deterministic random sequence (xorshift), so failures repeat.
fn random_times(count: usize, seed: u64, max: i64) -> Vec<MediaTime> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            MediaTime::from_ticks((state % max as u64) as i64)
        })
        .collect()
}

fn expected_index(time: MediaTime) -> i64 {
    (time.as_ticks() / FRAME).min(i64::from(FRAMES) - 1)
}

#[test]
fn decodes_every_frame_in_order() {
    let file = clip().write("sequential", "mp4");
    let mut decoder = VideoDecoder::open(&file.0).unwrap();
    let info = decoder.info().clone();
    assert_eq!((info.width, info.height), (160, 96));
    assert_eq!(info.frame_rate, Some(FrameRate::FPS_30));
    assert_eq!(info.codec, "mpeg4");

    let mut count = 0;
    while let Some(frame) = decoder.next_frame().unwrap() {
        assert_eq!(frame.index, count);
        assert_eq!(i64::from(read_index(&frame.image)), count);
        assert_eq!(frame.time, MediaTime::from_ticks(count * FRAME));
        assert_eq!(frame.duration, MediaTime::from_ticks(FRAME));
        assert_eq!(frame.image.byte_len(), 160 * 96 * 4);
        count += 1;
    }
    assert_eq!(count, i64::from(FRAMES));
}

fn check_random_seeks(extension: &str) {
    let file = clip().write("seek", extension);
    let mut decoder = VideoDecoder::open(&file.0).unwrap();
    // Up to half a second past the end.
    let span = i64::from(FRAMES) * FRAME + TICKS_PER_SECOND / 2;
    for (step, time) in random_times(150, 0x5eed, span).into_iter().enumerate() {
        let frame = decoder.frame_at(time).unwrap().expect("a frame");
        let expected = expected_index(time);
        assert_eq!(
            (frame.index, i64::from(read_index(&frame.image))),
            (expected, expected),
            "{extension} step {step}: frame at {time:?}"
        );
    }
}

#[test]
fn random_seeks_return_the_frame_showing_mp4() {
    check_random_seeks("mp4");
}

#[test]
fn random_seeks_return_the_frame_showing_mkv() {
    check_random_seeks("mkv");
}

#[test]
fn seeks_mix_with_sequential_decoding() {
    let file = clip().write("mixed", "mp4");
    let mut decoder = VideoDecoder::open(&file.0).unwrap();
    let at = |decoder: &mut VideoDecoder, frame: i64| {
        read_index(
            &decoder
                .frame_at(MediaTime::from_ticks(frame * FRAME))
                .unwrap()
                .unwrap()
                .image,
        )
    };
    assert_eq!(at(&mut decoder, 70), 70);
    assert_eq!(
        read_index(&decoder.next_frame().unwrap().unwrap().image),
        71
    );
    // Backwards: a seek.
    assert_eq!(at(&mut decoder, 5), 5);
    // The same frame again, then a little forward.
    assert_eq!(at(&mut decoder, 5), 5);
    assert_eq!(at(&mut decoder, 6), 6);
    assert_eq!(read_index(&decoder.next_frame().unwrap().unwrap().image), 7);
    // Every frame in reverse, as scrubbing backwards does.
    for frame in (0..i64::from(FRAMES)).rev() {
        assert_eq!(i64::from(at(&mut decoder, frame)), frame);
    }
    // Before the start is the first frame; at the end, the last; then done.
    assert_eq!(at(&mut decoder, -3), 0);
    assert_eq!(at(&mut decoder, 1000), FRAMES - 1);
    assert!(decoder.next_frame().unwrap().is_none());
    assert_eq!(decoder.frame_at_index(42).unwrap().unwrap().index, 42);
}

#[test]
fn scales_frames_on_conversion() {
    let file = clip().write("scaled", "mp4");
    let mut decoder = VideoDecoder::open(&file.0).unwrap();
    decoder.set_output_size(Some((80, 48)));
    let frame = decoder
        .frame_at(MediaTime::from_ticks(33 * FRAME))
        .unwrap()
        .unwrap();
    assert_eq!((frame.image.width, frame.image.height), (80, 48));
    assert_eq!(frame.image.byte_len(), 80 * 48 * 4);
    assert_eq!(read_index(&frame.image), 33);
    assert_eq!(frame.image.pixel(0, 0)[3], 255);
}

#[test]
fn poster_frame_is_one_second_in() {
    let file = clip().write("poster", "mp4");
    let poster = poster_frame(&file.0, 80, 80).unwrap();
    assert_eq!((poster.width, poster.height), (80, 48));
    assert_eq!(read_index(&poster), 30);
}

#[test]
fn thumbnail_strip_takes_the_middle_of_each_slice() {
    let file = clip().write("strip", "mp4");
    let strip = thumbnail_strip(&file.0, 3, 48).unwrap();
    let indices: Vec<_> = strip.iter().map(|tile| read_index(&tile.image)).collect();
    assert_eq!(indices, [15, 45, 75]);
    let times: Vec<_> = strip.iter().map(|tile| tile.time.as_ticks()).collect();
    assert_eq!(times, [60_000, 180_000, 300_000]);
    assert!(
        strip
            .iter()
            .all(|tile| (tile.image.width, tile.image.height) == (80, 48))
    );
    assert!(thumbnail_strip(&file.0, 0, 48).unwrap().is_empty());
}

#[test]
fn worker_decodes_off_thread_through_the_cache() {
    let file = clip().write("worker", "mp4");
    let cache = FrameCache::shared(64 << 20);
    let (worker, responses) = DecodeWorker::spawn(Arc::clone(&cache));
    let receive = || {
        responses
            .recv_timeout(Duration::from_secs(30))
            .expect("an answer")
    };

    let first = worker.request_frame("clip", &file.0, MediaTime::from_ticks(10 * FRAME));
    let response = receive();
    assert_eq!(response.id, first);
    let Ok(DecodeOutput::Frame(frame)) = response.result else {
        panic!("expected a frame, got {:?}", response.result);
    };
    assert_eq!((frame.index, read_index(&frame.image)), (10, 10));
    assert!(cache.lock().unwrap().contains("clip", 10));

    // Asked again: the very same frame, from the cache.
    worker.request_frame("clip", &file.0, MediaTime::from_ticks(10 * FRAME + 7));
    let Ok(DecodeOutput::Frame(again)) = receive().result else {
        panic!("expected a frame");
    };
    assert!(Arc::ptr_eq(&frame, &again));

    let poster = worker.request_poster(&file.0, 1280, 720);
    let strip = worker.request_strip(&file.0, 2, 24);
    let response = receive();
    assert_eq!(response.id, poster);
    assert!(
        matches!(response.result, Ok(DecodeOutput::Poster(ref image)) if read_index(image) == 30)
    );
    let response = receive();
    assert_eq!(response.id, strip);
    assert!(matches!(response.result, Ok(DecodeOutput::Strip(ref tiles)) if tiles.len() == 2));

    let missing = worker.request_frame("gone", "/no/such/file.mp4", MediaTime::ZERO);
    let response = receive();
    assert_eq!(response.id, missing);
    assert!(response.result.is_err());
}

#[test]
fn worker_answers_superseded_scrub_requests() {
    let file = clip().write("scrub", "mp4");
    let (worker, responses) = DecodeWorker::spawn(FrameCache::shared(64 << 20));
    // Keep the worker busy so the scrub requests queue up behind it.
    worker.request_strip(&file.0, 30, 96);
    let ids: Vec<_> = (0..20)
        .map(|frame| worker.request_frame("clip", &file.0, MediaTime::from_ticks(frame * FRAME)))
        .collect();

    let mut answers = std::collections::HashMap::new();
    while answers.len() < ids.len() + 1 {
        let response = responses
            .recv_timeout(Duration::from_secs(30))
            .expect("an answer");
        answers.insert(response.id, response.result);
    }
    // The last one always decodes; the rest were either decoded or dropped.
    let Ok(DecodeOutput::Frame(last)) = &answers[ids.last().unwrap()] else {
        panic!("the latest scrub request decodes");
    };
    assert_eq!(read_index(&last.image), 19);
    let superseded = ids
        .iter()
        .filter(|id| matches!(answers[id], Ok(DecodeOutput::Superseded)))
        .count();
    assert!(superseded > 0, "queued scrub requests are dropped");
}

/// Sequential decode + RGBA conversion speed of a 1080p30 clip: must keep
/// up with real time. Decodes `OPENCUT_THROUGHPUT_CLIP` if set (use a real
/// H.264 file to check what users see), else a generated MPEG-4 clip.
/// Run with `cargo test -p opencut-media --release -- --ignored throughput --nocapture`.
#[test]
#[ignore = "slow; a benchmark"]
fn throughput_1080p_keeps_up_with_real_time() {
    let (path, _generated) = benchmark_clip();
    let mut decoder = VideoDecoder::open(&path).unwrap();
    let fps = decoder
        .info()
        .frame_rate
        .and_then(FrameRate::as_f64)
        .unwrap_or(30.0);
    let started = Instant::now();
    let mut frames = 0u32;
    while decoder.next_frame().unwrap().is_some() {
        frames += 1;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let rate = f64::from(frames) / elapsed;
    println!(
        "{} {}x{}: {frames} frames in {elapsed:.2} s = {rate:.0} fps ({:.1}x real time)",
        decoder.info().codec,
        decoder.info().width,
        decoder.info().height,
        rate / fps
    );
    assert!(frames > 0);
    assert!(rate >= fps, "decoded {rate:.1} fps, need {fps}");
}

/// Random-seek latency on the same clip as the throughput benchmark.
#[test]
#[ignore = "slow; a benchmark"]
fn seek_latency_1080p() {
    let (path, _generated) = benchmark_clip();
    let mut decoder = VideoDecoder::open(&path).unwrap();
    let duration = decoder.info().duration.unwrap().as_ticks();
    let times = random_times(50, 0xbeef, duration);
    let started = Instant::now();
    for &time in &times {
        assert!(decoder.frame_at(time).unwrap().is_some());
    }
    let average = started.elapsed().as_secs_f64() * 1000.0 / times.len() as f64;
    println!("{}: {average:.1} ms per random seek", decoder.info().codec);
}

/// `OPENCUT_THROUGHPUT_CLIP`, or a generated 5 s 1080p30 MPEG-4 clip (kept
/// alive by the returned guard).
fn benchmark_clip() -> (std::path::PathBuf, Option<test_clip::TempClip>) {
    if let Some(path) = std::env::var_os("OPENCUT_THROUGHPUT_CLIP") {
        return (path.into(), None);
    }
    let clip = Clip {
        width: 1920,
        height: 1080,
        frames: 150,
        fps: 30,
        gop: 30,
        b_frames: 2,
    }
    .write("throughput", "mp4");
    (clip.0.clone(), Some(clip))
}
