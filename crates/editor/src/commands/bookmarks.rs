use opencut_model::time::MediaTime;
use opencut_model::{Bookmark, Project};

use crate::command::{Command, Outcome};
use crate::error::EditError;

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
        let fps = project.settings.fps;
        let time = self.time.round_to_frame(fps).unwrap_or(self.time);
        let scene = project
            .current_scene_mut()
            .ok_or(EditError::NoCurrentScene)?;
        if let Some(index) = scene
            .bookmarks
            .iter()
            .position(|bookmark| bookmark.time == time)
        {
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
