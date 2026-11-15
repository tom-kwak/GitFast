#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    /// Separate push URL (`remote.<name>.pushurl`), if configured.
    pub push_url: Option<String>,
}

impl Remote {
    pub fn effective_push_url(&self) -> &str {
        self.push_url.as_deref().unwrap_or(&self.fetch_url)
    }
}
