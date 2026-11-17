use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

use crate::frame_rate::FrameRate;

pub const TICKS_PER_SECOND: i64 = 120_000;
#[allow(clippy::cast_precision_loss)]
const TICKS_PER_SECOND_F64: f64 = TICKS_PER_SECOND as f64;

/// A point or span on the timeline, stored as whole ticks.
///
/// Serializes as a bare integer, which matches how the classic app stored it.
#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
#[serde(transparent)]
pub struct MediaTime(i64);

impl MediaTime {
    pub const ZERO: Self = Self(0);
    pub const ONE_TICK: Self = Self(1);

    pub const fn from_ticks(ticks: i64) -> Self {
        Self(ticks)
    }

    pub const fn as_ticks(self) -> i64 {
        self.0
    }

    pub fn from_seconds_f64(seconds: f64) -> Option<Self> {
        Self::from_ticks_f64(seconds * TICKS_PER_SECOND_F64)
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn to_seconds_f64(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND_F64
    }

    /// Multiplies by a float factor (e.g. a playback rate), rounding to the
    /// nearest tick. `None` when the result is not finite or overflows.
    #[allow(clippy::cast_precision_loss)]
    pub fn mul_f64_round(self, factor: f64) -> Option<Self> {
        Self::from_ticks_f64(self.0 as f64 * factor)
    }

    /// Divides by a float factor, rounding to the nearest tick.
    #[allow(clippy::cast_precision_loss)]
    pub fn div_f64_round(self, divisor: f64) -> Option<Self> {
        Self::from_ticks_f64(self.0 as f64 / divisor)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn from_ticks_f64(ticks: f64) -> Option<Self> {
        let rounded = ticks.round();
        // `i64::MAX as f64` rounds up to 2^63, so compare with `<`.
        if !rounded.is_finite() || rounded < i64::MIN as f64 || rounded >= i64::MAX as f64 {
            return None;
        }
        Some(Self(rounded as i64))
    }

    pub fn from_frame(frame: i64, rate: FrameRate) -> Option<Self> {
        let ticks_per_frame = rate.ticks_per_frame()?;
        Some(Self(frame.checked_mul(ticks_per_frame)?))
    }

    pub fn to_frame_round(self, rate: FrameRate) -> Option<i64> {
        let ticks_per_frame = rate.ticks_per_frame()?;
        let remainder = self.0.rem_euclid(ticks_per_frame);
        let floor = self.0.div_euclid(ticks_per_frame);
        if remainder * 2 >= ticks_per_frame {
            Some(floor + 1)
        } else {
            Some(floor)
        }
    }

    pub fn to_frame_floor(self, rate: FrameRate) -> Option<i64> {
        let ticks_per_frame = rate.ticks_per_frame()?;
        Some(self.0.div_euclid(ticks_per_frame))
    }

    pub fn round_to_frame(self, rate: FrameRate) -> Option<Self> {
        Self::from_frame(self.to_frame_round(rate)?, rate)
    }

    pub fn floor_to_frame(self, rate: FrameRate) -> Option<Self> {
        let ticks_per_frame = rate.ticks_per_frame()?;
        Some(Self(self.0.div_euclid(ticks_per_frame) * ticks_per_frame))
    }

    pub fn is_frame_aligned(self, rate: FrameRate) -> Option<bool> {
        let ticks_per_frame = rate.ticks_per_frame()?;
        Some(self.0.rem_euclid(ticks_per_frame) == 0)
    }

    /// Start time of the last frame inside a span of this length.
    pub fn last_frame_time(self, rate: FrameRate) -> Option<Self> {
        if self <= Self::ZERO {
            return Some(Self::ZERO);
        }

        Self::from_ticks(self.0 - 1).floor_to_frame(rate)
    }

    /// Rounds to the nearest frame and clamps into `0..=duration`.
    pub fn snapped_seek_time(self, duration: Self, rate: FrameRate) -> Option<Self> {
        let snapped = self.round_to_frame(rate)?;
        Some(snapped.clamp(Self::ZERO, duration.max(Self::ZERO)))
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }
}

impl Add for MediaTime {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for MediaTime {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for MediaTime {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for MediaTime {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Neg for MediaTime {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Mul<i64> for MediaTime {
    type Output = Self;

    fn mul(self, rhs: i64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Div<i64> for MediaTime {
    type Output = Self;

    fn div(self, rhs: i64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

#[cfg(test)]
mod tests {
    use crate::frame_rate::FrameRate;

    use super::{MediaTime, TICKS_PER_SECOND};

    #[test]
    fn converts_between_seconds_and_ticks() {
        assert_eq!(
            MediaTime::from_seconds_f64(1.5),
            Some(MediaTime::from_ticks(180_000))
        );
        assert!((MediaTime::from_ticks(180_000).to_seconds_f64() - 1.5).abs() < f64::EPSILON);
        assert_eq!(TICKS_PER_SECOND, 120_000);
    }

    #[test]
    fn rejects_non_finite_seconds() {
        assert_eq!(MediaTime::from_seconds_f64(f64::NAN), None);
        assert_eq!(MediaTime::from_seconds_f64(f64::INFINITY), None);
        assert_eq!(MediaTime::from_seconds_f64(f64::NEG_INFINITY), None);
        assert_eq!(MediaTime::from_seconds_f64(1e300), None);
    }

    #[test]
    fn scales_by_float_factors() {
        let time = MediaTime::from_ticks(1_000);
        assert_eq!(time.mul_f64_round(2.0), Some(MediaTime::from_ticks(2_000)));
        assert_eq!(
            time.mul_f64_round(1.0 / 3.0),
            Some(MediaTime::from_ticks(333))
        );
        assert_eq!(time.div_f64_round(4.0), Some(MediaTime::from_ticks(250)));
        assert_eq!(time.div_f64_round(0.0), None);
    }

    #[test]
    fn snaps_to_the_nearest_frame() {
        let rate = FrameRate::FPS_30;
        let time = MediaTime::from_seconds_f64(1.26).unwrap();

        assert_eq!(time.to_frame_round(rate), Some(38));
        assert_eq!(
            time.round_to_frame(rate),
            Some(MediaTime::from_ticks(152_000))
        );
    }

    #[test]
    fn floors_to_frame() {
        let rate = FrameRate::FPS_30;
        let ticks_per_frame = 4_000;
        let time = MediaTime::from_ticks(ticks_per_frame * 5 + 1);

        assert_eq!(time.to_frame_floor(rate), Some(5));
        assert_eq!(time.to_frame_round(rate), Some(5));

        let almost_next = MediaTime::from_ticks(ticks_per_frame * 5 + ticks_per_frame / 2);
        assert_eq!(almost_next.to_frame_floor(rate), Some(5));
        assert_eq!(almost_next.to_frame_round(rate), Some(6));
    }

    #[test]
    fn computes_last_frame_time_and_snapped_seek_time() {
        let rate = FrameRate::new(5, 1);
        let duration = MediaTime::from_seconds_f64(10.0).unwrap();

        assert_eq!(
            duration.last_frame_time(rate),
            Some(MediaTime::from_seconds_f64(9.8).unwrap()),
        );
        assert_eq!(
            MediaTime::from_seconds_f64(10.0)
                .unwrap()
                .snapped_seek_time(duration, rate),
            Some(MediaTime::from_seconds_f64(10.0).unwrap()),
        );
        assert_eq!(
            MediaTime::from_seconds_f64(-1.0)
                .unwrap()
                .snapped_seek_time(duration, rate),
            Some(MediaTime::ZERO),
        );
    }

    #[test]
    fn serializes_as_a_bare_integer() {
        let time = MediaTime::from_ticks(42);
        assert_eq!(serde_json::to_string(&time).unwrap(), "42");
        assert_eq!(serde_json::from_str::<MediaTime>("42").unwrap(), time);
    }
}
