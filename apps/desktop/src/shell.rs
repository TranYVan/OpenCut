use std::sync::Arc;

use gpui::{
    App, Context, Entity, ExternalPaths, FocusHandle, KeyBinding, Subscription, Window, actions,
    div, prelude::*, px,
};
use opencut_editor::model::Project;
use opencut_project_store::ProjectStore;

use crate::components::{Button, ButtonSize, ButtonVariant, Label};
use crate::media::MediaThumbnails;
use crate::panels::{
    BROWSER_KEY_CONTEXT, Browser, Inspector, Preview, RemoveSelectedMedia, Timeline,
};
use crate::projects::{OpenProject, ProjectsView};
use crate::theme::ActiveTheme;
use crate::workspace::{Transport, Workspace};

actions!(opencut, [Undo, Redo, ImportMedia, Paste, CloseProject]);

const APP_NAME: &str = "OpenCut";

/// The app's key bindings. `secondary` is Cmd on macOS and Ctrl elsewhere.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-z", Undo, None),
        KeyBinding::new("secondary-shift-z", Redo, None),
        KeyBinding::new("secondary-i", ImportMedia, None),
        KeyBinding::new("secondary-v", Paste, None),
        KeyBinding::new("secondary-shift-o", CloseProject, None),
        // Classic playback shortcuts.
        KeyBinding::new("space", Transport::TogglePlay, None),
        KeyBinding::new("k", Transport::TogglePlay, None),
        KeyBinding::new("shift-space", Transport::StopPlayback, None),
        KeyBinding::new("l", Transport::SeekForward, None),
        KeyBinding::new("j", Transport::SeekBackward, None),
        KeyBinding::new("right", Transport::FrameStepForward, None),
        KeyBinding::new("left", Transport::FrameStepBackward, None),
        KeyBinding::new("shift-right", Transport::JumpForward, None),
        KeyBinding::new("shift-left", Transport::JumpBackward, None),
        KeyBinding::new("home", Transport::GotoStart, None),
        KeyBinding::new("enter", Transport::GotoStart, None),
        KeyBinding::new("end", Transport::GotoEnd, None),
        KeyBinding::new("backspace", RemoveSelectedMedia, Some(BROWSER_KEY_CONTEXT)),
        KeyBinding::new("delete", RemoveSelectedMedia, Some(BROWSER_KEY_CONTEXT)),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("ctrl-y", Redo, None)]);
}

/// What the window shows: the projects list, or one project in the editor.
enum Screen {
    Projects {
        view: Entity<ProjectsView>,
        _open: Subscription,
    },
    Editor(EditorPanels),
}

// Panels are entities created once per opened project, not via `cx.new`
// inline in `render`, so each keeps its own state across renders instead of
// being torn down and rebuilt on every frame.
struct EditorPanels {
    workspace: Entity<Workspace>,
    browser: Entity<Browser>,
    preview: Entity<Preview>,
    inspector: Entity<Inspector>,
    timeline: Entity<Timeline>,
}

impl EditorPanels {
    fn new(workspace: Entity<Workspace>, cx: &mut App) -> Self {
        let waveform_dir = workspace.read(cx).files().cache_dir().join("waveforms");
        let thumbnails = Arc::new(MediaThumbnails::new(waveform_dir));
        Self {
            browser: cx.new(|cx| Browser::new(workspace.clone(), thumbnails.clone(), cx)),
            preview: cx.new(|cx| Preview::new(workspace.clone(), cx)),
            inspector: cx.new(|_| Inspector),
            timeline: cx.new(|cx| Timeline::new(workspace.clone(), thumbnails, cx)),
            workspace,
        }
    }
}

pub(crate) struct Shell {
    store: ProjectStore,
    screen: Screen,
    focus_handle: FocusHandle,
    _save_on_quit: Subscription,
}

