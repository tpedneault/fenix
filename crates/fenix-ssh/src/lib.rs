//! Fenix's side of a remote project.
//!
//! One `ssh` process per host carries everything. A [`Host`] starts it,
//! puts the helper (`fenix-remote`) on the machine if it isn't there,
//! mounts the machine's files under its remote paths (`fenix-vfs`), and
//! connects again when the connection is lost. Under it, [`connect`]
//! gives one [`Session`] to ask files and run programs through.
//! Signing in is `ssh`'s own business, with its config, keys and agent;
//! what it has to ask comes back through [`askpass`].
//!
//! [`bridge`] lets code that only knows how to start a local program
//! (`git`, a language server) run one on the remote machine, through a
//! stand-in process that joins the open connection.

pub mod askpass;
pub mod bridge;
pub mod config;
mod host;
mod local;
mod machine;
mod session;
mod ssh;
pub mod testing;

pub use fenix_remote::{Entry, Exec, Failure, FailureCode, FileKind, Hello, Pty, Search, Searched, Version};
pub use host::{Host, Status};
pub use session::{Error, Finished, Output, Process, ProcessInput, RemoteTerminal, Session, StartError, TerminalOutput, DEADLINE};
pub use ssh::{connect, ConnectError, Connected, Helpers, Target};
