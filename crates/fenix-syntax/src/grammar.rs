//! Grammars an extension brings: tree-sitter grammars compiled to
//! WebAssembly (`tree-sitter build --wasm`), named by a language file, with
//! their queries from the extension's own folder.
//!
//! tree-sitter runs them with its `wasm` feature, on the wasmtime its
//! wasm store is written against (36, a long-term-support release) -- not
//! the one Fenix runs extensions on: tree-sitter's store uses wasmtime's C
//! API at the version it pins, so the two can't be one. A grammar's code
//! reaches nothing but the text it's given: tree-sitter's store gives it
//! the lexer's callbacks and a small C library, no WASI.
//!
//! A grammar is compiled the first time a buffer needs it, once a run.
//! Each parser on one has a store of its own, as tree-sitter wants.

use std::sync::{Mutex, OnceLock};

use tree_sitter::{wasmtime, Language, Parser, WasmStore};

/// A grammar from a language file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grammar {
    /// The language's name, as settings and language files call it
    /// (`kotlin`).
    pub name: String,
    /// Its name inside the WebAssembly: `tree_sitter_<symbol>` is what it
    /// exports (`kotlin`, `c_sharp`).
    pub symbol: String,
    pub wasm: Vec<u8>,
    /// `highlights.scm`; "" leaves it plain.
    pub highlights: String,
    /// `folds.scm`: what a `@fold` capture spans folds, and is a scope.
    pub folds: String,
    /// `indents.scm`: Enter inside an `@indent` that starts on the line
    /// indents the new line a level more.
    pub indents: String,
}

struct Loaded {
    grammar: Grammar,
    language: OnceLock<Result<Language, String>>,
}

static GRAMMARS: OnceLock<Vec<Loaded>> = OnceLock::new();

/// What went wrong with a grammar as it was loaded, to be said once.
static PROBLEMS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The most a grammar's WebAssembly may be.
pub const MAX_GRAMMAR: u64 = 32 << 20;

/// Settles the language files' grammars for this run, before any file is
/// opened. Once: a second call changes nothing.
pub fn set_grammars(grammars: Vec<Grammar>) {
    let _ = GRAMMARS.set(grammars.into_iter().map(|grammar| Loaded { grammar, language: OnceLock::new() }).collect());
}

/// What went wrong loading a grammar since this was last asked: a grammar
/// that doesn't load, a query that doesn't compile.
pub fn take_grammar_problems() -> Vec<String> {
    std::mem::take(&mut *PROBLEMS.lock().unwrap_or_else(|e| e.into_inner()))
}

pub(crate) fn said(problem: String) {
    PROBLEMS.lock().unwrap_or_else(|e| e.into_inner()).push(problem);
}

fn all() -> &'static [Loaded] {
    GRAMMARS.get().map(Vec::as_slice).unwrap_or(&[])
}

/// How many grammars there are.
pub(crate) fn count() -> usize {
    all().len()
}

/// Grammar `i`, as it was read.
pub(crate) fn grammar(i: u16) -> Option<&'static Grammar> {
    all().get(i as usize).map(|l| &l.grammar)
}

/// The grammar called `name`, any case.
pub(crate) fn find(name: &str) -> Option<u16> {
    all().iter().position(|l| l.grammar.name.eq_ignore_ascii_case(name.trim())).map(|i| i as u16)
}

/// The engine every grammar's store is made from.
fn engine() -> &'static wasmtime::Engine {
    static ENGINE: OnceLock<wasmtime::Engine> = OnceLock::new();
    ENGINE.get_or_init(wasmtime::Engine::default)
}

/// Grammar `i` as a tree-sitter language: compiled the first time, and
/// then kept. None when it doesn't load (said once).
pub(crate) fn language(i: u16) -> Option<Language> {
    let loaded = all().get(i as usize)?;
    let language = loaded.language.get_or_init(|| {
        let language = load(&loaded.grammar.symbol, &loaded.grammar.wasm);
        if let Err(e) = &language {
            said(format!("{}'s grammar: {e}", loaded.grammar.name));
        }
        language
    });
    language.as_ref().ok().cloned()
}

/// How much stack compiling a grammar is given. Cranelift's rewriting
/// recurses deep on a grammar's large functions: an unoptimised build
/// overflowed the main thread's 1 MiB on Windows compiling the HTTP
/// extension's.
const COMPILING_STACK: usize = 64 << 20;

/// `f`, which compiles WebAssembly, on a thread with `COMPILING_STACK`,
/// waited for.
fn compiling<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("grammar".into())
            .stack_size(COMPILING_STACK)
            .spawn_scoped(scope, f)
            .expect("a thread for compiling a grammar")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// A wasm store for grammars, made where compiling has room.
fn new_store() -> Result<WasmStore, String> {
    compiling(|| WasmStore::new(engine()).map_err(|e| e.to_string()))
}

/// `wasm`, a grammar exporting `tree_sitter_<symbol>`, as a language.
pub fn load(symbol: &str, wasm: &[u8]) -> Result<Language, String> {
    compiling(|| {
        let mut store = WasmStore::new(engine()).map_err(|e| format!("{symbol}: {e}"))?;
        store.load_language(symbol, wasm).map_err(|e| format!("{symbol}: {e}"))
    })
}

/// A parser for `language`: with a wasm store of its own when it's a
/// grammar from WebAssembly. None when it can't take it.
pub(crate) fn parser(language: &Language) -> Option<Parser> {
    let mut parser = Parser::new();
    if language.is_wasm() {
        let store = new_store().ok()?;
        parser.set_wasm_store(store).ok()?;
    }
    parser.set_language(language).ok()?;
    Some(parser)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// tree-sitter-json 0.24.8's own release build (`tree-sitter-json.wasm`
    /// from its GitHub release; MIT, Max Brunsfeld). See
    /// `tests/grammars/README.md`.
    pub(crate) const JSON_WASM: &[u8] = include_bytes!("../tests/grammars/tree-sitter-json.wasm");

    #[test]
    fn a_grammar_from_webassembly_parses() {
        let language = load("json", JSON_WASM).unwrap();
        assert!(language.is_wasm());
        let mut parser = parser(&language).unwrap();
        let tree = parser.parse("{\"a\": [1, true]}", None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(document (object (pair key: (string (string_content)) value: (array (number) (true)))))");
        // A second parser, with a store of its own, on the same language.
        let mut other = super::parser(&language).unwrap();
        assert!(!other.parse("[1, 2", None).unwrap().root_node().to_sexp().is_empty());
    }

    /// Compiling a grammar needs more stack than a caller may have (the
    /// main thread's 1 MiB on Windows, which the HTTP extension's grammar
    /// overflowed in an unoptimised build): it gets its own.
    #[test]
    fn a_grammar_loads_from_a_thread_with_little_stack() {
        let parsed = std::thread::Builder::new()
            .stack_size(128 << 10)
            .spawn(|| {
                let language = load("json", JSON_WASM).unwrap();
                let mut parser = parser(&language).unwrap();
                parser.parse("[1]", None).unwrap().root_node().to_sexp()
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(parsed, "(document (array (number)))");
    }

    #[test]
    fn what_isnt_a_grammar_is_said() {
        assert!(load("json", b"\0asm\x01\0\0\0").unwrap_err().starts_with("json: "));
        assert!(load("yaml", JSON_WASM).is_err(), "no tree_sitter_yaml in it");
    }
}
