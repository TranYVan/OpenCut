use serde::{Deserialize, Serialize};

use crate::{
    frame_rate::FrameRate,
    media_time::{MediaTime, TICKS_PER_SECOND},
};

const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_MINUTE: i64 = 60;
const CENTISECONDS_PER_SECOND: i64 = 100;
const TICKS_PER_CENTISECOND: i64 = TICKS_PER_SECOND / CENTISECONDS_PER_SECOND;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeCodeFormat {
    #[serde(rename = "MM:SS")]
    MmSs,
    #[serde(rename = "HH:MM:SS")]
    HhMmSs,
    #[default]
    #[serde(rename = "HH:MM:SS:CS")]
    HhMmSsCs,
    #[serde(rename = "HH:MM:SS:FF")]
    HhMmSsFf,
}

/// Guesses the format from the number of `:`-separated numeric parts.
/// Four parts are read as frames (`HH:MM:SS:FF`), not centiseconds.
pub fn guess_timecode_format(time_code: &str) -> Option<TimeCodeFormat> {
    let time_code = time_code.trim();
    if time_code.is_empty() {
        return None;
    }

    let part_count = time_code.split(':').try_fold(0usize, |count, part| {
        part.parse::<u32>().ok().map(|_| count + 1)
    })?;

    match part_count {
        2 => Some(TimeCodeFormat::MmSs),
        3 => Some(TimeCodeFormat::HhMmSs),
        4 => Some(TimeCodeFormat::HhMmSsFf),
        _ => None,
    }
}

/// Formats a time. Negative times format as zero. `HH:MM:SS:FF` needs `rate`.
pub fn format_timecode(
    time: MediaTime,
    format: TimeCodeFormat,
    rate: Option<FrameRate>,
) -> Option<String> {
    let total_ticks = time.as_ticks().max(0);
    let hour_ticks = SECONDS_PER_HOUR * TICKS_PER_SECOND;
    let minute_ticks = SECONDS_PER_MINUTE * TICKS_PER_SECOND;

    let hours = total_ticks / hour_ticks;
    let minutes = (total_ticks % hour_ticks) / minute_ticks;
    let seconds = (total_ticks / TICKS_PER_SECOND) % SECONDS_PER_MINUTE;
    let second_ticks = total_ticks % TICKS_PER_SECOND;
    let centiseconds = second_ticks / TICKS_PER_CENTISECOND;

    match format {
        TimeCodeFormat::MmSs => Some(format!("{minutes:02}:{seconds:02}")),
        TimeCodeFormat::HhMmSs => Some(format!("{hours:02}:{minutes:02}:{seconds:02}")),
        TimeCodeFormat::HhMmSsCs => Some(format!(
            "{hours:02}:{minutes:02}:{seconds:02}:{centiseconds:02}"
        )),
        TimeCodeFormat::HhMmSsFf => {
            let frames = second_ticks / rate?.ticks_per_frame()?;
            Some(format!("{hours:02}:{minutes:02}:{seconds:02}:{frames:02}"))
        }
    }
}

/// Parses a timecode. Rejects out-of-range minutes, seconds, centiseconds or
/// frames. `HH:MM:SS:FF` needs `rate`.
pub fn parse_timecode(
    time_code: &str,
    format: TimeCodeFormat,
    rate: Option<FrameRate>,
) -> Option<MediaTime> {
    let time_code = time_code.trim();
    if time_code.is_empty() {
        return None;
    }

    let parts = time_code
        .split(':')
        .map(|part| part.parse::<u32>().ok().map(i64::from))
        .collect::<Option<Vec<_>>>()?;

    let whole_seconds = |hours: i64, minutes: i64, seconds: i64| -> Option<MediaTime> {
        if minutes >= SECONDS_PER_MINUTE || seconds >= SECONDS_PER_MINUTE {
            return None;
        }
        Some(MediaTime::from_ticks(
            (hours * SECONDS_PER_HOUR + minutes * SECONDS_PER_MINUTE + seconds) * TICKS_PER_SECOND,
        ))
    };

    match (format, parts.as_slice()) {
        (TimeCodeFormat::MmSs, &[minutes, seconds]) => {
            if seconds >= SECONDS_PER_MINUTE {
                return None;
            }
            Some(MediaTime::from_ticks(
                (minutes * SECONDS_PER_MINUTE + seconds) * TICKS_PER_SECOND,
            ))
        }
        (TimeCodeFormat::HhMmSs, &[hours, minutes, seconds]) => {
            whole_seconds(hours, minutes, seconds)
        }
        (TimeCodeFormat::HhMmSsCs, &[hours, minutes, seconds, centiseconds]) => {
            if centiseconds >= CENTISECONDS_PER_SECOND {
                return None;
            }
            Some(
                whole_seconds(hours, minutes, seconds)?
                    + MediaTime::from_ticks(centiseconds * TICKS_PER_CENTISECOND),
            )
        }
        (TimeCodeFormat::HhMmSsFf, &[hours, minutes, seconds, frames]) => {
            let rate = rate?;
            if frames >= i64::from(rate.frame_number_upper_bound()?) {
                return None;
            }
            Some(whole_seconds(hours, minutes, seconds)? + MediaTime::from_frame(frames, rate)?)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::frame_rate::FrameRate;
    use crate::media_time::MediaTime;

    use super::{TimeCodeFormat, format_timecode, guess_timecode_format, parse_timecode};

    fn seconds(value: f64) -> MediaTime {
        MediaTime::from_seconds_f64(value).unwrap()
    }

    #[test]
    fn formats_default_and_frame_timecodes() {
        assert_eq!(
            format_timecode(seconds(3723.45), TimeCodeFormat::default(), None),
            Some("01:02:03:45".to_string()),
        );
        assert_eq!(
            format_timecode(
                seconds(1.5),
                TimeCodeFormat::HhMmSsFf,
                Some(FrameRate::FPS_30)
            ),
            Some("00:00:01:15".to_string()),
        );
        assert_eq!(
            format_timecode(seconds(1.5), TimeCodeFormat::HhMmSsFf, None),
            None
        );
    }

    #[test]
    fn parses_timecodes() {
        assert_eq!(
            parse_timecode("01:05", TimeCodeFormat::MmSs, None),
            Some(seconds(65.0)),
        );
        assert_eq!(
            parse_timecode("01:02:03:45", TimeCodeFormat::HhMmSsCs, None),
            Some(seconds(3723.45)),
        );
        assert_eq!(
            parse_timecode(
                "00:00:01:15",
                TimeCodeFormat::HhMmSsFf,
                Some(FrameRate::FPS_30)
            ),
            Some(seconds(1.5)),
        );
        assert_eq!(
            parse_timecode(
                "00:00:01:30",
                TimeCodeFormat::HhMmSsFf,
                Some(FrameRate::FPS_30)
            ),
            None,
        );
        assert_eq!(parse_timecode("00:61", TimeCodeFormat::MmSs, None), None);
        assert_eq!(parse_timecode("01:05", TimeCodeFormat::HhMmSs, None), None);
    }

    #[test]
    fn guesses_timecode_formats() {
        assert_eq!(guess_timecode_format("01:05"), Some(TimeCodeFormat::MmSs));
        assert_eq!(
            guess_timecode_format("00:00:01"),
            Some(TimeCodeFormat::HhMmSs)
        );
        assert_eq!(
            guess_timecode_format("00:00:01:15"),
            Some(TimeCodeFormat::HhMmSsFf)
        );
        assert_eq!(guess_timecode_format("1:x"), None);
        assert_eq!(guess_timecode_format("  "), None);
    }
}
