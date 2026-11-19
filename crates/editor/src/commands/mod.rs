//! Built-in edit commands. Each is a plain struct; run it with
//! [`Editor::execute`](crate::Editor::execute).

mod audio;
mod bookmarks;
mod clipboard;
mod elements;
mod media;
mod properties;
mod scenes;
mod settings;
mod tracks;

pub use audio::{ToggleElementsMuted, ToggleSourceAudio};
pub use bookmarks::{MoveBookmark, ToggleBookmark, UpdateBookmark};
pub use clipboard::{Clipboard, ClipboardItem, PasteElements};
pub use elements::{
    DEFAULT_ELEMENT_DURATION, DeleteElements, DuplicateElements, Edge, ElementMove, InsertElement,
    MoveElements, Placement, SetElementTiming, SplitElements, SplitRetain, ToggleElementsHidden,
    UpdateElement,
};
pub use media::{AddMediaAsset, RemoveMediaAssets};
pub use properties::{SetElementParam, SetElementRetime, ToggleSourceAudioSeparation};
pub use scenes::{CreateScene, DeleteScene, RenameScene};
pub use settings::UpdateProjectSettings;
pub use tracks::{AddTrack, RemoveTrack, ToggleTrackMute, ToggleTrackVisibility};
