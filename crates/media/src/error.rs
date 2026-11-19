use std::ffi::{CStr, c_char};
use std::path::PathBuf;

use crate::ffi;

/// What went wrong reading or writing a media file.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("can't read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("can't open {path}: {message}")]
    Open { path: PathBuf, message: String },
    #[error("{path} has no video, image or audio stream")]
    NoMediaStreams { path: PathBuf },
    #[error("{path} has no decodable video")]
    NoVideo { path: PathBuf },
    #[error("FFmpeg: {operation} failed: {message}")]
    Ffmpeg {
        operation: &'static str,
        code: i32,
        message: String,
    },
    #[error("{path} has no audio stream")]
    NoAudioStream { path: PathBuf },
    #[error("no video encoder could be opened (tried {tried})")]
    NoVideoEncoder { tried: String },
}

impl MediaError {
    #[allow(dead_code, reason = "for the decoders")]
    pub(crate) fn ffmpeg(operation: &'static str, code: i32) -> Self {
        Self::Ffmpeg {
            operation,
            code,
            message: error_string(code),
        }
    }
}

/// `av_err2str`: FFmpeg's message for an error code.
pub(crate) fn error_string(code: i32) -> String {
    let mut buffer = [0 as c_char; ffi::AV_ERROR_MAX_STRING_SIZE as usize];
    // SAFETY: the buffer is as long as we say, and av_strerror always
    // NUL-terminates what it writes.
    unsafe {
        ffi::av_strerror(code, buffer.as_mut_ptr(), buffer.len());
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}
