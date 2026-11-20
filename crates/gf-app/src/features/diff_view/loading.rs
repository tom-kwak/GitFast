//! A panel owns the order of its reads. Highlights belong to the exact
//! displayed diff, including while its replacement is still loading.
use std::sync::Arc;

use gf_git::{DiffOptions, Repository};
use iced::Task;

use super::{DiffSource, FileSpans, LoadedDiff, Pos, ScrollPos, State, highlight_file};

#[derive(Debug, Clone)]
pub enum Completion {
    Loaded(u64, Result<Arc<LoadedDiff>, String>),
    Highlighted(Arc<LoadedDiff>, usize, Arc<FileSpans>),
}

impl State {
    pub fn request(&mut self, repo: Repository, source: DiffSource) -> Task<Completion> {
        let generation = self.show(source.clone());
        Task::perform(load(repo, source, self.ignore_whitespace), move |result| {
            Completion::Loaded(generation, result)
        })
    }

    pub fn highlight_file(&self, file: usize) -> Task<Completion> {
        let diff = self.diff.clone();
        let input = diff.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || Arc::new(highlight_file(&input.files[file])))
                    .await
                    .unwrap_or_default()
            },
            move |spans| Completion::Highlighted(diff.clone(), file, spans),
        )
    }

    pub fn finish(&mut self, completion: Completion) -> Task<Completion> {
        match completion {
            Completion::Loaded(generation, result) => return self.loaded(generation, result),
            Completion::Highlighted(diff, file, spans) if Arc::ptr_eq(&self.diff, &diff) => {
                self.spans.insert(file, spans);
            }
            Completion::Highlighted(..) => {}
        }
        Task::none()
    }

    /// Starts showing `source`; the app loads it and calls [`Self::loaded`].
    pub(super) fn show(&mut self, source: DiffSource) -> u64 {
        if self.source.as_ref() != Some(&source) {
            self.clear_selection();
            self.diff = Arc::default();
            self.spans.clear();
            self.file = 0;
            self.scroll = ScrollPos::default();
            self.files_scroll = ScrollPos::default();
            self.folders.focused = None;
        }
        self.source = Some(source);
        self.loading = true;
        self.error = None;
        self.generation += 1;
        self.generation
    }

    pub fn close(&mut self) {
        let mut folders = std::mem::take(&mut self.folders);
        folders.focused = None;
        *self = Self {
            generation: self.generation,
            ignore_whitespace: self.ignore_whitespace,
            folders,
            ..Self::new(self.role)
        };
    }

    pub(super) fn loaded(
        &mut self,
        generation: u64,
        result: Result<Arc<LoadedDiff>, String>,
    ) -> Task<Completion> {
        if self.source.is_none() || generation != self.generation {
            return Task::none(); // the user moved on
        }
        self.loading = false;
        match result {
            Ok(diff) => {
                // Keep the selection while its lines are still there (the
                // file was saved again, say).
                let exists = |((f, h, l), _): Pos| {
                    diff.files
                        .get(f)
                        .and_then(|f| f.hunks.get(h))
                        .is_some_and(|h| l < h.lines.len())
                };
                self.ranges.retain(|r| exists(r.anchor) && exists(r.head));
                self.caret = self.caret.filter(|p| exists(*p));
                self.dragging = false;
                self.drag_unit = None;
                self.last_click = None;
                if self.file >= diff.files.len() {
                    self.file = 0;
                }
                self.spans.clear();
                if let Some(s) = &diff.first_spans {
                    self.spans.insert(0, s.clone());
                }
                self.diff = diff;
                self.error = None;
                if self.file != 0 {
                    return self.highlight_file(self.file);
                }
            }
            Err(e) => self.error = Some(e),
        }
        Task::none()
    }
}

/// Loads the diff for `source` and highlights its first file off the UI thread.
async fn load(
    repo: Repository,
    source: DiffSource,
    ignore_whitespace: bool,
) -> Result<Arc<LoadedDiff>, String> {
    let (target, paths) = source.target();
    let opts = DiffOptions {
        ignore_whitespace,
        paths,
        ..DiffOptions::default()
    };
    let files = repo.diff(&target, &opts).await.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || Arc::new(LoadedDiff::new(files, true)))
        .await
        .map_err(|e| e.to_string())
}
