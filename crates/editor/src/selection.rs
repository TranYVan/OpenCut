use opencut_model::{ElementRef, SceneTracks};

/// The selected timeline elements, in selection order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    elements: Vec<ElementRef>,
}

impl Selection {
    pub fn elements(&self) -> &[ElementRef] {
        &self.elements
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn contains(&self, element: &ElementRef) -> bool {
        self.elements.contains(element)
    }

    /// Selects only `element`.
    pub fn select(&mut self, element: ElementRef) {
        self.elements = vec![element];
    }

    /// Adds `element` if absent, removes it if present (shift/cmd-click).
    pub fn toggle(&mut self, element: ElementRef) {
        if let Some(index) = self
            .elements
            .iter()
            .position(|selected| *selected == element)
        {
            self.elements.remove(index);
        } else {
            self.elements.push(element);
        }
    }

    pub fn set(&mut self, elements: Vec<ElementRef>) {
        self.elements = elements;
        self.dedup();
    }

    pub fn clear(&mut self) {
        self.elements.clear();
    }

    /// Selects every element in the scene, in display order.
    pub fn select_all(&mut self, tracks: &SceneTracks) {
        self.elements = tracks
            .iter()
            .flat_map(|track| {
                track
                    .elements
                    .iter()
                    .map(|element| ElementRef::new(&track.id, element.id()))
            })
            .collect();
    }

    /// Drops references to elements that no longer exist.
    pub fn retain_existing(&mut self, tracks: &SceneTracks) {
        self.elements
            .retain(|element| tracks.element(element).is_some());
    }

    fn dedup(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.elements.retain(|element| seen.insert(element.clone()));
    }
}
