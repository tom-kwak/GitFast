//! Routes commit searches and reveals results in their original graph scope.

use std::time::Duration;

use gf_core::model::{Oid, RepoId};
use iced::Task;
use iced::widget::operation;

use super::{App, Message, dialogs::Purpose, reveal::ASK_OVER};
use crate::features::dialog::{Dialog, Severity};
use crate::features::history::{
    commit_search::{self, Hit, Scope},
    search::{self, Mode},
};
use crate::repo_tab::RepoTab;

fn scope(tab: &RepoTab) -> Scope {
    Scope {
        revision: tab.history.revision().map(str::to_owned),
        all_branches: tab.history.revision().is_none() && tab.all_branches,
        refs_key: tab.snapshot.as_ref().map(|snap| snap.history_key),
    }
}

impl App {
    /// Keep the search open across branch, tag and ref changes.
    pub(super) fn sync_history_search(&mut self) -> Task<Message> {
        let Some(tab) = self
            .active_tab_mut()
            .filter(|tab| tab.history.file_search_is_open())
        else {
            return Task::none();
        };
        let current = scope(tab);
        if tab.history.search_scope.as_ref() == Some(&current) {
            return Task::none();
        }
        tab.history.search_scope = Some(current);
        let task = self.restart_history_search();
        if self.dialog.is_none() {
            Task::batch([task, operation::focus(search::INPUT_ID)])
        } else {
            task
        }
    }

    pub(super) fn set_history_search_mode(&mut self, mode: Mode) -> Task<Message> {
        let Some(tab) = self
            .active_tab_mut()
            .filter(|tab| tab.history.file_search_is_open() && tab.history.search_mode != mode)
        else {
            return Task::none();
        };
        tab.history.search_mode = mode;
        Task::batch([
            self.restart_history_search(),
            operation::focus(search::INPUT_ID),
        ])
    }

    pub(super) fn restart_history_search(&mut self) -> Task<Message> {
        let Some(tab) = self
            .active_tab_mut()
            .filter(|tab| tab.history.file_search_is_open())
        else {
            return Task::none();
        };
        tab.history.reset_search_results();
        match tab.history.search_mode {
            Mode::Files => self.restart_file_search(),
            Mode::Commits => self.run_commit_search(),
        }
    }

    pub(super) fn query_history_search(&mut self, query: String) -> Task<Message> {
        let Some(tab) = self
            .active_tab_mut()
            .filter(|tab| tab.history.file_search_is_open())
        else {
            return Task::none();
        };
        match tab.history.search_mode {
            Mode::Files => self.query_file_search(query),
            Mode::Commits => {
                if let Some(search) = tab.history.search_state_mut() {
                    search.query_changed(query);
                }
                self.run_commit_search()
            }
        }
    }

    fn run_commit_search(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let Some(search) = tab.history.search_state_mut() else {
            return Task::none();
        };
        let term = search.query().trim().to_owned();
        if term.is_empty() {
            search.reset_results();
            tab.history.commit_search.clear();
            return Task::none();
        }
        let repo = tab.repo.clone();
        let id = repo.id();
        let query = scope(tab).query();
        let state = &mut tab.history.commit_search;
        state.invalidate();
        let generation = state.generation;
        let cancel = gf_git::CancellationToken::new();
        state.cancel = Some(cancel.clone());
        Task::perform(
            async move {
                tokio::select! {
                    () = cancel.cancelled() => return Err("Search cancelled".into()),
                    () = tokio::time::sleep(Duration::from_millis(150)) => {}
                }
                let commits = repo
                    .search_commits(&query, &term, cancel)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(commit_search::prepare(commits, &term))
            },
            move |result| Message::CommitSearchFinished(id, generation, result),
        )
    }

    pub(super) fn commit_search_finished(
        &mut self,
        id: RepoId,
        generation: u64,
        result: Result<Vec<Hit>, String>,
    ) -> Task<Message> {
        let Some(tab) = self.current_commit_search(id, generation) else {
            return Task::none();
        };
        let state = &mut tab.history.commit_search;
        state.cancel = None;
        match result {
            Ok(mut hits) => {
                state.truncated = hits.len() > gf_git::COMMIT_SEARCH_LIMIT;
                hits.truncate(gf_git::COMMIT_SEARCH_LIMIT);
                let count = hits.len();
                state.set_hits(hits);
                if let Some(search) = tab.history.search_state_mut() {
                    search.results(count);
                }
                search::State::scroll_to_top()
            }
            Err(error) => {
                state.error = Some(error);
                Task::none()
            }
        }
    }

