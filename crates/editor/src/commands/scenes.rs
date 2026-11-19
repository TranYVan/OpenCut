use chrono::{SubsecRound, Utc};
use opencut_model::{Project, Scene};

use crate::command::{Command, Outcome};
use crate::error::EditError;

/// Adds an empty, non-main scene after the others (classic
/// `CreateSceneCommand`). It does not become the current scene; switch with
/// [`Editor::switch_scene`](crate::Editor::switch_scene).
pub struct CreateScene {
    scene: Scene,
}

impl CreateScene {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            scene: Scene::new(name, false, Utc::now()),
        }
    }

    /// The id the new scene gets.
    pub fn scene_id(&self) -> &str {
        &self.scene.id
    }
}

impl Command for CreateScene {
    fn label(&self) -> &'static str {
        "Create scene"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        project.scenes.push(self.scene.clone());
        Ok(Outcome::unchanged())
    }
}

/// Classic `RenameSceneCommand`.
pub struct RenameScene {
    pub scene_id: String,
    pub name: String,
}

impl Command for RenameScene {
    fn label(&self) -> &'static str {
        "Rename scene"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let scene = project
            .scene_mut(&self.scene_id)
            .ok_or_else(|| EditError::SceneNotFound(self.scene_id.clone()))?;
        if scene.name != self.name {
            scene.name.clone_from(&self.name);
            scene.updated_at = Utc::now().trunc_subsecs(3);
        }
        Ok(Outcome::unchanged())
    }
}

/// Removes a scene (classic `DeleteSceneCommand`). The main scene can't be
/// deleted. Deleting the current scene switches to the main scene.
pub struct DeleteScene {
    pub scene_id: String,
}

impl Command for DeleteScene {
    fn label(&self) -> &'static str {
        "Delete scene"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let index = project
            .scenes
            .iter()
            .position(|scene| scene.id == self.scene_id)
            .ok_or_else(|| EditError::SceneNotFound(self.scene_id.clone()))?;
        // Classic only guards the main scene; a project without one must
        // still keep a scene to edit.
        if project.scenes[index].is_main || project.scenes.len() == 1 {
            return Err(EditError::CannotDeleteMainScene);
        }
        project.scenes.remove(index);
        if project.current_scene_id == self.scene_id {
            let fallback = project
                .scenes
                .iter()
                .find(|scene| scene.is_main)
                .unwrap_or(&project.scenes[0]);
            project.current_scene_id = fallback.id.clone();
        }
        Ok(Outcome::unchanged())
    }
}
