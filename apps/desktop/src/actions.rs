//! Keyboard actions: the classic app's editing shortcuts (classic
//! `actions/definitions.ts`), bound to GPUI actions. [`SHORTCUTS`] is the one
//! table behind both the key bindings and the shortcuts dialog.
//!
//! What each editing action does lives in `opencut_editor`
//! ([`EditorAction`]); these handlers only run it through the workspace, so
//! it's undoable and autosaved like any other edit.

use gpui::{Action, App, ClipboardItem, Div, Entity, KeyBinding, Stateful, actions, prelude::*};
use opencut_editor::EditorAction;

use crate::shell::{Paste, Redo, Undo};
use crate::workspace::{Transport, Workspace};

actions!(
    opencut,
    [
        Split,
        SplitLeft,
        SplitRight,
        DeleteSelected,
        CopySelected,
        DuplicateSelected,
        SelectAll,
        DeselectAll,
        CancelInteraction,
        ToggleMutedSelected,
        ToggleVisibilitySelected,
        ToggleSourceAudio,
        ToggleBookmark,
        ShowShortcuts,
    ]
);

/// Where the editing shortcuts apply: anywhere in the open project except
/// while typing in a text field.
pub(crate) const KEY_CONTEXT: &str = "Editor";
const BINDING_CONTEXT: &str = "Editor && !TextInput";

/// Put on the system clipboard by copy, so a stale file path or image copied
/// earlier isn't imported by the next paste.
const COPIED_ELEMENTS: &str = "OpenCut timeline elements";

pub(crate) struct Shortcut {
    pub description: &'static str,
    pub category: &'static str,
    /// GPUI keystrokes; `secondary` is Cmd on macOS and Ctrl elsewhere.
    pub keys: &'static [&'static str],
    bind: fn(&str) -> KeyBinding,
}

