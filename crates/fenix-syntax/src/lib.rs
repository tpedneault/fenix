mod edit;
mod highlight;
mod language;
mod state;

pub use edit::RawEdit;
pub use language::{detect_language, LanguageId};
pub use state::SyntaxState;
