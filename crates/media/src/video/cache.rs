//! Decoded frames kept for reuse, least recently used dropped first once
//! they take more memory than allowed.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use super::VideoFrame;

/// A [`FrameCache`] shared between threads, as [`super::DecodeWorker`]
/// takes it.
pub type SharedFrameCache = Arc<Mutex<FrameCache>>;

struct Entry {
    frame: Arc<VideoFrame>,
    /// When it was last used; its key in `FrameCache::order`.
    stamp: u64,
}

/// An LRU cache of frames keyed by media id and frame index, bounded by the
/// bytes of pixel data it holds.
///
/// Frames are stored at whatever size they were decoded at; use one cache
/// per output size.
pub struct FrameCache {
    capacity: usize,
    used: usize,
    clock: u64,
    media: HashMap<Arc<str>, HashMap<i64, Entry>>,
    /// Use stamp → key, oldest first.
    order: BTreeMap<u64, (Arc<str>, i64)>,
}

impl FrameCache {
    /// A cache holding at most `capacity` bytes of pixels (a 1080p RGBA
    /// frame is about 8 MB).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            used: 0,
            clock: 0,
            media: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    /// A new cache wrapped for sharing.
    pub fn shared(capacity: usize) -> SharedFrameCache {
        Arc::new(Mutex::new(Self::new(capacity)))
    }

    /// Frame `index` of `media_id`, marking it as just used.
    pub fn get(&mut self, media_id: &str, index: i64) -> Option<Arc<VideoFrame>> {
        self.clock += 1;
        let stamp = self.clock;
        let entry = self.media.get_mut(media_id)?.get_mut(&index)?;
        let key = self
            .order
            .remove(&entry.stamp)
            .expect("every entry is ordered");
        entry.stamp = stamp;
        self.order.insert(stamp, key);
        Some(Arc::clone(&entry.frame))
    }

    /// Whether frame `index` of `media_id` is cached, without touching its
    /// place in the LRU order.
    pub fn contains(&self, media_id: &str, index: i64) -> bool {
        self.media
            .get(media_id)
            .is_some_and(|frames| frames.contains_key(&index))
    }

    /// Caches `frame` as frame `frame.index` of `media_id`, replacing what
    /// was there and evicting the least recently used frames to fit. A
    /// frame larger than the whole capacity isn't kept.
    pub fn insert(&mut self, media_id: &str, frame: Arc<VideoFrame>) {
        let index = frame.index;
        self.remove(media_id, index);
        let size = frame.image.byte_len();
        if size > self.capacity {
            return;
        }
        self.evict_to(self.capacity - size);

        self.clock += 1;
        let media_id: Arc<str> = match self.media.get_key_value(media_id) {
            Some((existing, _)) => Arc::clone(existing),
            None => Arc::from(media_id),
        };
        self.order
            .insert(self.clock, (Arc::clone(&media_id), index));
        self.media.entry(media_id).or_default().insert(
            index,
            Entry {
                frame,
                stamp: self.clock,
            },
        );
        self.used += size;
    }

    /// Drops frame `index` of `media_id`, if cached.
    pub fn remove(&mut self, media_id: &str, index: i64) -> Option<Arc<VideoFrame>> {
        let frames = self.media.get_mut(media_id)?;
        let entry = frames.remove(&index)?;
        if frames.is_empty() {
            self.media.remove(media_id);
        }
        self.order.remove(&entry.stamp);
        self.used -= entry.frame.image.byte_len();
        Some(entry.frame)
    }

    /// Drops every frame of `media_id`, e.g. when the media is removed.
    pub fn remove_media(&mut self, media_id: &str) {
        if let Some(frames) = self.media.remove(media_id) {
            for entry in frames.into_values() {
                self.order.remove(&entry.stamp);
                self.used -= entry.frame.image.byte_len();
            }
        }
    }

    pub fn clear(&mut self) {
        self.media.clear();
        self.order.clear();
        self.used = 0;
    }

    /// Changes the byte limit, evicting to fit it.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.evict_to(capacity);
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes of pixels held.
    pub fn bytes_used(&self) -> usize {
        self.used
    }

    /// Frames held.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Evicts least recently used frames until at most `limit` bytes are
    /// held.
    fn evict_to(&mut self, limit: usize) {
        while self.used > limit {
            let Some((_, (media_id, index))) = self.order.pop_first() else {
                break;
            };
            let frames = self
                .media
                .get_mut(&media_id)
                .expect("ordered entries exist");
            let entry = frames.remove(&index).expect("ordered entries exist");
            if frames.is_empty() {
                self.media.remove(&media_id);
            }
            self.used -= entry.frame.image.byte_len();
        }
    }
}

#[cfg(test)]
mod tests {
    use opencut_time::MediaTime;

    use super::super::RgbaImage;
    use super::*;

    fn frame(index: i64, bytes: usize) -> Arc<VideoFrame> {
        Arc::new(VideoFrame {
            index,
            time: MediaTime::ZERO,
            duration: MediaTime::ZERO,
            image: RgbaImage {
                width: 1,
                height: 1,
                data: vec![0; bytes],
            },
        })
    }

    #[test]
    fn evicts_least_recently_used_beyond_capacity() {
        let mut cache = FrameCache::new(300);
        cache.insert("a", frame(0, 100));
        cache.insert("a", frame(1, 100));
        cache.insert("b", frame(0, 100));
        assert_eq!(cache.bytes_used(), 300);

        // Touch a/0 so a/1 is now the oldest.
        assert!(cache.get("a", 0).is_some());
        cache.insert("b", frame(1, 100));

        assert!(cache.contains("a", 0));
        assert!(!cache.contains("a", 1));
        assert!(cache.contains("b", 0));
        assert!(cache.contains("b", 1));
        assert_eq!((cache.len(), cache.bytes_used()), (3, 300));
    }

    #[test]
    fn replacing_a_frame_keeps_the_byte_count_right() {
        let mut cache = FrameCache::new(1000);
        cache.insert("a", frame(5, 100));
        cache.insert("a", frame(5, 250));
        assert_eq!((cache.len(), cache.bytes_used()), (1, 250));
        assert_eq!(cache.get("a", 5).unwrap().image.byte_len(), 250);
    }

    #[test]
    fn skips_frames_larger_than_capacity() {
        let mut cache = FrameCache::new(50);
        cache.insert("a", frame(0, 40));
        cache.insert("a", frame(1, 60));
        assert!(cache.contains("a", 0));
        assert!(!cache.contains("a", 1));
    }

    #[test]
    fn removes_a_whole_media_and_shrinks() {
        let mut cache = FrameCache::new(1000);
        for index in 0..4 {
            cache.insert("a", frame(index, 100));
        }
        cache.insert("b", frame(0, 100));
        cache.remove_media("a");
        assert_eq!((cache.len(), cache.bytes_used()), (1, 100));

        cache.insert("c", frame(0, 100));
        cache.set_capacity(100);
        assert!(!cache.contains("b", 0));
        assert!(cache.contains("c", 0));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes_used(), 0);
    }
}
