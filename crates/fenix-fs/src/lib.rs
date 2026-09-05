//! The platform facts `std::fs` does not give you, for a file manager
//! that has to work on a real Windows machine and on real network
//! shares.
//!
//! Kept apart from `fenix-explorer` (which owns the listing *model* --
//! selection, marks, expansion) so that the awkward, platform-specific
//! half can be tested on its own against a real filesystem, the same
//! split `fenix-recovery` already uses. Nothing here knows about
//! rendering, windows, or fenix at all.

mod listing;
mod pathbar;
mod places;
mod recycle;
mod transfer;

#[cfg(test)]
mod test_util;

pub use listing::{list_dir, Attributes, DirEntryInfo, EntryKind};
pub use recycle::{permanently, to_recycle_bin, Outcome};
pub use pathbar::{common_prefix, complete, expand};
pub use places::{shares, volumes, Volume, VolumeKind};
pub use transfer::{conflicts_in, copy_into, move_into, OnConflict};
