use std::path::PathBuf;

/// Result alias used across gf crates.
pub type Result<T, E = GfError> = std::result::Result<T, E>;

/// Errors shared by every layer. Crates with richer failure modes (hosting,
/// auth) define their own error and convert into this one at their boundary.
#[derive(Debug, thiserror::Error)]
pub enum GfError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("`{}` failed with exit code {exit_code:?}: {stderr}", argv.join(" "))]
    GitCommand {
        argv: Vec<String>,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("failed to parse {what}: {detail}")]
    Parse { what: &'static str, detail: String },

    #[error("invalid object id: {0}")]
    InvalidOid(String),

    #[error("not a git repository: {}", .0.display())]
    NotARepository(PathBuf),

    #[error("file exceeds the preview size limit")]
    FilePreviewTooLarge,

    /// A command printed more than the caller allowed (`limit` bytes).
    #[error("git output exceeds the {} MiB limit", limit / (1024 * 1024))]
    OutputTooLarge { limit: usize },

    /// A diff larger than the diff limit (`limit` bytes): too large to show.
    #[error("diff is over {} MiB, too large to show", limit / (1024 * 1024))]
    DiffTooLarge { limit: usize },

    #[error("operation cancelled")]
    Cancelled,

    #[error("not supported: {0}")]
    Unsupported(&'static str),

    #[error("{0}")]
    Other(String),
}

impl GfError {
    pub fn parse(what: &'static str, detail: impl Into<String>) -> Self {
        Self::Parse {
            what,
            detail: detail.into(),
        }
    }
}
