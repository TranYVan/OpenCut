//! Scenes and bookmarks: classic `commands/scene` and `ScenesManager`.

use opencut_editor::commands::{
    CreateScene, DeleteScene, MoveBookmark, RenameScene, ToggleBookmark, UpdateBookmark,
};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{Bookmark, ElementRef, Project};
use opencut_editor::{EditError, Editor};
use pretty_assertions::assert_eq;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/classic-v31-scenes.json");
/// One frame at the fixture's 30 fps.
const FRAME: i64 = 4000;

fn ticks(ticks: i64) -> MediaTime {
    MediaTime::from_ticks(ticks)
}

fn load() -> Editor {
    Editor::new(serde_json::from_str(FIXTURE).expect("fixture parses")).unwrap()
}

fn scene_names(editor: &Editor) -> Vec<&str> {
    editor
        .project()
        .scenes
        .iter()
        .map(|scene| scene.name.as_str())
        .collect()
}

fn bookmarks(editor: &Editor) -> Vec<Bookmark> {
    editor.project().current_scene().unwrap().bookmarks.clone()
}

fn mark(time: i64) -> Bookmark {
    Bookmark {
        time: ticks(time),
        note: None,
        color: None,
        duration: None,
    }
}

/// JSON `1` and `1.0` compare unequal in `serde_json::Value`.
fn normalize(value: Value) -> Value {
    match value {
        Value::Number(number) => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, normalize(value)))
                .collect(),
        ),
        other => other,
    }
}

#[test]
fn create_scene_appends_a_non_main_scene_without_switching() {
    let mut editor = load();
    let mut command = CreateScene::new("Outro");
    let id = command.scene_id().to_owned();
    editor.execute(&mut command).unwrap();

    assert_eq!(scene_names(&editor), ["Main scene", "Intro", "Outro"]);
    let scene = editor.project().scene(&id).unwrap();
    assert!(!scene.is_main);
    assert!(scene.tracks.iter().all(|track| track.elements.is_empty()));
    assert_eq!(editor.project().current_scene_id, "scene-intro");

    editor.undo();
    assert_eq!(scene_names(&editor), ["Main scene", "Intro"]);
    editor.redo();
    assert_eq!(editor.project().scene(&id).unwrap().name, "Outro");
}

#[test]
fn rename_scene_renames_any_scene() {
    let mut editor = load();
    editor
        .execute(&mut RenameScene {
            scene_id: "scene-main".to_owned(),
            name: "Story".to_owned(),
        })
        .unwrap();
    assert_eq!(scene_names(&editor), ["Story", "Intro"]);

    editor.undo();
    assert_eq!(scene_names(&editor), ["Main scene", "Intro"]);
    editor.redo();
    assert_eq!(scene_names(&editor), ["Story", "Intro"]);

    let missing = editor.execute(&mut RenameScene {
        scene_id: "nope".to_owned(),
        name: "x".to_owned(),
    });
    assert_eq!(missing, Err(EditError::SceneNotFound("nope".to_owned())));
}

#[test]
fn delete_current_scene_falls_back_to_main() {
    let mut editor = load();
    let title = ElementRef::new("track-text", "el-title");
    editor.selection_mut().set(vec![title]);

    editor
        .execute(&mut DeleteScene {
            scene_id: "scene-intro".to_owned(),
        })
        .unwrap();
    assert_eq!(scene_names(&editor), ["Main scene"]);
    assert_eq!(editor.project().current_scene_id, "scene-main");
    assert!(editor.selection().is_empty());

    editor.undo();
    assert_eq!(scene_names(&editor), ["Main scene", "Intro"]);
    // Undo restores the scene but, like classic's scene list, not the switch.
    assert_eq!(editor.project().current_scene_id, "scene-main");
    editor.redo();
    assert_eq!(scene_names(&editor), ["Main scene"]);
}

