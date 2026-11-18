//! Audio decoding and waveform summaries, on the fixtures in
//! `tests/fixtures/` and on WAV files written here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opencut_media::audio::{FileIdentity, WaveformPeak};
use opencut_media::{
    AudioDecoder, AudioFormat, MediaError, WaveformCache, WaveformSummary, decode_audio, probe,
    waveform_summary,
};

/// AAC's frame: the most a decoder's output may run past the source
/// (unsignalled padding in the last packet).
const AAC_BLOCK: i64 = 1024;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A fresh scratch folder for one test, under Cargo's target tmp dir.
fn scratch(test: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("audio")
        .join(test);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// Writes a 16-bit PCM WAV of `frames` frames: a sine of `frequency` Hz at
/// `amplitude` in every channel.
fn write_sine_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    frames: u32,
    frequency: f64,
    amplitude: f64,
) {
    let data_length = frames * u32::from(channels) * 2;
    let mut bytes = Vec::with_capacity(44 + data_length as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for frame in 0..frames {
        let t = f64::from(frame) / f64::from(sample_rate);
        let value = (amplitude * (std::f64::consts::TAU * frequency * t).sin() * 32767.0) as i16;
        for _ in 0..channels {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).unwrap();
}

/// Frames the source should have, from its probed duration.
fn expected_frames(path: &Path, sample_rate: u32) -> i64 {
    let duration = probe(path).unwrap().duration.unwrap();
    (duration * f64::from(sample_rate)).round() as i64
}

fn assert_frames_within(actual: usize, expected: i64, tolerance: i64) {
    let actual = actual as i64;
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected} ± {tolerance} frames, got {actual}"
    );
}

#[test]
fn decodes_pcm_wav_sample_exact() {
    let path = fixture("audio.wav");
    let audio = decode_audio(&path, None).unwrap();
    assert_eq!(audio.format, AudioFormat::new(44_100, 1));
    assert_eq!(audio.frames() as i64, expected_frames(&path, 44_100));
    assert_eq!(audio.samples.len(), audio.frames());
}

#[test]
fn resamples_and_remixes_to_the_target_format() {
    let directory = scratch("resample");
    let path = directory.join("tone.wav");
    // 0.75 s of 44.1 kHz mono at half scale.
    write_sine_wav(&path, 44_100, 1, 33_075, 440.0, 0.5);

    let audio = decode_audio(&path, Some(AudioFormat::STEREO_48K)).unwrap();
    assert_eq!(audio.format, AudioFormat::STEREO_48K);
    assert_eq!(audio.samples.len(), audio.frames() * 2);
    // swresample's output is the input's length at the new rate, give or
    // take its rounding.
    assert_frames_within(audio.frames(), 36_000, 2);
    // Mono is copied to both channels, at the same level.
    for frame in audio.samples.chunks_exact(2) {
        assert!((frame[0] - frame[1]).abs() < 1e-6);
    }
    let peak = audio.samples.iter().fold(0f32, |peak, s| peak.max(s.abs()));
    assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
}

#[test]
fn decodes_aac_within_one_block_of_the_source() {
    // audio.m4a holds 1 s; the container's duration also counts the
    // encoder delay, which the decoder trims.
    let audio = decode_audio(fixture("audio.m4a"), None).unwrap();
    assert_frames_within(
        audio.frames(),
        i64::from(audio.format.sample_rate),
        AAC_BLOCK,
    );

    let resampled = decode_audio(fixture("audio.m4a"), Some(AudioFormat::STEREO_48K)).unwrap();
    assert_frames_within(resampled.frames(), 48_000, AAC_BLOCK);
}

#[test]
fn decodes_the_audio_of_a_video_file() {
    let path = fixture("video.mp4");
    let audio = decode_audio(&path, Some(AudioFormat::STEREO_48K)).unwrap();
    assert_frames_within(audio.frames(), expected_frames(&path, 48_000), AAC_BLOCK);
    // A 440 Hz tone, not silence.
    let peak = audio.samples.iter().fold(0f32, |peak, s| peak.max(s.abs()));
    assert!(peak > 0.1, "peak {peak}");
}

