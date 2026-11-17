use opencut_model::{ElementRef, Project, SceneTracks};

use crate::error::EditError;

/// What a successful command asks the editor to do besides changing the project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `Some` replaces the element selection (an empty list clears it);
    /// `None` leaves the selection alone.
    pub selection: Option<Vec<ElementRef>>,
}

impl Outcome {
    pub const fn unchanged() -> Self {
        Self { selection: None }
    }

    pub const fn select(elements: Vec<ElementRef>) -> Self {
        Self {
            selection: Some(elements),
        }
    }
}

/// One undoable edit.
///
/// `apply` mutates the project in place. The [`Editor`](crate::Editor)
/// snapshots the project before calling it, so commands don't implement undo
/// themselves, and a command that returns `Err` may leave the project half
/// edited: the editor rolls it back.
pub trait Command {
    /// Short human-readable name for the history (e.g. "Split").
    fn label(&self) -> &'static str;

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError>;
}

/// Runs several commands as one undo step. The last command that sets a
/// selection wins.
pub struct Batch {
    label: &'static str,
    commands: Vec<Box<dyn Command>>,
}

impl Batch {
    pub fn new(label: &'static str, commands: Vec<Box<dyn Command>>) -> Self {
        Self { label, commands }
    }
}

impl Command for Batch {
    fn label(&self) -> &'static str {
        self.label
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let mut outcome = Outcome::unchanged();
        for command in &mut self.commands {
            let result = command.apply(project)?;
            if result.selection.is_some() {
                outcome = result;
            }
        }
        Ok(outcome)
    }
}

/// The tracks of the scene being edited.
pub(crate) fn current_tracks(project: &mut Project) -> Result<&mut SceneTracks, EditError> {
    project
        .current_scene_mut()
        .map(|scene| &mut scene.tracks)
        .ok_or(EditError::NoCurrentScene)
}