    fn current_commit_search(&mut self, id: RepoId, generation: u64) -> Option<&mut RepoTab> {
        self.active_tab_mut().filter(|tab| {
            tab.repo.id() == id
                && tab.history.file_search_is_open()
                && tab.history.search_mode == Mode::Commits
                && tab.history.search_scope.as_ref() == Some(&scope(tab))
                && tab.history.commit_search.generation == generation
        })
    }

    pub(super) fn activate_history_search(&mut self, row: usize) -> Task<Message> {
        let tokens = self.tokens.clone();
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        if tab.history.search_mode == Mode::Files {
            return self.activate_file_search(row);
        }
        let state = &tab.history.commit_search;
        if state.locating || state.error.is_some() {
            return Task::none();
        }
        if tab
            .history
            .search_state_mut()
            .and_then(|search| search.activate(row))
            .is_none()
        {
            return Task::none();
        }
        let Some(hit) = tab.history.commit_search.hits.get(row) else {
            return Task::none();
        };
        let oid = hit.id.clone();
        // The snapshot can still show the previous graph during a range switch.
        if let Some(snap) = tab.snapshot.clone()
            && (tab.history.revision().is_some() || snap.history_all_branches == tab.all_branches)
            && let Some(scroll) = tab.history.reveal(&oid, &snap, &tokens)
        {
            let scroll = if scroll.units() == 0 {
                tab.history.restore_graph_scroll()
            } else {
                scroll
            };
            return self.finish_commit_search(oid, scroll);
        }
        let repo = tab.repo.clone();
        let id = repo.id();
        let query = scope(tab).query();
        let state = &mut tab.history.commit_search;
        state.locating = true;
        let generation = state.generation;
        let cancel = gf_git::CancellationToken::new();
        state.cancel = Some(cancel.clone());
        Task::perform(
            {
                let oid = oid.clone();
                async move {
                    repo.log_position_cancellable(&query, &oid, cancel)
                        .await
                        .map_err(|e| e.to_string())
                }
            },
            move |result| Message::CommitSearchLocated(id, generation, oid.clone(), result),
        )
    }

    pub(super) fn commit_search_located(
        &mut self,
        id: RepoId,
        generation: u64,
        oid: Oid,
        result: Result<Option<usize>, String>,
    ) -> Task<Message> {
        let Some(tab) = self.current_commit_search(id, generation) else {
            return Task::none();
        };
        tab.history.commit_search.locating = false;
        tab.history.commit_search.cancel = None;
        let rows = match result {
            Ok(Some(row)) => row + 1,
            other => {
                tab.history.commit_search.error =
                    Some(other.err().unwrap_or_else(|| {
                        format!("{} is no longer in this history", oid.short())
                    }));
                return Task::none();
            }
        };
        if rows > ASK_OVER {
            return self.open_dialog(
                Dialog::confirm(
                    "Load history",
                    vec![
                        format!("{} is commit #{} in this history.", oid.short(), rows),
                        "Load the history down to it? It may take a moment.".into(),
                    ],
                    "Load",
                    Purpose::RevealSearchCommit {
                        id,
                        generation,
                        oid,
                        rows,
                    },
                )
                .severity(Severity::Normal),
            );
        }
        self.load_search_commit(id, generation, oid, rows)
    }

    pub(super) fn load_search_commit(
        &mut self,
        id: RepoId,
        generation: u64,
        oid: Oid,
        rows: usize,
    ) -> Task<Message> {
        let Some(tab) = self.current_commit_search(id, generation) else {
            return Task::none();
        };
        tab.history.close_file_search();
        tab.history.reveal_search_after_load(oid.clone(), rows);
        let scroll = tab.history.restore_graph_scroll();
        self.focus_pane(crate::nav::Pane::History);
        Task::batch([
            scroll,
            self.on_history(crate::features::history::Output::LoadMore),
            self.show_history_commit(oid),
        ])
    }

    fn finish_commit_search(
        &mut self,
        oid: Oid,
        scroll: Task<crate::features::history::Message>,
    ) -> Task<Message> {
        if let Some(tab) = self.active_tab_mut() {
            tab.history.close_file_search();
        }
        self.focus_pane(crate::nav::Pane::History);
        Task::batch([scroll.map(Message::History), self.show_history_commit(oid)])
    }
}
