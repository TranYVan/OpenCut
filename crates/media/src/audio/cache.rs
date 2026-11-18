//! Per-file cache of waveform summaries (classic `WaveformCache`): in
//! memory, and optionally on disk so they survive restarts.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use super::waveform::{DEFAULT_BUCKET_SIZE, WaveformPeak, WaveformSummary, waveform_summary};
use crate::MediaError;

/// Which version of which file: its absolute path, size and modification
/// time. Editing or replacing the file changes the identity, so stale
/// summaries are never served.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub path: PathBuf,
    pub size: u64,
    /// Nanoseconds since the Unix epoch; 0 where the platform can't say.
    pub modified: u128,
}

impl FileIdentity {
    pub fn of(path: impl AsRef<Path>) -> Result<Self, MediaError> {
        let path = path.as_ref();
        let io_error = |source| MediaError::Io {
            path: path.to_owned(),
            source,
        };
        let path = fs::canonicalize(path).map_err(io_error)?;
        let metadata = fs::metadata(&path).map_err(io_error)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        Ok(Self {
            path,
            size: metadata.len(),
            modified,
        })
    }
}

/// Waveform summaries by file, computed on first request.
///
/// Thread-safe: share it behind an `Arc`. Two threads asking for the same
/// new file at once both decode it; the results are identical.
pub struct WaveformCache {
    bucket_size: u32,
    directory: Option<PathBuf>,
    memory: Mutex<HashMap<FileIdentity, Arc<WaveformSummary>>>,
}

impl Default for WaveformCache {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveformCache {
    /// An in-memory cache with [`DEFAULT_BUCKET_SIZE`] buckets.
    pub fn new() -> Self {
        Self {
            bucket_size: DEFAULT_BUCKET_SIZE,
            directory: None,
            memory: Mutex::default(),
        }
    }

    /// Also keeps summaries as files in `directory` (created when first
    /// written), one per media file. Disk errors only cost a recompute.
    #[must_use]
    pub fn with_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    /// Frames per bucket of the summaries this cache makes.
    #[must_use]
    pub fn with_bucket_size(mut self, bucket_size: u32) -> Self {
        self.bucket_size = bucket_size.max(1);
        self
    }

    pub fn bucket_size(&self) -> u32 {
        self.bucket_size
    }

    /// The summary of `path`'s audio: from memory, else from disk, else
    /// decoded now (which can take a while for long files).
    pub fn get(&self, path: impl AsRef<Path>) -> Result<Arc<WaveformSummary>, MediaError> {
        let path = path.as_ref();
        let identity = FileIdentity::of(path)?;
        if let Some(summary) = self.lock().get(&identity) {
            return Ok(Arc::clone(summary));
        }
        let summary = match self.read_disk(&identity) {
            Some(summary) => summary,
            None => {
                let summary = waveform_summary(path, self.bucket_size)?;
                self.write_disk(&identity, &summary);
                summary
            }
        };
        let summary = Arc::new(summary);
        self.lock().insert(identity, Arc::clone(&summary));
        Ok(summary)
    }

    /// The summary of `path` if it's in memory already; never decodes.
    pub fn cached(&self, path: impl AsRef<Path>) -> Option<Arc<WaveformSummary>> {
        let identity = FileIdentity::of(path).ok()?;
        self.lock().get(&identity).cloned()
    }

    /// Forgets `path`'s summaries, in memory and on disk.
    pub fn remove(&self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        self.lock().retain(|identity, _| identity.path != canonical);
        if let Some(file) = self.disk_path(&canonical) {
            let _ = fs::remove_file(file);
        }
    }

