use std::collections::HashSet;

use opencut_model::time::FrameRate;
use opencut_model::{MediaAsset, MediaType, Project};

use crate::command::{Command, Outcome};
use crate::error::EditError;

/// Adds an imported file's asset to the project (classic
/// `AddMediaAssetCommand`). Importing a video with a higher frame rate than
/// the project's raises the project's rate to match, as in classic.
///
/// The file itself is the caller's business: it must already sit where the
/// app keeps the project's media before the asset points at it.
pub struct AddMediaAsset {
    pub asset: MediaAsset,
}

impl Command for AddMediaAsset {
    fn label(&self) -> &'static str {
        "Import media"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        if project.media_asset(&self.asset.id).is_some() {
            return Err(EditError::DuplicateMediaAsset(self.asset.id.clone()));
        }
        if let Some(fps) = raised_fps(project.settings.fps, &self.asset) {
            project.settings.fps = fps;
        }
        project.media_assets.push(self.asset.clone());
        Ok(Outcome::unchanged())
    }
}

/// Removes assets and every element that plays them, in all scenes (classic
/// `RemoveMediaAssetCommand`, which only cleaned the active scene). Unknown
/// ids fail the whole command.
pub struct RemoveMediaAssets {
    pub asset_ids: Vec<String>,
}

impl Command for RemoveMediaAssets {
    fn label(&self) -> &'static str {
        "Remove media"
    }

    fn apply(&mut self, project: &mut Project) -> Result<Outcome, EditError> {
        if self.asset_ids.is_empty() {
            return Err(EditError::Empty);
        }
        for id in &self.asset_ids {
            if project.media_asset(id).is_none() {
                return Err(EditError::MediaAssetNotFound(id.clone()));
            }
        }
        let removed: HashSet<&str> = self.asset_ids.iter().map(String::as_str).collect();
        project
            .media_assets
            .retain(|asset| !removed.contains(asset.id.as_str()));
        for scene in &mut project.scenes {
            for track in scene.tracks.iter_mut() {
                track
                    .elements
                    .retain(|element| element.media_id().is_none_or(|id| !removed.contains(id)));
            }
        }
        // The editor prunes selected elements that no longer exist.
        Ok(Outcome::unchanged())
    }
}

/// The project rate after importing `asset`, if it should change: a video
/// faster than the project raises it (classic `ratchetFpsForImportedMedia`).
fn raised_fps(current: FrameRate, asset: &MediaAsset) -> Option<FrameRate> {
    if asset.media_type != MediaType::Video {
        return None;
    }
    let fps = asset.fps.filter(|fps| fps.is_finite() && *fps > 0.0)?;
    if fps <= current.as_f64()? {
        return None;
    }
    frame_rate_from_f64(fps)
}

/// Common rates within this of a float snap to their exact fraction.
const STANDARD_RATE_TOLERANCE: f64 = 0.01;

const STANDARD_RATES: [FrameRate; 10] = [
    FrameRate::FPS_23_976,
    FrameRate::FPS_24,
    FrameRate::FPS_25,
    FrameRate::FPS_29_97,
    FrameRate::FPS_30,
    FrameRate::FPS_48,
    FrameRate::FPS_50,
    FrameRate::FPS_59_94,
    FrameRate::FPS_60,
    FrameRate::FPS_120,
];

/// The frame rate a float fps stands for (classic `floatToFrameRate`):
/// standard rates snap to their exact fraction (29.97 → 30000/1001). `None`
/// when the rate doesn't divide into whole ticks, which the project can't
/// use.
pub(crate) fn frame_rate_from_f64(fps: f64) -> Option<FrameRate> {
    if !fps.is_finite() || fps <= 0.0 {
        return None;
    }
    let rate = STANDARD_RATES
        .into_iter()
        .find(|rate| {
            rate.as_f64()
                .is_some_and(|value| (fps - value).abs() <= STANDARD_RATE_TOLERANCE)
        })
        .or_else(|| {
            const DENOMINATOR: u64 = 1_000_000;
            let numerator = (fps * DENOMINATOR as f64).round() as u64;
            let divisor = gcd(numerator, DENOMINATOR);
            Some(FrameRate::new(
                u32::try_from(numerator / divisor).ok()?,
                u32::try_from(DENOMINATOR / divisor).ok()?,
            ))
        })?;
    rate.ticks_per_frame().is_some().then_some(rate)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_snap_to_standard_rates() {
        assert_eq!(frame_rate_from_f64(29.97), Some(FrameRate::FPS_29_97));
        assert_eq!(frame_rate_from_f64(23.976_02), Some(FrameRate::FPS_23_976));
        assert_eq!(frame_rate_from_f64(60.0), Some(FrameRate::FPS_60));
        assert_eq!(frame_rate_from_f64(15.0), Some(FrameRate::new(15, 1)));
        assert_eq!(frame_rate_from_f64(12.5), Some(FrameRate::new(25, 2)));
        assert_eq!(frame_rate_from_f64(0.0), None);
        assert_eq!(frame_rate_from_f64(f64::NAN), None);
        // 120000 / 7 ticks per frame is not whole.
        assert_eq!(frame_rate_from_f64(7.0), None);
    }
}
