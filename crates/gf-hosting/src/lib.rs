//! Shared account models and interfaces for Git hosting providers.

mod account;
mod provider;

pub use account::Account;
pub use provider::{HostingProvider, ProviderKind, Providers, TokenSetup};