/// Classic's default bindings. Toggle muted, visibility, source audio,
/// bookmark and deselect-all have none in classic; the keys here are new.
pub(crate) const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        description: "Split at playhead",
        category: "Editing",
        keys: &["s"],
        bind: |keys| KeyBinding::new(keys, Split, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Split and remove left",
        category: "Editing",
        keys: &["q"],
        bind: |keys| KeyBinding::new(keys, SplitLeft, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Split and remove right",
        category: "Editing",
        keys: &["w"],
        bind: |keys| KeyBinding::new(keys, SplitRight, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Delete selection",
        category: "Editing",
        keys: &["backspace", "delete"],
        bind: |keys| KeyBinding::new(keys, DeleteSelected, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Copy selected elements",
        category: "Editing",
        keys: &["secondary-c"],
        bind: |keys| KeyBinding::new(keys, CopySelected, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Paste elements at playhead",
        category: "Editing",
        keys: &["secondary-v"],
        bind: |keys| KeyBinding::new(keys, Paste, None),
    },
    Shortcut {
        description: "Duplicate selected elements",
        category: "Editing",
        keys: &["secondary-d"],
        bind: |keys| KeyBinding::new(keys, DuplicateSelected, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Extract or recover source audio",
        category: "Editing",
        keys: &["shift-e"],
        bind: |keys| KeyBinding::new(keys, ToggleSourceAudio, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Mute or unmute selected elements",
        category: "Selection",
        keys: &["shift-m"],
        bind: |keys| KeyBinding::new(keys, ToggleMutedSelected, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Show or hide selected elements",
        category: "Selection",
        keys: &["shift-h"],
        bind: |keys| KeyBinding::new(keys, ToggleVisibilitySelected, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Select all elements",
        category: "Selection",
        keys: &["secondary-a"],
        bind: |keys| KeyBinding::new(keys, SelectAll, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Deselect all elements",
        category: "Selection",
        keys: &["secondary-shift-a"],
        bind: |keys| KeyBinding::new(keys, DeselectAll, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Cancel the current interaction",
        category: "Selection",
        keys: &["escape"],
        bind: |keys| KeyBinding::new(keys, CancelInteraction, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Toggle bookmark at playhead",
        category: "Timeline",
        keys: &["m"],
        bind: |keys| KeyBinding::new(keys, ToggleBookmark, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Undo",
        category: "History",
        keys: &["secondary-z"],
        bind: |keys| KeyBinding::new(keys, Undo, None),
    },
    Shortcut {
        description: "Redo",
        category: "History",
        keys: &["secondary-shift-z", "secondary-y"],
        bind: |keys| KeyBinding::new(keys, Redo, None),
    },
    Shortcut {
        description: "Show keyboard shortcuts",
        category: "Help",
        keys: &["secondary-/"],
        bind: |keys| KeyBinding::new(keys, ShowShortcuts, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Play or pause",
        category: "Playback",
        keys: &["space", "k"],
        bind: |keys| KeyBinding::new(keys, Transport::TogglePlay, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Stop playback",
        category: "Playback",
        keys: &["shift-space"],
        bind: |keys| KeyBinding::new(keys, Transport::StopPlayback, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Seek forward 1 s",
        category: "Playback",
        keys: &["l"],
        bind: |keys| KeyBinding::new(keys, Transport::SeekForward, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Seek backward 1 s",
        category: "Playback",
        keys: &["j"],
        bind: |keys| KeyBinding::new(keys, Transport::SeekBackward, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Step one frame forward",
        category: "Playback",
        keys: &["right"],
        bind: |keys| KeyBinding::new(keys, Transport::FrameStepForward, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Step one frame backward",
        category: "Playback",
        keys: &["left"],
        bind: |keys| KeyBinding::new(keys, Transport::FrameStepBackward, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Jump forward 5 s",
        category: "Playback",
        keys: &["shift-right"],
        bind: |keys| KeyBinding::new(keys, Transport::JumpForward, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Jump backward 5 s",
        category: "Playback",
        keys: &["shift-left"],
        bind: |keys| KeyBinding::new(keys, Transport::JumpBackward, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Go to start",
        category: "Playback",
        keys: &["home", "enter"],
        bind: |keys| KeyBinding::new(keys, Transport::GotoStart, Some(BINDING_CONTEXT)),
    },
    Shortcut {
        description: "Go to end",
        category: "Playback",
        keys: &["end"],
        bind: |keys| KeyBinding::new(keys, Transport::GotoEnd, Some(BINDING_CONTEXT)),
    },
];

pub(crate) fn bind_keys(cx: &mut App) {
    for shortcut in SHORTCUTS {
        cx.bind_keys(shortcut.keys.iter().map(|keys| (shortcut.bind)(keys)));
    }
}

/// A binding as shown to the user, e.g. `secondary-shift-z` as `Cmd+Shift+Z`.
pub(crate) fn display_keys(keys: &str) -> String {
    keys.split('-')
        .map(|part| match part {
            "secondary" if cfg!(target_os = "macos") => "Cmd".to_owned(),
            "secondary" => "Ctrl".to_owned(),
            "shift" => "Shift".to_owned(),
            "escape" => "Esc".to_owned(),
            "backspace" => "Backspace".to_owned(),
            "delete" => "Delete".to_owned(),
            other => other.to_uppercase(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

impl Workspace {
    /// Runs an editing action on the current selection and playhead; does
    /// nothing when it has no target (nothing selected, empty clipboard).
    pub(crate) fn run_action(&mut self, action: EditorAction, cx: &mut gpui::Context<Self>) {
        if let Some(mut command) = self.editor().command_for(action) {
            self.execute(command.as_mut(), cx);
        }
    }

    /// Pastes the copied elements at the playhead.
    pub(crate) fn paste_elements(&mut self, cx: &mut gpui::Context<Self>) {
        self.run_action(EditorAction::PasteCopied, cx);
    }
}

/// Whether the system clipboard holds what copy left there, meaning the
/// next paste is of elements.
pub(crate) fn clipboard_is_copied_elements(item: Option<&ClipboardItem>) -> bool {
    item.and_then(ClipboardItem::text).as_deref() == Some(COPIED_ELEMENTS)
}

/// Registers the handlers that only need the workspace. The shell registers
/// the rest (paste, shortcuts dialog, cancel), which also touch its own state.
pub(crate) fn register(root: Stateful<Div>, workspace: &Entity<Workspace>) -> Stateful<Div> {
    fn edit<A: Action>(
        root: Stateful<Div>,
        workspace: &Entity<Workspace>,
        action: EditorAction,
    ) -> Stateful<Div> {
        let workspace = workspace.clone();
        root.on_action(move |_: &A, _, cx| {
            workspace.update(cx, |workspace, cx| workspace.run_action(action, cx));
        })
    }

    let copy = workspace.clone();
    let select_all = workspace.clone();
    let deselect_all = workspace.clone();
    let root = edit::<Split>(root, workspace, EditorAction::Split);
    let root = edit::<SplitLeft>(root, workspace, EditorAction::SplitLeft);
    let root = edit::<SplitRight>(root, workspace, EditorAction::SplitRight);
    let root = edit::<DeleteSelected>(root, workspace, EditorAction::DeleteSelected);
    let root = edit::<DuplicateSelected>(root, workspace, EditorAction::DuplicateSelected);
    let root =
        edit::<ToggleMutedSelected>(root, workspace, EditorAction::ToggleElementsMutedSelected);
    let root = edit::<ToggleVisibilitySelected>(
        root,
        workspace,
        EditorAction::ToggleElementsVisibilitySelected,
    );
    let root = edit::<ToggleSourceAudio>(root, workspace, EditorAction::ToggleSourceAudio);
    let root = edit::<ToggleBookmark>(root, workspace, EditorAction::ToggleBookmark);
    root.on_action(move |_: &CopySelected, _, cx| {
        if copy.update(cx, |workspace, _| workspace.copy_selection()) {
            cx.write_to_clipboard(ClipboardItem::new_string(COPIED_ELEMENTS.to_owned()));
        }
    })
    .on_action(move |_: &SelectAll, _, cx| {
        select_all.update(cx, |workspace, cx| workspace.select_all(cx));
    })
    .on_action(move |_: &DeselectAll, _, cx| {
        deselect_all.update(cx, |workspace, cx| workspace.set_selection(Vec::new(), cx));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_naturally() {
        let secondary = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        assert_eq!(
            display_keys("secondary-shift-z"),
            format!("{secondary}+Shift+Z")
        );
        assert_eq!(display_keys("escape"), "Esc");
        assert_eq!(display_keys("s"), "S");
    }

    #[test]
    fn every_shortcut_has_a_key() {
        assert!(SHORTCUTS.iter().all(|shortcut| !shortcut.keys.is_empty()));
    }
}
