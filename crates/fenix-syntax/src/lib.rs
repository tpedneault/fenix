mod edit;
mod grammar;
mod highlight;
mod language;
mod state;
pub mod todo;
pub mod xml;

pub use edit::RawEdit;
pub use grammar::{load as load_grammar, set_grammars, take_grammar_problems, Grammar, MAX_GRAMMAR};
pub use language::{detect_language, detect_language_from_path, set_file_types, FileTypes, LanguageId};
pub use state::SyntaxState;
pub use todo::{TodoItem, TodoKind};
