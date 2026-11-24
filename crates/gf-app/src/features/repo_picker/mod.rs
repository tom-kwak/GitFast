//! Quick-open window for repositories (⌘O, `+ open`), modelled on the
//! repository switchers of GitHub Desktop / Fork / VS Code "Open Recent":
//!
//! ```text
//! ┌─ OPEN REPOSITORY ──────────────────────────────────────────────┐
//! │ ❯ gf▏                                                           │
//! │ ▸ gf                 ⎇ main     ~/claudework/gf       ● tab 2 ⌘1│
//! │   license-list-XML   ⎇ main     ~/claudework/…/license-list-XML ⌘2│
//! │   gitignore          not found  ~/claudework/…/gitignore     × │
//! │ [ Browse… ⌘O ]   ↑↓ select · ⏎ open · ⌘⌫ forget · esc close     │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! - The filter has focus as soon as it opens: type to narrow the list
//!   (fuzzy, name before path, matched letters highlighted), ↑/↓ and ⏎ or
//!   ⌘1–9 to open. Typing a path (`~/…`, `/…`) offers to open that folder
//!   and says as you type whether it is a git repository (a path typed
//!   with the Korean input method is read as the keys pressed).
//! - Opening something that cannot be opened (no git there, a missing
//!   folder) shakes the window and shows why in red, like a rejected
//!   password field; the window stays open to correct the path.
//! - Repositories already open in a tab are marked and switch to the tab.
//! - Missing folders stay listed (a disk may be unmounted) but dimmed;
//!   `×` / ⌘⌫ removes an entry from the list (after a confirmation); in a
//!   non-empty filter ⌘⌫ first clears the text, as in any text field.
//! - Typing a URL (`https://…`, `git@host:path`) offers to clone it; the
//!   app asks where to, clones it and opens it.
//! - `Browse…` (⌘O again) falls back to the system folder dialog.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iced::widget::{
    Column, Row, button, column, container, mouse_area, responsive, row, space, text_input,
};
use iced::{Background, Border, Element, Length, Padding, mouse};

use crate::feature::{Cx, Feature, Update};
use crate::theme::{Tokens, style};
use crate::widgets::fuzzy::{self, fuzzy};
use crate::widgets::{self, colored, dim, format, mono};

/// Id of the filter field (focused when the window opens).
pub const FILTER_ID: &str = "gf-repo-picker-filter";

/// Characters of a path shown on one line (the end is kept).
const PATH_COLS: usize = 64;

/// Keeps one line: `~/a/b/c/repo` → `…/c/repo`, cut at a `/` when it can.
pub(crate) fn elide_path(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_owned();
    }
    let tail: String = path.chars().skip(n - (max - 1)).collect();
    match tail.find(crate::platform::is_path_sep) {
        Some(i) if i + 1 < tail.len() => format!("…{}", &tail[i..]),
        _ => format!("…{tail}"),
    }
}

/// A remembered repository, read once when the window opens.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    name: String,
    /// Current branch (or short commit when detached), when readable.
    head: Option<String>,
    exists: bool,
    /// Index of the tab it is open in.
    tab: Option<usize>,
}

impl Entry {
    pub fn read(path: PathBuf, tab: Option<usize>) -> Self {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let exists = path.is_dir();
        let head = exists.then(|| read_head(&path)).flatten();
        Self {
            path,
            name,
            head,
            exists,
            tab,
        }
    }
}

/// The branch HEAD names, read straight from `.git/HEAD` (also through a
/// worktree's `.git` file) so opening the window costs no git process.
fn read_head(workdir: &Path) -> Option<String> {
    let dot_git = workdir.join(".git");
    let git_dir = if dot_git.is_file() {
        let link = std::fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(link.strip_prefix("gitdir:")?.trim());
        if dir.is_relative() {
            workdir.join(dir)
        } else {
            dir
        }
    } else {
        dot_git
    };
    head_label(&std::fs::read_to_string(git_dir.join("HEAD")).ok()?)
}

fn head_label(head: &str) -> Option<String> {
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(r) => {
            let r = r.trim();
            Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_owned())
        }
        None if head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(head[..7].to_owned())
        }
        None => None,
    }
}

/// What a typed path points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathKind {
    /// Inside a work tree (git looks upwards too) or a bare repository.
    Repo,
    /// A folder without git.
    Folder,
    Missing,
}

fn path_kind(path: &Path) -> PathKind {
    if !path.is_dir() {
        return PathKind::Missing;
    }
    let bare = path.join("HEAD").is_file() && path.join("objects").is_dir();
    if bare || path.ancestors().any(|a| a.join(".git").exists()) {
        PathKind::Repo
    } else {
        PathKind::Folder
    }
}

