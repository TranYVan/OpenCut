//! The classic app's keyboard actions that edit the timeline (classic
//! `actions/use-editor-actions`), minus playback, which has its own
//! controls. [`Editor::command_for`] turns one into the [`Command`] to
//! execute, so the desktop UI, scripts and an MCP server share the rules for
//! what each action does to the current selection and playhead.

use opencut_model::ElementRef;

use crate::command::Command;
use crate::commands::{
    Clipboard, DeleteElements, DuplicateElements, PasteElements, SplitElements, SplitRetain,
    ToggleBookmark, ToggleElementsHidden, ToggleElementsMuted, ToggleSourceAudio,
};
use crate::editor::Editor;

/// An editing action that runs as a [`Command`]. Undo, redo, select-all,
/// deselect-all and copy don't change the project, so they're `Editor`
/// methods instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    /// Cut the selection (or the elements under the playhead) at the playhead.
    Split,
    /// Split and drop the part before the playhead.
    SplitLeft,
    /// Split and drop the part after the playhead.
    SplitRight,
    DeleteSelected,
    DuplicateSelected,
    PasteCopied,
    ToggleElementsMutedSelected,
    ToggleElementsVisibilitySelected,
    /// Extract a selected video's audio, or recover it.
    ToggleSourceAudio,
    ToggleBookmark,
}

impl Editor {
    /// The command for `action` given the current selection, playhead and
    /// clipboard; `None` when it has nothing to act on.
    pub fn command_for(&self, action: EditorAction) -> Option<Box<dyn Command>> {
        let selected = self.selection().elements().to_vec();
        let at = self.playback().position;
        let command: Box<dyn Command> = match action {
            EditorAction::Split | EditorAction::SplitLeft | EditorAction::SplitRight => {
                let elements = self.elements_to_split();
                if elements.is_empty() {
                    return None;
                }
                let retain = match action {
                    EditorAction::SplitLeft => SplitRetain::Right,
                    EditorAction::SplitRight => SplitRetain::Left,
                    _ => SplitRetain::Both,
                };
                Box::new(SplitElements {
                    elements,
                    at,
                    retain,
                })
            }
            EditorAction::DeleteSelected if !selected.is_empty() => {
                Box::new(DeleteElements { elements: selected })
            }
            EditorAction::DuplicateSelected if !selected.is_empty() => {
                Box::new(DuplicateElements { elements: selected })
            }
            EditorAction::ToggleElementsMutedSelected if !selected.is_empty() => {
                Box::new(ToggleElementsMuted { elements: selected })
            }
            EditorAction::ToggleElementsVisibilitySelected if !selected.is_empty() => {
                Box::new(ToggleElementsHidden { elements: selected })
            }
            // Classic acts on a single selected element only.
            EditorAction::ToggleSourceAudio if selected.len() == 1 => Box::new(ToggleSourceAudio {
                element: selected[0].clone(),
            }),
            EditorAction::PasteCopied if !self.clipboard.is_empty() => {
                Box::new(PasteElements::new(&self.clipboard, at))
            }
            EditorAction::ToggleBookmark => Box::new(ToggleBookmark { time: at }),
            _ => return None,
        };
        Some(command)
    }

    /// The selection, or with nothing selected every element under the
    /// playhead; only those the playhead falls strictly inside, since the
    /// others can't be cut.
    fn elements_to_split(&self) -> Vec<ElementRef> {
        let at = self.playback().position;
        let tracks = self.tracks();
        let candidates: Vec<ElementRef> = if self.selection().is_empty() {
            self.all_elements()
        } else {
            self.selection().elements().to_vec()
        };
        candidates
            .into_iter()
            .filter(|element| {
                tracks
                    .element(element)
                    .is_some_and(|found| found.start_time() < at && at < found.end_time())
            })
            .collect()
    }

    fn all_elements(&self) -> Vec<ElementRef> {
        self.tracks()
            .iter()
            .flat_map(|track| {
                track
                    .elements
                    .iter()
                    .map(|element| ElementRef::new(&track.id, element.id()))
            })
            .collect()
    }

    /// Copies the selected elements to the editor's clipboard. Returns
    /// `false`, leaving the clipboard as it was, when nothing is selected.
    pub fn copy_selection(&mut self) -> bool {
        match Clipboard::copy(self.tracks(), self.selection().elements()) {
            Some(clipboard) => {
                self.clipboard = clipboard;
                true
            }
            None => false,
        }
    }

    pub fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    pub fn select_all(&mut self) {
        let all = self.all_elements();
        self.selection_mut().set(all);
    }

    pub fn deselect_all(&mut self) {
        self.selection_mut().clear();
    }
}
