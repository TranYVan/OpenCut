//! Media time primitives shared by every OpenCut crate.
//!
//! Ported from the classic app's `rust/crates/time` (tag `pre-rewrite`), minus
//! the wasm-bindgen wrappers. Time is an integer count of ticks
//! ([`TICKS_PER_SECOND`] per second), so every standard frame rate lands on a
//! whole number of ticks per frame and edits never accumulate float drift.

mod frame_rate;
mod media_time;
mod timecode;

pub use frame_rate::FrameRate;
pub use media_time::{MediaTime, TICKS_PER_SECOND};
pub use timecode::{TimeCodeFormat, format_timecode, guess_timecode_format, parse_timecode};
