use std::collections::HashSet;

use opencut_model::time::MediaTime;
use opencut_model::{
    CanvasSize, Element, ElementKind, ElementRef, MediaAsset, MediaType, ParamValues, Project,
    RetimeConfig, SceneTracks, Track, TrackKind, new_id,
};

use crate::animation;
use crate::command::{Command, Outcome, current_tracks};
use crate::commands::media::frame_rate_from_f64;
use crate::error::EditError;
use crate::placement::{
    check_compatible, check_no_overlap, check_timing, default_insert_index,
    enforce_main_track_start, find_free_track, highest_insert_index, insert_track,
};

fn element_mut<'a>(
    tracks: &'a mut SceneTracks,
    element_ref: &ElementRef,
) -> Result<&'a mut Element, EditError> {
    tracks
        .element_mut(element_ref)
        .ok_or_else(|| EditError::ElementNotFound(element_ref.clone()))
}

fn take_element(tracks: &mut SceneTracks, element_ref: &ElementRef) -> Result<Element, EditError> {
    let track = tracks
        .track_mut(&element_ref.track_id)
        .ok_or_else(|| EditError::ElementNotFound(element_ref.clone()))?;
    let index = track
        .elements
        .iter()
        .position(|element| element.id() == element_ref.element_id)
        .ok_or_else(|| EditError::ElementNotFound(element_ref.clone()))?;
    Ok(track.elements.remove(index))
}

fn track_mut<'a>(tracks: &'a mut SceneTracks, track_id: &str) -> Result<&'a mut Track, EditError> {
    tracks
        .track_mut(track_id)
        .ok_or_else(|| EditError::TrackNotFound(track_id.to_owned()))
}

/// Where [`InsertElement`] puts the element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// On this track; fails if the element doesn't fit there.
    Track(String),
    /// On the first compatible track with room, or a new track.
    Auto,
}

/// Adds an element to the timeline and selects it.
///
/// The first video or image on an empty timeline also sizes the canvas to
/// its media and, for a video, sets the project frame rate to the video's
/// (classic `InsertElementCommand`).
pub struct InsertElement {
    element: Element,
    placement: Placement,
    track_id: Option<String>,
}

impl InsertElement {
    /// An empty element id is replaced with a fresh one.
    pub fn new(mut element: Element, placement: Placement) -> Self {
        if element.id().is_empty() {
            element.base_mut().id = new_id();
        }
        Self {
            element,
            placement,
            track_id: None,
        }
    }

    pub fn element_id(&self) -> &str {
        self.element.id()
    }

    /// The track the element landed on, once the command has run.
    pub fn track_id(&self) -> Option<&str> {
        self.track_id.as_deref()
    }
}

impl Command for InsertElement {
    fn label(&self) -> &'static str {
        "Insert element"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        check_timing(&self.element)?;
        let first_media = first_visual_media(project, &self.element);
        let tracks = current_tracks(project)?;
        let mut element = self.element.clone();
        let kind = element.kind().track_kind();

        let track_id = match &self.placement {
            Placement::Track(track_id) => track_id.clone(),
            Placement::Auto => {
                find_free_track(tracks, kind, element.start_time(), element.end_time())
                    .unwrap_or_else(|| {
                        let track = Track::new(new_id(), kind);
                        let id = track.id.clone();
                        let index = default_insert_index(tracks, kind);
                        insert_track(tracks, track, index);
                        id
                    })
            }
        };

        element.base_mut().start_time =
            enforce_main_track_start(tracks, &track_id, element.start_time(), &[]);
        let track = track_mut(tracks, &track_id)?;
        check_compatible(&element, track)?;
        track.elements.push(element);
        check_no_overlap(track)?;

        if let Some(asset) = first_media {
            adopt_media_settings(project, &asset);
        }

        self.track_id = Some(track_id.clone());
        Ok(Outcome::select(vec![ElementRef::new(
            track_id,
            self.element.id(),
        )]))
    }
}

/// The asset behind `element` when it is the first element of the current
/// scene and a video or image.
fn first_visual_media(project: &Project, element: &Element) -> Option<MediaAsset> {
    if !matches!(element.kind(), ElementKind::Video | ElementKind::Image) {
        return None;
    }
    if project.current_scene()?.tracks.element_count() != 0 {
        return None;
    }
    project.media_asset(element.media_id()?).cloned()
}

/// Canvas size (and the original size, if unset) from the media's
/// dimensions; frame rate from a video's. Rates the project can't use are
/// ignored.
fn adopt_media_settings(project: &mut Project, asset: &MediaAsset) {
    let settings = &mut project.settings;
    if let (Some(width), Some(height)) = (asset.width, asset.height)
        && width > 0
        && height > 0
    {
        let size = CanvasSize { width, height };
        settings.canvas_size = size;
        settings.original_canvas_size.get_or_insert(size);
    }
    if asset.media_type == MediaType::Video
        && let Some(fps) = asset.fps.and_then(frame_rate_from_f64)
    {
        settings.fps = fps;
    }
}

/// Removes elements and clears the selection.
pub struct DeleteElements {
    pub elements: Vec<ElementRef>,
}

