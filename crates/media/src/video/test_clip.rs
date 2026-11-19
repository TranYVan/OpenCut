//! Writes test clips whose frames show their own index, with FFmpeg's
//! built-in MPEG-4 Part 2 encoder (in every LGPL build), so seek tests can
//! check which frame came back.
//!
//! Each frame is [`BITS`] vertical stripes, the index in binary, most
//! significant bit on the left: white for 1, black for 0.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::ffi;

use super::RgbaImage;
use crate::raw::Frame;

pub(super) const BITS: u32 = 10;

pub(super) struct Clip {
    pub width: i32,
    pub height: i32,
    pub frames: u32,
    pub fps: i32,
    pub gop: i32,
    pub b_frames: i32,
}

/// A clip file in the temp folder, deleted when dropped.
pub(super) struct TempClip(pub PathBuf);

impl Drop for TempClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Clip {
    /// Writes the clip as `<name>.<extension>` in a fresh temp file; the
    /// extension picks the container.
    pub(super) fn write(&self, name: &str, extension: &str) -> TempClip {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "opencut-media-{name}-{}-{}.{extension}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // SAFETY: test-only; every FFmpeg call is checked.
        unsafe { self.encode(&path) };
        TempClip(path)
    }

    unsafe fn encode(&self, path: &Path) {
        // SAFETY (whole body): pointers come from FFmpeg and are checked.
        unsafe {
            let c_path = CString::new(path.to_str().unwrap()).unwrap();
            let mut output = ptr::null_mut();
            ok(ffi::avformat_alloc_output_context2(
                &mut output,
                ptr::null(),
                ptr::null(),
                c_path.as_ptr(),
            ));

            let codec = ffi::avcodec_find_encoder(ffi::AVCodecID_AV_CODEC_ID_MPEG4);
            assert!(!codec.is_null(), "no MPEG-4 encoder");
            let stream = ffi::avformat_new_stream(output, ptr::null());
            let mut context = ffi::avcodec_alloc_context3(codec);
            let c = &mut *context;
            c.width = self.width;
            c.height = self.height;
            c.time_base = ffi::AVRational {
                num: 1,
                den: self.fps,
            };
            c.framerate = ffi::AVRational {
                num: self.fps,
                den: 1,
            };
            c.pix_fmt = ffi::AVPixelFormat_AV_PIX_FMT_YUV420P;
            c.gop_size = self.gop;
            c.max_b_frames = self.b_frames;
            // Fixed quantiser, near lossless: stripes stay crisp.
            c.flags |= ffi::AV_CODEC_FLAG_QSCALE as i32;
            c.global_quality = 3 * ffi::FF_QP2LAMBDA as i32;
            if (*(*output).oformat).flags & ffi::AVFMT_GLOBALHEADER as i32 != 0 {
                c.flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            ok(ffi::avcodec_open2(context, codec, ptr::null_mut()));
            ok(ffi::avcodec_parameters_from_context(
                (*stream).codecpar,
                context,
            ));
            (*stream).time_base = c.time_base;
            (*stream).avg_frame_rate = c.framerate;

            ok(ffi::avio_open(
                &mut (*output).pb,
                c_path.as_ptr(),
                ffi::AVIO_FLAG_WRITE as i32,
            ));
            ok(ffi::avformat_write_header(output, ptr::null_mut()));

            let mut packet = ffi::av_packet_alloc();
            for index in 0..self.frames {
                let mut frame = Frame::new().unwrap();
                let f = &mut *frame.as_mut_ptr();
                f.width = self.width;
                f.height = self.height;
                f.format = ffi::AVPixelFormat_AV_PIX_FMT_YUV420P;
                ok(ffi::av_frame_get_buffer(frame.as_mut_ptr(), 0));
                self.paint(f, index);
                f.pts = i64::from(index);
                ok(ffi::avcodec_send_frame(context, frame.as_mut_ptr()));
                write_packets(context, output, stream, packet);
            }
            ok(ffi::avcodec_send_frame(context, ptr::null()));
            write_packets(context, output, stream, packet);

            ok(ffi::av_write_trailer(output));
            ffi::av_packet_free(&mut packet);
            ffi::avcodec_free_context(&mut context);
            ffi::avio_closep(&mut (*output).pb);
            ffi::avformat_free_context(output);
        }
    }

    unsafe fn paint(&self, frame: &mut ffi::AVFrame, index: u32) {
        let stripe = self.width / BITS as i32;
        // SAFETY: av_frame_get_buffer allocated the planes for this size.
        unsafe {
            for y in 0..self.height {
                let row = frame.data[0].offset((y * frame.linesize[0]) as isize);
                for x in 0..self.width {
                    let bit = (x / stripe).min(BITS as i32 - 1) as u32;
                    let on = index >> (BITS - 1 - bit) & 1 == 1;
                    *row.offset(x as isize) = if on { 235 } else { 16 };
                }
            }
            for plane in 1..3 {
                for y in 0..self.height / 2 {
                    let row = frame.data[plane].offset((y * frame.linesize[plane]) as isize);
                    ptr::write_bytes(row, 128, (self.width / 2) as usize);
                }
            }
        }
    }
}

unsafe fn write_packets(
    context: *mut ffi::AVCodecContext,
    output: *mut ffi::AVFormatContext,
    stream: *mut ffi::AVStream,
    packet: *mut ffi::AVPacket,
) {
    // SAFETY: all open/allocated by `encode`.
    unsafe {
        loop {
            let code = ffi::avcodec_receive_packet(context, packet);
            if code == ffi::AVERROR_EAGAIN || code == ffi::AVERROR_EOF {
                return;
            }
            ok(code);
            ffi::av_packet_rescale_ts(packet, (*context).time_base, (*stream).time_base);
            (*packet).stream_index = (*stream).index;
            ok(ffi::av_interleaved_write_frame(output, packet));
        }
    }
}

fn ok(code: i32) {
    assert!(
        code >= 0,
        "FFmpeg error {code}: {}",
        crate::error::error_string(code)
    );
}

/// The index a frame painted by [`Clip`] shows.
pub(super) fn read_index(image: &RgbaImage) -> u32 {
    let stripe = image.width / BITS;
    let y = image.height / 2;
    (0..BITS).fold(0, |index, bit| {
        let x = bit * stripe + stripe / 2;
        (index << 1) | u32::from(image.pixel(x, y)[0] > 128)
    })
}
