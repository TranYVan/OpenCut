//! Headless OpenCut editor.
//!
//! [`Editor`] owns an open [`Project`](opencut_model::Project) and changes it
//! only through [`Command`]s, which makes every edit undoable and lets the
//! desktop UI, scripting and a future MCP server share one code path. Ported
//! from the classic app's `core/managers` and `commands` (tag `pre-rewrite`).

mod actions;
mod animation;
mod command;
pub mod commands;
pub mod drop_target;
mod editor;
mod error;
pub mod group_edit;
pub mod placement;
mod playback;
pub mod ripple;
mod selection;
pub mod snapping;

pub use actions::EditorAction;
pub use command::{Batch, Command, Outcome};
pub use commands::{Clipboard, ClipboardItem};
pub use editor::{Editor, HISTORY_LIMIT};
pub use error::EditError;
pub use playback::Playback;
pub use selection::Selection;

pub use opencut_model as model;
