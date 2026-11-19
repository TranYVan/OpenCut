use chrono::{SubsecRound, Utc};
use opencut_model::time::MediaTime;
use opencut_model::{Project, SceneTracks, TimelineViewState, ValidationError};

use crate::command::{Command, Outcome};
use crate::error::EditError;
use crate::playback::Playback;
use crate::ripple::{apply_ripple_adjustments, compute_ripple_adjustments};
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
    /// Set by [`Editor::execute_coalescing`]: the next command with the same
    /// key folds into this step instead of adding one.
    coalesce_key: Option<String>,
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
        let playhead = project
            .timeline_view_state
            .as_ref()
            .map(|view| view.playhead_time);
        let mut editor = Self {
            project,
            selection: Selection::default(),
            playback: Playback::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            revision: 0,
            snapping_enabled: true,
            ripple_enabled: false,
        };
        // A saved project reopens with the playhead where it was left.
        if let Some(playhead) = playhead {
            editor.seek(playhead);
        }
        Ok(editor)
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
        self.execute_with_key(command, None)
    }

    /// Like [`Editor::execute`], but folds into the previous undo step when
    /// that was also run with `key` and nothing else has been run since, so
    /// a drag of many small edits undoes in one. End the gesture with
    /// [`Editor::end_coalescing`].
    pub fn execute_coalescing(
        &mut self,
        command: &mut dyn Command,
        key: &str,
    ) -> Result<Outcome, EditError> {
        self.execute_with_key(command, Some(key))
    }

    /// The next [`Editor::execute_coalescing`] starts a new undo step.
    pub fn end_coalescing(&mut self) {
        if let Some(entry) = self.undo_stack.last_mut() {
            entry.coalesce_key = None;
        }
    }

    fn execute_with_key(
        &mut self,
        command: &mut dyn Command,
        key: Option<&str>,
    ) -> Result<Outcome, EditError> {
        let before = self.project.clone();
        let tracks_before = self.ripple_enabled.then(|| self.tracks().clone());
        let result = command.apply(&mut self.project).and_then(|outcome| {
            // `tracks()` relies on this.
            if self.project.current_scene().is_none() {
                return Err(EditError::NoCurrentScene);
            }
            if let Some(tracks_before) = &tracks_before {
                self.ripple(tracks_before);
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
        if let Some(key) = key
            && outcome.selection.is_none()
            && let Some(entry) = self
                .undo_stack
                .last_mut()
                .filter(|entry| entry.coalesce_key.as_deref() == Some(key))
        {
            entry.after = self.project.clone();
            return Ok(outcome);
        }
        self.undo_stack.push(HistoryEntry {
            label: command.label(),
            before,
            after: self.project.clone(),
            selection_before,
            selection_after: outcome.selection.is_some().then(|| self.selection.clone()),
            coalesce_key: key.map(str::to_owned),
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
        self.restore(entry.before.clone());
        if entry.selection_after.is_some() {
            self.selection = entry.selection_before.clone();
        }
        self.prune_selection();
        self.revision += 1;
        self.redo_stack.push(entry);
        self.end_coalescing();
        true
    }

    /// Returns `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        self.restore(entry.after.clone());
        if let Some(selection) = &entry.selection_after {
            self.selection = selection.clone();
        }
        self.prune_selection();
        self.revision += 1;
        self.undo_stack.push(entry);
        self.end_coalescing();
        true
    }

    /// Where the timeline was zoomed and scrolled. Saved with the project
    /// (classic `timelineViewState`); `playhead_time` is filled from the
    /// playhead when the project is saved, see [`Editor::project_to_save`].
    pub fn timeline_view_state(&self) -> Option<&TimelineViewState> {
        self.project.timeline_view_state.as_ref()
    }

    /// Updates the saved timeline view. Not an edit: no undo step, no
    /// revision bump, and undo/redo leave it alone. Returns whether it
    /// changed, so the caller knows to schedule a save.
    pub fn set_timeline_view_state(&mut self, state: TimelineViewState) -> bool {
        if self.project.timeline_view_state.as_ref() == Some(&state) {
            return false;
        }
        self.project.timeline_view_state = Some(state);
        true
    }

    /// The project as it should be written to disk: the document plus the
    /// current playhead in its timeline view state.
    pub fn project_to_save(&self) -> Project {
        let mut project = self.project.clone();
        let view = project
            .timeline_view_state
            // Classic `defaultTimelineViewState`.
            .get_or_insert(TimelineViewState {
                zoom_level: 1.0,
                scroll_left: 0.0,
                playhead_time: MediaTime::ZERO,
            });
        view.playhead_time = self.playback.position;
        project
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

    /// Swaps in a history snapshot, keeping the current timeline view: it
    /// isn't part of the edit history.
    fn restore(&mut self, mut project: Project) {
        project.timeline_view_state = self.project.timeline_view_state.take();
        self.project = project;
    }

    /// Closes the gaps a command left in the current scene (classic ripple).
    fn ripple(&mut self, before: &SceneTracks) {
        if let Some(scene) = self.project.current_scene_mut() {
            let adjustments = compute_ripple_adjustments(before, &scene.tracks);
            apply_ripple_adjustments(&mut scene.tracks, &adjustments);
        }
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
