//! Plain data types describing a repository. Parsers in `gf-git` produce them,
//! UI features consume them.

mod commit;
mod oid;
mod refs;
mod remote;
mod repo;
mod status;

pub use commit::{Commit, Signature, Timestamp};
pub use oid::Oid;
pub use refs::{Branch, RefKind, RefName, Tag, Upstream};
pub use remote::Remote;
pub use repo::{RepoHandle, RepoId};
pub use status::{ChangeKind, ConflictKind, StatusEntry};
