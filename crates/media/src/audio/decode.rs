//! Decodes a file's audio stream to interleaved `f32` PCM.

use std::path::Path;

use super::resample::Resampler;
use crate::MediaError;
use crate::ffi;
use crate::format::Input;
use crate::raw::{CodecContext, Frame, Packet, Threads};

/// A PCM layout: sample rate and channel count. Samples are always
/// interleaved `f32` in `[-1, 1]` (louder sources can exceed that).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    /// Frames per second, e.g. 48 000.
    pub sample_rate: u32,
    /// Channels per frame: 1 mono, 2 stereo, 6 for 5.1, … Remixing to a
    /// different count uses FFmpeg's default layout for it.
    pub channels: u16,
}

impl AudioFormat {
    pub const fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    /// 48 kHz stereo.
    pub const STEREO_48K: Self = Self::new(48_000, 2);
}

/// A whole stream's decoded audio.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedAudio {
    pub format: AudioFormat,
    /// Interleaved: `frames() * format.channels` samples.
    pub samples: Vec<f32>,
}

impl DecodedAudio {
    /// Number of sample frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels.max(1))
    }

    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        self.frames() as f64 / f64::from(self.format.sample_rate.max(1))
    }
}

/// Decodes all of `path`'s audio (the best audio stream, so video files
/// work too) at `target`, or at the source's own rate and channel count
/// when `None`.
pub fn decode_audio(
    path: impl AsRef<Path>,
    target: Option<AudioFormat>,
) -> Result<DecodedAudio, MediaError> {
    let mut decoder = AudioDecoder::open(path, target)?;
    let mut samples = Vec::new();
    while decoder.decode_into(&mut samples)? > 0 {}
    Ok(DecodedAudio {
        format: decoder.format(),
        samples,
    })
}

/// Streams a file's audio as blocks of interleaved `f32`, for when the
/// whole file shouldn't sit in memory (waveforms, playback, export).
///
/// Decoding starts at the beginning of the stream: the first sample is
/// the stream's first presented sample (encoder delay already trimmed).
pub struct AudioDecoder {
    // Field order is drop order: the codec and resampler go before the
    // demuxer they read from.
    resampler: Option<Resampler>,
    codec: CodecContext,
    packet: Packet,
    frame: Frame,
    input: Input,
    stream_index: i32,
    source: AudioFormat,
    output: AudioFormat,
    state: State,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Decoding,
    /// The demuxer ran out; the decoder is being drained.
    Draining,
    /// Everything, including the resampler's delay, has been returned.
    Done,
}

impl AudioDecoder {
    /// Opens `path`'s best audio stream. `target` is the output format;
    /// `None`, or a zero field in it, keeps the source's value.
    pub fn open(path: impl AsRef<Path>, target: Option<AudioFormat>) -> Result<Self, MediaError> {
        let path = path.as_ref();
        let input = Input::open(path)?;
        let index = input
            .best_stream(ffi::AVMediaType_AVMEDIA_TYPE_AUDIO)
            .ok_or_else(|| MediaError::NoAudioStream {
                path: path.to_owned(),
            })?;
        let stream = input.streams()[index];
        // SAFETY: stream belongs to the open input.
        let codec = unsafe { CodecContext::open_decoder(stream, Threads::Single) }?;

        // SAFETY: an opened codec context.
        let (rate, channels) = unsafe {
            let context = &*codec.as_ptr();
            (context.sample_rate, context.ch_layout.nb_channels)
        };
        let source = AudioFormat {
            // Unknown until the first frame for a few odd codecs; these
            // are only fallbacks for the output format.
            sample_rate: u32::try_from(rate)
                .ok()
                .filter(|&r| r > 0)
                .unwrap_or(48_000),
            channels: u16::try_from(channels).ok().filter(|&c| c > 0).unwrap_or(2),
        };
        let target = target.unwrap_or(source);
        let output = AudioFormat {
            sample_rate: if target.sample_rate == 0 {
                source.sample_rate
            } else {
                target.sample_rate
            },
            channels: if target.channels == 0 {
                source.channels
            } else {
                target.channels
            },
        };

        Ok(Self {
            resampler: None,
            codec,
            packet: Packet::new()?,
            frame: Frame::new()?,
            input,
            stream_index: i32::try_from(index).unwrap_or(i32::MAX),
            source,
            output,
            state: State::Decoding,
        })
    }

    /// The source stream's rate and channel count, as its header says.
    pub fn source_format(&self) -> AudioFormat {
        self.source
    }

    /// The format [`decode_into`](Self::decode_into) produces.
    pub fn format(&self) -> AudioFormat {
        self.output
    }

