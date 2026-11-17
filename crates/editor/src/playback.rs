use opencut_model::time::MediaTime;

/// Playhead position and play state. Advancing the playhead in real time is
/// the job of the playback clock (roadmap P2); this only holds the state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Playback {
    pub position: MediaTime,
    pub playing: bool,
}
