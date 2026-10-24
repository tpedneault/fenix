mod edit;
mod highlight;
mod language;
mod state;
pub mod todo;
pub mod xml;

pub use edit::RawEdit;
pub use language::{detect_language, detect_language_from_path, set_file_types, FileTypes, LanguageId};
pub use state::SyntaxState;
pub use todo::{TodoItem, TodoKind};
