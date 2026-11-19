use std::collections::HashSet;

use opencut_model::time::{FrameRate, MediaTime, TICKS_PER_SECOND};
use opencut_model::{
    AudioElement, AudioSource, CanvasSize, Element, ElementBase, ElementKind, ElementRef,
    ImageElement, MediaAsset, MediaType, ParamValues, Project, RetimeConfig, SceneTracks, Track,
    TrackKind, VideoElement, new_id,
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

/// Length of a new element whose media has no duration (classic
/// `DEFAULT_NEW_ELEMENT_DURATION`).
pub const DEFAULT_ELEMENT_DURATION: MediaTime = MediaTime::from_ticks(5 * TICKS_PER_SECOND);

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

    /// An element playing all of `asset`, starting at `start` (classic
    /// `buildElementFromMedia`). Media without a duration (images) gets
    /// [`DEFAULT_ELEMENT_DURATION`].
    pub fn from_media(asset: &MediaAsset, start: MediaTime, placement: Placement) -> Self {
        let duration = asset
            .duration
            .and_then(MediaTime::from_seconds_f64)
            .filter(|duration| *duration > MediaTime::ZERO)
            .unwrap_or(DEFAULT_ELEMENT_DURATION);
        let mut base = ElementBase::new(new_id(), asset.name.clone(), start, duration);
        let media_id = asset.id.clone();
        let element = match asset.media_type {
            MediaType::Image => Element::Image(ImageElement {
                base,
                media_id,
                hidden: Some(false),
                effects: None,
                masks: None,
            }),
            MediaType::Video => {
                base.source_duration = Some(duration);
                Element::Video(VideoElement {
                    base,
                    media_id,
                    is_source_audio_enabled: Some(true),
                    hidden: Some(false),
                    retime: None,
                    effects: None,
                    masks: None,
                })
            }
            MediaType::Audio => {
                base.source_duration = Some(duration);
                Element::Audio(AudioElement {
                    base,
                    source: AudioSource::Upload { media_id },
                    retime: None,
                })
            }
        };
        Self::new(element, placement)
    }

    pub fn element_id(&self) -> &str {
        self.element.id()
    }

    /// The element as it will be inserted (before main-track snapping).
    pub fn element(&self) -> &Element {
        &self.element
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

impl MoveElements {
    /// The move for dragging the `selection` by its `anchor` element so the
    /// anchor starts at `anchor_start` on `target_track_id` (classic
    /// `resolveGroupMove` onto existing tracks). Each source track maps to
    /// one target track: the anchor's to the target, the ones above and
    /// below it to the nearest unused track of their kind in that
    /// direction. A target of the wrong kind keeps the anchor on its track.
    /// The group never starts before zero. `None` when an element isn't
    /// found or a track has nowhere to go; overlaps are left for `apply` to
    /// reject.
    pub fn drag(
        tracks: &SceneTracks,
        selection: &[ElementRef],
        anchor: &ElementRef,
        target_track_id: &str,
        anchor_start: MediaTime,
    ) -> Option<Self> {
        let display: Vec<&Track> = tracks.iter().collect();
        let index_of = |track_id: &str| display.iter().position(|track| track.id == track_id);
        let anchor_element = tracks.element(anchor)?;
        let anchor_index = index_of(&anchor.track_id)?;

        let mut members: Vec<(&ElementRef, MediaTime, usize)> = Vec::new();
        for element_ref in std::iter::once(anchor).chain(selection) {
            if members
                .iter()
                .any(|(seen, _, _)| seen.element_id == element_ref.element_id)
            {
                continue;
            }
            let offset = tracks.element(element_ref)?.start_time() - anchor_element.start_time();
            members.push((element_ref, offset, index_of(&element_ref.track_id)?));
        }

        let target_index = index_of(target_track_id)
            .filter(|index| display[*index].kind == display[anchor_index].kind)
            .unwrap_or(anchor_index);
        let mut sources: Vec<usize> = members.iter().map(|(_, _, index)| *index).collect();
        sources.sort_unstable();
        sources.dedup();
        // (source display index, target display index)
        let mut targets = vec![(anchor_index, target_index)];
        let free = |source: usize, target: usize, taken: &[(usize, usize)]| {
            display[target].kind == display[source].kind
                && !taken.iter().any(|(_, used)| *used == target)
        };
        let mut bound = target_index;
        for &source in sources.iter().rev().filter(|index| **index < anchor_index) {
            bound = (0..bound).rev().find(|t| free(source, *t, &targets))?;
            targets.push((source, bound));
        }
        let mut bound = target_index;
        for &source in sources.iter().filter(|index| **index > anchor_index) {
            bound = (bound + 1..display.len()).find(|t| free(source, *t, &targets))?;
            targets.push((source, bound));
        }

        let earliest = members
            .iter()
            .map(|(_, offset, _)| *offset)
            .min()
            .unwrap_or(MediaTime::ZERO);
        let anchor_start = anchor_start.max(MediaTime::ZERO - earliest.min(MediaTime::ZERO));
        let moves = members
            .iter()
            .map(|(element_ref, offset, source)| {
                let (_, target) = targets.iter().find(|(from, _)| from == source)?;
                Some(ElementMove {
                    element: (*element_ref).clone(),
                    target_track_id: display[*target].id.clone(),
                    start_time: anchor_start + *offset,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { moves })
    }
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

/// Which edge of an element a trim drags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

impl SetElementTiming {
    /// The timing for dragging an element's `edge` by `delta` (classic
    /// `computeGroupResize` for one element): the delta snaps to a frame
    /// and is clamped so the element stays at least a frame long and
    /// doesn't pass a neighbour, time zero, or the ends of its media.
    /// `None` when the element isn't found.
    pub fn drag_edge(
        tracks: &SceneTracks,
        element_ref: &ElementRef,
        edge: Edge,
        delta: MediaTime,
        fps: FrameRate,
    ) -> Option<Self> {
        let element = tracks.element(element_ref)?;
        let track = tracks.track(&element_ref.track_id)?;
        let base = element.base();
        let (start, end) = (base.start_time, base.end_time());
        let rate = RetimeConfig::safe_rate(element.retime());
        let to_timeline = |source: MediaTime| source.div_f64_round(rate).unwrap_or(source);
        let frame = MediaTime::from_frame(1, fps).unwrap_or(MediaTime::from_ticks(1));
        let others = track
            .elements
            .iter()
            .filter(|other| other.id() != element.id());

        let (min, max) = match edge {
            Edge::Start => {
                let neighbour = others
                    .map(Element::end_time)
                    .filter(|other_end| *other_end <= start)
                    .max()
                    .unwrap_or(MediaTime::ZERO);
                let mut min = neighbour - start;
                if base.source_duration.is_some() {
                    min = min.max(MediaTime::ZERO - to_timeline(base.trim_start));
                }
                (min, Some(base.duration - frame))
            }
            Edge::End => {
                let neighbour = others
                    .map(Element::start_time)
                    .filter(|other_start| *other_start >= end)
                    .min()
                    .map(|other_start| other_start - end);
                let media = base
                    .source_duration
                    .map(|source| to_timeline(source - base.trim_start) - base.duration);
                let max = match (neighbour, media) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                (frame - base.duration, max)
            }
        };
        let clamp = |time: MediaTime| {
            let time = time.max(min);
            max.map_or(time, |max| time.min(max))
        };
        let delta = clamp(delta.round_to_frame(fps).unwrap_or(delta));
        let source_delta = delta.mul_f64_round(rate).unwrap_or(delta);

        Some(match edge {
            Edge::Start => Self {
                element: element_ref.clone(),
                start_time: start + delta,
                duration: base.duration - delta,
                trim_start: (base.trim_start + source_delta).max(MediaTime::ZERO),
                trim_end: base.trim_end,
            },
            Edge::End => Self {
                element: element_ref.clone(),
                start_time: start,
                duration: base.duration + delta,
                trim_start: base.trim_start,
                trim_end: (base.trim_end - source_delta).max(MediaTime::ZERO),
            },
        })
    }
}

impl Command for SetElementTiming {
    fn label(&self) -> &'static str {
        "Trim"
    }

    /// On the main track, a start at or before every other element's snaps
    /// to zero (no leading gap), as classic does for any start-time edit.
    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        let tracks = current_tracks(project)?;
        let start_time = enforce_main_track_start(
            tracks,
            &self.element.track_id,
            self.start_time,
            &[self.element.element_id.as_str()],
        );
        let element = element_mut(tracks, &self.element)?;
        let base = element.base_mut();
        base.start_time = start_time;
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