impl Command for DeleteElements {
    fn label(&self) -> &'static str {
        "Delete"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        if self.elements.is_empty() {
            return Err(EditError::Empty);
        }
        let tracks = current_tracks(project)?;
        for element_ref in &self.elements {
            take_element(tracks, element_ref)?;
        }
        Ok(Outcome::select(Vec::new()))
    }
}

/// One element's destination in a [`MoveElements`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementMove {
    pub element: ElementRef,
    pub target_track_id: String,
    pub start_time: MediaTime,
}

/// Moves elements in time and/or to other tracks of the same kind. All moves
/// apply together, so elements may swap places. Fails on any overlap.
pub struct MoveElements {
    pub moves: Vec<ElementMove>,
}

impl Command for MoveElements {
    fn label(&self) -> &'static str {
        "Move"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        if self.moves.is_empty() {
            return Err(EditError::Empty);
        }
        let tracks = current_tracks(project)?;
        let moved_ids: Vec<&str> = self
            .moves
            .iter()
            .map(|planned| planned.element.element_id.as_str())
            .collect();

        let mut taken = Vec::with_capacity(self.moves.len());
        for planned in &self.moves {
            taken.push(take_element(tracks, &planned.element)?);
        }

        let mut touched = HashSet::new();
        let mut selection = Vec::with_capacity(self.moves.len());
        for (planned, mut element) in self.moves.iter().zip(taken) {
            if planned.start_time < MediaTime::ZERO {
                return Err(EditError::InvalidTiming);
            }
            element.base_mut().start_time = planned.start_time;
            let track = track_mut(tracks, &planned.target_track_id)?;
            check_compatible(&element, track)?;
            track.elements.push(element);
            touched.insert(planned.target_track_id.clone());
            selection.push(ElementRef::new(
                &planned.target_track_id,
                &planned.element.element_id,
            ));
        }

        close_main_track_gap(tracks, &moved_ids);
        for track_id in &touched {
            check_no_overlap(track_mut(tracks, track_id)?)?;
        }
        Ok(Outcome::select(selection))
    }
}

/// Group form of [`enforce_main_track_start`]: when a moved element ends up
/// first on the main track, shift the moved main-track elements together so
/// the track starts at zero, keeping their spacing.
fn close_main_track_gap(tracks: &mut SceneTracks, moved_ids: &[&str]) {
    let main = &mut tracks.main;
    let Some(first) = main
        .elements
        .iter()
        .min_by_key(|element| element.start_time())
    else {
        return;
    };
    let gap = first.start_time();
    if gap <= MediaTime::ZERO || !moved_ids.contains(&first.id()) {
        return;
    }
    for element in &mut main.elements {
        if moved_ids.contains(&element.id()) {
            element.base_mut().start_time -= gap;
        }
    }
}

/// Sets an element's position, length and trims at once: what dragging a clip
/// edge does. Trims are in source time.
pub struct SetElementTiming {
    pub element: ElementRef,
    pub start_time: MediaTime,
    pub duration: MediaTime,
    pub trim_start: MediaTime,
    pub trim_end: MediaTime,
}

impl Command for SetElementTiming {
    fn label(&self) -> &'static str {
        "Trim"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let element = element_mut(tracks, &self.element)?;
        let base = element.base_mut();
        base.start_time = self.start_time;
        base.duration = self.duration;
        base.trim_start = self.trim_start;
        base.trim_end = self.trim_end;
        if let Some(source) = base.source_duration
            && self.trim_start + self.trim_end >= source
        {
            return Err(EditError::InvalidTiming);
        }
        check_timing(element)?;
        check_no_overlap(track_mut(tracks, &self.element.track_id)?)?;
        Ok(Outcome::unchanged())
    }
}

/// Which halves a split keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SplitRetain {
    #[default]
    Both,
    /// Keep the part before the split (classic "split left").
    Left,
    /// Keep the part after the split (classic "split right").
    Right,
}

/// Cuts elements at a timeline time. Elements the time doesn't fall strictly
/// inside are left alone. Right halves get new ids and become the selection.
pub struct SplitElements {
    pub elements: Vec<ElementRef>,
    pub at: MediaTime,
    pub retain: SplitRetain,
}

impl SplitElements {
    pub fn new(elements: Vec<ElementRef>, at: MediaTime) -> Self {
        Self {
            elements,
            at,
            retain: SplitRetain::Both,
        }
    }
}

/// Source time covered by `clip_time` of timeline time at the element's rate.
fn source_span(clip_time: MediaTime, retime: Option<&RetimeConfig>) -> MediaTime {
    clip_time
        .mul_f64_round(RetimeConfig::safe_rate(retime))
        .unwrap_or(clip_time)
        .max(MediaTime::ZERO)
}

