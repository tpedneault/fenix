mod edit;
mod highlight;
mod language;
mod state;

pub use edit::RawEdit;
pub use language::{detect_language, detect_language_from_path, LanguageId};
pub use state::SyntaxState;
