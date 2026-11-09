//! Fenix's side of a remote project.
//!
//! One `ssh` process per host carries everything: [`connect`] starts it,
//! puts the helper (`fenix-remote`) on the machine if it isn't there, and
//! gives back a [`Session`] to ask files and run programs through.
//! Signing in is `ssh`'s own business, with its config, keys and agent;
//! what it has to ask comes back through [`askpass`].
//!
//! [`bridge`] lets code that only knows how to start a local program
//! (`git`, a language server) run one on the remote machine, through a
//! stand-in process that joins the open connection.

pub mod askpass;
pub mod bridge;
mod local;
mod session;
mod ssh;

pub use fenix_remote::{Entry, Exec, Failure, FailureCode, FileKind, Hello, Version};
pub use session::{Error, Finished, Output, Process, ProcessInput, Session, StartError, DEADLINE};
pub use ssh::{connect, ConnectError, Connected, Helpers, Target};
