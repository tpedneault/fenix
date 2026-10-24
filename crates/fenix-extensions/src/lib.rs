//! Extensions you install: what one says it is (`extension.toml`), reading
//! one from a `.fenixext` package (a zip) or a folder, and the extensions
//! installed on this machine -- each under `<id>/<version>/` beside an
//! `extensions.toml` of what's installed and switched on.
//!
//! Phase 2 of `docs/EXTENSIONS_PLAN.md`: data-only extensions (themes,
//! snippets, templates). One with code of its own (`runtime = "wasm"` or
//! `"lua"`) is refused until a Fenix that can run it.

pub mod manifest;
pub mod package;
pub mod registry;
pub mod store;

pub use manifest::{Manifest, Runtime};
pub use package::Package;
pub use store::{Installed, Policy, Store};

/// The data folders an extension can carry, each read the way Fenix
/// reads yours of the same name (`keymaps/*.toml` like your
/// `keybindings.toml`).
pub const DATA_DIRS: &[&str] = &["themes", "snippets", "templates", "keymaps", "languages"];
