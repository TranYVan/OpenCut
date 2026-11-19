//! Writes an MP4: RGBA frames encoded to H.264 or HEVC, with the platform's
//! hardware encoder when one opens, and interleaved stereo `f32` to AAC.

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use opencut_time::FrameRate;

use crate::MediaError;
use crate::ffi;
use crate::format::{init, path_to_cstring};
use crate::raw::{CodecContext, Frame, Packet, check};

/// The video codec to export with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

/// What an [`Mp4Writer`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodeSettings {
    /// Size of the RGBA frames passed in. Encoded at that size rounded
    /// down to even, as 4:2:0 needs.
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub codec: VideoCodec,
    /// Rate of the interleaved stereo audio passed in; `None` writes no
    /// audio track.
    pub sample_rate: Option<u32>,
}

/// Hardware first, in the order a machine is likely to have them; each is
/// tried at runtime and skipped when it doesn't open. Then software:
/// openh264 / kvazaar where the build has them (the Linux and Windows
/// prebuilts), FFmpeg's own MPEG-4 Part 2 as the last resort (macOS source
/// builds, when VideoToolbox won't open).
const H264_ENCODERS: &[&str] = &[
    "h264_videotoolbox",
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    "h264_mf",
    "h264_vaapi",
    "libopenh264",
    "mpeg4",
];
/// HEVC falls back to the H.264 list when none of these opens.
const HEVC_ENCODERS: &[&str] = &[
    "hevc_videotoolbox",
    "hevc_nvenc",
    "hevc_qsv",
    "hevc_amf",
    "hevc_mf",
    "hevc_vaapi",
    "libkvazaar",
];

const AUDIO_CHANNELS: usize = 2;
const AUDIO_BIT_RATE: i64 = 192_000;
/// Bits per pixel per frame: about 9 Mbit/s at 1080p30.
const BITS_PER_PIXEL: f64 = 0.15;
const MIN_BIT_RATE: f64 = 1_000_000.0;

struct Audio {
    codec: CodecContext,
    stream: i32,
    /// Interleaved samples not yet a whole encoder frame.
    pending: Vec<f32>,
    next_pts: i64,
}

/// Encodes and muxes an MP4. Push one RGBA frame per video frame and the
/// audio that goes with it as you go (the muxer interleaves by time), then
/// [`finish`](Self::finish); a writer dropped unfinished leaves a broken
/// file.
pub struct Mp4Writer {
    // Field order is drop order: encoders before the muxer.
    video: CodecContext,
    audio: Option<Audio>,
    /// VAAPI's frame pool, when that's the encoder.
    hw_frames: *mut ffi::AVBufferRef,
    scaler: *mut ffi::SwsContext,
    yuv: Frame,
    packet: Packet,
    output: *mut ffi::AVFormatContext,
    video_stream: i32,
    encoder: &'static str,
    width: u32,
    height: u32,
    next_pts: i64,
}

// SAFETY: every FFmpeg object here is owned and used through `&mut self`.
unsafe impl Send for Mp4Writer {}

