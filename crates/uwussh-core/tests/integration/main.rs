//! The integration tests, one binary for all of them: every test file is a
//! module here, so the crate and its dependencies are linked once instead of
//! once per file.

#[path = "../support/file_server.rs"]
mod file_server;
#[path = "../support/forwarding.rs"]
mod forwarding;

mod files;
mod ssh;
