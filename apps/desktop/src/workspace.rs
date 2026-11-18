use std::path::PathBuf;

use gpui::{ClipboardEntry, ClipboardItem, Context, ImageFormat, PathPromptOptions, SharedString};
use opencut_editor::commands::{AddMediaAsset, RemoveMediaAssets};
use opencut_editor::model::{MediaAsset, Project};
use opencut_editor::{Batch, Command, Editor};

use crate::media::{ProjectFiles, import_file, import_image_bytes, paths_in_text};

/// The open project: the headless [`Editor`] plus where its files live.
/// Shared by the panels as an `Entity`; every project change goes through
/// an editor command so it's undoable.
pub(crate) struct Workspace {
    editor: Editor,
    files: ProjectFiles,
    /// Files being probed and copied right now.
    importing: usize,
    /// The last import or edit problem, shown in the Browser.
    notice: Option<SharedString>,
}

impl Workspace {
    pub(crate) fn new() -> Self {
        let project = Project::new("Untitled project", chrono::Utc::now());
        let files = ProjectFiles::for_project(&project.metadata.id);
        Self {
            editor: Editor::new(project).expect("a new project is valid"),
            files,
            importing: 0,
            notice: None,
        }
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

    pub(crate) fn undo(&mut self, cx: &mut Context<Self>) {
        if self.editor.undo() {
            cx.notify();
        }
    }

    pub(crate) fn redo(&mut self, cx: &mut Context<Self>) {
        if self.editor.redo() {
            cx.notify();
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
            self.notice = Some(message.into());
        }
        cx.notify();
    }

    fn execute(&mut self, command: &mut dyn Command, cx: &mut Context<Self>) {
        if let Err(error) = self.editor.execute(command) {
            self.notice = Some(format!("{}: {error}", command.label()).into());
        }
        cx.notify();
    }

    fn set_notice(&mut self, message: String, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        cx.notify();
    }
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