impl Mp4Writer {
    pub fn create(path: impl AsRef<Path>, settings: EncodeSettings) -> Result<Self, MediaError> {
        init();
        let path = path.as_ref();
        let open_error = |message: String| MediaError::Open {
            path: path.to_owned(),
            message,
        };
        let c_path =
            path_to_cstring(path).ok_or_else(|| open_error("path contains a NUL byte".into()))?;
        let (width, height) = ((settings.width & !1).max(2), (settings.height & !1).max(2));

        // SAFETY (whole block): every pointer is checked or comes from
        // FFmpeg; ownership moves into `writer` as soon as it exists, so
        // its Drop frees everything on the error paths.
        unsafe {
            let mut output = ptr::null_mut();
            check(
                "avformat_alloc_output_context2",
                ffi::avformat_alloc_output_context2(
                    &mut output,
                    ptr::null(),
                    c"mp4".as_ptr(),
                    c_path.as_ptr(),
                ),
            )?;
            let global_header = (*(*output).oformat).flags & ffi::AVFMT_GLOBALHEADER as i32 != 0;

            let tried: &[&str] = match settings.codec {
                VideoCodec::H264 => &[],
                VideoCodec::Hevc => HEVC_ENCODERS,
            };
            let opened = tried.iter().chain(H264_ENCODERS).find_map(|&name| {
                open_video(name, &settings, width, height, global_header).map(|o| (name, o))
            });
            let Some((encoder, (video, hw_frames, format))) = opened else {
                ffi::avformat_free_context(output);
                return Err(MediaError::NoVideoEncoder {
                    tried: tried
                        .iter()
                        .chain(H264_ENCODERS)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            };

            let mut writer = Self {
                video,
                audio: None,
                hw_frames,
                scaler: ptr::null_mut(),
                yuv: Frame::new()?,
                packet: Packet::new()?,
                output,
                video_stream: 0,
                encoder,
                width: settings.width,
                height: settings.height,
                next_pts: 0,
            };
            writer.video_stream = add_stream(output, &writer.video)?;
            if let Some(rate) = settings.sample_rate {
                let codec = open_audio(rate, global_header)?;
                let stream = add_stream(output, &codec)?;
                writer.audio = Some(Audio {
                    codec,
                    stream,
                    pending: Vec::new(),
                    next_pts: 0,
                });
            }

            let yuv = writer.yuv.as_mut_ptr();
            (*yuv).format = format;
            (*yuv).width = width as i32;
            (*yuv).height = height as i32;
            check("av_frame_get_buffer", ffi::av_frame_get_buffer(yuv, 0))?;

            writer.scaler = ffi::sws_getContext(
                settings.width as i32,
                settings.height as i32,
                ffi::AVPixelFormat_AV_PIX_FMT_RGBA,
                width as i32,
                height as i32,
                format,
                ffi::SwsFlags_SWS_BILINEAR as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            if writer.scaler.is_null() {
                return Err(MediaError::ffmpeg(
                    "sws_getContext",
                    ffi::AVERROR_INVALIDDATA,
                ));
            }
            // Full-range RGB in, limited-range BT.709 out, as the streams
            // are tagged.
            ffi::sws_setColorspaceDetails(
                writer.scaler,
                ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT as i32),
                1,
                ffi::sws_getCoefficients(ffi::SWS_CS_ITU709 as i32),
                0,
                0,
                1 << 16,
                1 << 16,
            );

            let code = ffi::avio_open(
                &mut (*output).pb,
                c_path.as_ptr(),
                ffi::AVIO_FLAG_WRITE as i32,
            );
            if code < 0 {
                return Err(open_error(crate::error::error_string(code)));
            }
            check(
                "avformat_write_header",
                ffi::avformat_write_header(output, ptr::null_mut()),
            )?;
            Ok(writer)
        }
    }

    /// FFmpeg's name of the video encoder in use, e.g. `h264_videotoolbox`.
    pub fn encoder(&self) -> &'static str {
        self.encoder
    }

    /// Encodes the next frame: straight RGBA at the settings' size, rows
    /// packed (alpha is dropped).
    ///
    /// # Panics
    /// When `rgba` is not `width * height * 4` bytes.
    pub fn push_video(&mut self, rgba: &[u8]) -> Result<(), MediaError> {
        assert_eq!(
            rgba.len(),
            self.width as usize * self.height as usize * 4,
            "RGBA data does not match {}x{}",
            self.width,
            self.height
        );
        // SAFETY: the scaler was made for these sizes and formats; `yuv`
        // has buffers of its size, made writable first (the encoder may
        // still hold the last ones).
        unsafe {
            let yuv = self.yuv.as_mut_ptr();
            check("av_frame_make_writable", ffi::av_frame_make_writable(yuv))?;
            let source = [rgba.as_ptr()];
            let stride = [self.width as i32 * 4];
            check(
                "sws_scale",
                ffi::sws_scale(
                    self.scaler,
                    source.as_ptr(),
                    stride.as_ptr(),
                    0,
                    self.height as i32,
                    (*yuv).data.as_ptr(),
                    (*yuv).linesize.as_ptr(),
                ),
            )?;
            (*yuv).pts = self.next_pts;
            self.next_pts += 1;

            if self.hw_frames.is_null() {
                return encode(
                    self.output,
                    &mut self.video,
                    self.video_stream,
                    &mut self.packet,
                    yuv,
                );
            }
            let mut hw = Frame::new()?;
            check(
                "av_hwframe_get_buffer",
                ffi::av_hwframe_get_buffer(self.hw_frames, hw.as_mut_ptr(), 0),
            )?;
            check(
                "av_hwframe_transfer_data",
                ffi::av_hwframe_transfer_data(hw.as_mut_ptr(), yuv, 0),
            )?;
            (*hw.as_mut_ptr()).pts = (*yuv).pts;
            encode(
                self.output,
                &mut self.video,
                self.video_stream,
                &mut self.packet,
                hw.as_ptr(),
            )
        }
    }

    /// Encodes interleaved stereo samples at the settings' rate, following
    /// those pushed before. Ignored when the settings have no audio.
    pub fn push_audio(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let Some(audio) = &mut self.audio else {
            return Ok(());
        };
        audio.pending.extend_from_slice(samples);
        // SAFETY: an opened encoder.
        let frame_len = unsafe { (*audio.codec.as_ptr()).frame_size } as usize * AUDIO_CHANNELS;
        while audio.pending.len() >= frame_len {
            send_audio(self.output, audio, &mut self.packet, frame_len)?;
        }
        Ok(())
    }

    /// Flushes the encoders and writes the index, completing the file.
    pub fn finish(mut self) -> Result<(), MediaError> {
        if let Some(audio) = &mut self.audio {
            if !audio.pending.is_empty() {
                // AAC takes a short last frame.
                send_audio(self.output, audio, &mut self.packet, audio.pending.len())?;
            }
            encode(
                self.output,
                &mut audio.codec,
                audio.stream,
                &mut self.packet,
                ptr::null(),
            )?;
        }
        encode(
            self.output,
            &mut self.video,
            self.video_stream,
            &mut self.packet,
            ptr::null(),
        )?;
        // SAFETY: the header was written; Drop closes the file.
        check("av_write_trailer", unsafe {
            ffi::av_write_trailer(self.output)
        })?;
        Ok(())
    }
}

impl Drop for Mp4Writer {
    fn drop(&mut self) {
        // SAFETY: each pointer is null or owned by this writer.
        unsafe {
            ffi::sws_freeContext(self.scaler);
            ffi::av_buffer_unref(&mut self.hw_frames);
            if !self.output.is_null() {
                ffi::avio_closep(&mut (*self.output).pb);
                ffi::avformat_free_context(self.output);
            }
        }
    }
}

/// Opens encoder `name` if this build has it and it works on this machine:
/// the context, VAAPI's frame pool (else null) and the software pixel
/// format it takes.
///
/// # Safety
/// Plain FFmpeg calls; every pointer is checked.
unsafe fn open_video(
    name: &str,
    settings: &EncodeSettings,
    width: u32,
    height: u32,
    global_header: bool,
) -> Option<(CodecContext, *mut ffi::AVBufferRef, ffi::AVPixelFormat)> {
    let c_name = CString::new(name).ok()?;
    // SAFETY (whole block): see the function's.
    unsafe {
        let codec = ffi::avcodec_find_encoder_by_name(c_name.as_ptr());
        if codec.is_null() {
            return None;
        }
        let mut video = CodecContext::alloc(codec).ok()?;
        let context = &mut *video.as_mut_ptr();
        let rate = settings.frame_rate;
        context.width = width as i32;
        context.height = height as i32;
        context.time_base = ffi::AVRational {
            num: rate.denominator as i32,
            den: rate.numerator as i32,
        };
        context.framerate = ffi::AVRational {
            num: rate.numerator as i32,
            den: rate.denominator as i32,
        };
        let fps = f64::from(rate.numerator) / f64::from(rate.denominator.max(1));
        context.bit_rate =
            (f64::from(width * height) * fps * BITS_PER_PIXEL).max(MIN_BIT_RATE) as i64;
        context.gop_size = (fps * 2.0).round() as i32;
        context.color_primaries = ffi::AVColorPrimaries_AVCOL_PRI_BT709;
        context.color_trc = ffi::AVColorTransferCharacteristic_AVCOL_TRC_BT709;
        context.colorspace = ffi::AVColorSpace_AVCOL_SPC_BT709;
        context.color_range = ffi::AVColorRange_AVCOL_RANGE_MPEG;
        if global_header {
            context.flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        }

        let formats = pixel_formats(codec);
        let has = |format| formats.contains(&format);
        let format = if has(ffi::AVPixelFormat_AV_PIX_FMT_NV12)
            || has(ffi::AVPixelFormat_AV_PIX_FMT_VAAPI)
        {
            ffi::AVPixelFormat_AV_PIX_FMT_NV12
        } else {
            ffi::AVPixelFormat_AV_PIX_FMT_YUV420P
        };
        context.pix_fmt = format;

        let mut hw_frames = ptr::null_mut();
        if has(ffi::AVPixelFormat_AV_PIX_FMT_VAAPI) {
            // VAAPI only takes frames in GPU memory: upload into its pool.
            let mut device = ptr::null_mut();
            let code = ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType_AV_HWDEVICE_TYPE_VAAPI,
                ptr::null(),
                ptr::null_mut(),
                0,
            );
            if code < 0 {
                return None;
            }
            hw_frames = ffi::av_hwframe_ctx_alloc(device);
            ffi::av_buffer_unref(&mut device);
            if hw_frames.is_null() {
                return None;
            }
            let frames = &mut *(*hw_frames).data.cast::<ffi::AVHWFramesContext>();
            frames.format = ffi::AVPixelFormat_AV_PIX_FMT_VAAPI;
            frames.sw_format = format;
            frames.width = context.width;
            frames.height = context.height;
            frames.initial_pool_size = 20;
            if ffi::av_hwframe_ctx_init(hw_frames) < 0 {
                ffi::av_buffer_unref(&mut hw_frames);
                return None;
            }
            context.pix_fmt = ffi::AVPixelFormat_AV_PIX_FMT_VAAPI;
            context.hw_frames_ctx = ffi::av_buffer_ref(hw_frames);
        }
        if name.ends_with("_videotoolbox") {
            // Apple's software encoder where there's no hardware one
            // (virtual machines); still H.264/HEVC.
            ffi::av_opt_set(context.priv_data, c"allow_sw".as_ptr(), c"1".as_ptr(), 0);
        }

        if ffi::avcodec_open2(context, codec, ptr::null_mut()) < 0 {
            ffi::av_buffer_unref(&mut hw_frames);
            return None;
        }
        Some((video, hw_frames, format))
    }
}

