//! Built-in edit commands. Each is a plain struct; run it with
//! [`Editor::execute`](crate::Editor::execute).

mod elements;
mod media;
mod settings;
mod tracks;

pub use elements::{
    DEFAULT_ELEMENT_DURATION, DeleteElements, DuplicateElements, Edge, ElementMove, InsertElement,
    MoveElements, Placement, SetElementTiming, SplitElements, SplitRetain, ToggleElementsHidden,
    UpdateElement,
};
pub use media::{AddMediaAsset, RemoveMediaAssets};
pub use settings::UpdateProjectSettings;
pub use tracks::{AddTrack, RemoveTrack, ToggleTrackMute, ToggleTrackVisibility};
