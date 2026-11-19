use opencut_model::time::MediaTime;
use opencut_model::{Element, ElementRef, Project, SceneTracks, Track, TrackKind, new_id};

use crate::animation;
use crate::command::{Command, Outcome, current_tracks};
use crate::error::EditError;
use crate::placement::{
    check_no_overlap, enforce_main_track_start, highest_insert_index, insert_track,
};

/// One copied element and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardItem {
    pub track_id: String,
    pub track_kind: TrackKind,
    pub element: Element,
}

/// Copied timeline elements (classic `ElementsClipboardHandler`). Lives on
/// the [`Editor`](crate::Editor) next to, not inside, the undo history:
/// copying isn't an edit and undo doesn't touch it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clipboard {
    items: Vec<ClipboardItem>,
}

impl Clipboard {
    /// The selected elements, in display order. `None` when none exist.
    pub fn copy(tracks: &SceneTracks, elements: &[ElementRef]) -> Option<Self> {
        let items: Vec<ClipboardItem> = tracks
            .iter()
            .flat_map(|track| {
                track
                    .elements
                    .iter()
                    .filter(|element| {
                        elements.iter().any(|selected| {
                            selected.track_id == track.id && selected.element_id == element.id()
                        })
                    })
                    .map(|element| ClipboardItem {
                        track_id: track.id.clone(),
                        track_kind: track.kind,
                        element: element.clone(),
                    })
            })
            .collect();
        (!items.is_empty()).then_some(Self { items })
    }

    pub fn items(&self) -> &[ClipboardItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Pastes copied elements so the earliest one starts at `at`, keeping their
/// spacing, and selects the copies (classic `PasteCommand`). Each source
/// track's elements go on the track just above it if they fit, else the
/// first track of that kind with room, else a new track.
pub struct PasteElements {
    pub items: Vec<ClipboardItem>,
    pub at: MediaTime,
}

impl PasteElements {
    pub fn new(clipboard: &Clipboard, at: MediaTime) -> Self {
        Self {
            items: clipboard.items.clone(),
            at,
        }
    }
}

impl Command for PasteElements {
    fn label(&self) -> &'static str {
        "Paste"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let Some(earliest) = self
            .items
            .iter()
            .map(|item| item.element.start_time())
            .min()
        else {
            return Err(EditError::Empty);
        };
        let tracks = current_tracks(project)?;

        // Source tracks in the order the items list them.
        let mut sources: Vec<&str> = Vec::new();
        for item in &self.items {
            if !sources.contains(&item.track_id.as_str()) {
                sources.push(&item.track_id);
            }
        }

        let mut pasted = Vec::new();
        for source in sources {
            let group: Vec<&ClipboardItem> = self
                .items
                .iter()
                .filter(|item| item.track_id == source)
                .collect();
            let kind = group[0].track_kind;
            let mut elements: Vec<Element> = group
                .iter()
                .map(|item| {
                    let mut element = item.element.clone();
                    let base = element.base_mut();
                    base.id = new_id();
                    base.start_time = (self.at + (base.start_time - earliest)).max(MediaTime::ZERO);
                    base.animations = animation::clone_with_new_ids(base.animations.as_ref());
                    element
                })
                .collect();

            let target = target_track(tracks, source, kind, &elements);
            let track_id = match target {
                Some(id) => id,
                None => {
                    let track = Track::new(new_id(), kind);
                    let id = track.id.clone();
                    let index = highest_insert_index(tracks, kind);
                    insert_track(tracks, track, index);
                    id
                }
            };

            // The main track has no leading gap: shift the group to fit.
            let first = elements.iter().map(Element::start_time).min();
            if let Some(first) = first {
                let shifted = enforce_main_track_start(tracks, &track_id, first, &[]);
                if shifted != first {
                    for element in &mut elements {
                        let base = element.base_mut();
                        base.start_time =
                            (base.start_time + (shifted - first)).max(MediaTime::ZERO);
                    }
                }
            }

            let track = tracks
                .track_mut(&track_id)
                .ok_or_else(|| EditError::TrackNotFound(track_id.clone()))?;
            for element in elements {
                pasted.push(ElementRef::new(&track_id, element.id()));
                track.elements.push(element);
            }
            check_no_overlap(track)?;
        }
        Ok(Outcome::select(pasted))
    }
}

/// An existing track of `kind` with room for all `elements`: the one just
/// above `source`, else the first in display order.
fn target_track(
    tracks: &SceneTracks,
    source: &str,
    kind: TrackKind,
    elements: &[Element],
) -> Option<String> {
    let fits = |track: &&Track| {
        track.kind == kind
            && elements
                .iter()
                .all(|element| track.is_span_free(element.start_time(), element.end_time(), &[]))
    };
    let display: Vec<&Track> = tracks.iter().collect();
    let above = display
        .iter()
        .position(|track| track.id == source)
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| display.get(index));
    above
        .filter(|track| fits(track))
        .or_else(|| display.iter().find(|track| fits(track)))
        .map(|track| track.id.clone())
}