/// The pixel formats `codec` lists, empty when it doesn't say.
///
/// # Safety
/// `codec` is a valid encoder.
unsafe fn pixel_formats(codec: *const ffi::AVCodec) -> Vec<ffi::AVPixelFormat> {
    let mut formats: *const std::ffi::c_void = ptr::null();
    let mut count = 0;
    // SAFETY: a codec-level query; the list is static.
    unsafe {
        if ffi::avcodec_get_supported_config(
            ptr::null(),
            codec,
            ffi::AVCodecConfig_AV_CODEC_CONFIG_PIX_FORMAT,
            0,
            &mut formats,
            &mut count,
        ) < 0
            || formats.is_null()
        {
            return Vec::new();
        }
        std::slice::from_raw_parts(formats.cast::<ffi::AVPixelFormat>(), count as usize).to_vec()
    }
}

/// FFmpeg's native AAC encoder, stereo at `rate`.
fn open_audio(rate: u32, global_header: bool) -> Result<CodecContext, MediaError> {
    // SAFETY: every pointer is checked or comes from FFmpeg.
    unsafe {
        let codec = ffi::avcodec_find_encoder_by_name(c"aac".as_ptr());
        if codec.is_null() {
            return Err(MediaError::ffmpeg(
                "avcodec_find_encoder",
                ffi::AVERROR_ENCODER_NOT_FOUND,
            ));
        }
        let mut audio = CodecContext::alloc(codec)?;
        let context = &mut *audio.as_mut_ptr();
        context.sample_fmt = ffi::AVSampleFormat_AV_SAMPLE_FMT_FLTP;
        context.sample_rate = rate as i32;
        ffi::av_channel_layout_default(&mut context.ch_layout, AUDIO_CHANNELS as i32);
        context.bit_rate = AUDIO_BIT_RATE;
        context.time_base = ffi::AVRational {
            num: 1,
            den: rate as i32,
        };
        if global_header {
            context.flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        }
        check(
            "avcodec_open2",
            ffi::avcodec_open2(context, codec, ptr::null_mut()),
        )?;
        Ok(audio)
    }
}

