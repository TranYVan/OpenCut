//! The projects list: create, open, rename, delete and sort the projects
//! in the [`ProjectStore`]. Port of the classic `/projects` page (tag
//! `pre-rewrite`, `apps/web/src/app/projects/`), without search, multi-select
//! or duplicate yet.

use chrono::{DateTime, Local, Utc};
use gpui::{
    AnyElement, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, PathPromptOptions, Pixels, Point, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{Project, ProjectMetadata};
use opencut_project_store::{
    DEFAULT_PROJECT_NAME, ProjectList, ProjectStore, SortKey, SortOrder, StoreError, sort_projects,
};

use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, ContextMenu, ContextMenuItem,
    ContextMenuItemVariant, Label, TextField, TextFieldEvent,
};
use crate::theme::ActiveTheme;

const CARD_WIDTH: f32 = 220.0;
const PREVIEW_HEIGHT: f32 = 124.0;

/// Asks the shell to open a project in the editor.
pub(crate) struct OpenProject(pub(crate) Project);

enum Dialog {
    Rename { id: String },
    Delete { id: String, name: String },
}

pub(crate) struct ProjectsView {
    store: ProjectStore,
    /// Sorted by `sort_key` and `sort_order`.
    projects: Vec<ProjectMetadata>,
    loaded: bool,
    /// Project folders that couldn't be read.
    unreadable: usize,
    sort_key: SortKey,
    sort_order: SortOrder,
    /// The project the context menu was opened on.
    menu_target: Option<String>,
    context_menu: Entity<ContextMenu>,
    sort_menu: Entity<ContextMenu>,
    dialog: Option<Dialog>,
    rename_field: Entity<TextField>,
    /// A project is being created or opened.
    busy: bool,
    notice: Option<SharedString>,
    focus_handle: FocusHandle,
    _rename_events: Subscription,
}

impl EventEmitter<OpenProject> for ProjectsView {}

