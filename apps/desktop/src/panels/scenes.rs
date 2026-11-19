//! The scene switcher in the editor header (classic `ScenesView`): one
//! button per scene, click to edit it, right-click to rename or delete,
//! `+` to add one.

use gpui::{Context, Entity, Focusable, MouseButton, Subscription, Window, div, prelude::*, px};
use opencut_editor::commands::{CreateScene, DeleteScene, RenameScene};

use crate::components::{
    Button, ButtonSize, ButtonVariant, ContextMenu, ContextMenuItem, ContextMenuItemVariant,
    TextField, TextFieldEvent,
};
use crate::workspace::Workspace;

pub(crate) struct SceneBar {
    workspace: Entity<Workspace>,
    menu: Entity<ContextMenu>,
    /// The scene the menu was opened on.
    menu_scene: Option<String>,
    rename_field: Entity<TextField>,
    /// The scene being renamed; its button shows `rename_field` instead.
    renaming: Option<String>,
    _subscriptions: [Subscription; 2],
}

impl SceneBar {
    pub(crate) fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let rename = weak.clone();
        let menu = cx.new(|_| {
            ContextMenu::new([
                ContextMenuItem::entry("Rename").on_select(move |window, cx| {
                    let _ = rename.update(cx, |this, cx| this.start_rename(window, cx));
                }),
                ContextMenuItem::entry("Delete")
                    .variant(ContextMenuItemVariant::Destructive)
                    .on_select(move |_, cx| {
                        let _ = weak.update(cx, |this, cx| this.delete(cx));
                    }),
            ])
        });
        let rename_field = cx.new(|cx| TextField::new("Scene name", cx));
        let rename_events = cx.subscribe(&rename_field, |this, _, event, cx| match event {
            TextFieldEvent::Submit => this.finish_rename(cx),
            TextFieldEvent::Cancel => {
                this.renaming = None;
                cx.notify();
            }
        });
        let workspace_changed = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self {
            workspace,
            menu,
            menu_scene: None,
            rename_field,
            renaming: None,
            _subscriptions: [rename_events, workspace_changed],
        }
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let count = self.workspace.read(cx).editor().project().scenes.len();
        let mut command = CreateScene::new(format!("Scene {}", count + 1));
        let id = command.scene_id().to_owned();
        self.workspace.update(cx, |workspace, cx| {
            workspace.execute(&mut command, cx);
            workspace.switch_scene(&id, cx);
        });
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if let Some(scene_id) = self.menu_scene.take() {
            self.workspace.update(cx, |workspace, cx| {
                workspace.execute(&mut DeleteScene { scene_id }, cx);
            });
        }
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scene_id) = self.menu_scene.take() else {
            return;
        };
        let Some(scene) = self.workspace.read(cx).editor().project().scene(&scene_id) else {
            return;
        };
        let name = scene.name.clone();
        self.rename_field
            .update(cx, |field, cx| field.set_text(name, cx));
        window.focus(&self.rename_field.focus_handle(cx));
        self.renaming = Some(scene_id);
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(scene_id) = self.renaming.take() else {
            return;
        };
        let name = self.rename_field.read(cx).text().trim().to_owned();
        if !name.is_empty() {
            self.workspace.update(cx, |workspace, cx| {
                workspace.execute(&mut RenameScene { scene_id, name }, cx);
            });
        }
        cx.notify();
    }
}

impl Render for SceneBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.workspace.read(cx).editor().project();
        let buttons = project.scenes.iter().enumerate().map(|(index, scene)| {
            if self.renaming.as_deref() == Some(scene.id.as_str()) {
                return div()
                    .w(px(140.0))
                    .child(self.rename_field.clone())
                    .into_any_element();
            }
            let current = scene.id == project.current_scene_id;
            let switch_id = scene.id.clone();
            let menu_id = scene.id.clone();
            div()
                .id(("scene", index))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.menu_scene = Some(menu_id.clone());
                        this.menu
                            .update(cx, |menu, cx| menu.open(event.position, cx));
                    }),
                )
                .child(
                    Button::new(("scene-button", index), scene.name.clone())
                        .variant(if current {
                            ButtonVariant::Secondary
                        } else {
                            ButtonVariant::Ghost
                        })
                        .size(ButtonSize::Small)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |workspace, cx| {
                                workspace.switch_scene(&switch_id, cx);
                            });
                        })),
                )
                .into_any_element()
        });

        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .children(buttons.collect::<Vec<_>>())
            .child(
                Button::new("scene-create", "+")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSmall)
                    .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
            )
            .child(self.menu.clone())
    }
}
