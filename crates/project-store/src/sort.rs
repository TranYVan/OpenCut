use std::cmp::Ordering;

use opencut_model::ProjectMetadata;

/// What the projects list sorts by (classic `TProjectSortKey`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortKey {
    CreatedAt,
    #[default]
    UpdatedAt,
    Name,
    Duration,
}

impl SortKey {
    pub const ALL: [Self; 4] = [Self::CreatedAt, Self::UpdatedAt, Self::Name, Self::Duration];

    /// The classic projects page's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::CreatedAt => "Created",
            Self::UpdatedAt => "Modified",
            Self::Name => "Name",
            Self::Duration => "Duration",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortOrder {
    Ascending,
    /// Classic's default, with [`SortKey::UpdatedAt`]: newest first.
    #[default]
    Descending,
}

impl SortOrder {
    pub fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

/// Sorts `projects` by `key`. Names compare case-insensitively; ties fall
/// back to the id so the order is stable across reloads.
pub fn sort_projects(projects: &mut [ProjectMetadata], key: SortKey, order: SortOrder) {
    projects.sort_by(|a, b| {
        let ordering = compare(a, b, key).then_with(|| a.id.cmp(&b.id));
        match order {
            SortOrder::Ascending => ordering,
            SortOrder::Descending => ordering.reverse(),
        }
    });
}

fn compare(a: &ProjectMetadata, b: &ProjectMetadata, key: SortKey) -> Ordering {
    match key {
        SortKey::CreatedAt => a.created_at.cmp(&b.created_at),
        SortKey::UpdatedAt => a.updated_at.cmp(&b.updated_at),
        SortKey::Name => a
            .name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name)),
        SortKey::Duration => a.duration.cmp(&b.duration),
    }
}
