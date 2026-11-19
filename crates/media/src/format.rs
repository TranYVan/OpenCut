//! An opened media file: a safe owner of `AVFormatContext`, shared by
//! probing and the decoders.

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Once;

use crate::MediaError;
use crate::error::error_string;
use crate::ffi;

/// FFmpeg logs to stderr by default; only real errors are worth that.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: sets a global; FFmpeg reads it under its own lock.
        unsafe { ffi::av_log_set_level(ffi::AV_LOG_ERROR as i32) };
    });
}

/// A demuxer opened on a file, with stream info read.
pub(crate) struct Input {
    context: *mut ffi::AVFormatContext,
    path: PathBuf,
}

// SAFETY: an AVFormatContext may move between threads; it's only used from
// one at a time, which `&mut self` on the methods that touch it ensures.
unsafe impl Send for Input {}

impl Input {
    pub(crate) fn open(path: &Path) -> Result<Self, MediaError> {
        init();
        let c_path = path_to_cstring(path).ok_or_else(|| MediaError::Open {
            path: path.to_owned(),
            message: "path contains a NUL byte".into(),
        })?;

        let mut context = ptr::null_mut();
        // SAFETY: a null context makes FFmpeg allocate one; on failure it
        // frees it and leaves the pointer null.
        let code = unsafe {
            ffi::avformat_open_input(&mut context, c_path.as_ptr(), ptr::null(), ptr::null_mut())
        };
        if code < 0 {
            return Err(MediaError::Open {
                path: path.to_owned(),
                message: error_string(code),
            });
        }
        let input = Self {
            context,
            path: path.to_owned(),
        };

        // SAFETY: context is open.
        let code = unsafe { ffi::avformat_find_stream_info(input.context, ptr::null_mut()) };
        if code < 0 {
            return Err(MediaError::Open {
                path: path.to_owned(),
                message: error_string(code),
            });
        }
        Ok(input)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code, reason = "for the decoders")]
    pub(crate) fn as_ptr(&self) -> *const ffi::AVFormatContext {
        self.context
    }

    #[allow(dead_code, reason = "for the decoders")]
    pub(crate) fn as_mut_ptr(&mut self) -> *mut ffi::AVFormatContext {
        self.context
    }

    /// The demuxer's short name(s), e.g. `mov,mp4,m4a,3gp,3g2,mj2` or `png_pipe`.
    pub(crate) fn format_name(&self) -> String {
        // SAFETY: an open context always has iformat, with a static name.
        unsafe {
            CStr::from_ptr((*(*self.context).iformat).name)
                .to_string_lossy()
                .into_owned()
        }
    }

    /// Duration of the whole file in seconds, when the container knows it.
    pub(crate) fn duration(&self) -> Option<f64> {
        // SAFETY: context is open.
        let duration = unsafe { (*self.context).duration };
        (duration != ffi::AV_NOPTS_VALUE && duration > 0)
            .then(|| duration as f64 / f64::from(ffi::AV_TIME_BASE))
    }

    pub(crate) fn streams(&self) -> &[*mut ffi::AVStream] {
        // SAFETY: streams holds nb_streams valid pointers for the context's
        // lifetime, which the returned borrow is tied to.
        unsafe {
            let context = &*self.context;
            if context.streams.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts(context.streams, context.nb_streams as usize)
            }
        }
    }

    /// `av_find_best_stream`: the index of the stream FFmpeg would pick for
    /// `media_type`, if there is one.
    pub(crate) fn best_stream(&self, media_type: ffi::AVMediaType) -> Option<usize> {
        // SAFETY: context is open; no decoder is asked for.
        let index = unsafe {
            ffi::av_find_best_stream(self.context, media_type, -1, -1, ptr::null_mut(), 0)
        };
        usize::try_from(index).ok()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: context came from avformat_open_input; this frees it and
        // nulls the pointer.
        unsafe { ffi::avformat_close_input(&mut self.context) };
    }
}

/// FFmpeg takes UTF-8 paths on Windows and raw bytes elsewhere.
pub(crate) fn path_to_cstring(path: &Path) -> Option<CString> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    CString::new(bytes).ok()
}
