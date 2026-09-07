use fenix_core::{Buffer, Cursor};
use fenix_snippets::{Catalog, Context, Edit, Session, Snippet, Template};
use std::collections::BTreeMap;

fn start(body: &str) -> (Buffer, Cursor, Session) {
    let mut buffer = Buffer::from_text("  trig suffix");
    let mut cursor = Cursor {
        char_idx: 6,
        sticky_col: 0,
    };
    let session = Session::expand(
        &mut buffer,
        &mut cursor,
        2,
        Template::parse(body).unwrap(),
        Context::default(),
    )
    .unwrap();
    (buffer, cursor, session)
}

#[test]
fn expansion_navigation_mirrors_and_exit_use_character_offsets() {
    let (mut buffer, mut cursor, mut session) = start("${2:second} ${1:é} $1 $0!");
    assert_eq!(buffer.text(), "  second é é ! suffix");
    assert_eq!(session.selection(), Some(9..10));
    session.edit(&mut buffer, &mut cursor, Edit::Insert("你好"));
    assert_eq!(buffer.text(), "  second 你好 你好 ! suffix");
    assert_eq!(cursor.char_idx, 11);
    assert!(session.advance(&mut cursor, false));
    assert_eq!(session.selection(), Some(2..8));
    assert!(session.advance(&mut cursor, true));
    assert_eq!(session.selection(), Some(9..11));
    assert!(session.advance(&mut cursor, false));
    assert!(!session.advance(&mut cursor, false));
    assert_eq!(cursor.char_idx, 15);
}

#[test]
fn transformations_update_live_and_center_after_removing_unicode_whitespace() {
    let (mut buffer, mut cursor, mut session) =
        start("${1: a b }\n[${1|remove_whitespace|upper|center:8}]");
    assert_eq!(buffer.text(), "   a b \n  [   AB   ] suffix");
    session.edit(&mut buffer, &mut cursor, Edit::Insert("é\u{2003} z"));
    assert_eq!(buffer.text(), "  é\u{2003} z\n  [   ÉZ   ] suffix");
}

#[test]
fn backspace_deletes_selected_default_then_unicode_scalars() {
    let (mut buffer, mut cursor, mut session) = start("${1:default}:$1");
    session.edit(&mut buffer, &mut cursor, Edit::Backspace);
    assert_eq!(buffer.text(), "  : suffix");
    session.edit(&mut buffer, &mut cursor, Edit::Insert("é猫"));
    session.edit(&mut buffer, &mut cursor, Edit::Backspace);
    assert_eq!(buffer.text(), "  é:é suffix");
    session.edit(&mut buffer, &mut cursor, Edit::Delete);
    assert_eq!(buffer.text(), "  é:é suffix");
}

#[test]
fn multiline_field_and_template_preserve_base_indentation() {
    let (mut buffer, mut cursor, mut session) = start("${1:x}\n  $1\n$0");
    session.edit(&mut buffer, &mut cursor, Edit::Insert("a\nb"));
    assert_eq!(buffer.text(), "  a\n  b\n    a\n  b\n   suffix");
    assert_eq!(cursor.char_idx, 7);
}

#[test]
fn undo_redo_and_external_edits_invalidate_ranges() {
    let (mut buffer, mut cursor, mut session) = start("${1:a} $1");
    session.edit(&mut buffer, &mut cursor, Edit::Insert("b"));
    assert!(session.valid(&buffer, &cursor));
    buffer.undo(&mut cursor);
    assert_eq!(buffer.text(), "  a a suffix");
    assert!(!session.valid(&buffer, &cursor));
    buffer.undo(&mut cursor);
    assert_eq!(buffer.text(), "  trig suffix");
    buffer.redo(&mut cursor);
    assert!(!session.valid(&buffer, &cursor));
}

#[test]
fn movement_invalidates_session_without_changing_text() {
    let (buffer, mut cursor, session) = start("${1:a}");
    cursor.char_idx += 1;
    assert!(!session.valid(&buffer, &cursor));
    assert_eq!(buffer.text(), "  a suffix");
}

#[test]
fn variables_are_snapshots_and_accept_transform_pipelines() {
    let template = Template::parse("${FILENAME} ${USER_NAME|trim|upper} ${DATE} ${TIME}").unwrap();
    let context = Context(BTreeMap::from([
        ("FILENAME".into(), "é.rs".into()),
        ("USER_NAME".into(), " Ada ".into()),
        ("DATE".into(), "2026-09-07".into()),
        ("TIME".into(), "12:34:56".into()),
    ]));
    assert_eq!(
        template.render(&BTreeMap::new(), &context, "").text,
        "é.rs ADA 2026-09-07 12:34:56"
    );
    let empty = Context::current(None);
    assert_eq!(empty.0["FILENAME"], "");
    assert_eq!(empty.0["DATE"].len(), 10);
    let path = Context::current(Some(std::path::Path::new("dir/example.rs")));
    assert_eq!(path.0["FILE_STEM"], "example");
    assert_eq!(path.0["DIRECTORY"], "dir");
}