/// A stream for `codec`'s output; its index.
///
/// # Safety
/// `output` is a muxer whose header isn't written yet; `codec` is open.
unsafe fn add_stream(
    output: *mut ffi::AVFormatContext,
    codec: &CodecContext,
) -> Result<i32, MediaError> {
    // SAFETY: see the function's.
    unsafe {
        let stream = ffi::avformat_new_stream(output, ptr::null());
        if stream.is_null() {
            return Err(MediaError::ffmpeg("avformat_new_stream", -12));
        }
        (*stream).time_base = (*codec.as_ptr()).time_base;
        check(
            "avcodec_parameters_from_context",
            ffi::avcodec_parameters_from_context((*stream).codecpar, codec.as_ptr()),
        )?;
        Ok((*stream).index)
    }
}

/// Encodes the first `len` pending samples (deinterleaved to the planar
/// float AAC takes).
fn send_audio(
    output: *mut ffi::AVFormatContext,
    audio: &mut Audio,
    packet: &mut Packet,
    len: usize,
) -> Result<(), MediaError> {
    let frames = len / AUDIO_CHANNELS;
    let mut frame = Frame::new()?;
    // SAFETY: the frame gets buffers for `frames` planar samples per
    // channel before they're written.
    unsafe {
        let raw = frame.as_mut_ptr();
        let context = &*audio.codec.as_ptr();
        (*raw).nb_samples = frames as i32;
        (*raw).format = context.sample_fmt;
        (*raw).sample_rate = context.sample_rate;
        check(
            "av_channel_layout_copy",
            ffi::av_channel_layout_copy(&mut (*raw).ch_layout, &context.ch_layout),
        )?;
        check("av_frame_get_buffer", ffi::av_frame_get_buffer(raw, 0))?;
        for channel in 0..AUDIO_CHANNELS {
            let plane = std::slice::from_raw_parts_mut((*raw).data[channel].cast::<f32>(), frames);
            for (i, sample) in plane.iter_mut().enumerate() {
                *sample = audio.pending[i * AUDIO_CHANNELS + channel];
            }
        }
        (*raw).pts = audio.next_pts;
    }
    audio.next_pts += frames as i64;
    audio.pending.drain(..frames * AUDIO_CHANNELS);
    encode(
        output,
        &mut audio.codec,
        audio.stream,
        packet,
        frame.as_ptr(),
    )
}

