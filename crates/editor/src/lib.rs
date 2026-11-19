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
mod editor;
mod error;
pub mod placement;
mod playback;
mod selection;

pub use actions::EditorAction;
pub use command::{Batch, Command, Outcome};
pub use commands::{Clipboard, ClipboardItem};
pub use editor::{Editor, HISTORY_LIMIT};
pub use error::EditError;
pub use playback::Playback;
pub use selection::Selection;

pub use opencut_model as model;
