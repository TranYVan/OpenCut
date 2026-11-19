use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui::{
    ClipboardEntry, ClipboardItem, Context, ImageFormat, PathPromptOptions, SharedString, Task,
};
use opencut_editor::commands::{AddMediaAsset, RemoveMediaAssets};
use opencut_editor::model::time::MediaTime;
use opencut_editor::model::{ElementRef, MediaAsset, Project, TimelineViewState};
use opencut_editor::{Batch, Command, Editor};
use opencut_project_store::{ProjectStore, StoreError};

use crate::media::{ProjectFiles, import_file, import_image_bytes, paths_in_text};

/// How long the project must sit unchanged before it's autosaved
/// (classic `SaveManager`'s 800 ms).
const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);
/// How long an import or edit notice stays up. Save failures stay until
/// replaced, since they mean unsaved work.
const NOTICE_DURATION: Duration = Duration::from_secs(5);

/// The open project: the headless [`Editor`] plus where its files live.
/// Shared by the panels as an `Entity`; every project change goes through
/// an editor command so it's undoable.
///
/// Autosaves: every edit (any [`Editor::revision`] bump, undo and redo
/// included) and every timeline view or playhead change schedules a save
/// [`AUTOSAVE_DELAY`] later; further changes push it back. The write runs
/// off the UI thread. Closing the project or quitting the app saves at once.
pub(crate) struct Workspace {
    editor: Editor,
    store: ProjectStore,
    files: ProjectFiles,
    autosave: Autosave,
    /// Files being probed and copied right now.
    importing: usize,
    /// The last import, edit or save problem, shown in the Browser.
    notice: Option<SharedString>,
    /// Clears `notice`; replacing it cancels the previous one.
    notice_timer: Option<Task<()>>,
}

/// Whatever drops the project (closing it, closing the window) saves it
/// first. Quitting saves through the shell's quit hook, since GPUI doesn't
/// promise to drop entities on quit.
impl Drop for Workspace {
    fn drop(&mut self) {
        self.save_now();
    }
}

struct Autosave {
    /// The editor revision in the last snapshot handed to a writer.
    saved_revision: u64,
    /// The timeline view or playhead changed since the last snapshot.
    view_changed: bool,
    /// The debounce timer; replacing it cancels the previous one.
    timer: Option<Task<()>>,
    /// Numbers every snapshot handed to a writer, in order.
    next_sequence: u64,
    /// The sequence number of the snapshot on disk. Writers hold this lock
    /// while writing, so writes never interleave and an older snapshot
    /// never overwrites a newer one.
    written: Arc<Mutex<u64>>,
}

impl Workspace {
    /// Opens `project`, whose folder in `store` holds its media.
    pub(crate) fn open(store: ProjectStore, project: Project) -> Result<Self, String> {
        let root = store
            .project_dir(&project.metadata.id)
            .map_err(|error| error.to_string())?;
        let editor = Editor::new(project).map_err(|error| error.to_string())?;
        Ok(Self {
            autosave: Autosave {
                saved_revision: editor.revision(),
                view_changed: false,
                timer: None,
                next_sequence: 0,
                written: Arc::new(Mutex::new(0)),
            },
            editor,
            store,
            files: ProjectFiles::new(root),
            importing: 0,
            notice: None,
            notice_timer: None,
        })
    }

    pub(crate) fn editor(&self) -> &Editor {
        &self.editor
    }

    pub(crate) fn files(&self) -> &ProjectFiles {
        &self.files
    }

    pub(crate) fn importing(&self) -> usize {
        self.importing
    }

    pub(crate) fn notice(&self) -> Option<&SharedString> {
        self.notice.as_ref()
    }

    pub(crate) fn project_name(&self) -> &str {
        &self.editor.project().metadata.name
    }

    /// The timeline's saved zoom and scroll; `None` for a project whose
    /// timeline view was never saved. The playhead is
    /// `editor().playback().position`, restored on open.
    pub(crate) fn timeline_view(&self) -> Option<&TimelineViewState> {
        self.editor.timeline_view_state()
    }

    /// Records the timeline's zoom and horizontal scroll (pixels) so they're
    /// saved with the project. Not undoable. Call it whenever the timeline
    /// zooms or scrolls; saving is debounced.
    pub(crate) fn set_timeline_view(
        &mut self,
        zoom_level: f64,
        scroll_left: f64,
        cx: &mut Context<Self>,
    ) {
        let state = TimelineViewState {
            zoom_level,
            scroll_left,
            playhead_time: self.editor.playback().position,
        };
        if self.editor.set_timeline_view_state(state) {
            self.autosave.view_changed = true;
            self.schedule_save(cx);
        }
    }

