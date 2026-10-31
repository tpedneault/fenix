//! Markdown as the reading view reads it: front matter, `[[links]]`,
//! `#tags`, headings and checkboxes (`meta`), and the blocks it lays out
//! (`blocks`). The notebook is an extension now (`fenix.notebook`); what
//! a link goes to is its to say.

pub mod blocks;
pub mod meta;

pub use meta::Link;
