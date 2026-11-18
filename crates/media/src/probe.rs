//! What a media file is, read from its container and stream headers
//! (classic `processMediaAssets`, minus the thumbnail).

use std::ffi::CStr;
use std::path::Path;
use std::time::UNIX_EPOCH;

use opencut_model::{MediaAsset, MediaType};
use opencut_time::FrameRate;

use crate::MediaError;
use crate::ffi;
use crate::format::Input;

/// Basic facts about a media file.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub media_type: MediaType,
    /// Seconds. `None` for images, and for streams that don't say.
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Exact frame rate of the video stream, e.g. 30000/1001.
    pub frame_rate: Option<FrameRate>,
    /// Whether there's an audio stream (always for audio files).
    pub has_audio: bool,
    /// FFmpeg's codec name of the video or image stream, e.g. `h264`, `png`.
    pub video_codec: Option<String>,
    /// FFmpeg's codec name of the audio stream, e.g. `aac`, `pcm_s16le`.
    pub audio_codec: Option<String>,
    /// The demuxer's name(s), e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    pub container: String,
}

impl MediaInfo {
    /// Frames per second as a float, e.g. 29.97.
    pub fn fps(&self) -> Option<f64> {
        self.frame_rate
            .map(|rate| f64::from(rate.numerator) / f64::from(rate.denominator))
    }

    /// The asset the project stores for this file. `size` and
    /// `last_modified` (ms since the epoch) describe the file itself.
    pub fn to_asset(&self, id: String, name: String, size: u64, last_modified: i64) -> MediaAsset {
        MediaAsset {
            id,
            name,
            media_type: self.media_type,
            size,
            last_modified,
            width: self.width,
            height: self.height,
            duration: self.duration,
            fps: self.fps(),
            has_audio: matches!(self.media_type, MediaType::Video | MediaType::Audio)
                .then_some(self.has_audio),
            ephemeral: None,
            thumbnail_url: None,
        }
    }
}

/// Reads what `path` is: kind, duration, size, frame rate and codecs.
pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo, MediaError> {
    let input = Input::open(path.as_ref())?;
    let container = input.format_name();

    let video = input
        .best_stream(ffi::AVMediaType_AVMEDIA_TYPE_VIDEO)
        .map(|index| input.streams()[index])
        // Cover art in audio files is a one-picture "video" stream.
        // SAFETY: stream pointers from an open context are valid.
        .filter(|&stream| unsafe { (*stream).disposition } & ffi::AV_DISPOSITION_ATTACHED_PIC as i32 == 0);
    let audio = input
        .best_stream(ffi::AVMediaType_AVMEDIA_TYPE_AUDIO)
        .map(|index| input.streams()[index]);

    let media_type = match (video, audio) {
        (Some(_), _) if is_image_container(&container) => MediaType::Image,
        (Some(_), _) => MediaType::Video,
        (None, Some(_)) => MediaType::Audio,
        (None, None) => {
            return Err(MediaError::NoMediaStreams {
                path: input.path().to_owned(),
            });
        }
    };

    // SAFETY (all derefs below): stream pointers and their codecpar are
    // valid while `input` is open.
    let (width, height, frame_rate, video_codec) = match video {
        Some(stream) => unsafe {
            let parameters = &*(*stream).codecpar;
            let frame_rate = if media_type == MediaType::Video {
                frame_rate(&*stream)
            } else {
                None
            };
            (
                u32::try_from(parameters.width)
                    .ok()
                    .filter(|&width| width > 0),
                u32::try_from(parameters.height)
                    .ok()
                    .filter(|&height| height > 0),
                frame_rate,
                codec_name(parameters.codec_id),
            )
        },
        None => (None, None, None, None),
    };
    let audio_codec =
        audio.and_then(|stream| unsafe { codec_name((*(*stream).codecpar).codec_id) });

    let duration = match media_type {
        MediaType::Image => None,
        MediaType::Video | MediaType::Audio => input.duration().or_else(|| {
            video
                .or(audio)
                .and_then(|stream| unsafe { stream_duration(&*stream) })
        }),
    };

    Ok(MediaInfo {
        media_type,
        duration,
        width,
        height,
        frame_rate,
        has_audio: audio.is_some(),
        video_codec,
        audio_codec,
        container,
    })
}

/// Probes `path` and makes the project's asset for it, named after the
/// file, with its size and modification time.
pub fn probe_asset(path: impl AsRef<Path>, id: String) -> Result<MediaAsset, MediaError> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path).map_err(|source| MediaError::Io {
        path: path.to_owned(),
        source,
    })?;
    let info = probe(path)?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let last_modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    Ok(info.to_asset(id, name, metadata.len(), last_modified))
}

/// Still images open through the image2 demuxer (by extension) or a
/// `<codec>_pipe` one (by content). GIFs count as images too, as in the
/// classic app.
fn is_image_container(container: &str) -> bool {
    container == "image2" || container == "gif" || container.ends_with("_pipe")
}

/// The stream's average frame rate, else its base rate, as a whole rational.
fn frame_rate(stream: &ffi::AVStream) -> Option<FrameRate> {
    [stream.avg_frame_rate, stream.r_frame_rate]
        .into_iter()
        .find_map(|rate| {
            let numerator = u32::try_from(rate.num).ok().filter(|&n| n > 0)?;
            let denominator = u32::try_from(rate.den).ok().filter(|&d| d > 0)?;
            Some(FrameRate {
                numerator,
                denominator,
            })
        })
}

fn stream_duration(stream: &ffi::AVStream) -> Option<f64> {
    (stream.duration != ffi::AV_NOPTS_VALUE && stream.duration > 0)
        .then(|| stream.duration as f64 * ffi::q2d(stream.time_base))
}

fn codec_name(id: ffi::AVCodecID) -> Option<String> {
    // SAFETY: avcodec_get_name returns a static string for every id.
    let name = unsafe { CStr::from_ptr(ffi::avcodec_get_name(id)) }.to_string_lossy();
    (name != "none").then(|| name.into_owned())
}
