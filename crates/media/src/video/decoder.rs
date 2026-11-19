//! Frames of a video file at a source time: keyframe seek, then decode
//! forward (classic `VideoCache`, which did the same through mediabunny).

use std::path::Path;

use opencut_time::{FrameRate, MediaTime, TICKS_PER_SECOND};

use crate::MediaError;
use crate::ffi;
use crate::format::Input;

use super::VideoFrame;
use super::raw::Scaler;
use crate::raw::{CodecContext, Frame, Packet, Threads, check};

/// How far ahead of the last decoded frame a request may be and still be
/// reached by decoding forward rather than seeking, when the container has
/// no keyframe index to tell (the classic app used the same window).
const FORWARD_WINDOW: MediaTime = MediaTime::from_ticks(2 * TICKS_PER_SECOND);

/// How often to seek further back when a seek lands after its target.
const SEEK_RETRIES: u32 = 4;

/// Facts about the decoded video stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoInfo {
    /// Coded size in pixels.
    pub width: u32,
    pub height: u32,
    /// Size to show it at: the coded size stretched by the sample aspect
    /// ratio (anamorphic video). Rotation isn't applied.
    pub display_width: u32,
    pub display_height: u32,
    /// The stream's average frame rate, else its base rate.
    pub frame_rate: Option<FrameRate>,
    /// Length of the stream, when the container says.
    pub duration: Option<MediaTime>,
    /// FFmpeg's name of the decoder, e.g. `h264`.
    pub codec: String,
}

impl VideoInfo {
    /// Index of the frame showing at `time` in a constant-frame-rate
    /// stream: what a [`VideoFrame::index`] decoded there will be. Without
    /// a frame rate (stills), 0.
    pub fn frame_index_at(&self, time: MediaTime) -> i64 {
        self.frame_rate
            .map_or(0, |rate| frames_in(time, rate, Rounding::Floor))
    }

    /// When frame `index` starts, at the stream's frame rate.
    pub fn frame_time(&self, index: i64) -> MediaTime {
        self.frame_rate.map_or(MediaTime::ZERO, |rate| {
            let ticks =
                i128::from(index) * i128::from(TICKS_PER_SECOND) * i128::from(rate.denominator);
            MediaTime::from_ticks(div_round(ticks, i128::from(rate.numerator)) as i64)
        })
    }
}

/// A frame decoded but not yet converted to RGBA.
struct Decoded {
    frame: Frame,
    time: MediaTime,
    duration: MediaTime,
}

/// Decodes one video file. Not thread-safe by itself; move it to the thread
/// that decodes (see [`super::DecodeWorker`]).
pub struct VideoDecoder {
    input: Input,
    stream_index: usize,
    codec: CodecContext,
    packet: Packet,
    scaler: Scaler,
    info: VideoInfo,
    time_base: ffi::AVRational,
    start_pts: i64,
    output_size: Option<(u32, u32)>,
    /// The frame most recently decoded in presentation order: what shows at
    /// times from its own up to `lookahead`'s.
    current: Option<Decoded>,
    /// The frame after `current`, decoded to learn where `current` ends.
    lookahead: Option<Decoded>,
    /// Nothing decoded since opening: reading starts at the beginning.
    at_start: bool,
    /// The demuxer is exhausted and the decoder has been told to drain.
    draining: bool,
}

impl VideoDecoder {
    /// Opens the best video stream of `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MediaError> {
        let path = path.as_ref();
        let input = Input::open(path)?;
        let stream_index = input
            .best_stream(ffi::AVMediaType_AVMEDIA_TYPE_VIDEO)
            // Cover art in audio files is a one-picture "video" stream.
            // SAFETY: stream pointers from an open input are valid.
            .filter(|&index| unsafe {
                (*input.streams()[index]).disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as i32 == 0
            })
            .ok_or_else(|| MediaError::NoVideo {
                path: path.to_owned(),
            })?;
        let stream = input.streams()[stream_index];

        // SAFETY: `stream` belongs to the open input.
        let codec = unsafe { CodecContext::open_decoder(stream, Threads::PerCore)? };

        // Skip demuxing streams nobody reads.
        for (index, &other) in input.streams().iter().enumerate() {
            if index != stream_index {
                // SAFETY: valid stream; discard is read by the demuxer.
                unsafe { (*other).discard = ffi::AVDiscard_AVDISCARD_ALL };
            }
        }

