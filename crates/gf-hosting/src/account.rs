//! The account details gf can remember without persisting a credential.

use serde::{Deserialize, Serialize};

/// An account identified by its server and username. Credentials live in Git's helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// `https`, or `http` for a server without TLS.
    pub protocol: String,
    /// Host name, including an explicit port when present.
    pub host: String,
    pub username: String,
}