    /// The stream's duration in seconds, when the container knows it.
    pub fn duration(&self) -> Option<f64> {
        let stream = self.input.streams()[self.stream_index as usize];
        // SAFETY: stream belongs to the open input.
        let stream = unsafe { &*stream };
        if stream.duration != ffi::AV_NOPTS_VALUE && stream.duration > 0 {
            Some(stream.duration as f64 * ffi::q2d(stream.time_base))
        } else {
            self.input.duration()
        }
    }

    /// Decodes the next block and appends it to `out` (interleaved, in
    /// [`format`](Self::format)). Returns the number of frames appended;
    /// 0 means the stream has ended.
    pub fn decode_into(&mut self, out: &mut Vec<f32>) -> Result<usize, MediaError> {
        loop {
            if self.state == State::Done {
                return Ok(0);
            }
            // SAFETY: the codec context is open and the frame allocated.
            let code = unsafe {
                ffi::avcodec_receive_frame(self.codec.as_mut_ptr(), self.frame.as_mut_ptr())
            };
            if code >= 0 {
                let appended = self.convert_frame(out);
                self.frame.unref();
                match appended? {
                    0 => continue,
                    frames => return Ok(frames),
                }
            } else if code == ffi::AVERROR_EAGAIN {
                self.feed()?;
            } else if code == ffi::AVERROR_EOF {
                self.state = State::Done;
                return match &mut self.resampler {
                    Some(resampler) => resampler.flush(out),
                    None => Ok(0),
                };
            } else if code == ffi::AVERROR_INVALIDDATA {
                // A corrupt frame: drop it and carry on, like players do.
                continue;
            } else {
                return Err(MediaError::ffmpeg("avcodec_receive_frame", code));
            }
        }
    }

    /// Sends the decoder the next packet of our stream, or the flush
    /// signal at the end of the file.
    fn feed(&mut self) -> Result<(), MediaError> {
        if self.state == State::Draining {
            // The decoder asked for more after being told it's the end;
            // FFmpeg doesn't do that, but don't spin if it did.
            self.state = State::Done;
            return Ok(());
        }
        loop {
            // SAFETY: the input is open and the packet allocated.
            let code =
                unsafe { ffi::av_read_frame(self.input.as_mut_ptr(), self.packet.as_mut_ptr()) };
            if code == ffi::AVERROR_EOF {
                // SAFETY: a null packet starts draining.
                let code =
                    unsafe { ffi::avcodec_send_packet(self.codec.as_mut_ptr(), std::ptr::null()) };
                if code < 0 && code != ffi::AVERROR_EOF {
                    return Err(MediaError::ffmpeg("avcodec_send_packet", code));
                }
                self.state = State::Draining;
                return Ok(());
            }
            if code < 0 {
                return Err(MediaError::ffmpeg("av_read_frame", code));
            }
            // SAFETY: av_read_frame filled the packet.
            let ours = unsafe { (*self.packet.as_ptr()).stream_index } == self.stream_index;
            if !ours {
                self.packet.unref();
                continue;
            }
            // SAFETY: the codec is open and the packet holds data.
            let code =
                unsafe { ffi::avcodec_send_packet(self.codec.as_mut_ptr(), self.packet.as_ptr()) };
            self.packet.unref();
            if code < 0 && code != ffi::AVERROR_INVALIDDATA {
                return Err(MediaError::ffmpeg("avcodec_send_packet", code));
            }
            return Ok(());
        }
    }

    fn convert_frame(&mut self, out: &mut Vec<f32>) -> Result<usize, MediaError> {
        // SAFETY: the frame holds a decoded audio frame.
        let frame = unsafe { &*self.frame.as_ptr() };
        if frame.nb_samples <= 0 {
            return Ok(0);
        }
        let mut appended = 0;
        let fits = self
            .resampler
            .as_ref()
            .is_some_and(|r| r.accepts(&frame.ch_layout, frame.format, frame.sample_rate));
        if !fits {
            // The stream changed format (or this is the first frame):
            // finish the old conversion before starting a new one.
            if let Some(mut old) = self.resampler.take() {
                appended += old.flush(out)?;
            }
            let resampler = Resampler::new(
                self.output,
                &frame.ch_layout,
                frame.format,
                frame.sample_rate,
            )?;
            debug_assert_eq!(resampler.output_format(), self.output);
            self.resampler = Some(resampler);
        }
        let resampler = self.resampler.as_mut().expect("set above");
        // SAFETY: the resampler accepts this frame's format.
        appended += unsafe { resampler.convert(frame, out) }?;
        Ok(appended)
    }
}