#[test]
fn main_and_only_scenes_cannot_be_deleted() {
    let mut editor = load();
    let main = editor.execute(&mut DeleteScene {
        scene_id: "scene-main".to_owned(),
    });
    assert_eq!(main, Err(EditError::CannotDeleteMainScene));

    let mut project: Project = serde_json::from_str(FIXTURE).unwrap();
    project.scenes.retain(|scene| !scene.is_main);
    let mut editor = Editor::new(project).unwrap();
    let only = editor.execute(&mut DeleteScene {
        scene_id: "scene-intro".to_owned(),
    });
    assert_eq!(only, Err(EditError::CannotDeleteMainScene));
    assert!(!editor.can_undo());
}

#[test]
fn switching_scenes_clears_selection_and_is_not_undoable() {
    let mut editor = load();
    editor
        .selection_mut()
        .set(vec![ElementRef::new("track-text", "el-title")]);
    let revision = editor.revision();

    editor.switch_scene("scene-main").unwrap();
    assert_eq!(editor.project().current_scene_id, "scene-main");
    assert_eq!(editor.tracks().main.elements[0].base().id, "el-photo");
    assert!(editor.selection().is_empty());
    assert!(editor.revision() > revision);
    assert!(!editor.can_undo());

    assert_eq!(
        editor.switch_scene("nope"),
        Err(EditError::SceneNotFound("nope".to_owned()))
    );
}

#[test]
fn undo_stays_on_the_current_scene() {
    let mut editor = load();
    editor
        .execute(&mut ToggleBookmark {
            time: ticks(10 * FRAME),
        })
        .unwrap();
    editor.switch_scene("scene-main").unwrap();

    editor.undo();
    assert_eq!(editor.project().current_scene_id, "scene-main");
    assert_eq!(
        editor
            .project()
            .scene("scene-intro")
            .unwrap()
            .bookmarks
            .len(),
        2
    );
}

#[test]
fn toggle_bookmark_adds_and_removes_on_the_current_scene() {
    let mut editor = load();
    // Rounds to frame 3.
    editor
        .execute(&mut ToggleBookmark {
            time: ticks(3 * FRAME + 100),
        })
        .unwrap();
    assert_eq!(
        bookmarks(&editor)
            .iter()
            .map(|b| b.time)
            .collect::<Vec<_>>(),
        [ticks(0), ticks(3 * FRAME), ticks(10 * FRAME)]
    );
    editor
        .execute(&mut ToggleBookmark {
            time: ticks(3 * FRAME),
        })
        .unwrap();
    assert_eq!(bookmarks(&editor).len(), 2);

    editor.undo();
    assert_eq!(bookmarks(&editor).len(), 3);
    // The main scene's bookmarks are untouched.
    assert_eq!(
        editor
            .project()
            .scene("scene-main")
            .unwrap()
            .bookmarks
            .len(),
        1
    );
}

#[test]
fn move_bookmark_keeps_its_fields_and_order() {
    let mut editor = load();
    editor
        .execute(&mut ToggleBookmark {
            time: ticks(20 * FRAME),
        })
        .unwrap();
    // Moves the ranged bookmark past the new one; `to` rounds to frame 30.
    editor
        .execute(&mut MoveBookmark {
            from: ticks(10 * FRAME),
            to: ticks(30 * FRAME + 10),
        })
        .unwrap();
    let mut range = mark(30 * FRAME);
    range.duration = Some(ticks(20000));
    assert_eq!(
        bookmarks(&editor),
        [mark(0), mark(20 * FRAME), range.clone()]
    );

    editor.undo();
    assert_eq!(bookmarks(&editor)[1].time, ticks(10 * FRAME));
    editor.redo();
    assert_eq!(bookmarks(&editor)[2], range);

    let negative = editor.execute(&mut MoveBookmark {
        from: ticks(0),
        to: ticks(-FRAME),
    });
    assert_eq!(negative, Err(EditError::InvalidTiming));
    let missing = editor.execute(&mut MoveBookmark {
        from: ticks(7 * FRAME),
        to: ticks(8 * FRAME),
    });
    assert_eq!(missing, Err(EditError::BookmarkNotFound(ticks(7 * FRAME))));
}