/// Splits one element. Returns the left and right halves.
fn split_element(element: &Element, at: MediaTime) -> (Element, Element) {
    let base = element.base();
    let relative = at - base.start_time;
    let retime = element.retime();
    // Round the source split point once and derive the right half from it, so
    // left + right source spans always equal the whole.
    let left_source = source_span(relative, retime);
    let total_source = source_span(base.duration, retime);
    let right_source = total_source - left_source;
    let (left_animations, right_animations) =
        animation::split_at(base.animations.as_ref(), relative);

    let mut left = element.clone();
    let left_base = left.base_mut();
    left_base.duration = relative;
    left_base.trim_end = base.trim_end + right_source;
    left_base.name = format!("{} (left)", base.name);
    left_base.animations = left_animations;

    let mut right = element.clone();
    let right_base = right.base_mut();
    right_base.id = new_id();
    right_base.start_time = at;
    right_base.duration = base.duration - relative;
    right_base.trim_start = base.trim_start + left_source;
    right_base.name = format!("{} (right)", base.name);
    right_base.animations = right_animations;

    (left, right)
}

impl Command for SplitElements {
    fn label(&self) -> &'static str {
        "Split"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let mut right_refs = Vec::new();
        let mut split_any = false;

        for element_ref in &self.elements {
            let track = track_mut(tracks, &element_ref.track_id)?;
            let index = track
                .elements
                .iter()
                .position(|element| element.id() == element_ref.element_id)
                .ok_or_else(|| EditError::ElementNotFound(element_ref.clone()))?;
            let element = &track.elements[index];
            if self.at <= element.start_time() || self.at >= element.end_time() {
                continue;
            }
            split_any = true;

            let (left, right) = split_element(element, self.at);
            match self.retain {
                SplitRetain::Both => {
                    right_refs.push(ElementRef::new(&track.id, right.id()));
                    track.elements[index] = left;
                    track.elements.insert(index + 1, right);
                }
                SplitRetain::Left => track.elements[index] = left,
                SplitRetain::Right => {
                    right_refs.push(ElementRef::new(&track.id, right.id()));
                    track.elements[index] = right;
                }
            }
        }

        if !split_any {
            return Err(EditError::NothingToSplit(self.at));
        }
        Ok(if right_refs.is_empty() {
            Outcome::unchanged()
        } else {
            Outcome::select(right_refs)
        })
    }
}

/// Copies elements onto a new track per source track, placed as high as that
/// kind allows, at the same times. Selects the copies.
pub struct DuplicateElements {
    pub elements: Vec<ElementRef>,
}

impl Command for DuplicateElements {
    fn label(&self) -> &'static str {
        "Duplicate"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        if self.elements.is_empty() {
            return Err(EditError::Empty);
        }
        let tracks = current_tracks(project)?;
        for element_ref in &self.elements {
            if tracks.element(element_ref).is_none() {
                return Err(EditError::ElementNotFound(element_ref.clone()));
            }
        }

        // Source tracks in display order, each with its copies.
        let mut groups: Vec<(TrackKind, Vec<Element>)> = Vec::new();
        for track in tracks.iter() {
            let copies: Vec<Element> = track
                .elements
                .iter()
                .filter(|element| {
                    self.elements.iter().any(|selected| {
                        selected.track_id == track.id && selected.element_id == element.id()
                    })
                })
                .map(|element| {
                    let mut copy = element.clone();
                    let base = copy.base_mut();
                    base.id = new_id();
                    base.name = format!("{} (copy)", element.base().name);
                    base.animations =
                        animation::clone_with_new_ids(element.base().animations.as_ref());
                    copy
                })
                .collect();
            if !copies.is_empty() {
                groups.push((track.kind, copies));
            }
        }

        let mut selection = Vec::new();
        for (kind, copies) in groups {
            let mut track = Track::new(new_id(), kind);
            for copy in &copies {
                selection.push(ElementRef::new(&track.id, copy.id()));
            }
            track.elements = copies;
            let index = highest_insert_index(tracks, kind);
            insert_track(tracks, track, index);
        }
        Ok(Outcome::select(selection))
    }
}

/// Hides the elements if any of them is visible, otherwise shows them all.
/// Audio and effect elements can't be hidden and are skipped.
pub struct ToggleElementsHidden {
    pub elements: Vec<ElementRef>,
}

impl Command for ToggleElementsHidden {
    fn label(&self) -> &'static str {
        "Toggle visibility"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let mut any_visible = false;
        let mut any_hideable = false;
        for element_ref in &self.elements {
            let element = tracks
                .element(element_ref)
                .ok_or_else(|| EditError::ElementNotFound(element_ref.clone()))?;
            if let Some(hidden) = element.is_hidden() {
                any_hideable = true;
                any_visible |= !hidden;
            }
        }
        if !any_hideable {
            return Err(EditError::Empty);
        }
        for element_ref in &self.elements {
            element_mut(tracks, element_ref)?.set_hidden(any_visible);
        }
        Ok(Outcome::unchanged())
    }
}

/// Renames an element and/or merges parameter values into it.
pub struct UpdateElement {
    pub element: ElementRef,
    pub name: Option<String>,
    pub params: ParamValues,
}

impl Command for UpdateElement {
    fn label(&self) -> &'static str {
        "Edit element"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let base = element_mut(tracks, &self.element)?.base_mut();
        if let Some(name) = &self.name {
            base.name.clone_from(name);
        }
        base.params.extend(
            self.params
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Ok(Outcome::unchanged())
    }
}
