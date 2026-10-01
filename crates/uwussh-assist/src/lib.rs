//! The command assistant: a request in words — "liste alle benutzer auf" —
//! to one command for the system the terminal is connected to.
//!
//! - [`platform`] — which system, shell and package manager the command is
//!   for; the cache key
//! - [`normalize`] — the offline cache's idea of "the same request"
//! - [`prompt`] — what the model is asked, how its answer is read, and the
//!   floor for what counts as dangerous
//! - [`provider`] — Ollama, OpenAI-compatible servers, OpenAI, Anthropic,
//!   Mistral
//!
//! Storage and sync live in `uwussh-store` (settings, keys in the vault, the
//! cache's slots); the app puts the pieces together. Nothing in here ever
//! runs a command: the answer is text the person types or does not.

pub mod normalize;
pub mod platform;
pub mod prompt;
pub mod provider;

pub use normalize::{best_match, normalize, similarity, THRESHOLD};
pub use platform::{Family, Packages, Platform, Shell};
pub use prompt::{looks_dangerous, Answer, Language};
pub use provider::{
    check_base_url, AssistError, Client, Endpoint, Failure, ProviderKind, Timeouts,
    OLLAMA_DEFAULT_URL,
};