impl Shell {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = ProjectStore::in_app_data();
        let focus_handle = cx.focus_handle();
        // Actions dispatch from the focused element up, so start focused.
        window.focus(&focus_handle);
        // GPUI doesn't promise to drop entities on quit, so save explicitly.
        let save_on_quit = cx.on_app_quit(|this, cx| {
            if let Screen::Editor(panels) = &this.screen {
                panels
                    .workspace
                    .update(cx, |workspace, _| workspace.save_now());
            }
            async {}
        });
        let screen = Self::projects_screen(&store, window, cx);
        Self {
            store,
            screen,
            focus_handle,
            _save_on_quit: save_on_quit,
        }
    }

    fn projects_screen(
        store: &ProjectStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Screen {
        let store = store.clone();
        let view = cx.new(|cx| ProjectsView::new(store, cx));
        window.focus(view.read(cx).focus_handle());
        window.set_window_title(APP_NAME);
        let open = cx.subscribe_in(&view, window, |this, _, event: &OpenProject, window, cx| {
            this.open_project(event.0.clone(), window, cx);
        });
        Screen::Projects { view, _open: open }
    }

    fn open_project(&mut self, project: Project, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = match Workspace::open(self.store.clone(), project) {
            Ok(workspace) => workspace,
            Err(error) => {
                // The store validates on load, so this is a bug if it happens.
                eprintln!("can't open the project: {error}");
                return;
            }
        };
        window.set_window_title(&format!("{} — {APP_NAME}", workspace.project_name()));
        let workspace = cx.new(|_| workspace);
        self.screen = Screen::Editor(EditorPanels::new(workspace, cx));
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Saves the open project and goes back to the projects list.
    fn close_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Editor(panels) = &self.screen else {
            return;
        };
        panels
            .workspace
            .update(cx, |workspace, _| workspace.save_now());
        // Dropping the panels drops the workspace (which saves again, a
        // no-op now) before the list reads the store.
        self.screen = Self::projects_screen(&self.store, window, cx);
        cx.notify();
    }

    fn workspace(&self) -> Option<&Entity<Workspace>> {
        match &self.screen {
            Screen::Editor(panels) => Some(&panels.workspace),
            Screen::Projects { .. } => None,
        }
    }

    fn render_editor(
        &self,
        panels: &EditorPanels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = window.theme().colors;
        let name = panels.workspace.read(cx).project_name().to_owned();

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(36.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(colors.border)
            .child(
                Button::new("close-project", "← Projects")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, window, cx| this.close_project(window, cx))),
            )
            .child(Label::new(name));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(panels.browser.clone())
                    .child(panels.preview.clone())
                    .child(panels.inspector.clone()),
            )
            .child(panels.timeline.clone())
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;
        let content = match &self.screen {
            Screen::Projects { view, .. } => view.clone().into_any_element(),
            Screen::Editor(panels) => self.render_editor(panels, window, cx).into_any_element(),
        };

        div()
            .id("shell")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if let Some(workspace) = this.workspace() {
                    workspace.update(cx, |workspace, cx| workspace.undo(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if let Some(workspace) = this.workspace() {
                    workspace.update(cx, |workspace, cx| workspace.redo(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ImportMedia, _, cx| {
                if let Some(workspace) = this.workspace() {
                    workspace.update(cx, |workspace, cx| workspace.prompt_import(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let (Some(workspace), Some(item)) = (this.workspace(), cx.read_from_clipboard())
                {
                    workspace.update(cx, |workspace, cx| workspace.paste(&item, cx));
                }
            }))
            .on_action(cx.listener(|this, action: &Transport, _, cx| {
                match this.workspace() {
                    Some(workspace) => {
                        workspace.update(cx, |workspace, cx| workspace.run_transport(*action, cx));
                    }
                    // Not editing: let the key reach whatever is focused.
                    None => cx.propagate(),
                }
            }))
            .on_action(cx.listener(|this, _: &CloseProject, window, cx| {
                this.close_project(window, cx);
            }))
            // Files dropped anywhere on the editor are imported.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(workspace) = this.workspace() {
                    let paths = paths.paths().to_vec();
                    workspace.update(cx, |workspace, cx| workspace.import_paths(paths, cx));
                }
            }))
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(content)
    }
}
