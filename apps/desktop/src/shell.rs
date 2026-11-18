use std::sync::Arc;

use gpui::{
    App, Context, Entity, ExternalPaths, FocusHandle, KeyBinding, Window, actions, div, prelude::*,
};

use crate::media::PlaceholderThumbnails;
use crate::panels::{
    BROWSER_KEY_CONTEXT, Browser, Inspector, Preview, RemoveSelectedMedia, Timeline,
};
use crate::theme::ActiveTheme;
use crate::workspace::Workspace;

actions!(opencut, [Undo, Redo, ImportMedia, Paste]);

/// The app's key bindings. `secondary` is Cmd on macOS and Ctrl elsewhere.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-z", Undo, None),
        KeyBinding::new("secondary-shift-z", Redo, None),
        KeyBinding::new("secondary-i", ImportMedia, None),
        KeyBinding::new("secondary-v", Paste, None),
        KeyBinding::new("backspace", RemoveSelectedMedia, Some(BROWSER_KEY_CONTEXT)),
        KeyBinding::new("delete", RemoveSelectedMedia, Some(BROWSER_KEY_CONTEXT)),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("ctrl-y", Redo, None)]);
}

// Panels are entities created once in `new`, not via `cx.new` inline in
// `render`, so each keeps its own state across renders instead of being
// torn down and rebuilt on every frame.
pub(crate) struct Shell {
    workspace: Entity<Workspace>,
    browser: Entity<Browser>,
    preview: Entity<Preview>,
    inspector: Entity<Inspector>,
    timeline: Entity<Timeline>,
    focus_handle: FocusHandle,
}

impl Shell {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = cx.new(|_| Workspace::new());
        let browser = cx.new(|cx| {
            // TODO(#3, #4): swap in a provider backed by opencut-media's
            // poster frames and waveforms.
            Browser::new(workspace.clone(), Arc::new(PlaceholderThumbnails), cx)
        });
        let focus_handle = cx.focus_handle();
        // Actions dispatch from the focused element up, so start focused.
        window.focus(&focus_handle);
        Self {
            workspace,
            browser,
            preview: cx.new(|_| Preview),
            inspector: cx.new(|_| Inspector),
            timeline: cx.new(|_| Timeline),
            focus_handle,
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = window.theme().colors;

        div()
            .id("shell")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.workspace
                    .update(cx, |workspace, cx| workspace.undo(cx));
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.workspace
                    .update(cx, |workspace, cx| workspace.redo(cx));
            }))
            .on_action(cx.listener(|this, _: &ImportMedia, _, cx| {
                this.workspace
                    .update(cx, |workspace, cx| workspace.prompt_import(cx));
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let Some(item) = cx.read_from_clipboard() {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.paste(&item, cx));
                }
            }))
            // Files dropped anywhere on the window are imported.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                this.workspace
                    .update(cx, |workspace, cx| workspace.import_paths(paths, cx));
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(
                div()
                    .flex()
                    .h_2_3()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(self.browser.clone())
                    .child(self.preview.clone())
                    .child(self.inspector.clone()),
            )
            .child(self.timeline.clone())
    }
}