/// How long a rejected open shakes the window.
const SHAKE: Duration = Duration::from_millis(450);

/// Horizontal offset of the shake `elapsed` after it started: a few
/// fast swings that die down.
fn shake_offset(elapsed: Duration) -> f32 {
    if elapsed >= SHAKE {
        return 0.0;
    }
    let t = elapsed.as_secs_f32();
    let left = 1.0 - t / SHAKE.as_secs_f32();
    12.0 * left * (t * std::f32::consts::TAU * 9.0).sin()
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    /// Index into `entries` and the matched characters of its name.
    Recent { index: usize, marks: Vec<usize> },
    /// The typed path itself.
    Path(PathBuf, PathKind),
    /// A repository or folder inside the typed folder (`~/work/` lists
    /// the services there; ⇥ / ⏎ on a folder goes into it).
    Found(Found),
    /// The typed URL: clone it.
    Clone(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    path: PathBuf,
    name: String,
    marks: Vec<usize>,
    repo: bool,
    head: Option<String>,
}

/// Children listed for a typed folder, at most.
const MAX_FOUND: usize = 400;

#[derive(Debug, Clone, Default)]
pub struct State {
    entries: Vec<Entry>,
    filter: String,
    items: Vec<Item>,
    selected: usize,
    /// First row shown, and how many rows fit (known after layout); the
    /// list follows the selection.
    first: Cell<usize>,
    rows: Cell<usize>,
    /// The folder being browsed when the filter is a path.
    browsing: Option<PathBuf>,
    /// When the filter text last changed. ⌘⌫ reaches the field before the
    /// app: when it just cleared the text, it must not also forget.
    edited: Option<Instant>,
    /// Why the last open was refused (cleared when the filter changes).
    warning: Option<String>,
    /// When the window started shaking for that refusal.
    shake: Option<Instant>,
    /// The last folder listed: typing a name in it filters this instead of
    /// reading the folder (and every repository's HEAD) on each key.
    listing: Option<Listing>,
}

/// The subfolders of one folder, read once.
#[derive(Debug, Clone, Default)]
struct Listing {
    dir: PathBuf,
    folders: Vec<Listed>,
}

#[derive(Debug, Clone)]
struct Listed {
    path: PathBuf,
    name: String,
    repo: bool,
    head: Option<String>,
}

impl Listing {
    fn read(dir: &Path) -> Self {
        let folders = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .take(5000)
            .filter_map(|d| {
                let path = d.path();
                let name = d.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !path.is_dir() {
                    return None;
                }
                let repo = path.join(".git").exists()
                    || (path.join("HEAD").is_file() && path.join("objects").is_dir());
                let head = repo.then(|| read_head(&path)).flatten();
                Some(Listed {
                    path,
                    name,
                    repo,
                    head,
                })
            })
            .collect();
        Self {
            dir: dir.to_path_buf(),
            folders,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Filter(String),
    /// Open the item at this row (click, ⌘1–9).
    Pick(usize),
    Forget(PathBuf),
    /// Mouse wheel over the list.
    Scroll(mouse::ScrollDelta),
    Browse,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Open(PathBuf),
    /// Clone this URL (the app asks where to).
    Clone(String),
    SwitchTab(usize),
    /// Remove it from the recent list (the app confirms first).
    Forget(PathBuf),
    /// The filter text was replaced (⇥, going into a folder): put the
    /// cursor at its end.
    Completed,
    Browse,
    Close,
}

impl State {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut s = Self {
            entries,
            ..Self::default()
        };
        s.refilter();
        s
    }

    /// Drops a forgotten entry, keeping the selection in place.
    pub fn forgotten(&mut self, path: &Path) {
        let Some(i) = self.entries.iter().position(|e| e.path == path) else {
            return;
        };
        self.entries.remove(i);
        let selected = self.selected;
        self.refilter();
        self.selected = selected.min(self.items.len().saturating_sub(1));
    }

    /// ↑/↓ (and paging): moves the selection, clamped.
    pub fn move_by(&mut self, by: isize) {
        let last = self.items.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(by).min(last);
    }

    /// Rows that fit on screen (for ⇞/⇟).
    pub fn page(&self) -> isize {
        isize::try_from(self.rows.get().max(2) - 1).unwrap_or(10)
    }

    /// ⏎ on the selected row.
    pub fn activate(&mut self) -> Option<Output> {
        self.output_for(self.selected)
    }

    /// ⇥: completes the filter to the selected row's path (a folder gets
    /// a trailing `/`, which lists what is inside).
    pub fn complete(&mut self) -> Option<Output> {
        let (path, folder) = match self.items.get(self.selected)? {
            Item::Recent { index, .. } => (self.entries[*index].path.clone(), false),
            Item::Path(p, kind) => (p.clone(), *kind == PathKind::Folder),
            Item::Found(f) => (f.path.clone(), !f.repo),
            Item::Clone(_) => return None,
        };
        // Typed text may mix / and \ on Windows: write the path cleanly.
        let path: PathBuf = path.components().collect();
        self.set_filter(format!(
            "{}{}",
            format::tilde(&path),
            if folder {
                std::path::MAIN_SEPARATOR_STR
            } else {
                ""
            }
        ));
        Some(Output::Completed)
    }

    /// Whether the window is still shaking (the app redraws every frame).
    pub fn shaking(&self) -> bool {
        self.shake.is_some_and(|s| s.elapsed() < SHAKE)
    }

    /// Refuses to open: shake and say why.
    fn reject(&mut self, why: String) {
        self.warning = Some(why);
        self.shake = Some(Instant::now());
    }

    /// ⌘⌫ on the selected row, unless the same key just cleared the
    /// filter text (the field handles it first).
    pub fn forget_selected(&self) -> Option<Output> {
        if self
            .edited
            .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
        {
            return None;
        }
        match self.items.get(self.selected)? {
            Item::Recent { index, .. } => Some(Output::Forget(self.entries[*index].path.clone())),
            Item::Path(..) | Item::Found(_) | Item::Clone(_) => None,
        }
    }

    /// Esc: clears the filter first, then closes.
    pub fn escape(&mut self) -> Option<Output> {
        if self.filter.is_empty() {
            Some(Output::Close)
        } else {
            self.set_filter(String::new());
            None
        }
    }

    fn set_filter(&mut self, f: String) {
        if f != self.filter {
            self.edited = Some(Instant::now());
        }
        self.filter = f;
        self.warning = None;
        self.refilter();
    }

    fn output_for(&mut self, row: usize) -> Option<Output> {
        let refused = match self.items.get(row)? {
            Item::Path(p, PathKind::Repo) => return Some(Output::Open(p.clone())),
            Item::Clone(url) => return Some(Output::Clone(url.clone())),
            Item::Found(f) if f.repo => return Some(Output::Open(f.path.clone())),
            // A folder in the list: go into it, like a file dialog.
            Item::Found(_) => {
                self.selected = row;
                return self.complete();
            }
            Item::Path(p, PathKind::Folder) => format!(
                "not a git repository: {} — type `/` or ⇥ to list the repositories in it",
                format::tilde(p)
            ),
            Item::Path(p, PathKind::Missing) => {
                format!("no such folder: {}", format::tilde(p))
            }
            Item::Recent { index, .. } => {
                let e = &self.entries[*index];
                match e.tab {
                    Some(tab) => return Some(Output::SwitchTab(tab)),
                    None if e.exists => return Some(Output::Open(e.path.clone())),
                    None => format!(
                        "{} no longer exists — × or ⌘⌫ forgets it",
                        format::tilde(&e.path)
                    ),
                }
            }
        };
        self.selected = row;
        self.reject(refused);
        None
    }

    fn refilter(&mut self) {
        let (items, browsing) = matches(&self.entries, &self.filter, &mut self.listing);
        self.items = items;
        self.browsing = browsing;
        self.selected = 0;
        self.first.set(0);
    }

    /// First row to show so the selection stays in view, scrolling as
    /// little as possible.
    fn first_row(&self, rows: usize) -> usize {
        let mut first = self.first.get();
        if self.selected < first {
            first = self.selected;
        } else if self.selected >= first + rows {
            first = self.selected + 1 - rows;
        }
        first = first.min(self.items.len().saturating_sub(rows));
        self.first.set(first);
        self.rows.set(rows);
        first
    }
}

/// The query as a path, reading Korean input-method text as the keys
/// pressed (`/ㅕㄴㄷㄱㄴ` → `/Users`) when only that names a real place.
fn resolve_path(query: &str) -> String {
    let keys = crate::features::prompt::hangul::to_qwerty(query);
    let candidates = || std::iter::once(query).chain(keys.as_deref());
    // A path that exists wins; then one whose folder exists (a name being
    // typed).
    let exists = |q: &&str| format::expand_tilde(q).exists();
    let in_folder = |q: &&str| {
        let p = format::expand_tilde(q);
        listed_dir(q, &p).is_some_and(|d| d.is_dir())
    };
    candidates()
        .find(exists)
        .or_else(|| candidates().find(in_folder))
        .unwrap_or(query)
        .to_owned()
}

/// The folder whose contents a typed path lists: itself after a `/`,
/// else its parent (the last part narrows the list).
fn listed_dir(query: &str, path: &Path) -> Option<PathBuf> {
    if query.ends_with(crate::platform::is_path_sep) {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

/// Repositories (first) and folders in `dir` whose names match `needle`.
/// `listing` keeps the folder's contents for the next key typed in it.
fn children(dir: &Path, needle: &str, skip: &Path, listing: &mut Option<Listing>) -> Vec<Found> {
    if listing.as_ref().is_none_or(|l| l.dir != dir) {
        *listing = Some(Listing::read(dir));
    }
    let folders = listing.as_ref().map_or(&[][..], |l| &l.folders);
    let mut found: Vec<(i32, Found)> = folders
        .iter()
        .filter(|f| f.path != skip)
        .filter_map(|f| {
            let (score, marks) = if needle.is_empty() {
                (0, Vec::new())
            } else {
                fuzzy(needle, &f.name)?
            };
            Some((
                score,
                Found {
                    path: f.path.clone(),
                    name: f.name.clone(),
                    marks,
                    repo: f.repo,
                    head: f.head.clone(),
                },
            ))
        })
        .collect();
    found.sort_by(|(sa, a), (sb, b)| {
        b.repo
            .cmp(&a.repo)
            .then(sb.cmp(sa))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    found.truncate(MAX_FOUND);
    found.into_iter().map(|(_, f)| f).collect()
}

/// Items for `filter`, best first, and the folder being browsed when the
/// filter is a path.
fn matches(
    entries: &[Entry],
    filter: &str,
    listing: &mut Option<Listing>,
) -> (Vec<Item>, Option<PathBuf>) {
    let query = filter.trim();
    if crate::git_actions::is_clone_url(query) {
        return (vec![Item::Clone(query.to_owned())], None);
    }
    if crate::platform::looks_like_path(query) {
        return path_items(entries, query, listing);
    }
    let mut scored: Vec<(i32, usize, Vec<usize>)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, e)| {
            if query.is_empty() {
                return Some((0, index, Vec::new()));
            }
            if let Some((score, marks)) = fuzzy(query, &e.name) {
                return Some((score + fuzzy::RANK, index, marks));
            }
            fuzzy(query, &format::tilde(&e.path)).map(|(score, _)| (score, index, Vec::new()))
        })
        .collect();
    // Stable: equal scores keep the most-recent-first order.
    scored.sort_by_key(|a| std::cmp::Reverse(a.0));
    let items = scored
        .into_iter()
        .map(|(_, index, marks)| Item::Recent { index, marks })
        .collect();
    (items, None)
}

/// A typed path: the path itself, then what is in the folder it lists.
fn path_items(
    entries: &[Entry],
    query: &str,
    listing: &mut Option<Listing>,
) -> (Vec<Item>, Option<PathBuf>) {
    let query = resolve_path(query);
    let path = format::expand_tilde(&query);
    let kind = path_kind(&path);
    let dir = listed_dir(&query, &path).filter(|d| d.is_dir());
    let needle = if query.ends_with(crate::platform::is_path_sep) {
        ""
    } else {
        path.file_name().and_then(|n| n.to_str()).unwrap_or("")
    };
    let found = dir
        .as_deref()
        .map(|d| children(d, needle, &path, listing))
        .unwrap_or_default();
    let recent = |p: &Path| entries.iter().position(|e| e.path == p);

    let mut items = Vec::new();
    // `~/work/` lists the folder: the folder itself is only worth a row
    // when it can be opened, or when there is nothing to list.
    // Nor is a name still being typed (`~/work/ord`).
    let worth_a_row = match kind {
        PathKind::Repo => true,
        PathKind::Folder => !query.ends_with(crate::platform::is_path_sep),
        PathKind::Missing => false,
    };
    if worth_a_row || found.is_empty() {
        items.push(match recent(&path) {
            Some(index) => Item::Recent {
                index,
                marks: Vec::new(),
            },
            None => Item::Path(path, kind),
        });
    }
    items.extend(found.into_iter().map(|f| match recent(&f.path) {
        Some(index) => Item::Recent {
            index,
            marks: f.marks,
        },
        None => Item::Found(f),
    }));
    (items, dir)
}

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.9).round()
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Filter(f) => {
                self.set_filter(f);
                Update::none()
            }
            Message::Pick(row) => match self.output_for(row) {
                Some(out) => Update::output(out),
                None => Update::none(),
            },
            Message::Scroll(delta) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 20.0,
                };
                #[allow(clippy::cast_possible_truncation)]
                let by = -(lines * 3.0).round() as isize;
                self.move_by(by);
                Update::none()
            }
            Message::Forget(p) => Update::output(Output::Forget(p)),
            Message::Browse => Update::output(Output::Browse),
            Message::Close => Update::output(Output::Close),
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let accent = t.accent();
        let filter = row![
            colored(t, "❯", accent),
            text_input(
                "type to filter · a folder like ~/work/ lists the repositories in it · a URL clones it",
                &self.filter
            )
            .id(FILTER_ID)
            .on_input(Message::Filter)
            .size(t.font_size)
            .padding(4)
            .style(style::field_input(t)),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let list: Element<'a, Message> = if self.items.is_empty() {
            widgets::empty_hint(
                t,
                match (&self.browsing, self.entries.is_empty()) {
                    (Some(_), _) => "nothing here matches — keep typing, or ⌫ to go back",
                    (None, true) => "no recent repositories yet — type a folder (~/…/) or Browse…",
                    (None, false) => "nothing matches — type a folder (~/…/) or Browse…",
                },
            )
        } else {
            let row_h = row_height(t);
            responsive(move |size| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let rows = (((size.height + 1.0) / (row_h + 1.0)).floor() as usize).max(1);
                let first = self.first_row(rows);
                let mut col = Column::new().spacing(1);
                for (row, item) in self.items.iter().enumerate().skip(first).take(rows) {
                    col = col.push(self.item_row(t, row, item));
                }
                mouse_area(col).on_scroll(Message::Scroll).into()
            })
            .into()
        };

        let shown = {
            let n = self.items.len();
            let first = self.first.get();
            let last = (first + self.rows.get()).min(n);
            if n > self.rows.get() && self.rows.get() > 0 {
                format!("{}–{last} of {n} · ", first + 1)
            } else {
                String::new()
            }
        };
        let footer = row![
            widgets::text_button(t, "Browse…", Some("⌘O"), Some(Message::Browse)),
            space().width(Length::Fill),
            dim(
                t,
                format!("{shown}↑↓ select · ⏎ open · ⇥ complete path · ⌘⌫ forget · esc close")
            ),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let what = match &self.browsing {
            Some(dir) => {
                let repos = self
                    .items
                    .iter()
                    .filter(|i| match i {
                        Item::Found(f) => f.repo,
                        Item::Recent { .. } => true,
                        Item::Path(_, k) => *k == PathKind::Repo,
                        Item::Clone(_) => false,
                    })
                    .count();
                format!(
                    "in {}{} · {repos} repositories, {} folders",
                    format::tilde(dir).trim_end_matches(crate::platform::is_path_sep),
                    std::path::MAIN_SEPARATOR,
                    self.items.len() - repos
                )
            }
            None if matches!(self.items.first(), Some(Item::Clone(_))) => {
                "clone from a server".to_owned()
            }
            None if self.filter.trim().is_empty() => {
                format!("recent {}", self.entries.len())
            }
            None => format!("recent {}/{}", self.items.len(), self.entries.len()),
        };
        let header = row![
            colored(t, "┌─ OPEN REPOSITORY ", accent),
            dim(t, what),
            space().width(Length::Fill),
            widgets::icon_button(t, "×", t.dim, "close  esc", Message::Close),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let mut body = column![header, filter].spacing(8).height(Length::Fill);
        if let Some(w) = &self.warning {
            body = body.push(colored(t, format!("✗ {w}"), t.error()));
        }
        let border = if self.warning.is_some() {
            t.error()
        } else {
            accent
        };
        let window = container(body.push(container(list).height(Length::Fill)).push(footer))
            .padding(Padding::from([10, 14]))
            .width(Length::Fill)
            .max_width(t.cols(150))
            .height(Length::Fill)
            .style(move |_| container::Style {
                text_color: Some(t.fg),
                background: Some(Background::Color(t.surface)),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            });
        // Nearly the whole window, so many repositories fit; shaking moves
        // it sideways by changing the side margins.
        let dx = self.shake.map_or(0.0, |s| shake_offset(s.elapsed()));
        container(window)
            .padding(Padding {
                top: 32.0,
                bottom: 32.0,
                left: (48.0 + dx).max(0.0),
                right: (48.0 - dx).max(0.0),
            })
            .center_x(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl State {
    fn item_row<'a>(&'a self, t: &'a Tokens, row: usize, item: &'a Item) -> Element<'a, Message> {
        let selected = row == self.selected;
        let pointer = colored(t, if selected { "▸" } else { " " }, t.accent());
        let key = dim(
            t,
            if row < 9 {
                format!("⌘{}", row + 1)
            } else {
                String::new()
            },
        )
        .width(Length::Fixed(t.cols(3)));
        let name_of = |name: &'a str, marks: &[usize], base: iced::Color| {
            let mut out = Row::new();
            for (i, ch) in name.chars().enumerate() {
                let color = if marks.contains(&i) { t.accent() } else { base };
                out = out.push(colored(t, ch.to_string(), color));
            }
            container(out).width(Length::Fixed(t.cols(28)))
        };
        let branch = |h: &Option<String>| match h {
            Some(h) => colored(t, format!("⎇ {}", format::truncate(h, 18)), t.head()),
            None => dim(t, ""),
        };
        let path_col = |p: &Path| {
            dim(t, elide_path(&format::tilde(p), PATH_COLS))
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None)
        };
        let line: Element<'a, Message> = match item {
            Item::Clone(url) => {
                let name = crate::git_actions::humanish_name(url).unwrap_or_default();
                row![
                    pointer,
                    colored(t, "clone repository", t.accent()).width(Length::Fixed(t.cols(28))),
                    mono(t, format::truncate(&name, 20)).width(Length::Fixed(t.cols(21))),
                    dim(t, elide_path(url, PATH_COLS))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::None),
                    key,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            }
            Item::Path(path, kind) => {
                let (what, color) = match kind {
                    PathKind::Repo => ("open repository", t.accent()),
                    PathKind::Folder => ("✗ not a git repo", t.warning()),
                    PathKind::Missing => ("✗ no such folder", t.error()),
                };
                row![
                    pointer,
                    colored(t, what, color).width(Length::Fixed(t.cols(28))),
                    dim(t, "").width(Length::Fixed(t.cols(21))),
                    path_col(path),
                    key,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            }
            Item::Found(f) => {
                let (base, kind): (_, Element<'a, Message>) = if f.repo {
                    (t.branch(), branch(&f.head).into())
                } else {
                    (t.fg, dim(t, "▸ folder").into())
                };
                let name = if f.repo {
                    f.name.clone()
                } else {
                    format!("{}/", f.name)
                };
                let mut chars = Row::new();
                for (i, ch) in name.chars().enumerate() {
                    let color = if f.marks.contains(&i) {
                        t.accent()
                    } else {
                        base
                    };
                    chars = chars.push(colored(t, ch.to_string(), color));
                }
                row![
                    pointer,
                    container(chars).width(Length::Fixed(t.cols(28))),
                    container(kind).width(Length::Fixed(t.cols(21))),
                    path_col(&f.path),
                    key,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            }
            Item::Recent { index, marks } => {
                let e = &self.entries[*index];
                let base = if e.exists { t.branch() } else { t.dim };
                let head: Element<'a, Message> = if e.exists {
                    match &e.head {
                        Some(_) => branch(&e.head).into(),
                        None => dim(t, "not a repo?").into(),
                    }
                } else {
                    colored(t, "not found", t.error()).into()
                };
                let tail: Element<'a, Message> = match e.tab {
                    Some(tab) => colored(t, format!("● tab {}", tab + 1), t.success()).into(),
                    None => widgets::icon_button(
                        t,
                        "×",
                        t.dim,
                        "forget this repository  ⌘⌫",
                        Message::Forget(e.path.clone()),
                    ),
                };
                row![
                    pointer,
                    name_of(&e.name, marks, base),
                    container(head).width(Length::Fixed(t.cols(21))),
                    path_col(&e.path),
                    container(tail)
                        .width(Length::Fixed(t.cols(7)))
                        .align_x(iced::Alignment::End),
                    key,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            }
        };
        button(
            container(line)
                .height(Length::Fill)
                .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(row_height(t)))
        .padding(Padding::from([0, 6]))
        .style(style::row_button(t, selected))
        .on_press(Message::Pick(row))
        .into()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn entry(path: &str, tab: Option<usize>, exists: bool) -> Entry {
        let path = PathBuf::from(path);
        Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            head: Some("main".into()),
            exists,
            tab,
        }
    }

    fn picker() -> State {
        State::new(vec![
            entry("/src/gf", Some(1), true),
            entry("/src/github-desktop", None, true),
            entry("/old/gone", None, false),
        ])
    }

    #[test]
    fn a_typed_url_offers_to_clone_it() {
        let mut s = picker();
        let _ = s.update(Message::Filter(" git@github.com:team/app.git ".into()));
        assert_eq!(names(&s), ["git@github.com:team/app.git"]);
        assert_eq!(
            s.activate(),
            Some(Output::Clone("git@github.com:team/app.git".into()))
        );
        assert_eq!(s.complete(), None, "nothing to complete");
        assert_eq!(s.forget_selected(), None);
    }

    fn names(s: &State) -> Vec<String> {
        s.items
            .iter()
            .map(|i| match i {
                Item::Recent { index, .. } => s.entries[*index].name.clone(),
                Item::Path(p, _) => p.display().to_string(),
                Item::Found(f) => f.name.clone(),
                Item::Clone(url) => url.clone(),
            })
            .collect()
    }

    #[test]
    fn empty_filter_lists_recent_order() {
        assert_eq!(names(&picker()), ["gf", "github-desktop", "gone"]);
    }

    #[test]
    fn filter_is_fuzzy_and_prefers_names() {
        let mut s = picker();
        let _ = s.update(Message::Filter("gd".into()));
        assert_eq!(names(&s), ["github-desktop"]);
        // "src" is only in paths: still found, after name matches.
        let _ = s.update(Message::Filter("src".into()));
        assert_eq!(names(&s), ["gf", "github-desktop"]);
        let _ = s.update(Message::Filter("GF".into()));
        assert_eq!(names(&s)[0], "gf");
    }

    #[test]
    fn word_starts_match_and_loose_letters_do_not() {
        let (_, marks) = fuzzy("gd", "github-desktop").unwrap();
        assert_eq!(marks, [0, 7]);
        assert!(fuzzy("gd", "xgxxxxxd").is_none(), "letters from anywhere");
        assert!(fuzzy("zz", "gf").is_none());
    }

    #[test]
    fn enter_switches_opens_or_reports_missing() {
        let mut s = picker();
        assert_eq!(s.activate(), Some(Output::SwitchTab(1)));
        s.move_by(1);
        assert_eq!(
            s.activate(),
            Some(Output::Open("/src/github-desktop".into()))
        );
        s.move_by(5);
        assert_eq!(s.activate(), None);
        assert!(s.warning.as_deref().unwrap().contains("no longer exists"));
        assert!(s.shaking());
        s.move_by(-10);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn typed_path_is_offered_first_and_checked_for_git() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();

        let mut s = picker();
        let typed = |s: &mut State, p: &Path| {
            let _ = s.update(Message::Filter(p.display().to_string()));
        };
        typed(&mut s, &repo);
        assert_eq!(s.activate(), Some(Output::Open(repo.clone())));
        // A folder inside a work tree is a repository too (git looks up).
        typed(&mut s, &repo.join("src"));
        assert_eq!(s.activate(), Some(Output::Open(repo.join("src"))));

        typed(&mut s, &plain);
        assert_eq!(s.activate(), None);
        assert!(
            s.warning
                .as_deref()
                .unwrap()
                .contains("not a git repository")
        );
        assert!(s.shaking());
        // Editing the path clears the warning.
        typed(&mut s, &dir.path().join("nope"));
        assert_eq!(s.warning, None);
        assert_eq!(s.activate(), None);
        assert!(s.warning.as_deref().unwrap().contains("no such folder"));
        // A known path is not offered twice.
        let _ = s.update(Message::Filter("/src/gf".into()));
        assert!(!s.items.iter().any(|i| matches!(i, Item::Path(..))));
    }

    // /tmp and /private/tmp exist on macOS only.
    #[cfg(target_os = "macos")]
    #[test]
    fn path_typed_with_the_korean_input_method_is_read_as_keys() {
        // "/ㅅㅡㅔ" is what /tmp looks like with the 2-set Korean layout.
        let mut s = picker();
        let _ = s.update(Message::Filter("/ㅅㅡㅔ".into()));
        assert_eq!(s.items[0], Item::Path("/tmp".into(), PathKind::Folder));
        // Syllables the input method composed are taken apart too.
        let _ = s.update(Message::Filter("/ㅔ걒ㅁㅅㄷ/ㅅㅡㅔ".into()));
        assert_eq!(
            s.items[0],
            Item::Path("/private/tmp".into(), PathKind::Folder)
        );
    }

    #[test]
    fn a_typed_folder_lists_its_repositories_then_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for svc in ["order-svc", "billing-svc"] {
            std::fs::create_dir_all(root.join(svc).join(".git")).unwrap();
        }
        std::fs::create_dir_all(root.join("team")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("notes.txt"), "").unwrap();
        let typed = |s: &mut State, text: String| {
            let _ = s.update(Message::Filter(text));
        };
        let base = root.display().to_string();

        let mut s = picker();
        typed(&mut s, format!("{base}/"));
        assert_eq!(names(&s), ["billing-svc", "order-svc", "team"]);
        assert_eq!(s.browsing.as_deref(), Some(root));

        // The last part narrows the list (fuzzy).
        typed(&mut s, format!("{base}/osv"));
        // (no row for the half-typed name itself)
        assert_eq!(names(&s), ["order-svc"]);
        assert_eq!(s.activate(), Some(Output::Open(root.join("order-svc"))));

        // ⏎ on a folder goes into it; with nothing inside, the folder
        // itself is offered — and refused, it is not a repository.
        typed(&mut s, format!("{base}/"));
        s.move_by(2);
        assert_eq!(s.activate(), Some(Output::Completed));
        // Completion writes the path as the platform does (`~`, separators).
        let shown = |p: &Path| format::tilde(p);
        assert_eq!(
            s.filter,
            format!("{}{}", shown(&root.join("team")), std::path::MAIN_SEPARATOR)
        );
        assert_eq!(s.activate(), None);
        assert!(s.shaking());

        // ⇥ on a repository completes its path.
        typed(&mut s, format!("{base}/bil"));
        assert_eq!(s.complete(), Some(Output::Completed));
        assert_eq!(s.filter, shown(&root.join("billing-svc")));
        assert_eq!(
            s.items[0],
            Item::Path(root.join("billing-svc"), PathKind::Repo)
        );
    }

    #[test]
    fn the_list_scrolls_as_little_as_possible() {
        let entries = (0..50)
            .map(|i| entry(&format!("/src/svc-{i}"), None, true))
            .collect();
        let mut s = State::new(entries);
        assert_eq!(s.first_row(10), 0);
        s.move_by(30);
        assert_eq!(s.first_row(10), 21);
        s.move_by(-5);
        assert_eq!(s.first_row(10), 21);
        s.move_by(-20);
        assert_eq!(s.first_row(10), 5);
        assert_eq!(s.page(), 9);
    }

    #[test]
    fn escape_clears_the_filter_then_closes() {
        let mut s = picker();
        let _ = s.update(Message::Filter("x".into()));
        assert_eq!(s.escape(), None);
        assert_eq!(names(&s).len(), 3);
        assert_eq!(s.escape(), Some(Output::Close));
    }

    #[test]
    fn command_backspace_that_cleared_the_filter_does_not_forget() {
        let mut s = picker();
        let _ = s.update(Message::Filter("gd".into()));
        let _ = s.update(Message::Filter(String::new()));
        assert_eq!(s.forget_selected(), None);
        // Unchanged text (it was already empty) is not an edit.
        s.edited = None;
        let _ = s.update(Message::Filter(String::new()));
        assert_eq!(s.forget_selected(), Some(Output::Forget("/src/gf".into())));
    }

    #[test]
    fn forgetting_keeps_the_selection_in_range() {
        let mut s = picker();
        s.move_by(2);
        assert_eq!(
            s.forget_selected(),
            Some(Output::Forget("/old/gone".into()))
        );
        s.forgotten(Path::new("/old/gone"));
        assert_eq!(s.selected, 1);
        assert_eq!(names(&s), ["gf", "github-desktop"]);
    }

    #[test]
    fn long_paths_keep_their_end_on_one_line() {
        assert_eq!(elide_path("~/src/gf", 40), "~/src/gf");
        let long = "~/claudework/github-desktop-ai/app/static/common/gitignore";
        let short = elide_path(long, 30);
        assert_eq!(short, "…/app/static/common/gitignore");
        assert!(short.chars().count() <= 30);
    }

    #[test]
    fn the_shake_dies_down() {
        assert!(shake_offset(Duration::from_millis(30)).abs() > 1.0);
        assert!(shake_offset(Duration::from_millis(440)).abs() < 1.0);
        assert!(shake_offset(SHAKE).abs() < f32::EPSILON);
    }

    #[test]
    fn head_file_names_branch_or_short_commit() {
        assert_eq!(
            head_label("ref: refs/heads/main\n").as_deref(),
            Some("main")
        );
        assert_eq!(
            head_label("0123456789abcdef0123456789abcdef01234567\n").as_deref(),
            Some("0123456")
        );
        assert_eq!(head_label("garbage"), None);
    }
}
