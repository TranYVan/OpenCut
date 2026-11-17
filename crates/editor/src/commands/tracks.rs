use opencut_model::{Project, Track, TrackKind, new_id};

use crate::command::{Command, Outcome, current_tracks};
use crate::error::EditError;
use crate::placement::{default_insert_index, insert_track};

/// Adds an empty track. Without an index it goes where
/// [`default_insert_index`] says.
pub struct AddTrack {
    kind: TrackKind,
    display_index: Option<usize>,
    track_id: String,
}

impl AddTrack {
    pub fn new(kind: TrackKind) -> Self {
        Self {
            kind,
            display_index: None,
            track_id: new_id(),
        }
    }

    #[must_use]
    pub fn at(mut self, display_index: usize) -> Self {
        self.display_index = Some(display_index);
        self
    }

    /// Id the new track gets, known before the command runs.
    pub fn track_id(&self) -> &str {
        &self.track_id
    }
}

impl Command for AddTrack {
    fn label(&self) -> &'static str {
        "Add track"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let index = self
            .display_index
            .unwrap_or_else(|| default_insert_index(tracks, self.kind));
        insert_track(tracks, Track::new(self.track_id.clone(), self.kind), index);
        Ok(Outcome::unchanged())
    }
}

/// Removes a track and everything on it. The main track stays.
pub struct RemoveTrack {
    pub track_id: String,
}

impl Command for RemoveTrack {
    fn label(&self) -> &'static str {
        "Remove track"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        if tracks.is_main(&self.track_id) {
            return Err(EditError::CannotRemoveMainTrack);
        }
        let before = tracks.overlay.len() + tracks.audio.len();
        tracks.overlay.retain(|track| track.id != self.track_id);
        tracks.audio.retain(|track| track.id != self.track_id);
        if tracks.overlay.len() + tracks.audio.len() == before {
            return Err(EditError::TrackNotFound(self.track_id.clone()));
        }
        // Selected elements on the removed track no longer exist.
        Ok(Outcome::unchanged())
    }
}

/// Flips a video or audio track's mute.
pub struct ToggleTrackMute {
    pub track_id: String,
}

impl Command for ToggleTrackMute {
    fn label(&self) -> &'static str {
        "Toggle track mute"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let track = current_tracks(project)?
            .track_mut(&self.track_id)
            .ok_or_else(|| EditError::TrackNotFound(self.track_id.clone()))?;
        if !track.kind.can_have_audio() {
            return Err(EditError::CannotMute(track.kind));
        }
        track.muted = Some(!track.is_muted());
        Ok(Outcome::unchanged())
    }
}

/// Flips a non-audio track's visibility.
pub struct ToggleTrackVisibility {
    pub track_id: String,
}

impl Command for ToggleTrackVisibility {
    fn label(&self) -> &'static str {
        "Toggle track visibility"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let track = current_tracks(project)?
            .track_mut(&self.track_id)
            .ok_or_else(|| EditError::TrackNotFound(self.track_id.clone()))?;
        if !track.kind.can_be_hidden() {
            return Err(EditError::CannotHide(track.kind));
        }
        track.hidden = Some(!track.is_hidden());
        Ok(Outcome::unchanged())
    }
}