/// Sends `frame` (null to flush) and writes every packet that comes out.
fn encode(
    output: *mut ffi::AVFormatContext,
    codec: &mut CodecContext,
    stream: i32,
    packet: &mut Packet,
    frame: *const ffi::AVFrame,
) -> Result<(), MediaError> {
    // SAFETY: an open encoder and muxer; the packet is ours and the muxer
    // takes its contents.
    unsafe {
        check(
            "avcodec_send_frame",
            ffi::avcodec_send_frame(codec.as_mut_ptr(), frame),
        )?;
        loop {
            let code = ffi::avcodec_receive_packet(codec.as_mut_ptr(), packet.as_mut_ptr());
            if code == ffi::AVERROR_EAGAIN || code == ffi::AVERROR_EOF {
                return Ok(());
            }
            check("avcodec_receive_packet", code)?;
            let raw = packet.as_mut_ptr();
            if (*raw).duration == 0
                && (*codec.as_ptr()).codec_type == ffi::AVMediaType_AVMEDIA_TYPE_VIDEO
            {
                // One frame (the video time base); some hardware encoders
                // leave it out, and the last frame then has no length.
                (*raw).duration = 1;
            }
            let target = *(*output).streams.add(stream as usize);
            ffi::av_packet_rescale_ts(raw, (*codec.as_ptr()).time_base, (*target).time_base);
            (*raw).stream_index = stream;
            check(
                "av_interleaved_write_frame",
                ffi::av_interleaved_write_frame(output, raw),
            )?;
        }
    }
}
