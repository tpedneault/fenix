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
pub mod workspace;

pub use files::{list_project_files, list_project_files_including_ignored};
pub use grep::{files_matching, grep_project, GrepMatch};
pub use kind::{declared_kind, detect_kind, detect_kind_from_files, main_file, ProjectKind};
pub use known::{plain_path, read_path_list, KnownProjects};
pub use recent::RecentFiles;
pub use root::find_project_root;

pub mod template;
pub mod tools;