        // SAFETY: valid stream and parameters while `input` is open.
        let (time_base, start_pts, info) = unsafe {
            let stream = &*stream;
            let parameters = &*stream.codecpar;
            let start_pts = if stream.start_time == ffi::AV_NOPTS_VALUE {
                0
            } else {
                stream.start_time
            };
            let width = u32::try_from(parameters.width).unwrap_or(0);
            let height = u32::try_from(parameters.height).unwrap_or(0);
            let (display_width, display_height) =
                display_size(width, height, parameters.sample_aspect_ratio);
            let duration = if stream.duration != ffi::AV_NOPTS_VALUE && stream.duration > 0 {
                Some(pts_span(stream.duration, stream.time_base))
            } else {
                input.duration().and_then(MediaTime::from_seconds_f64)
            };
            let info = VideoInfo {
                width,
                height,
                display_width,
                display_height,
                frame_rate: frame_rate(stream),
                duration,
                codec: codec.codec_name(),
            };
            (stream.time_base, start_pts, info)
        };

        Ok(Self {
            input,
            stream_index,
            codec,
            packet: Packet::new()?,
            scaler: Scaler::new(),
            info,
            time_base,
            start_pts,
            output_size: None,
            current: None,
            lookahead: None,
            at_start: true,
            draining: false,
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    pub fn path(&self) -> &Path {
        self.input.path()
    }

    /// Scales frames to `width`×`height` as they're converted to RGBA;
    /// `None` keeps the coded size. Cheaper than scaling afterwards.
    pub fn set_output_size(&mut self, size: Option<(u32, u32)>) {
        self.output_size = size.filter(|&(width, height)| width > 0 && height > 0);
    }

    /// The frame showing at `time` (from the start of the source): the
    /// last one starting at or before it. Before the first frame that's the
    /// first frame, past the end the last. `None` only when the stream has
    /// no decodable frames.
    ///
    /// Requests moving forward a little decode on from the last one; others
    /// seek to the keyframe before `time` and decode up to it.
    pub fn frame_at(&mut self, time: MediaTime) -> Result<Option<VideoFrame>, MediaError> {
        let time = time.max(MediaTime::ZERO);
        if self.can_decode_forward_to(time) {
            self.advance_to(time)?;
        } else {
            self.seek_and_advance(time)?;
        }
        self.current_frame()
    }

    /// The frame showing at frame `index` of a constant-frame-rate stream.
    pub fn frame_at_index(&mut self, index: i64) -> Result<Option<VideoFrame>, MediaError> {
        self.frame_at(self.info.frame_time(index))
    }

    /// The next frame in presentation order (the first after opening), or
    /// `None` at the end. Continues from wherever [`Self::frame_at`] was.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        let next = match self.lookahead.take() {
            Some(next) => Some(next),
            None => self.decode_next()?,
        };
        match next {
            Some(next) => {
                self.current = Some(next);
                self.current_frame()
            }
            None => Ok(None),
        }
    }