    /// Empties the in-memory cache (files on disk stay).
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<FileIdentity, Arc<WaveformSummary>>> {
        // The map is valid even if a holder panicked.
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One file per media path and bucket size: a changed media file
    /// overwrites its old entry instead of piling up new ones.
    fn disk_path(&self, canonical: &Path) -> Option<PathBuf> {
        let directory = self.directory.as_ref()?;
        let hash = fnv1a(canonical.as_os_str().as_encoded_bytes());
        Some(directory.join(format!("{hash:016x}-{}.ocwf", self.bucket_size)))
    }

    fn read_disk(&self, identity: &FileIdentity) -> Option<WaveformSummary> {
        let bytes = fs::read(self.disk_path(&identity.path)?).ok()?;
        let (stored, summary) = decode(&bytes)?;
        (stored == *identity && summary.bucket_size == self.bucket_size).then_some(summary)
    }

    fn write_disk(&self, identity: &FileIdentity, summary: &WaveformSummary) {
        let Some(file) = self.disk_path(&identity.path) else {
            return;
        };
        // Best effort: a cache that can't be written is only slower.
        let _ = write_atomically(&file, &encode(identity, summary));
    }
}

/// Writes through a temporary file so readers never see half a summary.
fn write_atomically(file: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(directory) = file.parent() {
        fs::create_dir_all(directory)?;
    }
    let temporary = file.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, file).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

/// 64-bit FNV-1a: stable across runs, platforms and Rust versions, unlike
/// `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

const MAGIC: &[u8; 4] = b"OCWF";
const VERSION: u16 = 1;

/// Little-endian: magic, version, the file identity (path as UTF-8 with a
/// u32 length, size, modified), then the summary (sample rate, channels,
/// total frames, bucket size, peak count, and min/max `f32` pairs).
fn encode(identity: &FileIdentity, summary: &WaveformSummary) -> Vec<u8> {
    let path = identity.path.to_string_lossy();
    let mut bytes = Vec::with_capacity(64 + path.len() + summary.peaks.len() * 8);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(path.len()).unwrap_or(u32::MAX).to_le_bytes());
    bytes.extend_from_slice(path.as_bytes());
    bytes.extend_from_slice(&identity.size.to_le_bytes());
    bytes.extend_from_slice(&identity.modified.to_le_bytes());
    bytes.extend_from_slice(&summary.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&summary.channels.to_le_bytes());
    bytes.extend_from_slice(&summary.total_frames.to_le_bytes());
    bytes.extend_from_slice(&summary.bucket_size.to_le_bytes());
    bytes.extend_from_slice(&(summary.peaks.len() as u64).to_le_bytes());
    for peak in &summary.peaks {
        bytes.extend_from_slice(&peak.min.to_le_bytes());
        bytes.extend_from_slice(&peak.max.to_le_bytes());
    }
    bytes
}

/// `None` for anything that isn't a complete version-1 summary.
fn decode(bytes: &[u8]) -> Option<(FileIdentity, WaveformSummary)> {
    let mut reader = Reader(bytes);
    if reader.take(4)? != MAGIC || u16::from_le_bytes(reader.array()?) != VERSION {
        return None;
    }
    let path_length = u32::from_le_bytes(reader.array()?) as usize;
    let path = std::str::from_utf8(reader.take(path_length)?).ok()?;
    let identity = FileIdentity {
        path: PathBuf::from(path),
        size: u64::from_le_bytes(reader.array()?),
        modified: u128::from_le_bytes(reader.array()?),
    };
    let sample_rate = u32::from_le_bytes(reader.array()?);
    let channels = u16::from_le_bytes(reader.array()?);
    let total_frames = u64::from_le_bytes(reader.array()?);
    let bucket_size = u32::from_le_bytes(reader.array()?);
    let count = usize::try_from(u64::from_le_bytes(reader.array()?)).ok()?;
    if reader.0.len() != count.checked_mul(8)? {
        return None;
    }
    let peaks = reader
        .0
        .chunks_exact(8)
        .map(|pair| WaveformPeak {
            min: f32::from_le_bytes(pair[..4].try_into().unwrap()),
            max: f32::from_le_bytes(pair[4..].try_into().unwrap()),
        })
        .collect();
    Some((
        identity,
        WaveformSummary {
            sample_rate,
            channels,
            total_frames,
            bucket_size,
            peaks,
        },
    ))
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.0.len() < count {
            return None;
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_bytes() {
        let identity = FileIdentity {
            path: PathBuf::from("/media/song.wav"),
            size: 1234,
            modified: 1_700_000_000_123_456_789,
        };
        let summary = WaveformSummary {
            sample_rate: 44_100,
            channels: 2,
            total_frames: 300,
            bucket_size: 128,
            peaks: vec![
                WaveformPeak {
                    min: -0.5,
                    max: 0.25,
                },
                WaveformPeak {
                    min: -1.0,
                    max: 1.0,
                },
                WaveformPeak { min: 0.0, max: 0.0 },
            ],
        };
        let bytes = encode(&identity, &summary);
        assert_eq!(decode(&bytes), Some((identity, summary)));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(b"nope"), None);
    }

    #[test]
    fn fnv1a_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
