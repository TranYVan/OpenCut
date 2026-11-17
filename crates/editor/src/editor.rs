use chrono::{SubsecRound, Utc};
use opencut_model::time::MediaTime;
use opencut_model::{Project, SceneTracks, ValidationError};

use crate::command::{Command, Outcome};
use crate::error::EditError;
use crate::playback::Playback;
use crate::selection::Selection;

/// Undo steps kept before the oldest is dropped.
pub const HISTORY_LIMIT: usize = 200;

struct HistoryEntry {
    label: &'static str,
    before: Project,
    after: Project,
    selection_before: Selection,
    /// The selection the command set, if it set one. Undo restores the old
    /// selection only for such commands; otherwise selection changes made by
    /// clicking between edits survive the undo.
    selection_after: Option<Selection>,
}

/// One open project plus everything needed to edit it: selection, playhead,
/// undo history. Headless: the desktop UI, scripts and a future MCP server all
/// drive the same type.
pub struct Editor {
    project: Project,
    selection: Selection,
    playback: Playback,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    revision: u64,
    pub snapping_enabled: bool,
    pub ripple_enabled: bool,
}

impl Editor {
    pub fn new(project: Project) -> Result<Self, ValidationError> {
        project.validate()?;
        Ok(Self {
            project,
            selection: Selection::default(),
            playback: Playback::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            revision: 0,
            snapping_enabled: true,
            ripple_enabled: false,
        })
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Tracks of the scene being edited.
    pub fn tracks(&self) -> &SceneTracks {
        // `Editor::new` validates that the current scene exists and `execute`
        // rejects commands that remove it.
        &self
            .project
            .current_scene()
            .expect("validated project has a current scene")
            .tracks
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Selection changes from clicks are not undoable, as in classic.
    pub fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }

    pub fn playback(&self) -> Playback {
        self.playback
    }

    /// Bumps on every project change, including undo and redo. Use it to
    /// know when to re-render or autosave.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Runs a command as one undo step. On error the project is unchanged.
    pub fn execute(&mut self, command: &mut dyn Command) -> Result<Outcome, EditError> {
        let before = self.project.clone();
        let result = command.apply(&mut self.project).and_then(|outcome| {
            // `tracks()` relies on this.
            if self.project.current_scene().is_none() {
                return Err(EditError::NoCurrentScene);
            }
            Ok(outcome)
        });
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.project = before;
                return Err(error);
            }
        };

        let selection_before = self.selection.clone();
        if let Some(elements) = &outcome.selection {
            self.selection.set(elements.clone());
        }

        if self.project == before {
            return Ok(outcome);
        }

        self.after_change();
        self.undo_stack.push(HistoryEntry {
            label: command.label(),
            before,
            after: self.project.clone(),
            selection_before,
            selection_after: outcome.selection.is_some().then(|| self.selection.clone()),
        });
        if self.undo_stack.len() > HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        Ok(outcome)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo_stack.last().map(|entry| entry.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo_stack.last().map(|entry| entry.label)
    }

    /// Returns `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        self.project = entry.before.clone();
        if entry.selection_after.is_some() {
            self.selection = entry.selection_before.clone();
        }
        self.prune_selection();
        self.revision += 1;
        self.redo_stack.push(entry);
        true
    }

    /// Returns `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        self.project = entry.after.clone();
        if let Some(selection) = &entry.selection_after {
            self.selection = selection.clone();
        }
        self.prune_selection();
        self.revision += 1;
        self.undo_stack.push(entry);
        true
    }

    /// Forgets all undo history, e.g. after loading a project.
    pub fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Length of the current scene.
    pub fn duration(&self) -> MediaTime {
        self.tracks().duration()
    }

    /// Moves the playhead, snapped to a frame and clamped to the scene.
    pub fn seek(&mut self, time: MediaTime) {
        let rate = self.project.settings.fps;
        self.playback.position = time
            .snapped_seek_time(self.duration(), rate)
            .unwrap_or(MediaTime::ZERO);
    }

    /// Moves the playhead by whole frames (negative steps go back).
    pub fn step_frames(&mut self, frames: i64) {
        let rate = self.project.settings.fps;
        let target = self
            .playback
            .position
            .to_frame_round(rate)
            .and_then(|frame| MediaTime::from_frame(frame.saturating_add(frames), rate));
        if let Some(target) = target {
            self.seek(target);
        }
    }

    pub fn go_to_start(&mut self) {
        self.seek(MediaTime::ZERO);
    }

    pub fn go_to_end(&mut self) {
        self.seek(self.duration());
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playback.playing = playing;
    }

    pub fn toggle_playing(&mut self) {
        self.playback.playing = !self.playback.playing;
    }

    fn prune_selection(&mut self) {
        if let Some(scene) = self.project.current_scene() {
            self.selection.retain_existing(&scene.tracks);
        }
    }

    fn after_change(&mut self) {
        self.project.refresh_duration();
        // Milliseconds: what the file stores.
        let now = Utc::now().trunc_subsecs(3);
        self.project.metadata.updated_at = now;
        if let Some(scene) = self.project.current_scene_mut() {
            scene.updated_at = now;
        }
        self.prune_selection();
        self.revision += 1;
    }
}
