//! Audio: decoding a file's audio stream (audio files, and the sound of
//! video files) to `f32` PCM at any rate and channel count, and waveform
//! summaries for the timeline, cached per file.

mod cache;
mod decode;
mod raw;
mod resample;
mod waveform;

pub use cache::{FileIdentity, WaveformCache};
pub use decode::{AudioDecoder, AudioFormat, DecodedAudio, decode_audio};
pub use waveform::{DEFAULT_BUCKET_SIZE, WaveformPeak, WaveformSummary, waveform_summary};
