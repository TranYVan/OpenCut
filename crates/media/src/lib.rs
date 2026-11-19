//! OpenCut media I/O, built on the shared FFmpeg that `setup/setup.ts`
//! fetches (see `build.rs`).
//!
//! - [`probe`] reads what a file is: kind, duration, size, frame rate,
//!   codecs, and maps it into an [`opencut_model::MediaAsset`].
//! - [`audio`] decodes audio to `f32` PCM ([`decode_audio`]) and makes
//!   waveform summaries ([`waveform_summary`], [`WaveformCache`]).
//! - [`video`] decodes frames at a source time, caches them, and makes
//!   thumbnails, on a worker thread.
//! - [`Mp4Writer`] encodes RGBA frames (hardware H.264/HEVC first) and
//!   stereo audio (AAC) into an MP4, for export.

pub mod audio;
mod encode;
mod error;
mod ffi;
mod format;
mod probe;
mod raw;
pub mod video;

pub use audio::{
    AudioDecoder, AudioFormat, DecodedAudio, WaveformCache, WaveformSummary, decode_audio,
    waveform_summary,
};
pub use encode::{EncodeSettings, Mp4Writer, VideoCodec};
pub use error::MediaError;
pub use probe::{MediaInfo, probe, probe_asset};

/// The FFmpeg version linked, e.g. `8.1.3`.
pub fn ffmpeg_version() -> String {
    // SAFETY: av_version_info returns a static NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(ffi::av_version_info()) }
        .to_string_lossy()
        .into_owned()
}
