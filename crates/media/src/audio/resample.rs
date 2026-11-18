//! Converts decoded frames, in whatever sample format, rate and channel
//! layout the codec produces, into interleaved `f32` at the output format
//! (libswresample).

use std::ptr;

use super::AudioFormat;
use super::raw::ChannelLayout;
use crate::MediaError;
use crate::ffi;

pub(super) struct Resampler {
    context: *mut ffi::SwrContext,
    /// What swresample makes; mono when `duplicate_mono`.
    output_layout: ChannelLayout,
    output_rate: u32,
    /// Mono into stereo: swresample's remix would put the mono signal at
    /// -3 dB in each side; like the classic app (Web Audio), copy it to
    /// both at full level instead.
    duplicate_mono: bool,
    /// What the context was set up for, to notice when a stream changes
    /// format mid-way (it happens in some broadcast captures).
    input_layout: ChannelLayout,
    input_format: ffi::AVSampleFormat,
    input_rate: i32,
}

// SAFETY: an SwrContext may move between threads; it's only used through
// `&mut self`.
unsafe impl Send for Resampler {}

impl Resampler {
    pub(super) fn new(
        output: AudioFormat,
        input_layout: &ffi::AVChannelLayout,
        input_format: ffi::AVSampleFormat,
        input_rate: i32,
    ) -> Result<Self, MediaError> {
        let input_layout = ChannelLayout::copy_of(input_layout)?;
        let duplicate_mono = input_layout.channels() == 1 && output.channels == 2;
        let output_layout =
            ChannelLayout::default_for(if duplicate_mono { 1 } else { output.channels });
        let mut context = ptr::null_mut();
        // SAFETY: a null context makes swresample allocate one; the layouts
        // are valid for the call.
        let code = unsafe {
            ffi::swr_alloc_set_opts2(
                &mut context,
                &output_layout.0,
                ffi::AVSampleFormat_AV_SAMPLE_FMT_FLT,
                output_rate_i32(output.sample_rate),
                &input_layout.0,
                input_format,
                input_rate,
                0,
                ptr::null_mut(),
            )
        };
        let resampler = Self {
            context,
            output_layout,
            output_rate: output.sample_rate,
            duplicate_mono,
            input_layout,
            input_format,
            input_rate,
        };
        if code < 0 {
            return Err(MediaError::ffmpeg("swr_alloc_set_opts2", code));
        }
        // SAFETY: context was just configured.
        let code = unsafe { ffi::swr_init(resampler.context) };
        if code < 0 {
            return Err(MediaError::ffmpeg("swr_init", code));
        }
        Ok(resampler)
    }

    /// Whether frames with this layout, format and rate fit the context.
    pub(super) fn accepts(
        &self,
        layout: &ffi::AVChannelLayout,
        format: ffi::AVSampleFormat,
        rate: i32,
    ) -> bool {
        format == self.input_format
            && rate == self.input_rate
            && (layout.order == ffi::AVChannelOrder_AV_CHANNEL_ORDER_UNSPEC
                && layout.nb_channels == i32::from(self.input_layout.channels())
                || self.input_layout.same_as(layout))
    }

    /// Converts `frame`'s samples and appends them to `out`; returns the
    /// number of frames appended.
    ///
    /// # Safety
    /// `frame` must be a valid decoded audio frame that [`accepts`] this
    /// resampler.
    pub(super) unsafe fn convert(
        &mut self,
        frame: *const ffi::AVFrame,
        out: &mut Vec<f32>,
    ) -> Result<usize, MediaError> {
        // SAFETY: the caller guarantees frame is valid.
        let (data, count) = unsafe { ((*frame).extended_data, (*frame).nb_samples) };
        // SAFETY: extended_data holds one plane pointer per channel (or
        // one for packed formats), each with nb_samples samples.
        unsafe { self.run(data as *const *const u8, count, out) }
    }

    /// Drains the samples swresample still holds (its filter delay) into
    /// `out`; returns the number of frames appended.
    pub(super) fn flush(&mut self, out: &mut Vec<f32>) -> Result<usize, MediaError> {
        // SAFETY: a null input with no samples flushes.
        unsafe { self.run(ptr::null(), 0, out) }
    }

    unsafe fn run(
        &mut self,
        input: *const *const u8,
        input_count: i32,
        out: &mut Vec<f32>,
    ) -> Result<usize, MediaError> {
        // SAFETY: context is initialized.
        let capacity = unsafe { ffi::swr_get_out_samples(self.context, input_count) };
        if capacity < 0 {
            return Err(MediaError::ffmpeg("swr_get_out_samples", capacity));
        }
        if capacity == 0 && input_count == 0 {
            return Ok(0);
        }
        // A little headroom: the estimate can be one short with rounding.
        let capacity = capacity + 16;
        let channels = usize::from(self.output_layout.channels());
        let start = out.len();
        out.resize(start + capacity as usize * channels, 0.0);
        let output = out[start..].as_mut_ptr().cast::<u8>();
        // SAFETY: output has room for `capacity` interleaved frames; the
        // caller guarantees input matches the configured input format.
        let written =
            unsafe { ffi::swr_convert(self.context, &output, capacity, input, input_count) };
        if written < 0 {
            out.truncate(start);
            return Err(MediaError::ffmpeg("swr_convert", written));
        }
        let written = written as usize;
        out.truncate(start + written * channels);
        if self.duplicate_mono {
            // Expand in place from the back: mono sample i lands at 2i, 2i+1.
            out.resize(start + written * 2, 0.0);
            for index in (0..written).rev() {
                let sample = out[start + index];
                out[start + index * 2] = sample;
                out[start + index * 2 + 1] = sample;
            }
        }
        Ok(written)
    }

    pub(super) fn output_format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: self.output_rate,
            channels: if self.duplicate_mono {
                2
            } else {
                self.output_layout.channels()
            },
        }
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: frees the context (null is fine) and nulls the pointer.
        unsafe { ffi::swr_free(&mut self.context) };
    }
}

fn output_rate_i32(rate: u32) -> i32 {
    i32::try_from(rate).unwrap_or(i32::MAX)
}
