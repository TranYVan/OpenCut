use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Window, div,
    prelude::*, px,
};

use crate::theme::ActiveTheme;

/// What a [`TextField`] reports to whoever owns it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextFieldEvent {
    /// Enter was pressed.
    Submit,
    /// Escape was pressed.
    Cancel,
}

/// A single-line text input (shadcn `Input`). Unlike the other primitives
/// it's an `Entity`, since it keeps its text and focus between renders.
///
/// Deliberately minimal: typing appends, Backspace deletes the last
/// character (with Cmd/Ctrl or Alt, everything). No caret movement,
/// selection, clipboard or IME. GPUI 0.2.2 has no input element, and a full
/// one means implementing `EntityInputHandler`; do that here when a screen
/// needs more than a short name.
pub(crate) struct TextField {
    text: String,
    placeholder: SharedString,
    focus_handle: FocusHandle,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextField {
    pub(crate) fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" => cx.emit(TextFieldEvent::Submit),
            "escape" => cx.emit(TextFieldEvent::Cancel),
            "backspace" if modifiers.secondary() || modifiers.alt => self.text.clear(),
            "backspace" => {
                self.text.pop();
            }
            _ if modifiers.control || modifiers.platform => return,
            _ => match &keystroke.key_char {
                Some(typed) if typed.chars().all(|c| !c.is_control()) => {
                    self.text.push_str(typed);
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = window.theme();
        let colors = theme.colors;
        let focused = self.focus_handle.is_focused(window);
        let empty = self.text.is_empty();

        div()
            .id("text-field")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_click(cx.listener(|this, _, window, _| window.focus(&this.focus_handle)))
            .flex()
            .items_center()
            .w_full()
            .h(px(32.0))
            .px(px(10.0))
            .rounded(theme.radius)
            .border_1()
            .border_color(if focused { colors.ring } else { colors.input })
            .bg(colors.input.opacity(0.3))
            .text_sm()
            .cursor_text()
            .overflow_hidden()
            .when(!empty, |this| {
                this.child(
                    div()
                        .text_color(colors.foreground)
                        .whitespace_nowrap()
                        .child(self.text.clone()),
                )
            })
            // The caret sits at the end: typing only appends.
            .when(focused, |this| {
                this.child(div().w(px(1.0)).h(px(16.0)).bg(colors.foreground))
            })
            .when(empty, |this| {
                this.child(
                    div()
                        .pl(px(2.0))
                        .text_color(colors.muted_foreground)
                        .child(self.placeholder.clone()),
                )
            })
    }
}
