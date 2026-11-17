//! Timestamps as JavaScript's `Date.toISOString()` writes them
//! (`2026-05-02T09:15:00.000Z`), so files stay byte-compatible with classic.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(time: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&time.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
    DateTime::<Utc>::deserialize(deserializer)
}
