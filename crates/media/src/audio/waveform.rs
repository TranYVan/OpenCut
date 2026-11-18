//! Waveform summaries: the min and max sample of every fixed-size bucket
//! of frames, across all channels (classic `buildSourceWaveformSummary`,
//! which kept only the absolute peak).

use std::path::Path;

use super::decode::{AudioDecoder, AudioFormat};
use crate::MediaError;

/// Frames per bucket unless asked otherwise (the classic app's 128: about
/// 375 buckets a second at 48 kHz, finer than any timeline zoom needs).
pub const DEFAULT_BUCKET_SIZE: u32 = 128;

/// The lowest and highest sample in a stretch of audio.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaveformPeak {
    pub min: f32,
    pub max: f32,
}

impl WaveformPeak {
    /// The larger of `|min|` and `|max|`.
    pub fn amplitude(self) -> f32 {
        self.min.abs().max(self.max.abs())
    }

    fn merge(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
}

/// A file's audio, summarized into one [`WaveformPeak`] per
/// `bucket_size` frames at the source's own sample rate.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveformSummary {
    /// Of the audio summarized (the source's).
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames in the audio; the last bucket may hold fewer than
    /// `bucket_size`.
    pub total_frames: u64,
    pub bucket_size: u32,
    pub peaks: Vec<WaveformPeak>,
}

impl WaveformSummary {
    /// Summarizes interleaved samples in `format`.
    pub fn from_samples(samples: &[f32], format: AudioFormat, bucket_size: u32) -> Self {
        let mut builder = Builder::new(format, bucket_size);
        builder.push(samples);
        builder.finish()
    }

    /// Length of the audio in seconds.
    pub fn duration(&self) -> f64 {
        self.total_frames as f64 / f64::from(self.sample_rate.max(1))
    }

    /// The peak over frames `start..end`, from the buckets that overlap
    /// it; zero when the range is empty or past the end.
    pub fn peak(&self, start: u64, end: u64) -> WaveformPeak {
        let size = u64::from(self.bucket_size.max(1));
        let end = end.min(self.total_frames);
        if end <= start {
            return WaveformPeak::default();
        }
        let first = (start / size) as usize;
        let last = end.div_ceil(size) as usize;
        self.peaks
            .get(first..last.min(self.peaks.len()))
            .and_then(|peaks| peaks.iter().copied().reduce(WaveformPeak::merge))
            .unwrap_or_default()
    }

    /// `count` peaks evenly covering `start..end` seconds of the source,
    /// e.g. one per pixel column of a clip on the timeline.
    pub fn peaks_between(&self, start: f64, end: f64, count: usize) -> Vec<WaveformPeak> {
        let rate = f64::from(self.sample_rate);
        let step = (end - start) / count as f64;
        (0..count)
            .map(|index| {
                let from = start + step * index as f64;
                let to = from + step;
                self.peak(
                    (from * rate).floor().max(0.0) as u64,
                    (to * rate).ceil().max(0.0) as u64,
                )
            })
            .collect()
    }
}

/// Decodes `path`'s audio (at its own rate and channels) and summarizes it
/// into buckets of `bucket_size` frames, without holding the whole file's
/// PCM in memory.
pub fn waveform_summary(
    path: impl AsRef<Path>,
    bucket_size: u32,
) -> Result<WaveformSummary, MediaError> {
    let mut decoder = AudioDecoder::open(path, None)?;
    let mut builder = Builder::new(decoder.format(), bucket_size);
    let mut block = Vec::new();
    loop {
        block.clear();
        if decoder.decode_into(&mut block)? == 0 {
            break;
        }
        builder.push(&block);
    }
    Ok(builder.finish())
}

/// Accumulates peaks from blocks of any length.
struct Builder {
    format: AudioFormat,
    bucket_size: u32,
    total_frames: u64,
    peaks: Vec<WaveformPeak>,
    /// The bucket being filled, and how many frames it has.
    current: Option<WaveformPeak>,
    filled: u32,
}

impl Builder {
    fn new(format: AudioFormat, bucket_size: u32) -> Self {
        Self {
            format,
            bucket_size: bucket_size.max(1),
            total_frames: 0,
            peaks: Vec::new(),
            current: None,
            filled: 0,
        }
    }

    fn push(&mut self, samples: &[f32]) {
        let channels = usize::from(self.format.channels.max(1));
        for frame in samples.chunks_exact(channels) {
            let mut peak = self.current.unwrap_or(WaveformPeak {
                min: f32::INFINITY,
                max: f32::NEG_INFINITY,
            });
            for &sample in frame {
                // min/max skip NaN.
                peak.min = peak.min.min(sample);
                peak.max = peak.max.max(sample);
            }
            self.current = Some(peak);
            self.filled += 1;
            self.total_frames += 1;
            if self.filled == self.bucket_size {
                self.close_bucket();
            }
        }
    }

    fn close_bucket(&mut self) {
        if let Some(peak) = self.current.take() {
            // A bucket of only NaN stays at the infinities: call it silence.
            self.peaks.push(if peak.min > peak.max {
                WaveformPeak::default()
            } else {
                peak
            });
        }
        self.filled = 0;
    }

    fn finish(mut self) -> WaveformSummary {
        self.close_bucket();
        WaveformSummary {
            sample_rate: self.format.sample_rate,
            channels: self.format.channels,
            total_frames: self.total_frames,
            bucket_size: self.bucket_size,
            peaks: self.peaks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_min_and_max_across_channels() {
        // Stereo, 5 frames, buckets of 2: the last bucket is short.
        let samples = [0.1, -0.2, 0.5, 0.0, -0.9, 0.3, 0.2, 0.2, 0.4, -0.1];
        let summary = WaveformSummary::from_samples(&samples, AudioFormat::new(10, 2), 2);
        assert_eq!(summary.total_frames, 5);
        assert_eq!(
            summary.peaks,
            vec![
                WaveformPeak {
                    min: -0.2,
                    max: 0.5
                },
                WaveformPeak {
                    min: -0.9,
                    max: 0.3
                },
                WaveformPeak {
                    min: -0.1,
                    max: 0.4
                },
            ]
        );
        assert_eq!(
            summary.peak(0, 5),
            WaveformPeak {
                min: -0.9,
                max: 0.5
            }
        );
        assert_eq!(
            summary.peak(4, 5),
            WaveformPeak {
                min: -0.1,
                max: 0.4
            }
        );
        assert_eq!(summary.peak(5, 9), WaveformPeak::default());
        assert_eq!(summary.peaks_between(0.0, 0.4, 2).len(), 2);
    }

    #[test]
    fn splitting_blocks_does_not_change_the_result() {
        let samples: Vec<f32> = (0..1000)
            .map(|i| ((i * 37) % 101) as f32 / 50.0 - 1.0)
            .collect();
        let format = AudioFormat::new(48_000, 2);
        let whole = WaveformSummary::from_samples(&samples, format, 7);
        let mut builder = Builder::new(format, 7);
        for block in samples.chunks(62) {
            builder.push(block);
        }
        assert_eq!(builder.finish(), whole);
    }
}