    fn current_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        let Some(current) = &self.current else {
            return Ok(None);
        };
        let frame = current.frame.get();
        let (width, height) = self.output_size.unwrap_or((
            u32::try_from(frame.width).unwrap_or(0),
            u32::try_from(frame.height).unwrap_or(0),
        ));
        let image = self.scaler.convert(frame, width, height)?;
        Ok(Some(VideoFrame {
            index: self
                .info
                .frame_rate
                .map_or(0, |rate| frames_in(current.time, rate, Rounding::Nearest)),
            time: current.time,
            duration: current.duration,
            image,
        }))
    }

    fn can_decode_forward_to(&self, time: MediaTime) -> bool {
        let position = match &self.current {
            Some(current) if self.started_by(current.time, time) => current.time,
            Some(_) => return false,
            None if self.at_start => MediaTime::ZERO,
            None => return false,
        };
        time - position <= FORWARD_WINDOW
            || self
                .keyframe_before(time)
                .is_some_and(|keyframe| keyframe <= position)
    }

    /// Whether a frame starting at `start` is showing by `time`. With a
    /// frame rate this compares on the frame grid, so that timestamps
    /// rounded by the container (Matroska keeps milliseconds) still pick
    /// the frame a constant-rate source has there.
    fn started_by(&self, start: MediaTime, time: MediaTime) -> bool {
        match self.info.frame_rate {
            Some(rate) => {
                frames_in(start, rate, Rounding::Nearest) <= frames_in(time, rate, Rounding::Floor)
            }
            None => start <= time,
        }
    }

    /// Decodes until `current` is the frame showing at `time` (or the first
    /// frame decoded, if that's already later).
    fn advance_to(&mut self, time: MediaTime) -> Result<(), MediaError> {
        loop {
            if self.lookahead.is_none() {
                self.lookahead = self.decode_next()?;
            }
            match &self.lookahead {
                Some(next) if self.started_by(next.time, time) || self.current.is_none() => {
                    self.current = self.lookahead.take();
                }
                _ => return Ok(()),
            }
        }
    }

    fn seek_and_advance(&mut self, time: MediaTime) -> Result<(), MediaError> {
        let mut back = MediaTime::ZERO;
        for _ in 0..=SEEK_RETRIES {
            let target = (time - back).max(MediaTime::ZERO);
            self.seek(target)?;
            self.advance_to(time)?;
            // Some demuxers land after the target; try again from earlier.
            match &self.current {
                Some(current)
                    if !self.started_by(current.time, time) && target > MediaTime::ZERO =>
                {
                    back = if back == MediaTime::ZERO {
                        MediaTime::from_ticks(TICKS_PER_SECOND)
                    } else {
                        back * 2
                    };
                }
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn seek(&mut self, time: MediaTime) -> Result<(), MediaError> {
        let timestamp = self.time_to_pts(time);
        // SAFETY: the context is open and the stream index valid.
        let code = unsafe {
            ffi::av_seek_frame(
                self.input.as_mut_ptr(),
                self.stream_index as i32,
                timestamp,
                ffi::AVSEEK_FLAG_BACKWARD as i32,
            )
        };
        check("av_seek_frame", code)?;
        // SAFETY: the codec context is open.
        unsafe { ffi::avcodec_flush_buffers(self.codec.as_mut_ptr()) };
        self.current = None;
        self.lookahead = None;
        self.at_start = false;
        self.draining = false;
        Ok(())
    }

    /// Start of the last keyframe at or before `time`, from the container's
    /// index (MP4, MOV and most MKV have one).
    fn keyframe_before(&self, time: MediaTime) -> Option<MediaTime> {
        let stream = self.input.streams()[self.stream_index];
        // SAFETY: valid stream; the returned entry stays valid until the
        // index is next changed, and is read at once.
        let timestamp = unsafe {
            let entry = ffi::avformat_index_get_entry_from_timestamp(
                stream,
                self.time_to_pts(time),
                ffi::AVSEEK_FLAG_BACKWARD as i32,
            );
            if entry.is_null() {
                return None;
            }
            (*entry).timestamp
        };
        Some(self.pts_to_time(timestamp))
    }

    /// The next frame in presentation order, or `None` once drained.
    fn decode_next(&mut self) -> Result<Option<Decoded>, MediaError> {
        self.at_start = false;
        let mut frame = Frame::new()?;
        loop {
            // SAFETY: open codec context, allocated frame.
            let code =
                unsafe { ffi::avcodec_receive_frame(self.codec.as_mut_ptr(), frame.as_mut_ptr()) };
            if code >= 0 {
                return Ok(Some(self.decoded(frame)));
            }
            if code == ffi::AVERROR_EOF {
                return Ok(None);
            }
            if code != ffi::AVERROR_EAGAIN {
                return Err(MediaError::ffmpeg("avcodec_receive_frame", code));
            }
            if self.draining {
                // EAGAIN while draining shouldn't happen; treat as the end.
                return Ok(None);
            }
            self.send_next_packet()?;
        }
    }

    /// Feeds the decoder the next packet of our stream, or starts draining
    /// it at the end of the input.
    fn send_next_packet(&mut self) -> Result<(), MediaError> {
        loop {
            // SAFETY: open context, allocated (empty) packet.
            let code =
                unsafe { ffi::av_read_frame(self.input.as_mut_ptr(), self.packet.as_mut_ptr()) };
            if code < 0 {
                // The end, or a read error: decode what there is (a
                // truncated file still plays up to where it breaks).
                self.draining = true;
                // SAFETY: a null packet starts draining.
                let code =
                    unsafe { ffi::avcodec_send_packet(self.codec.as_mut_ptr(), std::ptr::null()) };
                if code < 0 && code != ffi::AVERROR_EOF {
                    return Err(MediaError::ffmpeg("avcodec_send_packet", code));
                }
                return Ok(());
            }
            // SAFETY: av_read_frame filled the packet.
            let ours =
                unsafe { (*self.packet.as_mut_ptr()).stream_index } as usize == self.stream_index;
            if !ours {
                self.packet.unref();
                continue;
            }
            // SAFETY: open context; the decoder takes its own reference.
            let code = unsafe {
                ffi::avcodec_send_packet(self.codec.as_mut_ptr(), self.packet.as_mut_ptr())
            };
            self.packet.unref();
            // Corrupt packets are skipped, as players do.
            if code < 0 && code != ffi::AVERROR_INVALIDDATA {
                return Err(MediaError::ffmpeg("avcodec_send_packet", code));
            }
            return Ok(());
        }
    }

    fn decoded(&self, frame: Frame) -> Decoded {
        let raw = frame.get();
        let fallback_step = self.info.frame_rate.map_or(MediaTime::ZERO, |rate| {
            MediaTime::from_ticks(div_round(
                i128::from(TICKS_PER_SECOND) * i128::from(rate.denominator),
                i128::from(rate.numerator),
            ) as i64)
        });
        let time = if raw.best_effort_timestamp == ffi::AV_NOPTS_VALUE {
            // No timestamp at all: one frame after the last we know.
            [&self.lookahead, &self.current]
                .into_iter()
                .flatten()
                .map(|previous| previous.time + fallback_step)
                .max()
                .unwrap_or(MediaTime::ZERO)
        } else {
            self.pts_to_time(raw.best_effort_timestamp)
        };
        let duration = if raw.duration > 0 {
            pts_span(raw.duration, self.time_base)
        } else {
            fallback_step
        };
        Decoded {
            frame,
            time,
            duration,
        }
    }

    fn pts_to_time(&self, pts: i64) -> MediaTime {
        pts_span(pts.saturating_sub(self.start_pts), self.time_base)
    }

    fn time_to_pts(&self, time: MediaTime) -> i64 {
        let numerator = i128::from(time.as_ticks()) * i128::from(self.time_base.den);
        let denominator = i128::from(TICKS_PER_SECOND) * i128::from(self.time_base.num);
        let pts = if denominator == 0 {
            0
        } else {
            numerator.div_euclid(denominator)
        };
        i64::try_from(pts)
            .unwrap_or(i64::MAX)
            .saturating_add(self.start_pts)
    }
}

/// `count` units of `time_base` as a [`MediaTime`], rounded to the tick.
fn pts_span(count: i64, time_base: ffi::AVRational) -> MediaTime {
    if time_base.den == 0 {
        return MediaTime::ZERO;
    }
    let ticks = i128::from(count) * i128::from(time_base.num) * i128::from(TICKS_PER_SECOND);
    let ticks = div_round(ticks, i128::from(time_base.den));
    MediaTime::from_ticks(i64::try_from(ticks).unwrap_or(i64::MAX))
}

enum Rounding {
    Floor,
    Nearest,
}

/// Whole frames of `rate` in `time`.
fn frames_in(time: MediaTime, rate: FrameRate, rounding: Rounding) -> i64 {
    if !rate.is_valid() {
        return 0;
    }
    let numerator = i128::from(time.as_ticks()) * i128::from(rate.numerator);
    let denominator = i128::from(TICKS_PER_SECOND) * i128::from(rate.denominator);
    let frames = match rounding {
        Rounding::Floor => numerator.div_euclid(denominator),
        Rounding::Nearest => div_round(numerator, denominator),
    };
    i64::try_from(frames).unwrap_or(i64::MAX)
}

/// `numerator / denominator` rounded half up; `denominator` positive.
fn div_round(numerator: i128, denominator: i128) -> i128 {
    (2 * numerator + denominator).div_euclid(2 * denominator)
}

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

fn display_size(width: u32, height: u32, aspect: ffi::AVRational) -> (u32, u32) {
    if aspect.num <= 0 || aspect.den <= 0 || aspect.num == aspect.den {
        return (width, height);
    }
    let stretched = u64::from(width) * aspect.num as u64 / aspect.den as u64;
    (u32::try_from(stretched).unwrap_or(width).max(1), height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_in_rounds_and_floors() {
        let rate = FrameRate::FPS_29_97;
        let frame_10 = MediaTime::from_ticks(10 * 4004);
        assert_eq!(frames_in(frame_10, rate, Rounding::Floor), 10);
        assert_eq!(
            frames_in(frame_10 - MediaTime::ONE_TICK, rate, Rounding::Floor),
            9
        );
        assert_eq!(
            frames_in(frame_10 - MediaTime::ONE_TICK, rate, Rounding::Nearest),
            10
        );
        // A rate with no whole number of ticks per frame.
        let odd = FrameRate::new(90_000, 3001);
        assert_eq!(
            frames_in(MediaTime::from_ticks(120_000), odd, Rounding::Nearest),
            30
        );
    }

    #[test]
    fn pts_span_converts_time_bases() {
        let mp4 = ffi::AVRational {
            num: 1,
            den: 15_360,
        };
        assert_eq!(pts_span(512, mp4), MediaTime::from_ticks(4000));
        let ntsc = ffi::AVRational {
            num: 1001,
            den: 30_000,
        };
        assert_eq!(pts_span(1, ntsc), MediaTime::from_ticks(4004));
    }

    #[test]
    fn display_size_applies_the_sample_aspect_ratio() {
        assert_eq!(
            display_size(720, 576, ffi::AVRational { num: 16, den: 15 }),
            (768, 576)
        );
        assert_eq!(
            display_size(64, 48, ffi::AVRational { num: 0, den: 1 }),
            (64, 48)
        );
    }
}