    /// Moves the playhead (snapped to a frame, clamped to the scene). Where
    /// it ends up is saved with the project.
    pub(crate) fn seek(&mut self, time: MediaTime, cx: &mut Context<Self>) {
        let before = self.editor.playback().position;
        self.editor.seek(time);
        if self.editor.playback().position != before {
            self.autosave.view_changed = true;
            self.schedule_save(cx);
            cx.notify();
        }
    }

    /// Writes the project now if it has unsaved changes, cancelling the
    /// pending autosave. Blocks; for closing the project and quitting.
    pub(crate) fn save_now(&mut self) {
        self.autosave.timer = None;
        let Some((snapshot, sequence)) = self.take_snapshot() else {
            return;
        };
        if let Err(error) = write_in_order(&self.store, &snapshot, sequence, &self.autosave.written)
        {
            self.save_failed(&error);
        }
    }

    /// Opens the system file picker and imports what the user picks.
    pub(crate) fn prompt_import(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = match picked.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_notice(format!("Can't open the file picker: {error}"), cx);
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| this.import_paths(paths, cx));
        })
        .detach();
    }

    /// Probes and copies `paths` into the project off the UI thread, then
    /// adds the readable ones as one undo step.
    pub(crate) fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let files = self.files.clone();
        let count = paths.len();
        let work = cx.background_executor().spawn(async move {
            paths
                .iter()
                .map(|path| import_file(path, &files))
                .collect::<Vec<_>>()
        });
        self.start_import(count, cx);
        cx.spawn(async move |this, cx| {
            let results = work.await;
            let _ = this.update(cx, |this, cx| this.finish_import(count, results, cx));
        })
        .detach();
    }

    /// Imports what the clipboard holds: an image, or text naming files.
    /// Returns `false` when there's nothing importable.
    ///
    /// GPUI 0.2.2's clipboard carries only text and images, not file
    /// references, so files copied in a file manager arrive only if it also
    /// put their paths on the clipboard as text.
    pub(crate) fn paste(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        for entry in item.entries() {
            match entry {
                ClipboardEntry::String(text) => {
                    let paths: Vec<PathBuf> = paths_in_text(text.text())
                        .into_iter()
                        .filter(|path| path.is_file())
                        .collect();
                    if !paths.is_empty() {
                        self.import_paths(paths, cx);
                        return true;
                    }
                }
                ClipboardEntry::Image(image) => {
                    let extension = image_extension(image.format());
                    let bytes = image.bytes().to_vec();
                    let files = self.files.clone();
                    let work = cx
                        .background_executor()
                        .spawn(async move { vec![import_image_bytes(&bytes, extension, &files)] });
                    self.start_import(1, cx);
                    cx.spawn(async move |this, cx| {
                        let results = work.await;
                        let _ = this.update(cx, |this, cx| this.finish_import(1, results, cx));
                    })
                    .detach();
                    return true;
                }
            }
        }
        false
    }

    /// Removes assets and the timeline elements that use them, undoably.
    /// The files stay in the media folder so undo can bring them back.
    pub(crate) fn remove_assets(&mut self, asset_ids: Vec<String>, cx: &mut Context<Self>) {
        if asset_ids.is_empty() {
            return;
        }
        self.execute(&mut RemoveMediaAssets { asset_ids }, cx);
    }

    /// Replaces the timeline element selection. Not undoable.
    pub(crate) fn set_selection(&mut self, elements: Vec<ElementRef>, cx: &mut Context<Self>) {
        if self.editor.selection().elements() != elements.as_slice() {
            self.editor.selection_mut().set(elements);
            cx.notify();
        }
    }

    pub(crate) fn undo(&mut self, cx: &mut Context<Self>) {
        if self.editor.undo() {
            self.changed(cx);
        }
    }

    pub(crate) fn redo(&mut self, cx: &mut Context<Self>) {
        if self.editor.redo() {
            self.changed(cx);
        }
    }

    fn start_import(&mut self, count: usize, cx: &mut Context<Self>) {
        self.importing += count;
        self.notice = None;
        cx.notify();
    }

    fn finish_import(
        &mut self,
        count: usize,
        results: Vec<Result<MediaAsset, String>>,
        cx: &mut Context<Self>,
    ) {
        self.importing = self.importing.saturating_sub(count);
        let mut commands: Vec<Box<dyn Command>> = Vec::new();
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(asset) => commands.push(Box::new(AddMediaAsset { asset })),
                Err(error) => errors.push(error),
            }
        }
        if !commands.is_empty() {
            self.execute(&mut Batch::new("Import media", commands), cx);
        }
        if let Some(first) = errors.first() {
            let message = match errors.len() {
                1 => format!("Couldn't import {first}"),
                more => format!("Couldn't import {more} files. {first}"),
            };
            self.set_notice(message, cx);
        }
        cx.notify();
    }

    pub(crate) fn execute(&mut self, command: &mut dyn Command, cx: &mut Context<Self>) {
        if let Err(error) = self.editor.execute(command) {
            self.set_notice(format!("{}: {error}", command.label()), cx);
        }
        self.changed(cx);
    }

    /// Like [`Workspace::execute`], but repeated calls with the same `key`
    /// (a drag) fold into one undo step until [`Workspace::end_coalescing`].
    pub(crate) fn execute_coalescing(
        &mut self,
        command: &mut dyn Command,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.editor.execute_coalescing(command, key) {
            self.set_notice(format!("{}: {error}", command.label()), cx);
        }
        self.changed(cx);
    }

    pub(crate) fn end_coalescing(&mut self) {
        self.editor.end_coalescing();
    }

    fn set_notice(&mut self, message: String, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        self.notice_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Schedules an autosave if the project changed, and re-renders.
    fn changed(&mut self, cx: &mut Context<Self>) {
        if self.editor.revision() != self.autosave.saved_revision {
            self.schedule_save(cx);
        }
        cx.notify();
    }

    /// (Re)starts the autosave timer.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(AUTOSAVE_DELAY);
        self.autosave.timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| this.save_in_background(cx));
        }));
    }

    fn save_in_background(&mut self, cx: &mut Context<Self>) {
        self.autosave.timer = None;
        let Some((snapshot, sequence)) = self.take_snapshot() else {
            return;
        };
        let store = self.store.clone();
        let written = Arc::clone(&self.autosave.written);
        let work = cx
            .background_executor()
            .spawn(async move { write_in_order(&store, &snapshot, sequence, &written) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.save_failed(&error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// What to write, if anything changed since the last snapshot. Marks
    /// the current state as saved.
    fn take_snapshot(&mut self) -> Option<(Project, u64)> {
        let revision = self.editor.revision();
        if revision == self.autosave.saved_revision && !self.autosave.view_changed {
            return None;
        }
        self.autosave.saved_revision = revision;
        self.autosave.view_changed = false;
        self.autosave.next_sequence += 1;
        Some((self.editor.project_to_save(), self.autosave.next_sequence))
    }

    /// Keeps the project marked unsaved, so the next change or closing the
    /// project tries again.
    fn save_failed(&mut self, error: &StoreError) {
        self.autosave.view_changed = true;
        self.notice_timer = None;
        self.notice = Some(format!("Couldn't save the project: {error}").into());
    }
}

/// Saves `snapshot` unless a newer one (higher `sequence`) is already on
/// disk. Holding the lock while writing keeps writers from interleaving.
fn write_in_order(
    store: &ProjectStore,
    snapshot: &Project,
    sequence: u64,
    written: &Mutex<u64>,
) -> Result<(), StoreError> {
    let mut written = written.lock().unwrap_or_else(PoisonError::into_inner);
    if *written >= sequence {
        return Ok(());
    }
    store.save(snapshot)?;
    *written = sequence;
    Ok(())
}

fn image_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_snapshots_never_overwrite_newer_ones() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::new(dir.path());
        let written = Mutex::new(0);
        let mut project = Project::new("v1", chrono::Utc::now());
        let id = project.metadata.id.clone();

        project.metadata.name = "newer".to_owned();
        write_in_order(&store, &project, 2, &written).unwrap();
        project.metadata.name = "older".to_owned();
        write_in_order(&store, &project, 1, &written).unwrap();

        assert_eq!(store.load(&id).unwrap().metadata.name, "newer");
        assert_eq!(*written.lock().unwrap(), 2);
    }
}