#[test]
fn move_bookmark_undo_redo() {
    let mut editor = load();
    editor
        .execute(&mut MoveBookmark {
            from: ticks(10 * FRAME),
            to: ticks(FRAME),
        })
        .unwrap();
    assert_eq!(bookmarks(&editor)[1].time, ticks(FRAME));
    editor.undo();
    assert_eq!(bookmarks(&editor)[1].time, ticks(10 * FRAME));
    editor.redo();
    assert_eq!(bookmarks(&editor)[1].time, ticks(FRAME));
}

#[test]
fn move_bookmark_onto_another_is_refused() {
    let mut editor = load();
    let result = editor.execute(&mut MoveBookmark {
        from: ticks(10 * FRAME),
        to: ticks(0),
    });
    assert_eq!(result, Err(EditError::BookmarkExists(ticks(0))));
    assert_eq!(
        bookmarks(&editor),
        [mark(0), {
            let mut range = mark(10 * FRAME);
            range.duration = Some(ticks(20000));
            range
        }]
    );
}

#[test]
fn update_bookmark_sets_note_colour_and_duration() {
    let mut editor = load();
    editor
        .execute(&mut UpdateBookmark {
            time: ticks(0),
            note: Some("start".to_owned()),
            color: Some("#00ff00".to_owned()),
            duration: Some(ticks(2 * FRAME)),
        })
        .unwrap();
    assert_eq!(
        bookmarks(&editor)[0],
        Bookmark {
            time: ticks(0),
            note: Some("start".to_owned()),
            color: Some("#00ff00".to_owned()),
            duration: Some(ticks(2 * FRAME)),
        }
    );

    editor.undo();
    assert_eq!(bookmarks(&editor)[0], mark(0));
    editor.redo();
    assert_eq!(bookmarks(&editor)[0].note.as_deref(), Some("start"));

    let zero = editor.execute(&mut UpdateBookmark {
        time: ticks(0),
        note: None,
        color: None,
        duration: Some(ticks(0)),
    });
    assert_eq!(zero, Err(EditError::InvalidTiming));
    let missing = editor.execute(&mut UpdateBookmark {
        time: ticks(5 * FRAME),
        note: None,
        color: None,
        duration: None,
    });
    assert_eq!(missing, Err(EditError::BookmarkNotFound(ticks(5 * FRAME))));
}

#[test]
fn classic_multi_scene_project_round_trips() {
    let editor = load();
    let written = serde_json::to_value(editor.project()).unwrap();
    let original: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(normalize(written), normalize(original));
}

#[test]
fn classic_multi_scene_project_edits_end_to_end() {
    let mut editor = load();
    assert_eq!(editor.tracks().overlay[0].elements[0].base().id, "el-title");

    editor.switch_scene("scene-main").unwrap();
    editor
        .execute(&mut ToggleBookmark {
            time: ticks(5 * FRAME),
        })
        .unwrap();
    editor
        .execute(&mut RenameScene {
            scene_id: "scene-intro".to_owned(),
            name: "Opening".to_owned(),
        })
        .unwrap();
    let mut create = CreateScene::new("Outro");
    let outro = create.scene_id().to_owned();
    editor.execute(&mut create).unwrap();
    editor.switch_scene(&outro).unwrap();

    let json = serde_json::to_string(&editor.project_to_save()).unwrap();
    let reloaded: Project = serde_json::from_str(&json).unwrap();
    reloaded.validate().unwrap();
    assert_eq!(reloaded.current_scene_id, outro);
    assert_eq!(
        reloaded
            .scenes
            .iter()
            .map(|scene| scene.name.as_str())
            .collect::<Vec<_>>(),
        ["Main scene", "Opening", "Outro"]
    );
    let main = reloaded.scene("scene-main").unwrap();
    assert_eq!(
        main.bookmarks.iter().map(|b| b.time).collect::<Vec<_>>(),
        [ticks(5 * FRAME), ticks(120000)]
    );
    assert_eq!(main.bookmarks[1].note.as_deref(), Some("beat"));
    // The project's duration follows the main scene, whatever is current.
    assert_eq!(reloaded.metadata.duration, ticks(240000));
    assert_eq!(&reloaded, editor.project());
}
