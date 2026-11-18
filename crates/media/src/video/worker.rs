//! A thread that decodes frames and thumbnails so the UI thread never does.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use opencut_time::MediaTime;

use crate::MediaError;

use super::{
    RgbaImage, SharedFrameCache, Thumbnail, VideoDecoder, VideoFrame, poster_frame, thumbnail_strip,
};

/// How many files a worker keeps a decoder open for, so that playing or
/// scrubbing any of them continues without reopening and reseeking.
const OPEN_DECODERS: usize = 4;

/// Identifies a request in its [`DecodeResponse`].
pub type RequestId = u64;

/// What a request produced.
#[derive(Debug)]
pub enum DecodeOutput {
    /// The frame showing at the requested time (also put in the cache).
    Frame(Arc<VideoFrame>),
    Poster(RgbaImage),
    Strip(Vec<Thumbnail>),
    /// A newer frame request for the same media came in before this one
    /// started, so it was dropped (scrubbing only needs the latest).
    Superseded,
}

/// The answer to request `id`.
#[derive(Debug)]
pub struct DecodeResponse {
    pub id: RequestId,
    pub result: Result<DecodeOutput, MediaError>,
}

enum Job {
    Frame {
        media_id: Arc<str>,
        path: PathBuf,
        time: MediaTime,
    },
    Poster {
        path: PathBuf,
        max_width: u32,
        max_height: u32,
    },
    Strip {
        path: PathBuf,
        count: usize,
        tile_height: u32,
    },
    Release {
        media_id: Arc<str>,
    },
}

#[derive(Default)]
struct State {
    queue: VecDeque<(RequestId, Job)>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Runs decode requests in order on its own thread; answers arrive on the
/// [`Receiver`] that [`DecodeWorker::spawn`] returns. Frames go through the
/// shared [`super::FrameCache`]: hits are answered from it, decoded frames
/// added to it.
///
/// One worker decodes one thing at a time (FFmpeg already spreads each
/// decode over the cores); use separate workers for, say, the preview and
/// thumbnails so one doesn't wait on the other. Dropping the worker stops
/// it after the request in progress; queued ones get no answer.
pub struct DecodeWorker {
    shared: Arc<Shared>,
    responses: Sender<DecodeResponse>,
    next_id: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl DecodeWorker {
    pub fn spawn(cache: SharedFrameCache) -> (Self, Receiver<DecodeResponse>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let (responses, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("opencut-video-decode".into())
            .spawn({
                let shared = Arc::clone(&shared);
                let responses = responses.clone();
                move || run(&shared, &cache, &responses)
            })
            .expect("can spawn the decode thread");
        let worker = Self {
            shared,
            responses,
            next_id: AtomicU64::new(1),
            thread: Some(thread),
        };
        (worker, receiver)
    }

    /// The frame of `media_id` (the file at `path`) showing at `time`. An
    /// earlier frame request for the same media that hasn't started yet is
    /// answered [`DecodeOutput::Superseded`].
    pub fn request_frame(
        &self,
        media_id: &str,
        path: impl Into<PathBuf>,
        time: MediaTime,
    ) -> RequestId {
        let id = self.next_id();
        let media_id: Arc<str> = Arc::from(media_id);
        let mut state = self.shared.lock();
        let mut superseded = Vec::new();
        state.queue.retain(|(queued, job)| match job {
            Job::Frame {
                media_id: other, ..
            } if *other == media_id => {
                superseded.push(*queued);
                false
            }
            _ => true,
        });
        state.queue.push_back((
            id,
            Job::Frame {
                media_id,
                path: path.into(),
                time,
            },
        ));
        drop(state);
        self.shared.wake.notify_one();
        for queued in superseded {
            self.respond(queued, Ok(DecodeOutput::Superseded));
        }
        id
    }

    /// A [`poster_frame`] of `path`.
    pub fn request_poster(
        &self,
        path: impl Into<PathBuf>,
        max_width: u32,
        max_height: u32,
    ) -> RequestId {
        self.push(Job::Poster {
            path: path.into(),
            max_width,
            max_height,
        })
    }

    /// A [`thumbnail_strip`] of `path`.
    pub fn request_strip(
        &self,
        path: impl Into<PathBuf>,
        count: usize,
        tile_height: u32,
    ) -> RequestId {
        self.push(Job::Strip {
            path: path.into(),
            count,
            tile_height,
        })
    }

    /// Closes the worker's decoder for `media_id`, once the requests before
    /// this are done. Answers nothing. The cache is the caller's to clear.
    pub fn release(&self, media_id: &str) {
        self.push(Job::Release {
            media_id: Arc::from(media_id),
        });
    }

    fn push(&self, job: Job) -> RequestId {
        let id = self.next_id();
        self.shared.lock().queue.push_back((id, job));
        self.shared.wake.notify_one();
        id
    }

    fn next_id(&self) -> RequestId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn respond(&self, id: RequestId, result: Result<DecodeOutput, MediaError>) {
        // Nobody listening any more is fine.
        let _ = self.responses.send(DecodeResponse { id, result });
    }
}

impl Drop for DecodeWorker {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: &Shared, cache: &SharedFrameCache, responses: &Sender<DecodeResponse>) {
    // Most recently used last.
    let mut decoders: Vec<(Arc<str>, VideoDecoder)> = Vec::new();
    loop {
        let (id, job) = {
            let mut state = shared.lock();
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(next) = state.queue.pop_front() {
                    break next;
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let result = match job {
            Job::Frame {
                media_id,
                path,
                time,
            } => decode_frame(&mut decoders, cache, media_id, path, time).map(DecodeOutput::Frame),
            Job::Poster {
                path,
                max_width,
                max_height,
            } => poster_frame(path, max_width, max_height).map(DecodeOutput::Poster),
            Job::Strip {
                path,
                count,
                tile_height,
            } => thumbnail_strip(path, count, tile_height).map(DecodeOutput::Strip),
            Job::Release { media_id } => {
                decoders.retain(|(open, _)| *open != media_id);
                continue;
            }
        };
        let _ = responses.send(DecodeResponse { id, result });
    }
}

fn decode_frame(
    decoders: &mut Vec<(Arc<str>, VideoDecoder)>,
    cache: &SharedFrameCache,
    media_id: Arc<str>,
    path: PathBuf,
    time: MediaTime,
) -> Result<Arc<VideoFrame>, MediaError> {
    let position = decoders
        .iter()
        .position(|(open, decoder)| *open == media_id && decoder.path() == path);
    let mut entry = match position {
        Some(position) => decoders.remove(position),
        None => {
            decoders.retain(|(open, _)| *open != media_id);
            if decoders.len() >= OPEN_DECODERS {
                decoders.remove(0);
            }
            (Arc::clone(&media_id), VideoDecoder::open(&path)?)
        }
    };
    let result = frame_through_cache(&mut entry.1, cache, &media_id, &path, time);
    decoders.push(entry);
    result
}

fn frame_through_cache(
    decoder: &mut VideoDecoder,
    cache: &SharedFrameCache,
    media_id: &str,
    path: &std::path::Path,
    time: MediaTime,
) -> Result<Arc<VideoFrame>, MediaError> {
    let index = decoder.info().frame_index_at(time);
    if let Some(hit) = lock(cache).get(media_id, index) {
        return Ok(hit);
    }
    let frame = Arc::new(decoder.frame_at(time)?.ok_or_else(|| MediaError::NoVideo {
        path: path.to_owned(),
    })?);
    lock(cache).insert(media_id, Arc::clone(&frame));
    Ok(frame)
}

fn lock(cache: &SharedFrameCache) -> MutexGuard<'_, super::FrameCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}