#[test]
fn escapes_literal_dollars_braces_backslashes_and_default_pipes() {
    let template = Template::parse(r"\${not_a_variable} \\ ${1:a\}b|c} $1 $name").unwrap();
    assert_eq!(
        template
            .render(&BTreeMap::new(), &Context::default(), "")
            .text,
        r"${not_a_variable} \ a}b|c a}b|c $name"
    );
}

#[test]
fn malformed_templates_are_rejected_without_partial_expansion() {
    for source in [
        "${1",
        "${NOPE}",
        "${1:a}${1:b}",
        "${1:${2:x}}",
        "${1|upper}",
        "${0:x}",
        "$0$0",
        "${1:a}${1|exec}",
        "${1:a}${1|center:99999999}",
        "${DATE:x}",
        "$99999999999999999999",
    ] {
        assert!(Template::parse(source).is_err(), "{source}");
    }
}

#[test]
fn forward_mirrors_empty_fields_and_implicit_exit() {
    let template = Template::parse("$1 ${1:abc} $3").unwrap();
    let rendered = template.render(&BTreeMap::new(), &Context::default(), "");
    assert_eq!(rendered.text, "abc abc ");
    assert_eq!(rendered.fields[&1], 0..3);
    assert_eq!(rendered.fields[&3], 8..8);
    assert_eq!(rendered.exit, 8);
}

#[test]
fn no_fields_finishes_immediately_at_explicit_exit() {
    let mut buffer = Buffer::from_text("x");
    let mut cursor = Cursor {
        char_idx: 1,
        sticky_col: 0,
    };
    assert!(Session::expand(
        &mut buffer,
        &mut cursor,
        0,
        Template::parse("a$0b").unwrap(),
        Context::default()
    )
    .is_none());
    assert_eq!(buffer.text(), "ab");
    assert_eq!(cursor.char_idx, 1);
    buffer.undo(&mut cursor);
    assert_eq!(buffer.text(), "x");
}

#[test]
fn center_never_truncates_and_handles_each_line() {
    let template = Template::parse("${USER_NAME|center:4}").unwrap();
    let context = Context(BTreeMap::from([("USER_NAME".into(), "x\nlonger".into())]));
    assert_eq!(
        template.render(&BTreeMap::new(), &context, "").text,
        " x  \nlonger"
    );
}

#[test]
fn loader_keeps_good_files_reports_bad_files_and_reloads_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("override.snippet");
    std::fs::write(&path, "\u{feff}# key: header\r\n# --\r\ncustom $0").unwrap();
    std::fs::write(
        dir.path().join("broken.snippet"),
        "# key: nope\n# --\n${bad}",
    )
    .unwrap();
    std::fs::write(dir.path().join("ignored.txt"), "bad").unwrap();
    let catalog = Catalog::load(dir.path());
    assert_eq!(catalog.errors.len(), 1);
    assert_eq!(
        catalog
            .matching("header", "text")
            .unwrap()
            .template
            .render(&BTreeMap::new(), &Context::default(), "")
            .text,
        "custom "
    );
    std::fs::write(&path, "# key: header\n# --\nchanged").unwrap();
    assert_eq!(
        Catalog::load(dir.path())
            .matching("header", "text")
            .unwrap()
            .template
            .render(&BTreeMap::new(), &Context::default(), "")
            .text,
        "changed"
    );
    assert!(Catalog::load(&dir.path().join("missing")).errors.is_empty());
}

#[test]
fn matching_prefers_scope_and_longest_trigger_and_respects_boundaries() {
    let snippet = |source| Snippet::parse(source).unwrap();
    let catalog = Catalog {
        snippets: vec![
            snippet("# name: scoped\n# key: fn\n# scope: rust, tcl\n# --\nA"),
            snippet("# name: global\n# key: fn\n# --\nB"),
            snippet("# name: long\n# key: !fn\n# --\nC"),
        ],
        errors: vec![],
    };
    assert_eq!(catalog.matching(" fn", "rust").unwrap().name, "scoped");
    assert_eq!(catalog.matching("fn", "text").unwrap().name, "global");
    assert_eq!(catalog.matching("!fn", "text").unwrap().name, "long");
    assert!(catalog.matching("stuff_fn", "rust").is_none());
    assert!(catalog.matching("éfn", "rust").is_none());
    assert!(catalog.matching("fn ", "rust").is_none());
}

#[test]
fn metadata_validation_and_bundled_examples() {
    for source in [
        "# --\nx",
        "# key: two words\n# --\nx",
        "# key: x\nx",
        "# key: x\n# scope: ,\n# --\nx",
    ] {
        assert!(Snippet::parse(source).is_err());
    }
    let catalog = Catalog::bundled();
    assert_eq!(catalog.snippets.len(), 5);
    assert!(catalog
        .matching("header", "rust")
        .unwrap()
        .template
        .render(&BTreeMap::new(), &Context::default(), "")
        .text
        .starts_with("// "));
    assert!(catalog
        .matching("header", "text")
        .unwrap()
        .template
        .render(&BTreeMap::new(), &Context::default(), "")
        .text
        .starts_with("# "));
    for snippet in catalog.snippets {
        assert!(!snippet
            .template
            .render(&BTreeMap::new(), &Context::current(None), "")
            .text
            .is_empty());
    }
}
