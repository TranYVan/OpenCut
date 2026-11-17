use opencut_model::time::FrameRate;
use opencut_model::{Background, CanvasSize, Project, ValidationError};

use crate::command::{Command, Outcome};
use crate::error::EditError;

/// Changes project settings. `None` fields stay as they are.
#[derive(Default)]
pub struct UpdateProjectSettings {
    pub fps: Option<FrameRate>,
    pub canvas_size: Option<CanvasSize>,
    pub background: Option<Background>,
}

impl Command for UpdateProjectSettings {
    fn label(&self) -> &'static str {
        "Project settings"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let settings = &mut project.settings;
        if let Some(fps) = self.fps {
            if fps.ticks_per_frame().is_none() {
                return Err(EditError::InvalidSettings(
                    ValidationError::UnsupportedFrameRate(fps),
                ));
            }
            settings.fps = fps;
        }
        if let Some(size) = self.canvas_size {
            settings.canvas_size = size;
        }
        if let Some(background) = &self.background {
            settings.background = background.clone();
        }
        Ok(Outcome::unchanged())
    }
}
