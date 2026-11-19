use opencut_model::time::MediaTime;
use opencut_model::{Bookmark, Project, Scene};

use crate::command::{Command, Outcome};
use crate::error::EditError;

/// `time` rounded to a frame, as classic `getFrameTime`.
fn frame_time(project: &Project, time: MediaTime) -> MediaTime {
    time.round_to_frame(project.settings.fps).unwrap_or(time)
}

fn current_scene(project: &mut Project) -> Result<&mut Scene, EditError> {
    project.current_scene_mut().ok_or(EditError::NoCurrentScene)
}

fn bookmark_index(scene: &Scene, time: MediaTime) -> Result<usize, EditError> {
    scene
        .bookmarks
        .iter()
        .position(|bookmark| bookmark.time == time)
        .ok_or(EditError::BookmarkNotFound(time))
}

/// Adds a bookmark at `time` (rounded to a frame), or removes the one there
/// (classic `ToggleBookmarkCommand`).
pub struct ToggleBookmark {
    pub time: MediaTime,
}

impl Command for ToggleBookmark {
    fn label(&self) -> &'static str {
        "Toggle bookmark"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let time = frame_time(project, self.time);
        let scene = current_scene(project)?;
        if let Ok(index) = bookmark_index(scene, time) {
            scene.bookmarks.remove(index);
        } else {
            scene.bookmarks.push(Bookmark {
                time,
                note: None,
                color: None,
                duration: None,
            });
            scene.bookmarks.sort_by_key(|bookmark| bookmark.time);
        }
        Ok(Outcome::unchanged())
    }
}

/// Moves the current scene's bookmark at `from` to `to`, both rounded to a
/// frame, keeping its note, colour and duration (classic
/// `MoveBookmarkCommand`). Refuses to land on another bookmark.
pub struct MoveBookmark {
    pub from: MediaTime,
    pub to: MediaTime,
}

impl Command for MoveBookmark {
    fn label(&self) -> &'static str {
        "Move bookmark"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let from = frame_time(project, self.from);
        let to = frame_time(project, self.to);
        if to < MediaTime::ZERO {
            return Err(EditError::InvalidTiming);
        }
        let scene = current_scene(project)?;
        let index = bookmark_index(scene, from)?;
        if from != to && bookmark_index(scene, to).is_ok() {
            return Err(EditError::BookmarkExists(to));
        }
        scene.bookmarks[index].time = to;
        scene.bookmarks.sort_by_key(|bookmark| bookmark.time);
        Ok(Outcome::unchanged())
    }
}

/// Replaces the note, colour and duration of the current scene's bookmark
/// at `time` (classic `UpdateBookmarkCommand`). `None` clears a field.
pub struct UpdateBookmark {
    pub time: MediaTime,
    pub note: Option<String>,
    /// CSS hex colour, e.g. `#ff0000`.
    pub color: Option<String>,
    /// Makes the bookmark a range; must be positive.
    pub duration: Option<MediaTime>,
}

impl Command for UpdateBookmark {
    fn label(&self) -> &'static str {
        "Edit bookmark"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        if self
            .duration
            .is_some_and(|duration| duration <= MediaTime::ZERO)
        {
            return Err(EditError::InvalidTiming);
        }
        let time = frame_time(project, self.time);
        let scene = current_scene(project)?;
        let index = bookmark_index(scene, time)?;
        scene.bookmarks[index] = Bookmark {
            time,
            note: self.note.clone(),
            color: self.color.clone(),
            duration: self.duration,
        };
        Ok(Outcome::unchanged())
    }
}