#[test]
fn streams_blocks_that_add_up_to_the_whole() {
    let path = fixture("video.mp4");
    let whole = decode_audio(&path, None).unwrap();

    let mut decoder = AudioDecoder::open(&path, None).unwrap();
    assert_eq!(decoder.source_format(), AudioFormat::new(48_000, 2));
    assert!(decoder.duration().is_some());
    let mut samples = Vec::new();
    let mut blocks = 0;
    loop {
        let frames = decoder.decode_into(&mut samples).unwrap();
        if frames == 0 {
            break;
        }
        blocks += 1;
    }
    assert!(blocks > 1);
    assert_eq!(samples, whole.samples);
    // Ended stays ended.
    assert_eq!(decoder.decode_into(&mut samples).unwrap(), 0);
}

#[test]
fn reports_files_without_audio() {
    let error = decode_audio(fixture("image.png"), None).unwrap_err();
    assert!(matches!(error, MediaError::NoAudioStream { .. }), "{error}");
}

#[test]
fn waveform_is_stable_across_runs() {
    for name in ["audio.wav", "audio.m4a", "video.mp4"] {
        let path = fixture(name);
        let first = waveform_summary(&path, 128).unwrap();
        let second = waveform_summary(&path, 128).unwrap();
        assert_eq!(first, second, "{name}");

        // Same as summarizing the fully decoded audio.
        let audio = decode_audio(&path, None).unwrap();
        assert_eq!(
            first,
            WaveformSummary::from_samples(&audio.samples, audio.format, 128),
            "{name}"
        );
        assert_eq!(first.total_frames, audio.frames() as u64);
        assert_eq!(first.peaks.len(), audio.frames().div_ceil(128));
    }
}

#[test]
fn waveform_peaks_follow_the_signal() {
    let directory = scratch("peaks");
    let path = directory.join("tone.wav");
    write_sine_wav(&path, 48_000, 2, 48_000, 100.0, 0.8);

    let summary = waveform_summary(&path, 480).unwrap();
    assert_eq!(
        (summary.sample_rate, summary.channels, summary.total_frames),
        (48_000, 2, 48_000)
    );
    assert_eq!(summary.peaks.len(), 100);
    // Each 10 ms bucket holds one full 100 Hz cycle.
    for peak in &summary.peaks {
        assert!((peak.max - 0.8).abs() < 0.01, "{peak:?}");
        assert!((peak.min + 0.8).abs() < 0.01, "{peak:?}");
    }
    let bars = summary.peaks_between(0.0, 1.0, 10);
    assert_eq!(bars.len(), 10);
    assert!(bars.iter().all(|bar| (bar.amplitude() - 0.8).abs() < 0.01));
    assert_eq!(summary.peak(48_000, 50_000), WaveformPeak::default());
}

#[test]
fn caches_waveforms_in_memory_and_on_disk() {
    let directory = scratch("cache");
    let media = directory.join("tone.wav");
    let store = directory.join("waveforms");
    write_sine_wav(&media, 44_100, 1, 22_050, 220.0, 0.5);

    let cache = WaveformCache::new().with_directory(&store);
    assert!(cache.cached(&media).is_none());
    let first = cache.get(&media).unwrap();
    assert!(Arc::ptr_eq(&first, &cache.get(&media).unwrap()));
    assert!(Arc::ptr_eq(&first, &cache.cached(&media).unwrap()));
    assert_eq!(
        *first,
        waveform_summary(&media, cache.bucket_size()).unwrap()
    );

    // A new cache (a restart) reads the file written by the first.
    let entries: Vec<_> = std::fs::read_dir(&store).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let restarted = WaveformCache::new().with_directory(&store);
    assert_eq!(*restarted.get(&media).unwrap(), *first);

    // Changing the media file changes its identity: recomputed, and the
    // disk entry replaced rather than added to.
    let before = FileIdentity::of(&media).unwrap();
    write_sine_wav(&media, 44_100, 1, 11_025, 220.0, 0.25);
    assert_ne!(FileIdentity::of(&media).unwrap(), before);
    let changed = restarted.get(&media).unwrap();
    assert_eq!(changed.total_frames, 11_025);
    assert!(changed.peak(0, 11_025).max < 0.3);
    assert_eq!(std::fs::read_dir(&store).unwrap().count(), 1);

    restarted.remove(&media);
    assert!(restarted.cached(&media).is_none());
    assert_eq!(std::fs::read_dir(&store).unwrap().count(), 0);
}

#[test]
fn cache_reports_missing_files() {
    let cache = WaveformCache::new();
    let error = cache.get(fixture("missing.wav")).unwrap_err();
    assert!(matches!(error, MediaError::Io { .. }), "{error}");
}
