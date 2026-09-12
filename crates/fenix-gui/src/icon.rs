use std::path::Path;

/// Font family the explorer's icon glyphs are rendered in -- a separate
/// `Attrs::family` from the body text's `Family::Monospace`, mixed into
/// the same row's rich-text spans exactly like `content_spans` already
/// mixes gutter-number spans with content-text spans.
///
/// The face is **bundled**, not assumed installed: `text.rs` embeds
/// `assets/fonts/symbols_nerd_font_mono.ttf` and registers it into the
/// shared `FontSystem` font database at `TextPipeline::new`, so these
/// codepoints resolve identically on Windows and Linux with nothing for
/// the user to install. Redistribution is covered by the SIL Open Font
/// License 1.1 (Copyright (c) 2014, Ryan L McIntyre), whose text ships
/// next to the font as `assets/fonts/LICENSE-symbols-nerd-font.txt` --
/// the OFL permits bundling precisely on the condition that the notice
/// travels with the binary.
///
/// One thing this family is *not*: the same cell width as the body
/// font. It's the `...Mono` build, which makes every icon the same
/// width as every other icon, not the same width as a body character
/// (it measures ~1.8x one at the same pt size -- see `text.rs`'s
/// `the_icon_fonts_cell_is_not_the_body_fonts_cell`). Anything laying
/// out a row that mixes the two must measure the shaped glyphs rather
/// than multiply a character count by one cell width.
pub const ICON_FONT_FAMILY: &str = "Symbols Nerd Font Mono";

// Codepoints verified two ways before being hardcoded here: against the
// authoritative glyph-name table (github.com/ryanoasis/nerd-fonts'
// `glyphnames.json`) and against the actual installed font's cmap, not
// guessed from memory or trusted from a summarized fetch.
const FOLDER: char = '\u{f07b}'; // nf-fa-folder
const FOLDER_OPEN: char = '\u{f07c}'; // nf-fa-folder_open
const FILE_GENERIC: char = '\u{f15b}'; // nf-fa-file
const FILE_CODE: char = '\u{f1c9}'; // nf-fa-file_code_o -- fallback for a recognized-but-iconless extension
const RUST: char = '\u{e7a8}'; // nf-dev-rust
const PYTHON: char = '\u{e73c}'; // nf-dev-python
const JAVASCRIPT: char = '\u{e781}'; // nf-dev-javascript
const TYPESCRIPT: char = '\u{e8ca}'; // nf-dev-typescript
const MARKDOWN: char = '\u{e73e}'; // nf-dev-markdown
const JSON: char = '\u{e60b}'; // nf-seti-json
const HTML: char = '\u{e736}'; // nf-dev-html5
const CSS: char = '\u{e749}'; // nf-dev-css3
const C_LANG: char = '\u{e771}'; // nf-dev-c
const CPP: char = '\u{e7a3}'; // nf-dev-cplusplus
const SHELL: char = '\u{e691}'; // nf-seti-shell -- also used for Tcl, which has no dedicated
// nerd-fonts icon (checked); closest honest fit is "scripting language."
const YAML: char = '\u{e6a8}'; // nf-seti-yml
const TOML: char = '\u{eb51}'; // nf-cod-settings_gear -- generic "config" fallback, TOML has no dedicated icon either
const ARCHIVE: char = '\u{f1c6}'; // nf-fa-file_archive_o
const IMAGE: char = '\u{f1c5}'; // nf-fa-file_image_o

/// The icon glyph for one explorer row. `expanded` only matters for
/// directories (open vs. closed folder); ignored for files.
pub fn icon_for(name: &str, is_dir: bool, expanded: bool) -> char {
    if is_dir {
        return if expanded { FOLDER_OPEN } else { FOLDER };
    }
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
    match ext.as_deref() {
        Some("rs") => RUST,
        Some("py" | "pyi") => PYTHON,
        Some("js" | "mjs" | "cjs" | "jsx") => JAVASCRIPT,
        Some("ts" | "mts" | "cts" | "tsx") => TYPESCRIPT,
        Some("md" | "markdown") => MARKDOWN,
        Some("json") => JSON,
        Some("html" | "htm") => HTML,
        Some("css") => CSS,
        Some("c" | "h") => C_LANG,
        Some("cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "inl") => CPP,
        Some("sh" | "bash" | "tcl" | "tm") => SHELL,
        Some("yaml" | "yml") => YAML,
        Some("toml") => TOML,
        Some("zip" | "tar" | "gz" | "xz" | "bz2" | "7z") => ARCHIVE,
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp") => IMAGE,
        Some(_) => FILE_CODE, // has *some* extension we don't specifically recognize
        None => FILE_GENERIC,
    }
}

/// A compact document-navigation icon. Tcl's shell glyph is useful in an
/// explorer, but reads as a literal `$` when it sits inline with breadcrumb
/// labels, so navigation deliberately uses the neutral code-file glyph.
pub fn navigation_icon_for(name: &str) -> char {
    match Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("tcl" | "tm") => FILE_CODE,
        _ => icon_for(name, false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_use_folder_icons_based_on_expansion() {
        assert_eq!(icon_for("src", true, false), FOLDER);
        assert_eq!(icon_for("src", true, true), FOLDER_OPEN);
    }

    #[test]
    fn known_extensions_map_to_their_language_icon() {
        assert_eq!(icon_for("main.rs", false, false), RUST);
        assert_eq!(icon_for("script.py", false, false), PYTHON);
        assert_eq!(icon_for("app.tsx", false, false), TYPESCRIPT);
        assert_eq!(icon_for("README.md", false, false), MARKDOWN);
    }

    #[test]
    fn tcl_and_toml_fall_back_to_their_closest_honest_icon() {
        // Neither has a dedicated nerd-fonts icon -- verified against the
        // authoritative glyph table, not assumed.
        assert_eq!(icon_for("script.tcl", false, false), SHELL);
        assert_eq!(icon_for("Cargo.toml", false, false), TOML);
    }

    #[test]
    fn navigation_uses_a_document_icon_for_tcl_not_a_literal_dollar_sign() {
        assert_eq!(navigation_icon_for("script.tcl"), FILE_CODE);
        assert_eq!(navigation_icon_for("main.rs"), RUST);
    }

    #[test]
    fn unrecognized_extension_gets_the_generic_code_file_icon() {
        assert_eq!(icon_for("thing.xyz", false, false), FILE_CODE);
    }

    #[test]
    fn no_extension_gets_the_plain_generic_file_icon() {
        assert_eq!(icon_for("Makefile", false, false), FILE_GENERIC);
        assert_eq!(icon_for("LICENSE", false, false), FILE_GENERIC);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(icon_for("Main.RS", false, false), RUST);
    }
}
