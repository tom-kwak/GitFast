//! The contract every UI feature follows.
//!
//! A feature is a folder under `features/` with its own `State`, `Message`
//! and (optionally) `Output`. The app owns feature states, wraps their
//! messages (`app::Message::Sidebar(sidebar::Message)`) and reacts to their
//! outputs. Features never reach into each other: anything that crosses
//! features or touches git goes out as an `Output` and the app decides.
//!
//! Adding a feature:
//! 1. create `features/<name>/mod.rs` implementing [`Feature`],
//! 2. add a field for its state (on `App` or on `RepoTab` if per-repo),
//! 3. add a `Message` variant and route it in `app::update`/`view`.

use iced::{Element, Task};

use crate::repo_tab::Snapshot;
use crate::theme::Tokens;

/// What a feature's view may read.
#[derive(Debug, Clone, Copy)]
pub struct Cx<'a> {
    pub tokens: &'a Tokens,
    /// Data of the active repository, if one is open and loaded.
    pub snapshot: Option<&'a Snapshot>,
}

/// Result of handling a message: async follow-up work for the feature
/// itself, plus an optional request to the app.
#[derive(Debug)]
pub struct Update<M, O> {
    pub task: Task<M>,
    pub output: Option<O>,
}

impl<M, O> Update<M, O> {
    pub fn none() -> Self {
        Self {
            task: Task::none(),
            output: None,
        }
    }

    pub fn output(output: O) -> Self {
        Self {
            task: Task::none(),
            output: Some(output),
        }
    }

    pub fn task(task: Task<M>) -> Self {
        Self { task, output: None }
    }
}

pub trait Feature {
    type Message: std::fmt::Debug + Clone + Send + 'static;
    /// Requests to the app (open a repo, run git, ...). Use
    /// `std::convert::Infallible` when the feature needs none.
    type Output;

    fn update(&mut self, message: Self::Message) -> Update<Self::Message, Self::Output>;

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Self::Message>;
}
