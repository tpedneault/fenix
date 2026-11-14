mod files;
mod grep;
pub mod search;
pub mod server_install;
mod process;
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
pub use kind::{declared_id, declared_kind, detect_kind, detect_kind_from_files, main_file, KindColor, KindDef, ProjectKind};
pub use known::{plain_path, read_path_list, KnownProjects};
pub use recent::RecentFiles;
pub use root::{find_project_root, is_sketch, set_extension_markers};
pub use kind::{extension_kind_notes, fenix_marker, is_invisible, main_is_below, marker_is_a_name, name_is_shown, set_extension_kinds, DEVICE_NAMES, FENIX_MARKERS, FENIX_NAMES, FENIX_TAGS, FORMAT_CHARACTERS, MAX_EXTENSION_KINDS, MAX_EXTENSION_MARKERS};

pub mod template;
pub mod tools;
pub mod trust;