impl ProjectsView {
    pub(crate) fn new(store: ProjectStore, cx: &mut Context<Self>) -> Self {
        let view = cx.entity().downgrade();
        let (open_view, rename_view, delete_view) = (view.clone(), view.clone(), view);
        let context_menu = cx.new(|_| {
            ContextMenu::new([
                ContextMenuItem::entry("Open").on_select(move |_, cx| {
                    let _ = open_view.update(cx, |this, cx| {
                        if let Some(id) = this.menu_target.clone() {
                            this.open(id, cx);
                        }
                    });
                }),
                ContextMenuItem::entry("Rename…").on_select(move |window, cx| {
                    let _ = rename_view.update(cx, |this, cx| {
                        if let Some(id) = this.menu_target.clone() {
                            this.start_rename(id, window, cx);
                        }
                    });
                }),
                ContextMenuItem::separator(),
                ContextMenuItem::entry("Delete…")
                    .variant(ContextMenuItemVariant::Destructive)
                    .on_select(move |_, cx| {
                        let _ = delete_view.update(cx, |this, cx| {
                            if let Some(id) = this.menu_target.clone() {
                                this.start_delete(id, cx);
                            }
                        });
                    }),
            ])
        });
        let rename_field = cx.new(|cx| TextField::new("Enter a new name", cx));
        let rename_events = cx.subscribe(&rename_field, |this, _, event, cx| match event {
            TextFieldEvent::Submit => this.confirm_dialog(cx),
            TextFieldEvent::Cancel => this.close_dialog(cx),
        });

        let mut this = Self {
            store,
            projects: Vec::new(),
            loaded: false,
            unreadable: 0,
            sort_key: SortKey::default(),
            sort_order: SortOrder::default(),
            menu_target: None,
            context_menu,
            sort_menu: cx.new(|_| ContextMenu::new([])),
            dialog: None,
            rename_field,
            busy: false,
            notice: None,
            focus_handle: cx.focus_handle(),
            _rename_events: rename_events,
        };
        this.reload(cx);
        this
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Re-reads the store off the UI thread.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move { store.list() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.set_list(result, cx));
        })
        .detach();
    }

    fn set_list(&mut self, result: Result<ProjectList, StoreError>, cx: &mut Context<Self>) {
        self.loaded = true;
        match result {
            Ok(list) => {
                self.projects = list.projects;
                self.unreadable = list.unreadable.len();
                self.sort();
            }
            Err(error) => self.notice = Some(format!("Can't list projects: {error}").into()),
        }
        cx.notify();
    }

    fn sort(&mut self) {
        sort_projects(&mut self.projects, self.sort_key, self.sort_order);
    }

    fn set_sort(&mut self, key: SortKey, order: SortOrder, cx: &mut Context<Self>) {
        self.sort_key = key;
        self.sort_order = order;
        self.sort();
        cx.notify();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        self.load_then_open(
            cx,
            move || store.create(DEFAULT_PROJECT_NAME, Utc::now()),
            "Can't create a project",
        );
    }

    /// Asks for a classic export zip, imports it as a new project and opens it.
    fn import_classic(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => paths.swap_remove(0),
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.notice = Some(format!("Can't open the file picker: {error}").into());
                        cx.notify();
                    });
                    return;
                }
                _ => return,
            };
            let _ = this.update(cx, |this, cx| {
                let store = this.store.clone();
                this.load_then_open(
                    cx,
                    move || store.import_classic_zip(&path, Utc::now()),
                    "Can't import the project",
                );
            });
        })
        .detach();
    }

    fn open(&mut self, id: String, cx: &mut Context<Self>) {
        let store = self.store.clone();
        self.load_then_open(cx, move || store.load(&id), "Can't open the project");
    }

    /// Runs `load` off the UI thread and asks the shell to open the result.
    fn load_then_open(
        &mut self,
        cx: &mut Context<Self>,
        load: impl FnOnce() -> Result<Project, StoreError> + Send + 'static,
        failure: &'static str,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.notice = None;
        cx.notify();
        let work = cx.background_executor().spawn(async move { load() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(project) => cx.emit(OpenProject(project)),
                    Err(error) => this.notice = Some(format!("{failure}: {error}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.projects.iter().find(|project| project.id == id) else {
            return;
        };
        let name = project.name.clone();
        self.rename_field
            .update(cx, |field, cx| field.set_text(name, cx));
        window.focus(&self.rename_field.focus_handle(cx));
        self.dialog = Some(Dialog::Rename { id });
        cx.notify();
    }

    fn start_delete(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(project) = self.projects.iter().find(|project| project.id == id) else {
            return;
        };
        self.dialog = Some(Dialog::Delete {
            name: project.name.clone(),
            id,
        });
        cx.notify();
    }

    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        cx.notify();
    }

    /// Renames or deletes on disk, then reloads the list.
    fn confirm_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let store = self.store.clone();
        let work = match dialog {
            Dialog::Rename { id } => {
                let name = self.rename_field.read(cx).text().to_owned();
                if name.trim().is_empty() {
                    // Keep the dialog open; there's nothing to rename to.
                    self.dialog = Some(Dialog::Rename { id });
                    return;
                }
                cx.background_executor().spawn(async move {
                    store
                        .rename(&id, &name, Utc::now())
                        .map(|_| ())
                        .map_err(|error| format!("Can't rename the project: {error}"))
                })
            }
            Dialog::Delete { id, .. } => cx.background_executor().spawn(async move {
                store
                    .delete(&id)
                    .map_err(|error| format!("Can't delete the project: {error}"))
            }),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.notice = Some(error.into());
                }
                this.reload(cx);
            });
        })
        .detach();
    }

    fn open_menu(&mut self, id: &str, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu_target = Some(id.to_owned());
        self.context_menu
            .update(cx, |menu, cx| menu.open(position, cx));
    }

    fn open_sort_menu(&mut self, event: &ClickEvent, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let order = self.sort_order;
        let items = SortKey::ALL.map(|key| {
            let view = view.clone();
            ContextMenuItem::entry(key.label())
                .checked(key == self.sort_key)
                .on_select(move |_, cx| {
                    let _ = view.update(cx, |this, cx| this.set_sort(key, order, cx));
                })
        });
        self.sort_menu = cx.new(|_| ContextMenu::new(items));
        self.sort_menu
            .update(cx, |menu, cx| menu.open(event.position(), cx));
        cx.notify();
    }

    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;
        let order_label = match self.sort_order {
            SortOrder::Ascending => "↑",
            SortOrder::Descending => "↓",
        };
        let key = self.sort_key;
        let order = self.sort_order;
        let view = cx.entity().downgrade();
        let sort_view = view.clone();

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(56.0))
            .px(px(24.0))
            .border_b_1()
            .border_color(colors.border)
            .child(div().text_base().child("All projects"))
            .when(self.loaded && !self.projects.is_empty(), |this| {
                this.child(
                    Badge::new(self.projects.len().to_string()).variant(BadgeVariant::Secondary),
                )
            })
            .child(div().flex_1())
            .child(Label::new("Sort by").muted())
            .child(
                Button::new("sort-key", key.label())
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Small)
                    .on_click(move |event, _, cx| {
                        let _ = sort_view.update(cx, |this, cx| this.open_sort_menu(event, cx));
                    }),
            )
            .child(
                Button::new("sort-order", order_label)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSmall)
                    .on_click(move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| this.set_sort(key, order.toggled(), cx));
                    }),
            )
            .child(
                Button::new("import-project", "Import…")
                    .variant(ButtonVariant::Outline)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.import_classic(cx))),
            )
            .child(
                Button::new("new-project", "New project")
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
            )
    }

    fn render_card(
        &self,
        index: usize,
        project: &ProjectMetadata,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = window.theme();
        let colors = theme.colors;
        let open_id = project.id.clone();
        let menu_id = project.id.clone();

        let preview = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(PREVIEW_HEIGHT))
            .rounded(theme.radius)
            .bg(colors.muted)
            .text_color(colors.muted_foreground)
            .text_xs()
            .child("No preview")
            .child(div().absolute().bottom(px(6.0)).right(px(6.0)).child(
                Badge::new(format_duration(project.duration)).variant(BadgeVariant::Secondary),
            ));

        div()
            .id(("project", index))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .w(px(CARD_WIDTH))
            .p(px(6.0))
            .rounded(theme.radius)
            .hover(|style| style.bg(colors.accent))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open(open_id.clone(), cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(&menu_id, event.position, cx);
                }),
            )
            .child(preview)
            .child(
                div()
                    .px(px(2.0))
                    .text_sm()
                    .text_color(colors.foreground)
                    .truncate()
                    .child(project.name.clone()),
            )
            .child(
                div().px(px(2.0)).child(
                    Label::new(format!("Modified {}", format_date(project.updated_at))).muted(),
                ),
            )
            .into_any_element()
    }

    fn render_dialog(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.as_ref()?;
        let theme = window.theme();
        let colors = theme.colors;

        let (title, body, confirm_label, confirm_variant): (
            SharedString,
            AnyElement,
            &str,
            ButtonVariant,
        ) = match dialog {
            Dialog::Rename { .. } => (
                "Rename project".into(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(Label::new("New name"))
                    .child(self.rename_field.clone())
                    .into_any_element(),
                "Rename",
                ButtonVariant::Default,
            ),
            Dialog::Delete { name, .. } => (
                format!("Delete \u{201c}{name}\u{201d}?").into(),
                Label::new(
                    "The project and its imported media will be deleted. This can't be undone.",
                )
                .muted()
                .into_any_element(),
                "Delete",
                ButtonVariant::Destructive,
            ),
        };

        let card = div()
            .id("project-dialog")
            .occlude()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .w(px(400.0))
            .p(px(20.0))
            .rounded(theme.radius)
            .bg(colors.popover)
            .text_color(colors.popover_foreground)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            // Clicks inside don't reach the backdrop, which closes it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_base().child(title))
            .child(body)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        Button::new("dialog-cancel", "Cancel")
                            .variant(ButtonVariant::Outline)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .child(
                        Button::new("dialog-confirm", confirm_label)
                            .variant(confirm_variant)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_dialog(cx))),
                    ),
            );

        Some(
            div()
                .id("project-dialog-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background.opacity(0.8))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_dialog(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

impl Render for ProjectsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;

        let body = if !self.loaded {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(Label::new("Loading projects…").muted())
                .into_any_element()
        } else if self.projects.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(div().text_base().child("No projects yet"))
                .child(Label::new("Start a new project to begin editing.").muted())
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .pt(px(8.0))
                        .child(
                            Button::new("empty-import-project", "Import classic project…")
                                .variant(ButtonVariant::Outline)
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.import_classic(cx))),
                        )
                        .child(
                            Button::new("empty-new-project", "New project")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                        ),
                )
                .into_any_element()
        } else {
            let cards: Vec<AnyElement> = self
                .projects
                .iter()
                .enumerate()
                .map(|(index, project)| self.render_card(index, project, window, cx))
                .collect();
            div()
                .id("projects")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .content_start()
                        .gap(px(12.0))
                        .p(px(18.0))
                        .children(cards),
                )
                .into_any_element()
        };

        let unreadable = (self.unreadable > 0).then(|| {
            let folders = if self.unreadable == 1 {
                "folder"
            } else {
                "folders"
            };
            format!(
                "{} project {folders} in {} couldn't be read.",
                self.unreadable,
                self.store.root().display()
            )
        });
        let footer = self
            .notice
            .clone()
            .map(|notice| (notice, true))
            .or_else(|| unreadable.map(|text| (text.into(), false)));

        let dialog = self.render_dialog(window, cx);

        div()
            .id("projects-view")
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(window, cx))
            .child(body)
            .when_some(footer, |this, (text, is_error)| {
                this.child(
                    div()
                        .flex_none()
                        .px(px(24.0))
                        .py(px(8.0))
                        .border_t_1()
                        .border_color(colors.border)
                        .text_xs()
                        .text_color(if is_error {
                            colors.destructive
                        } else {
                            colors.muted_foreground
                        })
                        .child(text),
                )
            })
            .children(dialog)
            .child(self.context_menu.clone())
            .child(self.sort_menu.clone())
    }
}

/// `MM:SS`, or `H:MM:SS` from an hour up (classic used the same cut-off).
fn format_duration(duration: MediaTime) -> String {
    let total = duration.to_seconds_f64().max(0.0).round() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn format_date(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local).format("%b %-d, %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_switch_to_hours_from_an_hour_up() {
        let seconds = |s: f64| MediaTime::from_seconds_f64(s).unwrap();
        assert_eq!(format_duration(MediaTime::ZERO), "00:00");
        assert_eq!(format_duration(seconds(75.4)), "01:15");
        assert_eq!(format_duration(seconds(3_661.0)), "1:01:01");
    }
}
