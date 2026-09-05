//! The platform facts `std::fs` does not give you, for a file manager
//! that has to work on a real Windows machine and on real network
//! shares.
//!
//! Kept apart from `fenix-explorer` (which owns the listing *model* --
//! selection, marks, expansion) so that the awkward, platform-specific
//! half can be tested on its own against a real filesystem, the same
//! split `fenix-recovery` already uses. Nothing here knows about
//! rendering, windows, or fenix at all.

mod archive;
mod listing;
mod pathbar;
mod places;
mod properties;
mod recycle;
mod transfer;

#[cfg(test)]
mod test_util;

pub use archive::{
    archiver, create as create_archive, extract as extract_archive, looks_like_archive, suggested_destination, suggested_name,
};
pub use listing::{list_dir, Attributes, DirEntryInfo, EntryKind};
pub use properties::{measure_tree, properties, set_readonly, Properties, Total};
pub use recycle::{permanently, to_recycle_bin, Outcome};
pub use pathbar::{common_prefix, complete, expand};
pub use places::{shares, volumes, Volume, VolumeKind};
pub use transfer::{
    conflicts_in, copy_into, copy_into_reporting, measure, move_into, move_into_reporting, rename_all, OnConflict, Progress,
};
