mod files;
mod grep;
mod root;

#[cfg(test)]
mod test_util;

pub mod doctor;
mod kind;
mod known;
pub mod meta;
mod recent;
pub mod vcs;

pub use files::{list_project_files, list_project_files_including_ignored};
pub use grep::{files_matching, grep_project, GrepMatch};
pub use kind::{declared_kind, detect_kind, ProjectKind};
pub use known::KnownProjects;
pub use recent::RecentFiles;
pub use root::find_project_root;

pub mod template;
pub mod tools;
