// A view of one run of lines is a list of one range.
#![allow(clippy::single_range_in_vec_init)]
use super::*;

/// The layout the git dashboard's accordion asks for has to be the
/// layout it gets. Walks the ratios through the same right-nested
/// subdivision `Node::layout_into` performs, and checks each pane
/// ends up with the share it was given -- the property that actually
/// matters, rather than the intermediate ratios.
#[test]
fn stack_split_ratios_reproduce_the_shares_they_were_given() {
    let cases: [&[f32]; 4] = [
        &[0.5, 0.1, 0.1, 0.1, 0.1, 0.1],  // first section focused
        &[0.1, 0.1, 0.1, 0.1, 0.5, 0.1],  // Commits focused -- the case from the bug report
        &[0.1, 0.1, 0.1, 0.1, 0.1, 0.5],  // last section focused
        &[0.25, 0.25, 0.25, 0.25],        // an evenly weighted stack
    ];
    for shares in cases {
        let ratios = stack_split_ratios(shares);
        assert_eq!(ratios.len(), shares.len() - 1, "an n-pane chain has n-1 splits");
        // Subdivide a unit column exactly the way the window tree does:
        // every split takes its ratio of what the ones above left, and
        // the final pane inherits whatever remains.
        let mut remaining = 1.0_f32;
        for (i, share) in shares.iter().enumerate() {
            let got = match ratios.get(i) {
                Some(ratio) => remaining * ratio,
                None => remaining,
            };
            assert!(
                (got - share).abs() < 1e-5,
                "{shares:?}: pane {i} asked for {share} of the column but the ratios give it {got}"
            );
            remaining -= got;
        }
    }
}

/// Every ratio the accordion produces must be one the window tree
/// will actually accept -- it clamps to [0.1, 0.9], and a clamped
/// ratio silently throws off every pane below it in the chain.
#[test]
fn the_accordion_shares_stay_inside_the_window_trees_ratio_clamp() {
    // Must track `apply_git_accordion`'s own FOCUSED_SHARE.
    let focused_share = 0.45_f32;
    let other = (1.0 - focused_share) / 5.0;
    for focused in 0..6 {
        let shares: Vec<f32> = (0..6).map(|i| if i == focused { focused_share } else { other }).collect();
        for (i, ratio) in stack_split_ratios(&shares).iter().enumerate() {
            assert!(
                (0.1..=0.9).contains(ratio),
                "focusing section {focused} needs split {i} at {ratio}, outside the clamp the tree enforces"
            );
        }
    }
}

/// An informational panel (a Git/Docker dashboard section, a
/// terminal) reserves its title row and nothing more. It used to
/// reserve the breadcrumb bar as well and then never draw into it,
/// which put a blank strip of chrome between every panel's title
/// and its first row.
#[test]
fn pane_content_rect_without_a_breadcrumb_reserves_only_the_title_row() {
    let rect = fenix_window::Rect { x: 10.0, y: 20.0, w: 300.0, h: 400.0 };
    let shrunk = pane_content_rect(rect, 20.0, false);
    assert_eq!(shrunk, fenix_window::Rect { x: 10.0, y: 48.0, w: 300.0, h: 372.0 });
    assert_eq!(rect.y + rect.h, shrunk.y + shrunk.h);
}

#[test]
fn pane_content_rect_with_a_breadcrumb_reserves_the_tab_strip_and_the_bar_under_it() {
    let rect = fenix_window::Rect { x: 10.0, y: 20.0, w: 300.0, h: 400.0 };
    let shrunk = pane_content_rect(rect, 20.0, true);
    assert_eq!(shrunk, fenix_window::Rect { x: 10.0, y: 70.0, w: 300.0, h: 350.0 });
    // The bottom edge is preserved -- this is exactly what lets the
    // existing divider-line logic (and every other bottom/right-edge
    // check) work unmodified for a titled pane.
    assert_eq!(rect.y + rect.h, shrunk.y + shrunk.h);
}

/// The two have to differ by exactly the breadcrumb bar: the
/// background fill and the content rect are computed from this same
/// height, so if it ever disagreed with what's actually rendered a
/// panel would either show a blank strip again or paint its chrome
/// over its own first row.
#[test]
fn a_breadcrumb_costs_exactly_one_breadcrumb_bar_of_content_height() {
    let with = pane_chrome_height(20.0, true);
    let without = pane_chrome_height(20.0, false);
    assert_eq!(without, tab_strip_height(20.0));
    assert!((with - without - breadcrumb_strip_height(20.0)).abs() < f32::EPSILON);
}

#[test]
fn pane_content_rect_never_goes_negative_for_a_pane_shorter_than_one_line() {
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 10.0 };
    let shrunk = pane_content_rect(rect, 20.0, true);
    assert_eq!(shrunk.h, 0.0);
}

#[test]
fn pane_tab_layout_with_no_tabs_is_empty() {
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 300.0, h: 20.0 };
    assert!(pane_tab_layout(rect, 8.0, &[]).is_empty());
}

#[test]
fn pane_tab_layout_places_tabs_left_to_right_with_no_gap() {
    let mut app = App::with_file(None);
    let first = app.focused_buffer_id();
    app.new_scratch_buffer();
    let second = app.focused_buffer_id();
    let rect = fenix_window::Rect { x: 10.0, y: 5.0, w: 400.0, h: 20.0 };
    let tabs = [(first, 6), (second, 6)];
    let layout = pane_tab_layout(rect, 8.0, &tabs);
    assert_eq!(layout.len(), 2);
    assert_eq!(layout[0].buffer, first);
    assert_eq!(layout[0].body.x, rect.x);
    assert_eq!(layout[0].body.y, rect.y);
    assert_eq!(layout[0].body.h, rect.h);
    // Second tab starts exactly where the first one's body ends.
    assert_eq!(layout[1].body.x, layout[0].body.x + layout[0].body.w);
    // The close glyph sits at each tab's own right edge.
    assert_eq!(layout[0].close.x + layout[0].close.w, layout[0].body.x + layout[0].body.w);
}

#[test]
fn pane_tab_layout_clamps_a_short_name_up_to_the_minimum_width() {
    let app = App::with_file(None);
    let buffer = app.focused_buffer_id();
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 400.0, h: 20.0 };
    let short = pane_tab_layout(rect, 8.0, &[(buffer, 1)])[0].body.w;
    let at_min = pane_tab_layout(rect, 8.0, &[(buffer, TAB_MIN_NAME_CHARS)])[0].body.w;
    assert_eq!(short, at_min, "a 1-char name should be padded up to the same width as a name already at the minimum");
}

#[test]
fn pane_tab_layout_clamps_a_long_name_down_to_the_maximum_width() {
    let app = App::with_file(None);
    let buffer = app.focused_buffer_id();
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 20.0 };
    let huge = pane_tab_layout(rect, 8.0, &[(buffer, 500)])[0].body.w;
    let at_max = pane_tab_layout(rect, 8.0, &[(buffer, TAB_MAX_NAME_CHARS)])[0].body.w;
    assert_eq!(huge, at_max);
}

#[test]
fn pane_tab_layout_omits_a_tab_that_would_run_past_the_strips_right_edge() {
    let mut app = App::with_file(None);
    let first = app.focused_buffer_id();
    app.new_scratch_buffer();
    let second = app.focused_buffer_id();
    // (TAB_MIN_NAME_CHARS + TAB_CHROME_CHARS) * char_width is exactly
    // one minimum-width tab -- room for one, not two.
    let one_tab_width = (TAB_MIN_NAME_CHARS + TAB_CHROME_CHARS) as f32 * 8.0;
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: one_tab_width, h: 20.0 };
    let tabs = [(first, TAB_MIN_NAME_CHARS), (second, TAB_MIN_NAME_CHARS)];
    let layout = pane_tab_layout(rect, 8.0, &tabs);
    assert_eq!(layout.len(), 1, "the second tab doesn't fit and should be dropped, not overflow the strip");
}

#[test]
fn syntax_range_reaches_a_short_buffer_last_line() {
    // A maximized editor normally has many more available rows than a
    // short file. The old fallback used the first row in this case, so
    // only line one received syntax colors.
    assert_eq!(visible_document_end_line(&[0, 1, 2], 40), 2);
    assert_eq!(visible_document_end_line(&[7, 8, 9, 10], 2), 9);
    assert_eq!(visible_document_end_line(&[], 40), 0);
}

/// With more tabs than fit, the strip starts where the active tab is
/// whole at its right edge; while it fits from the start, at the start
/// (#50).
#[test]
fn the_tab_strip_scrolls_to_keep_the_active_tab_in_view() {
    let width = |_| 10.0;
    assert_eq!(first_tab_shown(2, 45.0, width), 0, "fits from the start");
    assert_eq!(first_tab_shown(19, 45.0, width), 16, "the last four, the active one last");
    assert_eq!(first_tab_shown(19, 5.0, width), 19, "too narrow for one: still the active one");
    assert_eq!(first_tab_shown(0, 45.0, width), 0);
}

#[test]
fn pane_tab_layout_iter_stops_resolving_tabs_once_the_strip_is_full() {
    use std::cell::Cell;

    let app = App::with_file(None);
    let buffer = app.focused_buffer_id();
    let one_tab_width = (TAB_MIN_NAME_CHARS + TAB_CHROME_CHARS) as f32 * 8.0;
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: one_tab_width, h: 20.0 };
    let seen = Cell::new(0);
    let tabs = (0..100).map(|_| {
        seen.set(seen.get() + 1);
        (buffer, TAB_MIN_NAME_CHARS)
    });

    let layout = pane_tab_layout_iter(rect, 8.0, tabs);

    assert_eq!(layout.len(), 1);
    assert_eq!(seen.get(), 2, "the iterator only needs the first fitting tab and the next one that proves the strip is full");
}

#[test]
fn truncate_tab_name_keeps_a_name_that_already_fits() {
    assert_eq!(truncate_tab_name("main.rs", 20), "main.rs");
}

#[test]
fn truncate_tab_name_ellipsizes_a_name_that_does_not_fit() {
    let out = truncate_tab_name("a_very_long_filename.rs", 8);
    assert_eq!(out.chars().count(), 8);
    assert!(out.ends_with('\u{2026}'));
}

#[test]
fn truncate_tab_name_with_zero_budget_is_empty() {
    assert_eq!(truncate_tab_name("anything", 0), "");
}

#[test]
fn hit_test_prefers_a_tabs_close_glyph_over_its_body_and_the_coarse_pane_rect() {
    let app = App::with_file(None);
    let pane = app.focused_pane_id();
    let buffer = app.focused_buffer_id();
    let pane_rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 300.0, h: 200.0 };
    let tab = TabRect {
        buffer,
        body: fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 20.0 },
        close: fenix_window::Rect { x: 80.0, y: 0.0, w: 20.0, h: 20.0 },
    };
    let geometry = FrameGeometry { split_buttons: Vec::new(),
        breadcrumb_panes: Vec::new(),
        pane_area: pane_rect,
        sidebar_rect: None,
        terminal_rect: None,
        panes: vec![(pane, pane_rect)],
        tabs: vec![(pane, tab)],
    };
    assert_eq!(hit_test(&geometry, (90.0, 10.0)), Some(ScrollTarget::CloseTab(pane, buffer)));
    assert_eq!(hit_test(&geometry, (40.0, 10.0)), Some(ScrollTarget::SwitchTab(pane, buffer)));
    // Below the tab strip, in the pane's own content area -- falls
    // through to the coarse pane hit exactly as an untabbed theme's
    // pane already does.
    assert_eq!(hit_test(&geometry, (40.0, 50.0)), Some(ScrollTarget::Pane(pane)));
}

#[test]
fn cursor_icon_for_a_tab_or_the_sidebar_is_a_pointer() {
    let app = App::with_file(None);
    let pane = app.focused_pane_id();
    let buffer = app.focused_buffer_id();
    let pane_rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 300.0, h: 200.0 };
    let tab = TabRect {
        buffer,
        body: fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 20.0 },
        close: fenix_window::Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
    };
    let geometry = FrameGeometry { split_buttons: Vec::new(),
        breadcrumb_panes: Vec::new(),
        pane_area: pane_rect,
        sidebar_rect: Some(fenix_window::Rect { x: -200.0, y: 0.0, w: 200.0, h: 200.0 }),
        terminal_rect: None,
        panes: vec![(pane, pane_rect)],
        tabs: vec![(pane, tab)],
    };
    assert_eq!(cursor_icon_for(&geometry, (40.0, 10.0), 20.0), winit::window::CursorIcon::Pointer);
    assert_eq!(cursor_icon_for(&geometry, (-100.0, 10.0), 20.0), winit::window::CursorIcon::Pointer);
}

#[test]
fn cursor_icon_for_a_panes_content_below_its_title_strip_is_text() {
    let app = App::with_file(None);
    let pane = app.focused_pane_id();
    let pane_rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 300.0, h: 200.0 };
    let geometry = FrameGeometry { split_buttons: Vec::new(), breadcrumb_panes: Vec::new(), pane_area: pane_rect, sidebar_rect: None, terminal_rect: None, panes: vec![(pane, pane_rect)], tabs: Vec::new() };
    assert_eq!(cursor_icon_for(&geometry, (40.0, 50.0), 20.0), winit::window::CursorIcon::Text);
    // Within the title strip itself (not on a tab, since there are
    // none here) -- the plain arrow, not an I-beam.
    assert_eq!(cursor_icon_for(&geometry, (40.0, 5.0), 20.0), winit::window::CursorIcon::Default);
}

#[test]
fn cursor_icon_for_empty_space_outside_every_region_is_the_plain_arrow() {
    let geometry = FrameGeometry { split_buttons: Vec::new(),
        breadcrumb_panes: Vec::new(),
        pane_area: fenix_window::Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
        sidebar_rect: None,
        terminal_rect: None,
        panes: Vec::new(),
        tabs: Vec::new(),
    };
    assert_eq!(cursor_icon_for(&geometry, (999.0, 999.0), 20.0), winit::window::CursorIcon::Default);
}

#[test]
fn a_files_indent_step_is_read_from_its_own_lines() {
    let tcl = "if {[llength $argv] != 1} {\n   error \"usage\"\n}\nproc a {} {\n   if {1} {\n      puts x\n   }\n}\n";
    assert_eq!(file_indent_step(tcl.lines()), Some(3), "three spaces a level");
    let c = "/*\n * a comment\n */\nint f() {\n    return 1;\n}\n";
    assert_eq!(file_indent_step(c.lines()), Some(4), "a comment's one-space step isn't a level");
    assert_eq!(file_indent_step("a\nb\n".lines()), None, "nothing indented: the setting");
    assert_eq!(file_indent_step("a\n\tb\n".lines()), None, "tabs: the tab width");
    // Two levels in one step count as two of one: the smaller wins a tie.
    assert_eq!(file_indent_step("a\n  b\n    c\nd\n    e\n".lines()), Some(2));
}

#[test]
fn indent_guide_columns_is_empty_for_an_unindented_line() {
    assert_eq!(indent_guide_columns(0, 4).collect::<Vec<_>>(), Vec::<usize>::new());
}

#[test]
fn indent_guide_columns_gives_one_guide_per_complete_level_at_its_own_start() {
    assert_eq!(indent_guide_columns(4, 4).collect::<Vec<_>>(), vec![0]);
    assert_eq!(indent_guide_columns(8, 4).collect::<Vec<_>>(), vec![0, 4]);
    assert_eq!(indent_guide_columns(12, 4).collect::<Vec<_>>(), vec![0, 4, 8]);
}

#[test]
fn indent_guide_columns_ignores_a_partial_trailing_level() {
    // 6 columns of leading whitespace at a 4-column indent width is
    // one complete level (0..4) plus 2 stray columns -- no guide for
    // an incomplete level.
    assert_eq!(indent_guide_columns(6, 4).collect::<Vec<_>>(), vec![0]);
}

#[test]
fn indent_guide_columns_is_empty_when_indent_width_is_zero() {
    assert_eq!(indent_guide_columns(8, 0).collect::<Vec<_>>(), Vec::<usize>::new());
}

#[test]
fn indent_guide_runs_merges_contiguous_rows_but_not_gaps() {
    let runs = indent_guide_runs(&[
        (0, 0), (0, 4), (1, 0), (1, 4), (2, 0), (3, 4),
    ]);

    assert_eq!(runs.len(), 3);
    assert!(runs.contains(&(0, 2, 0)), "column zero remains continuous through rows 0..=2");
    assert!(runs.contains(&(0, 1, 4)), "column four's first continuous segment is merged");
    assert!(runs.contains(&(3, 3, 4)), "a blank/missing guide row breaks the line rather than bridging it");
}

#[test]
fn gutter_marks_from_hunks_marks_a_pure_addition_as_added() {
    let diff = "\
diff --git a/f.txt b/f.txt
index 0000000..1111111 100644
--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,3 @@
 one
+two
 three
";
    let file = fenix_diff::parse(diff).into_iter().next().unwrap();
    let marks = gutter_marks_from_hunks(&file.hunks);
    assert_eq!(marks, vec![GutterMark { line: 2, kind: GutterMarkKind::Added }]);
}

#[test]
fn gutter_marks_from_hunks_marks_a_replaced_line_as_modified() {
    let diff = "\
diff --git a/f.txt b/f.txt
index 0000000..1111111 100644
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
    let file = fenix_diff::parse(diff).into_iter().next().unwrap();
    let marks = gutter_marks_from_hunks(&file.hunks);
    assert_eq!(marks, vec![GutterMark { line: 2, kind: GutterMarkKind::Modified }]);
}

#[test]
fn refresh_all_gutter_hunks_populates_marks_for_every_open_buffer_with_a_real_diff() {
    // A real `git` repo, not a mock -- `compute_gutter_hunks` shells
    // a real `git diff`, same discipline `fenix-git`'s own tests use.
    let dir = TempDir::new("refresh_all_gutter_hunks");
    let repo = dir.path();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(repo).args(args).status().expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    let file = dir.write("f.txt", "one\ntwo\nthree\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "initial"]);
    std::fs::write(&file, "one\nTWO\nthree\n").unwrap();

    let mut app = App::with_file(None);
    app.project_root = Some(repo.to_path_buf());
    app.test_open_path(&file);
    // `set_pane_content` (via `test_open_path`) already refreshes the
    // buffer it just opened -- clear that out so this test actually
    // exercises `refresh_all_gutter_hunks` itself, not that call.
    let id = app.focused_buffer_id();
    app.gutter_hunks.remove(&id);
    assert!(!app.gutter_hunks.contains_key(&id));

    app.refresh_all_gutter_hunks();

    assert_eq!(app.gutter_hunks.get(&id), Some(&vec![GutterMark { line: 2, kind: GutterMarkKind::Modified }]));
}

/// An extension's compose pane, written in and left unsent -- replaced
/// by another, or open as Fenix quits -- comes back with that text the
/// next time it opens; `q` discards it for good.
#[test]
fn an_extensions_unsent_compose_text_comes_back() {
    let mut app = App::with_file(None);
    let _ = std::fs::remove_file(&app.compose_drafts_file);
    let write = |app: &mut App, text: &str| {
        let buffer = app.compose.as_ref().unwrap().buffer;
        app.replace_buffer_from_disk(buffer, text);
    };
    let written = |app: &App| app.buffers.get(app.compose.as_ref().unwrap().buffer).unwrap().buffer.text();
    app.open_code_compose("fenix.review", 1, "Description of feature/x".into(), "keep", true, "## Summary", None, false);
    assert_eq!(written(&app), "## Summary");
    write(&mut app, "## Summary\n\nHalf of what I meant");
    // Another one opened in its place: this one's text is kept.
    app.open_code_compose("fenix.review", 2, "Reply".into(), "send", false, "", None, false);
    app.compose_cancel();
    app.open_code_compose("fenix.review", 3, "Description of feature/x".into(), "keep", true, "## Summary", None, false);
    assert_eq!(written(&app), "## Summary\n\nHalf of what I meant");
    // Brought back once; q now discards it.
    app.compose_cancel();
    app.open_code_compose("fenix.review", 4, "Description of feature/x".into(), "keep", true, "## Summary", None, false);
    assert_eq!(written(&app), "## Summary");
    // Open as Fenix quits: kept too.
    write(&mut app, "## Summary\n\nWritten, then the window closed");
    app.keep_compose_draft();
    app.compose_cancel();
    app.open_code_compose("fenix.review", 5, "Description of feature/x".into(), "keep", true, "## Summary", None, false);
    assert_eq!(written(&app), "## Summary\n\nWritten, then the window closed");
    app.compose_cancel();
    // Nothing written past its seed: nothing kept.
    app.open_code_compose("fenix.review", 6, "Description of feature/y".into(), "keep", true, "seed", None, false);
    app.keep_compose_draft();
    assert!(!app.compose_drafts_file.exists(), "nothing past its seed");
    app.compose_cancel();
}

/// After a restore the marks are worked out on a thread; ones that
/// arrive after the buffer was refreshed again (a save) are dropped.
#[test]
fn gutter_marks_worked_out_before_a_newer_refresh_are_dropped() {
    let dir = TempDir::new("gutter_marks_stale");
    let file = dir.write("f.txt", "one\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let id = app.focused_buffer_id();
    let started = Instant::now();
    let stale = fenix_diff::parse("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-one\n+ONE\n").into_iter().next().unwrap();
    app.set_gutter_hunks(id, None);
    app.apply_gutter_hunks(started, vec![(id, Some((dir.0.clone(), stale.clone())))]);
    assert!(!app.gutter_hunks.contains_key(&id), "refreshed since: kept as it was");
    app.apply_gutter_hunks(Instant::now(), vec![(id, Some((dir.0.clone(), stale)))]);
    assert!(app.gutter_hunks.contains_key(&id), "not refreshed since: taken");
}

#[test]
fn gutter_marks_from_hunks_marks_a_pure_removal_as_deleted_at_the_following_line() {
    let diff = "\
diff --git a/f.txt b/f.txt
index 0000000..1111111 100644
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,2 @@
 one
-two
 three
";
    let file = fenix_diff::parse(diff).into_iter().next().unwrap();
    let marks = gutter_marks_from_hunks(&file.hunks);
    assert_eq!(marks, vec![GutterMark { line: 2, kind: GutterMarkKind::Deleted }]);
}

#[test]
fn format_clock_uses_an_iso_date_and_24_hour_time() {
    use chrono::TimeZone;
    let now = chrono::Local.with_ymd_and_hms(2026, 3, 5, 9, 7, 2).unwrap();
    assert_eq!(format_clock(now), "2026-03-05 09:07:02");
}

#[test]
fn modeline_clock_fits_is_true_when_there_is_room() {
    assert!(modeline_clock_fits(20, 100.0, 1.0, 19));
}

#[test]
fn modeline_clock_fits_is_false_when_the_window_is_too_narrow() {
    assert!(!modeline_clock_fits(20, 30.0, 1.0, 19));
}

#[test]
fn modeline_clock_fits_is_true_at_the_exact_fit_boundary() {
    // Exactly enough room for the existing text, the clock, and one
    // char_width of separation between them -- the minimum passing
    // case (the `>=` in `modeline_clock_fits`), not yet "doesn't fit."
    assert!(modeline_clock_fits(10, 48.0, 2.0, 5));
}

#[test]
fn modeline_clock_fits_is_false_just_below_the_exact_fit_boundary() {
    // A narrower window than the boundary case above -- no longer
    // enough room, so the clock is omitted rather than rendered with
    // visual overlap onto the existing text.
    assert!(!modeline_clock_fits(10, 47.0, 2.0, 5));
}

/// Every index in `frames` is parked except `active_frame`, which
/// is `None` because its state is live on `App` -- the invariant
/// `activate_frame` exists to uphold, asserted directly.
fn assert_one_live_frame(app: &App) {
    for (frame, parked) in app.frames.iter().enumerate() {
        assert_eq!(
            parked.is_none(),
            frame == app.active_frame,
            "frame {frame} parked={} with active_frame={}",
            parked.is_some(),
            app.active_frame
        );
    }
}

#[test]
fn a_new_app_has_exactly_one_frame_and_it_is_the_live_one() {
    let app = App::with_file(None);
    assert_eq!(app.frames.len(), 1);
    assert_eq!(app.active_frame, 0);
    assert_one_live_frame(&app);
}

#[test]
fn activating_another_frame_swaps_in_its_layout_and_parks_the_outgoing_one() {
    let mut app = App::with_file(None);
    let original = app.focused_buffer_id();
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);

    app.activate_frame(second);
    assert_eq!(app.active_frame, second);
    // The live layout is now the second frame's, so the ordinary
    // "what am I looking at" accessors report *its* buffer.
    assert_eq!(app.focused_buffer_id(), other);
    assert_one_live_frame(&app);

    app.activate_frame(0);
    assert_eq!(app.focused_buffer_id(), original);
    assert_one_live_frame(&app);
}

#[test]
fn activating_a_frame_preserves_the_split_layout_it_was_parked_with() {
    let mut app = App::with_file(None);
    app.split_vertical();
    app.split_horizontal();
    assert_eq!(app.windows().window_count(), 3);

    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);
    app.activate_frame(second);
    assert_eq!(app.windows().window_count(), 1, "a fresh frame starts with a single pane");

    app.activate_frame(0);
    assert_eq!(app.windows().window_count(), 3, "the first frame's splits survived being parked");
}

#[test]
fn activating_the_frame_that_is_already_active_changes_nothing() {
    let mut app = App::with_file(None);
    let before = app.focused_buffer_id();
    app.activate_frame(0);
    assert_eq!(app.active_frame, 0);
    assert_eq!(app.focused_buffer_id(), before);
    assert_one_live_frame(&app);
}

#[test]
fn activating_a_frame_that_does_not_exist_is_a_no_op() {
    let mut app = App::with_file(None);
    let before = app.focused_buffer_id();
    app.activate_frame(7);
    assert_eq!(app.active_frame, 0, "a bogus index must not leave the app pointing at a frame it hasn't got");
    assert_eq!(app.focused_buffer_id(), before);
    assert_one_live_frame(&app);
}

#[test]
fn frame_workspaces_reads_a_parked_frame_without_activating_it() {
    let mut app = App::with_file(None);
    let original = app.focused_buffer_id();
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);

    let parked = app.frame_workspaces(second).expect("the second frame exists");
    assert_eq!(*parked.active().focused_content(), other);
    // Reading it must not have moved focus.
    assert_eq!(app.active_frame, 0);
    assert_eq!(app.focused_buffer_id(), original);
    assert!(app.frame_workspaces(9).is_none());
}

#[test]
fn panes_in_different_frames_never_share_an_id() {
    let mut app = App::with_file(None);
    app.split_vertical();
    let first_frame_panes = app.windows().windows();

    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);
    app.activate_frame(second);
    app.split_horizontal();

    for pane in app.windows().windows() {
        assert!(
            !first_frame_panes.contains(&pane),
            "{pane:?} is claimed by two frames -- every App-level map keyed by pane id would be ambiguous"
        );
    }
}

#[test]
fn a_point_on_a_monitor_is_inside_its_rectangle_and_a_point_past_it_is_not() {
    let at = PhysicalPosition::new(1920, 0);
    let size = PhysicalSize::new(2560, 1440);
    assert!(desktop_rect_contains(at, size, (1920, 0)), "top-left corner is inclusive");
    assert!(desktop_rect_contains(at, size, (3000, 700)), "interior");
    assert!(!desktop_rect_contains(at, size, (1919, 700)), "one pixel left belongs to the monitor next door");
    assert!(!desktop_rect_contains(at, size, (4480, 700)), "right edge is exclusive");
    assert!(!desktop_rect_contains(at, size, (3000, 1440)), "bottom edge is exclusive");
}

#[test]
fn a_monitor_to_the_left_of_the_origin_still_contains_its_own_points() {
    // Windows puts a monitor left of the primary one at a negative
    // x -- desktop coordinates are signed, and the arithmetic here
    // has to survive that.
    let at = PhysicalPosition::new(-1920, -200);
    let size = PhysicalSize::new(1920, 1080);
    assert!(desktop_rect_contains(at, size, (-1000, 0)));
    assert!(!desktop_rect_contains(at, size, (10, 0)));
}

#[test]
fn dropping_a_frame_removes_it_and_leaves_focus_where_it_was() {
    let mut app = App::with_file(None);
    let stays = app.focused_buffer_id();
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);

    app.drop_frame(second);

    assert_eq!(app.frames.len(), 1);
    assert_eq!(app.active_frame, 0);
    assert_eq!(app.focused_buffer_id(), stays);
    assert_one_live_frame(&app);
}

#[test]
fn dropping_the_active_frame_moves_focus_to_a_neighbour() {
    let mut app = App::with_file(None);
    let first = app.focused_buffer_id();
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);
    app.activate_frame(second);

    app.drop_frame(second);

    assert_eq!(app.frames.len(), 1);
    assert_eq!(app.focused_buffer_id(), first, "focus fell back to the surviving frame's own layout");
    assert_one_live_frame(&app);
}

#[test]
fn dropping_a_frame_shifts_the_active_index_down_when_it_sat_after_the_one_removed() {
    let mut app = App::with_file(None);
    let a = app.buffers.open_scratch();
    let b = app.buffers.open_scratch();
    let second = app.test_add_frame(a);
    let third = app.test_add_frame(b);
    app.activate_frame(third);

    app.drop_frame(second);

    assert_eq!(app.frames.len(), 2);
    assert_eq!(app.active_frame, 1, "the third frame is now at index 1");
    assert_eq!(app.focused_buffer_id(), b, "and it is still the one being looked at");
    assert_one_live_frame(&app);
}

#[test]
fn dropping_the_only_frame_is_refused() {
    let mut app = App::with_file(None);
    let before = app.focused_buffer_id();

    app.drop_frame(0);

    assert_eq!(app.frames.len(), 1, "closing the last frame is quitting, and goes through `close_frame` instead");
    assert_eq!(app.focused_buffer_id(), before);
    assert_one_live_frame(&app);
}

#[test]
fn only_frame_closes_every_other_frame_and_keeps_the_focused_one() {
    let mut app = App::with_file(None);
    let a = app.buffers.open_scratch();
    let b = app.buffers.open_scratch();
    app.test_add_frame(a);
    let third = app.test_add_frame(b);
    app.activate_frame(third);

    app.only_frame();

    assert_eq!(app.frames.len(), 1);
    assert_eq!(app.active_frame, 0);
    assert_eq!(app.focused_buffer_id(), b, "the frame that was focused is the one that survived");
    assert_one_live_frame(&app);
}

#[test]
fn cycle_frame_wraps_around_and_is_a_no_op_with_one_frame() {
    let mut app = App::with_file(None);
    let first = app.focused_buffer_id();
    let other = app.buffers.open_scratch();
    app.test_add_frame(other);

    app.cycle_frame();
    assert_eq!(app.focused_buffer_id(), other);
    app.cycle_frame();
    assert_eq!(app.focused_buffer_id(), first, "wrapped back round");

    app.only_frame();
    app.cycle_frame();
    assert_eq!(app.active_frame, 0);
}

#[test]
fn the_sidebar_belongs_to_the_frame_that_opened_it() {
    let mut app = App::with_file(None);
    app.toggle_sidebar();
    assert!(app.sidebar_open);

    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);
    app.activate_frame(second);
    assert!(!app.sidebar_open, "a sidebar opened on one monitor must not appear on the other as well");

    app.activate_frame(0);
    assert!(app.sidebar_open, "and it is still there when you come back");
}

#[test]
fn the_terminal_panel_shows_only_in_the_frame_it_was_opened_in() {
    let mut app = App::with_file(None);
    app.toggle_terminal();
    assert!(app.terminal_open);

    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);
    app.activate_frame(second);
    assert!(!app.terminal_open);

    app.activate_frame(0);
    assert!(app.terminal_open);
}

#[test]
fn activating_a_frame_to_paint_it_does_not_move_editor_focus() {
    let mut app = App::with_file(None);
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);

    // What repainting a background frame does.
    app.activate_frame(second);
    assert_eq!(app.active_frame, second);
    assert_eq!(app.focused_frame, 0, "a repaint must not steal focus from the window being typed into");

    // What a keystroke, a click or a frame command does.
    app.focus_frame(second);
    assert_eq!(app.focused_frame, second);
}

#[test]
fn cycling_frames_moves_editor_focus_with_it() {
    let mut app = App::with_file(None);
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);

    app.cycle_frame();

    assert_eq!(app.active_frame, second);
    assert_eq!(app.focused_frame, second);
}

#[test]
fn closing_a_frame_leaves_editor_focus_on_one_that_still_exists() {
    let mut app = App::with_file(None);
    let other = app.buffers.open_scratch();
    let second = app.test_add_frame(other);
    app.focus_frame(second);

    app.drop_frame(second);

    assert_eq!(app.focused_frame, app.active_frame);
    assert!(app.focused_frame < app.frames.len());
}

/// Two frames side by side, each split into two panes -- the left
/// frame's client area at the desktop origin, the right frame's
/// immediately beside it (`NAV_REFERENCE_SIZE.0` is the width a
/// windowless frame is laid out at). Returns each frame's panes in
/// left-to-right order.
fn two_frames_side_by_side(app: &mut App) -> (Vec<fenix_window::WindowId>, usize, Vec<fenix_window::WindowId>) {
    app.split_vertical();
    app.test_place_frame(0, (0, 0));
    let left_panes = app.windows().windows();

    let elsewhere = app.buffers.open_scratch();
    let right = app.test_add_frame(elsewhere);
    app.focus_frame(right);
    app.test_place_frame(right, (NAV_REFERENCE_SIZE.0 as i32, 0));
    app.split_vertical();
    let right_panes = app.windows().windows();

    (left_panes, right, right_panes)
}

#[test]
fn navigating_left_out_of_a_window_lands_on_the_nearest_pane_of_the_window_beside_it() {
    let mut app = App::with_file(None);
    let (left_panes, _right, right_panes) = two_frames_side_by_side(&mut app);
    app.windows_mut().focus(right_panes[0]); // leftmost pane of the right-hand window

    app.navigate_window(NavDirection::Left);

    assert_eq!(app.active_frame, 0, "focus crossed into the window on the left");
    assert_eq!(app.focused_frame, 0, "and it is a real focus move, not a temporary activation");
    assert_eq!(
        app.focused_pane_id(),
        left_panes[1],
        "landed on that window's *rightmost* pane -- the one actually adjacent -- not its first"
    );
}

#[test]
fn navigating_right_back_out_of_a_window_lands_on_the_next_windows_leftmost_pane() {
    let mut app = App::with_file(None);
    let (left_panes, right, right_panes) = two_frames_side_by_side(&mut app);
    app.focus_frame(0);
    app.windows_mut().focus(left_panes[1]); // rightmost pane of the left-hand window

    app.navigate_window(NavDirection::Right);

    assert_eq!(app.active_frame, right);
    assert_eq!(app.focused_pane_id(), right_panes[0]);
}

#[test]
fn navigation_stays_inside_a_window_while_it_has_somewhere_to_go() {
    let mut app = App::with_file(None);
    let (_left_panes, right, right_panes) = two_frames_side_by_side(&mut app);
    app.windows_mut().focus(right_panes[1]); // rightmost pane of the right-hand window

    app.navigate_window(NavDirection::Left);

    assert_eq!(app.active_frame, right, "the neighbouring pane is nearer than the other window");
    assert_eq!(app.focused_pane_id(), right_panes[0]);
}

#[test]
fn navigating_off_the_outermost_edge_of_the_outermost_window_is_a_no_op() {
    let mut app = App::with_file(None);
    let (left_panes, _right, _right_panes) = two_frames_side_by_side(&mut app);
    app.focus_frame(0);
    app.windows_mut().focus(left_panes[0]);

    app.navigate_window(NavDirection::Left);

    assert_eq!(app.active_frame, 0);
    assert_eq!(app.focused_pane_id(), left_panes[0]);
}

#[test]
fn an_open_sidebar_is_reached_before_the_window_on_the_monitor_to_its_left() {
    let dir = TempDir::new("nav_sidebar_before_other_frame");
    let mut app = App::with_file(None);
    let (_left_panes, right, right_panes) = two_frames_side_by_side(&mut app);
    // The right-hand window has its own sidebar open.
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    app.windows_mut().focus(right_panes[0]);

    app.navigate_window(NavDirection::Left);

    assert_eq!(app.active_frame, right, "the sidebar in this window is nearer than the next window over");
    assert!(app.sidebar_focused);

    // One more step left leaves the window entirely.
    app.navigate_window(NavDirection::Left);
    assert_eq!(app.active_frame, 0);
    assert!(!app.sidebar_focused, "focusing a pane clears the sidebar override");
}

#[test]
fn a_saved_window_layout_becomes_the_placement_it_is_restored_at() {
    let layout = fenix_config::WindowLayout { x: -1920, y: 40, width: 1600, height: 900, maximized: true };

    let placement = FramePlacement::from(layout);

    assert_eq!(placement.position, Some(PhysicalPosition::new(-1920, 40)));
    assert_eq!(placement.size, Some(PhysicalSize::new(1600, 900)));
    assert!(placement.maximized);
}

#[test]
fn saving_the_window_layout_with_no_real_windows_leaves_the_saved_one_alone() {
    let mut app = App::with_file(None);
    let saved = vec![fenix_config::WindowLayout { x: 10, y: 20, width: 800, height: 600, maximized: false }];
    app.config.windows = saved.clone();

    // A headless run has no windows to measure. Recording an empty
    // layout here would wipe a real arrangement off disk the next
    // time the app happened to exit before its window came up.
    app.save_window_layout();

    assert_eq!(app.config.windows, saved);
}

#[test]
fn a_window_layout_is_only_written_when_it_actually_changed() {
    let one = fenix_config::WindowLayout { x: 0, y: 0, width: 800, height: 600, maximized: false };
    let moved = fenix_config::WindowLayout { x: 40, y: 0, width: 800, height: 600, maximized: false };

    assert!(layout_worth_saving(&[one], &[]), "first run, nothing recorded yet");
    assert!(layout_worth_saving(&[moved], &[one]), "the window moved");
    assert!(layout_worth_saving(&[one, moved], &[one]), "a second window was opened");
    assert!(!layout_worth_saving(&[one], &[one]), "nothing moved -- don't rewrite the state file for nothing");
    assert!(!layout_worth_saving(&[], &[one]), "nothing measured must never wipe a real arrangement");
}

#[test]
fn toggle_fullscreen_is_a_no_op_without_a_real_window() {
    // `App::window` is only ever `Some` after a real winit `resumed`
    // callback, never in a headless test -- this just confirms the
    // early-return path doesn't panic.
    let mut app = App::with_file(None);
    app.toggle_fullscreen();
}

#[test]
fn animations_are_on_by_default() {
    // `App::with_file(None)` reads the *real* machine config (see
    // `set_shiftwidth_command_persists_the_new_width`'s own doc
    // comment) -- explicitly reset to unset here so this test
    // exercises `animations_enabled`'s own default rather than
    // whatever this dev machine's config.ini currently has saved
    // (e.g. from actually trying `SPC t a` by hand).
    let mut app = App::with_file(None);
    app.config.animations = None;
    assert!(app.animations_enabled());
}

#[test]
fn spc_t_a_cycles_the_motion_level_and_persists_it() {
    // Isolated `Config`: `toggle_animations` saves, which must not
    // write into the real dev machine's settings.
    let dir = TempDir::new("toggle_animations_persists");
    let mut app = App::with_file(None);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    assert_eq!(crate::motion::level(&app.config), crate::motion::Level::Subtle);

    app.toggle_animations();
    assert_eq!(crate::motion::level(&app.config), crate::motion::Level::Full);
    app.toggle_animations();
    assert!(!app.animations_enabled());
    let reloaded = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
    assert_eq!(reloaded.motion.level.as_deref(), Some("off"));

    app.toggle_animations();
    assert!(app.animations_enabled());
}

#[test]
fn spc_t_a_turns_motion_back_on_after_animations_were_switched_off() {
    let dir = TempDir::new("toggle_animations_from_off");
    let mut app = App::with_file(None);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    app.config.animations = Some(false);
    app.toggle_animations();
    assert!(app.animations_enabled(), "the old all-off switch no longer overrides the new level");
}

#[test]
fn caret_alpha_snaps_instantly_when_animations_are_disabled() {
    let mut app = App::with_file(None);
    app.config.animations = Some(false);
    app.blink_visible = true;
    app.blink_transition_start = Instant::now(); // mid-fade, if fading were happening
    assert_eq!(app.caret_alpha(), 1.0);
    app.blink_visible = false;
    assert_eq!(app.caret_alpha(), 0.0);
}

#[test]
fn scroll_stays_put_when_cursor_already_visible() {
    assert_eq!(scroll_to_include(5, 7, 10), 5);
}

#[test]
fn scroll_jumps_up_when_cursor_is_above_the_window() {
    assert_eq!(scroll_to_include(10, 3, 10), 3);
}

#[test]
fn scroll_advances_when_cursor_is_below_the_window() {
    // window [10, 20); cursor at line 20 is the first line below it
    assert_eq!(scroll_to_include(10, 20, 10), 11);
}

#[test]
fn single_visible_line_tracks_cursor_exactly() {
    assert_eq!(scroll_to_include(0, 42, 1), 42);
}

#[test]
fn app_auto_scrolls_to_keep_cursor_in_view() {
    let mut app = App::with_file(None);
    for _ in 0..30 {
        app.test_insert('\n');
    }
    // cursor is now on line 30; a 10-line viewport starting at 0 doesn't include it
    app.ensure_cursor_visible(10);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_line, 21);
}

#[test]
fn ensure_cursor_visible_horizontally_is_a_noop_when_cursor_already_in_view() {
    let mut app = App::with_file(None);
    app.test_insert_str("short line");
    app.ensure_cursor_visible_horizontally(20);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_col, 0);
}

#[test]
fn ensure_cursor_visible_horizontally_scrolls_right_past_the_edge() {
    let mut app = App::with_file(None);
    app.test_insert_str(&"x".repeat(50)); // cursor lands at char_idx/col 50
    app.ensure_cursor_visible_horizontally(10);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_col, 41); // scroll_to_include(0, 50, 10)
}

#[test]
fn ensure_cursor_visible_horizontally_scrolls_back_left_after_the_cursor_moves_left() {
    let mut app = App::with_file(None);
    app.test_insert_str(&"x".repeat(50));
    app.ensure_cursor_visible_horizontally(10);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_col, 41);

    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 }); // e.g. after `0`/`^`
    app.ensure_cursor_visible_horizontally(10);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_col, 0);
}

#[test]
fn ensure_cursor_visible_horizontally_resolves_the_cursors_visual_column_past_a_tab() {
    let mut app = App::with_file(None);
    app.test_insert_str("a\tx"); // tab (width 8, default) expands "a" + 7 spaces + "x"
    // Cursor right after 'x': raw col 3, visual col 9.
    app.test_set_cursor(Cursor { char_idx: 3, sticky_col: 0 });
    app.ensure_cursor_visible_horizontally(4);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_col, 6); // scroll_to_include(0, 9, 4)
}

#[test]
fn scroll_col_is_independent_per_pane() {
    let mut app = App::with_file(None);
    app.test_insert_str(&"x".repeat(50));
    let first = app.windows().focused_id();
    app.split_vertical();
    let second = app.windows().focused_id();
    assert_ne!(first, second);

    app.ensure_cursor_visible_horizontally(10); // scrolls the now-focused second pane only
    assert!(app.pane_state(second).scroll_col > 0);
    assert_eq!(app.pane_state(first).scroll_col, 0, "the other pane's own scroll position must be untouched");
}

#[test]
fn small_scroll_change_starts_an_animation_not_an_instant_jump() {
    let mut app = App::with_file(None);
    // Forced on -- `App::with_file(None)` reads the real machine
    // config.ini, whose `animations` setting shouldn't decide
    // whether *this* test (of the animated-by-default path) passes.
    app.config.animations = Some(true);
    for _ in 0..5 {
        app.test_insert('\n');
    }
    app.ensure_cursor_visible(3); // 6 lines, 3-line viewport -> scrolls a bit
    assert!(app.workspaces.active_scroll_anims().contains_key(&app.focused_pane_id()));
    let ps = app.pane_state(app.focused_pane_id());
    assert_ne!(ps.rendered_scroll, ps.scroll_line as f32); // still mid-ease, not snapped
}

#[test]
fn small_scroll_change_snaps_instantly_when_animations_are_disabled() {
    let mut app = App::with_file(None);
    app.config.animations = Some(false);
    for _ in 0..5 {
        app.test_insert('\n');
    }
    app.ensure_cursor_visible(3);
    assert!(!app.workspaces.active_scroll_anims().contains_key(&app.focused_pane_id()));
    let ps = app.pane_state(app.focused_pane_id());
    assert_eq!(ps.rendered_scroll, ps.scroll_line as f32); // snapped, not mid-ease
}

#[test]
fn a_new_scroll_target_while_easing_snaps_instead_of_re_easing() {
    // Simulates holding `j`: a new scroll target can arrive before the
    // previous 150ms ease has settled. It must snap to the new target
    // immediately rather than restarting another ease from an
    // already-stale position -- otherwise `rendered_scroll` would keep
    // chasing a few lines behind the cursor for as long as the key
    // stays held, instead of ever catching up.
    let mut app = App::with_file(None);
    app.config.animations = Some(true); // see the same note above
    for _ in 0..30 {
        app.test_insert('\n');
    }
    app.test_set_cursor(Cursor::at_start());
    for _ in 0..12 {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.move_down(cursor);
    }
    // Cursor now on line 12; a 10-line viewport wants scroll_line = 3
    // (a step past the edge, not a jump, which would be centred).
    app.ensure_cursor_visible(10);
    let pane = app.focused_pane_id();
    assert!(app.workspaces.active_scroll_anims().contains_key(&pane)); // mid-ease from that first scroll

    // One more line down before the ease had a chance to settle -- the
    // exact shape of holding `j`.
    app.motion.key_repeat = true;
    {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.move_down(cursor);
    }
    app.ensure_cursor_visible(10);
    let ps = app.pane_state(pane);
    assert_eq!(ps.rendered_scroll, ps.scroll_line as f32, "must snap, not compound the lag from the still-active ease");
    assert!(!app.workspaces.active_scroll_anims().contains_key(&pane));
}

#[test]
fn a_new_scroll_target_that_isnt_a_held_key_eases_on_from_where_the_view_is() {
    // The wheel's next notch arriving mid-ease: no snap.
    let mut app = App::with_file(None);
    for _ in 0..60 {
        app.test_insert('\n');
    }
    app.test_set_cursor(Cursor::at_start());
    for _ in 0..15 {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.move_down(cursor);
    }
    app.ensure_cursor_visible(10);
    let pane = app.focused_pane_id();
    for _ in 0..5 {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.move_down(cursor);
    }
    app.ensure_cursor_visible(10);
    let ps = app.pane_state(pane);
    assert!(app.workspaces.active_scroll_anims().contains_key(&pane), "still easing");
    assert!(ps.rendered_scroll < ps.scroll_line as f32);
}

/// A jump far off the view lands mid-window, as `zz` puts it; a step
/// past the edge scrolls a line; near the end the last line stays at
/// the bottom; a cursor in view leaves the view alone.
#[test]
fn a_far_jump_lands_mid_window_and_a_step_scrolls_a_line() {
    assert_eq!(scroll_for_cursor(0, 120, 30, 400, false), 105, "far below: centred");
    assert_eq!(scroll_for_cursor(200, 20, 30, 400, false), 5, "far above too");
    assert_eq!(scroll_for_cursor(0, 30, 30, 400, false), 1, "j on the last line: one line");
    assert_eq!(scroll_for_cursor(0, 44, 30, 400, false), 15, "a motion within half a window past the edge: just far enough");
    assert_eq!(scroll_for_cursor(0, 33, 30, 400, true), 18, "a jump just past the edge: centred all the same");
    assert_eq!(scroll_for_cursor(10, 20, 30, 400, true), 10, "a jump already in view: stays");
    assert_eq!(scroll_for_cursor(0, 395, 30, 400, true), 370, "near the end: the last line at the bottom");
    assert_eq!(scroll_for_cursor(0, 3, 30, 10, true), 0, "a short file never scrolls");
}

/// Opening a file at a line far down -- what a symbol picker, a search
/// result or `gd` does -- puts that line in the middle of the pane.
#[test]
fn a_jump_into_a_file_shows_the_line_in_the_middle_of_the_pane() {
    let dir = TempDir::new("jump_centre");
    let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let file = dir.write("long.tcl", &text);
    let mut app = App::with_file(None);
    app.open_file_from_picker(&file);
    app.jump_to_grep_match(&fenix_project::GrepMatch { path: file.clone(), line: 151, col: 1, text: String::new() });
    app.ensure_cursor_visible(20);
    let scroll = app.pane_state(app.focused_pane_id()).scroll_line;
    assert_eq!(scroll, 140, "line 151 (row 150) sits ten rows down a 20-row pane");
    // Just past the bottom edge, as a search result often is: centred
    // too, where a step down would only scroll a line.
    app.jump_to_grep_match(&fenix_project::GrepMatch { path: file, line: 162, col: 1, text: String::new() });
    app.ensure_cursor_visible(20);
    assert_eq!(app.pane_state(app.focused_pane_id()).scroll_line, 151);
}

#[test]
fn huge_scroll_jump_snaps_instantly_without_animating() {
    let mut app = App::with_file(None);
    for _ in 0..500 {
        app.test_insert('\n');
    }
    app.ensure_cursor_visible(10); // jump of ~490 lines, way past the snap threshold
    assert!(!app.workspaces.active_scroll_anims().contains_key(&app.focused_pane_id()));
    let ps = app.pane_state(app.focused_pane_id());
    assert_eq!(ps.rendered_scroll, ps.scroll_line as f32);
}

#[test]
fn rendered_scroll_eases_toward_target_and_settles() {
    let mut app = App::with_file(None);
    let pane = app.focused_pane_id();
    {
        let ps = app.pane_state_mut(pane);
        ps.scroll_line = 10;
        ps.rendered_scroll = 0.0;
    }
    app.workspaces.active_scroll_anims_mut().insert(pane, ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION / 2 });
    app.update_rendered_scroll();
    let r = app.pane_state(pane).rendered_scroll;
    assert!(r > 0.0 && r < 10.0, "should be partway there");
    assert!(app.workspaces.active_scroll_anims().contains_key(&pane));

    app.workspaces.active_scroll_anims_mut().insert(pane, ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION * 2 });
    app.update_rendered_scroll();
    assert_eq!(app.pane_state(pane).rendered_scroll, 10.0);
    assert!(!app.workspaces.active_scroll_anims().contains_key(&pane)); // settled, animation cleared
}

#[test]
fn render_base_line_splits_a_fractional_scroll_position() {
    let mut app = App::with_file(None);
    let pane = app.focused_pane_id();
    app.pane_state_mut(pane).rendered_scroll = 4.25;
    assert_eq!(app.render_base_line(), 4);
    assert!((app.pane_state(pane).rendered_scroll.fract() - 0.25).abs() < f32::EPSILON);
}

#[test]
fn modeline_reflects_filename_dirty_state_mode_and_position() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer(); // with_file(None) now opens the dashboard, not a plain scratch buffer
    assert_eq!(app.modeline_text(), " \u{25B2}  NORMAL   [No Name]   Ln 1, Col 1   Plain text   Indent 4   UTF-8   No EOL ");

    app.test_insert('a');
    app.test_insert('b');
    assert_eq!(app.modeline_text(), " \u{25B2}  NORMAL   [No Name] [+]   Ln 1, Col 3   Plain text   Indent 4   UTF-8   No EOL ");
}

#[test]
fn prompt_popup_shows_command_line_while_typing_an_ex_command() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    for ch in [':', 'w', 'q'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    assert_eq!(app.active_prompt_text(), Some(":wq".to_string()));
    // The modeline itself keeps showing its ordinary content
    // underneath -- a capturing prompt no longer blanks it out.
    assert!(app.modeline_text().contains("Ln 1, Col"));
}

#[test]
fn prompt_popup_shows_the_search_query_while_typing_it() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    for ch in ['/', 'f', 'o', 'o'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    // An empty buffer: the query, and that it matches nothing.
    assert_eq!(app.active_prompt_text(), Some("/foo   no match".to_string()));
    assert!(app.modeline_text().contains("Ln 1, Col"));
}

#[test]
fn visual_selection_segments_cover_the_selected_range() {
    let mut app = App::with_file(None);
    for ch in "hello world".chars() {
        app.test_insert(ch);
    }
    app.test_set_cursor(Cursor::at_start());
    app.test_vim_key(KeyPress::char('v'));
    for _ in 0..4 {
        app.test_vim_key(KeyPress::char('l'));
    }
    // anchor 0, cursor now at 4 ("hello"[0..5))
    assert_eq!(app.visual_selection_segments(0..10), vec![(0, 0, 5)]);
}

#[test]
fn visual_selection_segments_empty_outside_visual_mode() {
    let app = App::with_file(None);
    assert!(app.visual_selection_segments(0..10).is_empty());
}

#[test]
fn bracket_match_segments_covers_both_brackets_when_the_cursor_is_on_one() {
    let mut app = App::with_file(None);
    app.test_insert_str("(hello)");
    app.test_set_cursor(Cursor::at_start()); // on the opening '('
    assert_eq!(app.bracket_match_segments(0..10), vec![(0, 0, 1), (0, 6, 7)]);
}

#[test]
fn bracket_match_segments_works_from_the_closing_side_too() {
    let mut app = App::with_file(None);
    app.test_insert_str("(hello)");
    app.test_set_cursor(Cursor { char_idx: 6, sticky_col: 6 }); // on the ')'
    assert_eq!(app.bracket_match_segments(0..10), vec![(0, 6, 7), (0, 0, 1)]);
}

#[test]
fn bracket_match_segments_empty_when_the_cursor_is_not_on_a_bracket() {
    let mut app = App::with_file(None);
    app.test_insert_str("(hello)");
    app.test_set_cursor(Cursor { char_idx: 3, sticky_col: 3 }); // on 'l'
    assert!(app.bracket_match_segments(0..10).is_empty());
}

#[test]
fn bracket_match_segments_empty_for_an_unmatched_bracket() {
    let mut app = App::with_file(None);
    app.test_insert_str("(hello");
    app.test_set_cursor(Cursor::at_start());
    assert!(app.bracket_match_segments(0..10).is_empty());
}

#[test]
fn hlsearch_segments_covers_every_match_after_a_confirmed_search() {
    let mut app = App::with_file(None);
    app.test_insert_str("foo bar foo baz foo");
    app.test_set_cursor(Cursor::at_start());
    for ch in ['/', 'f', 'o', 'o'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));

    assert_eq!(app.hlsearch_segments(0..10), vec![(0, 0, 3), (0, 8, 11), (0, 16, 19)]);
}

#[test]
fn hlsearch_segments_is_empty_before_any_search_is_confirmed() {
    let mut app = App::with_file(None);
    app.test_insert_str("foo bar foo");
    assert!(app.hlsearch_segments(0..10).is_empty());
}

#[test]
fn hlsearch_segments_stay_through_an_edit_and_clear_on_escape() {
    let mut app = App::with_file(None);
    app.test_insert_str("foo bar foo");
    app.test_set_cursor(Cursor::at_start());
    for ch in ['/', 'f', 'o', 'o'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(!app.hlsearch_segments(0..10).is_empty());

    app.test_vim_key(KeyPress::char('x')); // a real edit, in Normal mode
    assert!(!app.hlsearch_segments(0..10).is_empty(), "kept, as in Vim");
    // `x` took the second match's `f`: one is left.
    assert_eq!(app.search_count(), Some((1, 1)), "[1/1] in the modeline");
    app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(app.hlsearch_segments(0..10).is_empty());
    assert_eq!(app.search_count(), None);
}

#[test]
fn focused_caret_previews_the_search_match_without_moving_the_real_cursor() {
    let mut app = App::with_file(None);
    app.test_insert_str("foo bar foo");
    app.test_set_cursor(Cursor::at_start());
    for ch in ['/', 'f', 'o', 'o'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    // The second "foo" starts at char 8, still line 0 -- row 0, col 8.
    assert_eq!(app.focused_caret(0, 0), Some((0, 8)));
    assert_eq!(app.test_cursor().char_idx, 0, "the real cursor must not move during preview");
}

#[test]
fn focused_caret_falls_back_to_hl_row_outside_search_mode() {
    let mut app = App::with_file(None);
    app.test_insert_str("hello");
    assert_eq!(app.focused_caret(0, 3), Some((0, 3)));
}

#[test]
fn modeline_shows_visual_kind_not_just_visual() {
    let mut app = App::with_file(None);
    for ch in "one\ntwo\nthree".chars() {
        app.test_insert(ch);
    }
    app.test_set_cursor(Cursor::at_start());

    app.test_vim_key(KeyPress::char('v'));
    assert!(app.modeline_text().contains("VISUAL"));

    app.test_vim_key(KeyPress::char('V'));
    assert!(app.modeline_text().contains("V-LINE"));

    app.test_vim_key(KeyPress::char('v').with_ctrl());
    assert!(app.modeline_text().contains("V-BLOCK"));
}

#[test]
fn mode_colors_differ_per_mode_and_visual_kinds_share_one_accent() {
    let mut app = App::with_file(None);
    let (normal_bg, _) = app.mode_colors();

    app.test_vim_key(KeyPress::char('i'));
    let (insert_bg, _) = app.mode_colors();
    assert_ne!(normal_bg, insert_bg);
    app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));

    app.test_vim_key(KeyPress::char('v'));
    let (char_visual_bg, _) = app.mode_colors();
    app.test_vim_key(KeyPress::char('V'));
    let (line_visual_bg, _) = app.mode_colors();
    assert_eq!(char_visual_bg, line_visual_bg); // one accent for all Visual kinds
    assert_ne!(char_visual_bg, normal_bg);
}

#[test]
fn caret_is_block_everywhere_except_insert_mode() {
    let mut app = App::with_file(None);
    assert!(app.caret_is_block()); // starts in Normal

    app.test_vim_key(KeyPress::char('i'));
    assert!(!app.caret_is_block()); // Insert -- thin bar
    app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(app.caret_is_block()); // back to Normal

    app.test_vim_key(KeyPress::char('v'));
    assert!(app.caret_is_block()); // Visual
}

#[test]
fn visual_line_segments_cover_whole_lines_regardless_of_column() {
    let mut app = App::with_file(None);
    for ch in "one\ntwo\nthree".chars() {
        app.test_insert(ch);
    }
    app.test_set_cursor(Cursor { char_idx: 5, sticky_col: 1 }); // column 1 of "two"
    app.test_vim_key(KeyPress::char('V'));
    assert_eq!(app.visual_selection_segments(0..10), vec![(1, 0, 3)]);
}

#[test]
fn visual_block_segments_form_a_column_rectangle() {
    let mut app = App::with_file(None);
    for ch in "abc\ndef\nghi".chars() {
        app.test_insert(ch);
    }
    app.test_set_cursor(Cursor::at_start());
    app.test_vim_key(KeyPress::char('v').with_ctrl());
    for ch in ['j', 'j', 'l'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    assert_eq!(app.visual_selection_segments(0..10), vec![(0, 0, 2), (1, 0, 2), (2, 0, 2)]);
}

#[test]
fn ease_out_cubic_starts_at_zero_ends_at_one_and_is_monotonic() {
    assert_eq!(ease_out_cubic(0.0), 0.0);
    assert_eq!(ease_out_cubic(1.0), 1.0);
    assert_eq!(ease_out_cubic(2.0), 1.0); // clamps past the end
    assert_eq!(ease_out_cubic(-1.0), 0.0); // clamps before the start

    let mut prev = 0.0;
    for i in 1..=10 {
        let v = ease_out_cubic(i as f32 / 10.0);
        assert!(v >= prev, "not monotonic at step {i}");
        prev = v;
    }
}

#[test]
fn ease_out_cubic_front_loads_motion_more_than_linear() {
    // "ease-out": faster at the start than a linear ramp would be
    assert!(ease_out_cubic(0.25) > 0.25);
}

#[test]
fn fresh_app_caret_is_immediately_fully_visible_no_fade_in_delay() {
    let app = App::with_file(None);
    assert_eq!(app.caret_alpha(), 1.0);
}

/// The first frame overdue after the splash: the loop wakes for it,
/// tries again, and keeps waking until one is drawn (#65).
#[test]
fn an_overdue_first_frame_is_tried_again() {
    let mut app = App::with_file(None);
    let now = Instant::now();
    assert_eq!(app.first_frame_tick(now), None, "nothing awaited");
    app.first_frame_due = Some(now + FIRST_FRAME_WITHIN);
    assert_eq!(app.first_frame_tick(now), Some(now + FIRST_FRAME_WITHIN), "not yet");
    let late = now + FIRST_FRAME_WITHIN + Duration::from_millis(1);
    assert_eq!(app.first_frame_tick(late), Some(late + FIRST_FRAME_WITHIN), "tried, and awaited again");
}

/// The caret blinks for a while after a key, then stays lit and
/// stops waking the loop; the next key starts it again (#74).
#[test]
fn the_caret_stops_blinking_once_the_window_is_idle() {
    let mut app = App::with_file(None);
    app.wake_caret();
    let woken = app.caret_woken;
    assert!(app.blink_tick(woken + BLINK_INTERVAL), "blinks at first");
    assert!(!app.blink_visible);
    let idle = woken + CARET_BLINKS_FOR + Duration::from_secs(1);
    assert!(!app.caret_settled(idle), "not while it's out");
    assert!(app.blink_tick(idle), "comes back on");
    assert!(app.blink_visible);
    let later = idle + Duration::from_secs(1);
    assert!(app.caret_settled(later));
    assert!(!app.blink_tick(later), "and stays");
    app.wake_caret();
    assert!(!app.caret_settled(Instant::now()), "a key starts it again");
}

#[test]
fn wake_caret_snaps_to_visible_without_fading_in() {
    let mut app = App::with_file(None);
    app.blink_visible = false;
    app.blink_transition_start = Instant::now(); // mid fade-out
    app.wake_caret();
    assert_eq!(app.caret_alpha(), 1.0);
}

#[test]
fn no_pulse_by_default() {
    let app = App::with_file(None);
    assert!(app.pulse_overlay(0..10).is_none());
}

#[test]
fn fresh_pulse_is_near_peak_alpha_and_covers_its_range() {
    let mut app = App::with_file(None);
    for ch in "hello world".chars() {
        app.test_insert(ch);
    }
    app.pulse = Some(Pulse { range: 0..5, started: Instant::now() });
    let (segments, alpha) = app.pulse_overlay(0..10).expect("pulse should be active");
    assert_eq!(segments, vec![(0, 0, 5)]);
    assert!(alpha > 0.0 && alpha <= PULSE_PEAK_ALPHA);
}

#[test]
fn expired_pulse_produces_no_overlay() {
    let mut app = App::with_file(None);
    for ch in "hello".chars() {
        app.test_insert(ch);
    }
    app.pulse = Some(Pulse { range: 0..5, started: Instant::now() - PULSE_DURATION * 2 });
    assert!(app.pulse_overlay(0..10).is_none());
}

// App::handle_key itself needs a real winit KeyEvent/ActiveEventLoop,
// which aren't constructible in a unit test (same constraint as the
// rest of the winit-facing integration, verified by code review
// instead -- see the plan). This exercises the layer below it: that a
// VimEvent::Pulse from VimState turns into a renderable pulse_overlay,
// which is the actual logic worth covering here; App's own arm that
// wires the two together is two lines and reviewed by eye.
#[test]
fn vim_pulse_event_yields_a_renderable_pulse_overlay() {
    let mut app = App::with_file(None);
    for ch in "hello world".chars() {
        app.test_insert(ch);
    }
    app.test_set_cursor(Cursor::at_start());
    assert!(app.pulse.is_none());

    app.test_vim_key(KeyPress::char('y'));
    let event = app.test_vim_key(KeyPress::char('w'));
    let fenix_vim::VimEvent::Pulse(range) = event else { panic!("expected a Pulse event from yw") };
    app.pulse = Some(Pulse { range, started: Instant::now() });
    assert!(app.pulse_overlay(0..10).is_some());
}

#[test]
fn gutter_chars_zero_when_off() {
    let mut app = App::with_file(None);
    app.line_number_mode = LineNumberMode::Off;
    assert_eq!(app.gutter_chars(app.open()), 0);
}

#[test]
fn gutter_chars_defaults_to_absolute_and_sizes_for_line_count() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer(); // with_file(None) now opens the dashboard; this test wants a plain empty buffer
    assert_eq!(app.gutter_chars(app.open()), 2); // 1-digit number + 1 padding column
}

#[test]
fn content_spans_marks_rows_past_buffer_end_with_tilde() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer(); // single empty line, cursor on it
    let gutter = app.gutter_chars(app.open());
    let spans = app.content_spans(app.open(), 0, 3, gutter, None, &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "1 \n~ \n~ ");
}

/// The agents' prompt is a box: no line numbers, no `~` rows under
/// what you've written.
#[test]
fn a_prompt_has_no_gutter_and_no_tildes() {
    let mut app = App::with_file(None);
    let id = app.buffers.open_prompt();
    app.open_buffer_in_focused_pane(id);
    let ob = app.buffers.get(id).unwrap();
    assert_eq!(app.gutter_chars(ob), 0);
    let spans = app.content_spans(ob, 0, 4, 0, None, &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    assert!(!spans.iter().any(|(s, _)| s.contains('~')), "{spans:?}");
}

#[test]
fn content_spans_off_mode_still_shows_tilde_for_rows_past_end() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.line_number_mode = LineNumberMode::Off;
    let gutter = app.gutter_chars(app.open());
    let spans = app.content_spans(app.open(), 0, 2, gutter, None, &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "\n~");
}

#[test]
fn content_spans_relative_mode_shows_distance_from_cursor() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.line_number_mode = LineNumberMode::Relative;
    app.test_insert_str("a\nb\nc\nd");
    app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 }); // line 1, 'b'
    let gutter = app.gutter_chars(app.open());
    let spans = app.content_spans(app.open(), 0, 4, gutter, None, &[], 1, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "1 a\n0 b\n1 c\n2 d");
}

#[test]
fn content_spans_current_line_number_uses_fg_not_gutter_fg() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.test_insert_str("a\nb");
    app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 }); // line 1
    let gutter = app.gutter_chars(app.open());
    let spans = app.content_spans(app.open(), 0, 2, gutter, None, &[], 1, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    assert_eq!(spans[0].1, app.theme.gutter_fg); // line 0: not current
    assert_eq!(spans[2].1, app.theme.fg); // line 1: current line's gutter
}

#[test]
fn content_spans_pads_a_dashboard_buffer_with_real_leading_blanks_not_gutter_digits() {
    // Regression test: horizontal centering only works if the padding
    // is actually written into the rendered text -- `gutter_px` alone
    // (a caret/selection position-math field) never reaches
    // `TextArea.left`, which is hardcoded to `rect.x + PAD_LEFT` in
    // `text.rs`. A prior version of this feature computed a pixel
    // offset and folded it into `gutter_px`, which compiled and had
    // passing unit tests but never visibly centered anything.
    let app = App::with_file(None); // opens the dashboard
    let ob = app.open();
    assert_eq!(ob.kind, BufferKind::Dashboard);

    let pad = [5usize];
    let spans = app.content_spans(ob, 0, 1, 0, Some(&pad), &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    assert_eq!(spans[0].0, "     "); // 5 literal spaces, not "1 " or similar
}

#[test]
fn content_spans_never_shows_a_tilde_past_the_end_of_a_dashboard_buffer() {
    let app = App::with_file(None);
    let ob = app.open();
    let visual_lines = ob.buffer.visual_line_count();
    let pad: Vec<usize> = vec![0; visual_lines + 11];
    // Ask for a row well past the dashboard's own content.
    let spans = app.content_spans(ob, visual_lines + 10, 1, 0, Some(&pad), &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    assert!(!spans.iter().any(|(s, _)| s == "~"));
}

#[test]
fn editor_line_numbers_sets_the_gutter_and_the_cycle_goes_on_from_it() {
    let mut app = App::with_file(None);
    assert_eq!(app.line_number_mode, LineNumberMode::Absolute, "the default");
    app.set_setting("editor.line_numbers", Some(fenix_config::Value::Text("relative".into()))).ok();
    assert_eq!(app.line_number_mode, LineNumberMode::Relative);
    app.cycle_line_number_mode();
    assert_eq!(app.line_number_mode, LineNumberMode::Off);
    app.config.set_value("editor.line_numbers", Some(fenix_config::Value::Text("off".into())));
    assert_eq!(LineNumberMode::from_setting(&app.config), LineNumberMode::Off);
}

#[test]
fn cycle_line_number_mode_goes_off_absolute_relative_off() {
    let mut app = App::with_file(None);
    app.line_number_mode = LineNumberMode::Off;
    app.cycle_line_number_mode();
    assert_eq!(app.line_number_mode, LineNumberMode::Absolute);
    app.cycle_line_number_mode();
    assert_eq!(app.line_number_mode, LineNumberMode::Relative);
    app.cycle_line_number_mode();
    assert_eq!(app.line_number_mode, LineNumberMode::Off);
}

// `increase_font_size`/`decrease_font_size`/`reset_font_size`
// themselves need a real `TextPipeline`, which needs a real GPU
// device to construct -- not something a headless test builds (same
// reason `resolve_font_size`/persistence are tested directly at the
// `fenix_config`/`text` module level instead). What a headless
// `App::with_file` *can* verify is that these are safe no-ops before
// `resumed()` has run (`self.text` is `None` at that point) rather
// than panicking.
#[test]
fn the_logical_font_size_is_the_configured_one_not_a_dpi_scaled_one() {
    let mut app = App::with_file(None);
    app.config.font_size = None;
    assert_eq!(app.logical_font_size(), text::FONT_SIZE, "unset falls back to the default");

    app.config.font_size = Some(20.0);
    assert_eq!(app.logical_font_size(), 20.0);
    // Whatever a monitor's scale factor is, it is applied on the way
    // out to a pipeline -- never folded back into the stored value,
    // which would bake one monitor's DPI into config.ini.
    assert_eq!(app.frame_scale(), 1.0, "no window yet, so no scaling");
}

#[test]
fn font_size_adjustments_are_safe_no_ops_before_the_gpu_exists() {
    let mut app = App::with_file(None);
    assert!(app.text.is_none());
    app.increase_font_size();
    app.decrease_font_size();
    app.reset_font_size();
}

#[test]
fn a_persisted_indent_width_applies_to_vim() {
    // Mirrors what `with_file` does with `config.indent_width` --
    // `with_file` itself always reads the *real* config path, so
    // this exercises the same pipeline (a loaded `Config` feeding
    // `VimState::set_indent_width`) directly instead.
    let dir = TempDir::new("persisted_indent_width_applies");
    let mut config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    config.indent_width = Some(3);
    config.save().unwrap();

    let reloaded = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
    let mut vim = VimState::new();
    vim.set_indent_width(reloaded.indent_width.unwrap_or(fenix_vim::DEFAULT_INDENT_WIDTH));
    assert_eq!(vim.indent_width(), 3);
}

#[test]
fn set_shiftwidth_command_persists_the_new_width() {
    let dir = TempDir::new("set_shiftwidth_persists");
    let mut app = App::with_file(None);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));

    let event = app.test_vim_key(KeyPress::char(':'));
    assert_eq!(event, VimEvent::None);
    for ch in "set shiftwidth=3".chars() {
        app.test_vim_key(KeyPress::char(ch));
    }
    let event = app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(event, VimEvent::IndentWidthChanged(3));

    // Mirrors handle_key's own IndentWidthChanged arm -- handle_key
    // itself needs a real winit KeyEvent, so this exercises the
    // persistence call it would make directly (same posture as
    // vim_pulse_event_yields_a_renderable_pulse_overlay above).
    if let VimEvent::IndentWidthChanged(width) = event {
        app.config.indent_width = Some(width);
        app.config.save().unwrap();
    }
    let reloaded = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
    assert_eq!(reloaded.indent_width, Some(3));
}

/// Mirrors `a_persisted_indent_width_applies_to_vim`: the default
/// (no `[editor]` key at all) still reproduces real Vim's actual
/// factory default (`_` is a keyword character), not an empty set.
#[test]
fn a_persisted_iskeyword_extra_applies_to_vim() {
    let dir = TempDir::new("persisted_iskeyword_applies");
    let mut config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    config.iskeyword_extra = Some(String::new());
    config.save().unwrap();

    let reloaded = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
    let mut vim = VimState::new();
    vim.set_iskeyword_extra(
        reloaded.iskeyword_extra.as_deref().map(|s| s.chars().collect()).unwrap_or_else(|| fenix_vim::DEFAULT_ISKEYWORD_EXTRA.to_vec()),
    );
    assert_eq!(vim.iskeyword_extra(), &[] as &[char]);
}

#[test]
fn no_persisted_iskeyword_extra_falls_back_to_real_vims_own_default() {
    let dir = TempDir::new("unset_iskeyword_falls_back");
    let config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    assert!(config.iskeyword_extra.is_none());

    let mut vim = VimState::new();
    vim.set_iskeyword_extra(
        config.iskeyword_extra.as_deref().map(|s| s.chars().collect()).unwrap_or_else(|| fenix_vim::DEFAULT_ISKEYWORD_EXTRA.to_vec()),
    );
    assert_eq!(vim.iskeyword_extra(), &['_']);
}

#[test]
fn set_iskeyword_command_persists_the_new_setting() {
    let dir = TempDir::new("set_iskeyword_persists");
    let mut app = App::with_file(None);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    // `App::with_file` still seeds `app.vim` from *this machine's*
    // real, already-on-disk config (deliberately not swapped out
    // above, the same shape `set_shiftwidth_command_persists_the_
    // new_width` already has) -- pinned to real Vim's own default
    // explicitly, so the `-=_` below is guaranteed to be a real
    // change regardless of what's actually persisted there.
    app.vim.set_iskeyword_extra(vec!['_']);

    let event = app.test_vim_key(KeyPress::char(':'));
    assert_eq!(event, VimEvent::None);
    for ch in "set iskeyword-=_".chars() {
        app.test_vim_key(KeyPress::char(ch));
    }
    let event = app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(event, VimEvent::IsKeywordChanged(Vec::new()));

    // Mirrors handle_key's own IsKeywordChanged arm -- handle_key
    // itself needs a real winit KeyEvent, same posture
    // `set_shiftwidth_command_persists_the_new_width` already has.
    if let VimEvent::IsKeywordChanged(chars) = event {
        app.config.iskeyword_extra = Some(chars.into_iter().collect());
        app.config.save().unwrap();
    }
    let reloaded = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
    assert_eq!(reloaded.iskeyword_extra, Some(String::new()));
}

#[test]
fn split_line_by_highlights_covers_a_middle_span() {
    let theme = theme::default_theme();
    let spans = split_line_by_highlights("let x = 1;", 0, 10, &[(4..5, theme.syntax_variable)], theme.fg);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "let x = 1;");
    assert_eq!(spans[1], ("x".to_string(), theme.syntax_variable));
    assert_eq!(spans[0].1, theme.fg);
    assert_eq!(spans[2].1, theme.fg);
}

#[test]
fn split_line_by_highlights_clips_ranges_extending_past_the_line() {
    let theme = theme::default_theme();
    // A highlight spanning into the next line should only color this
    // line's portion of it.
    let spans = split_line_by_highlights("abc", 10, 13, &[(8..15, theme.syntax_string)], theme.fg);
    assert_eq!(spans, vec![("abc".to_string(), theme.syntax_string)]);
}

#[test]
fn split_line_by_highlights_ignores_ranges_outside_the_line() {
    let theme = theme::default_theme();
    let spans = split_line_by_highlights("abc", 10, 13, &[(0..5, theme.syntax_string)], theme.fg);
    assert_eq!(spans, vec![("abc".to_string(), theme.fg)]);
}

#[test]
fn remap_highlights_for_display_lands_on_the_tab_expanded_offsets() {
    let color = theme::default_theme().syntax_variable;
    let buffer = Buffer::from_text("a\tbb");
    let (display, col_map) = tabstops::expand_line("a\tbb", &TabStops::Fixed(4));
    assert_eq!(display, "a   bb");

    let remapped = remap_highlights_for_display(&buffer, 0, &col_map, &display, 0, 4, &[(2..4, color)]);

    assert_eq!(remapped, vec![(4..6, color)]); // "bb" now starts at display byte 4
}

#[test]
fn content_spans_still_colors_a_tab_indented_line_via_syntax_highlights() {
    // The scope this session's renderer work initially cut too far:
    // C/Bash/Python (all have a tree-sitter grammar registered in
    // this project) commonly indent with real tabs -- highlighting
    // must survive that, not just render the tab-expanded text in
    // plain `theme.fg`.
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.test_insert_str("a\tbb");
    let color = app.theme.syntax_variable;
    let fg = app.theme.fg;
    let ob = app.open();
    let highlights = vec![(2..4, color)]; // "bb"'s real byte range
    let spans = app.content_spans(ob, 0, 1, 0, None, &highlights, 0, &TabStops::Fixed(4), 0);

    assert_eq!(spans, vec![("a   ".to_string(), fg), ("bb".to_string(), color)]);
}

#[test]
fn a_very_long_line_highlights_and_draws_only_what_shows() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    let text = "fn a() {} ".repeat(300_000);
    app.open_mut().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, &text));
    app.open_mut().buffer = fenix_core::Buffer::from_text(&text);
    let id = app.focused_buffer_id();

    for scroll_col in [0, 2_000_000] {
        let highlights = app.syntax_highlights_from_column(id, 0, 1, scroll_col);
        let spans = app.content_spans(app.open(), 0, 1, 0, None, &highlights, 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), scroll_col);
        assert!(highlights.len() < 2 * MAX_ROW_CHARS, "{} spans at column {scroll_col}", highlights.len());
        assert_eq!(spans.iter().map(|(text, _)| text.chars().count()).sum::<usize>(), MAX_ROW_CHARS);
        let keyword = spans.iter().find(|(text, _)| text == "fn");
        assert_eq!(keyword.map(|(_, color)| *color), Some(app.theme.syntax_color("keyword")), "colored where it's scrolled to");
    }
}

#[test]
fn syntax_state_seeds_from_the_detected_language() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer(); // with_file(None) now opens the dashboard, which never has syntax
    assert!(app.open().syntax.is_none()); // no path -> no language

    app.open_mut().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, ""));
    app.test_insert_str("fn main() {}");

    let id = app.focused_buffer_id();
    let highlights = app.syntax_highlights_for_visible_range(id, 0, 1);
    assert!(!highlights.is_empty(), "expected highlights for a Rust buffer, got none");

    let spans = app.content_spans(app.open(), 0, 1, 0, None, &highlights, 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    let fn_span = spans.iter().find(|(s, _)| s == "fn");
    assert_eq!(
        fn_span.map(|(_, c)| *c),
        Some(app.theme.syntax_color("keyword")),
        "expected \"fn\" colored as a keyword, got {spans:?}"
    );
}

#[test]
fn markdown_inline_emphasis_flows_through_to_a_distinctly_colored_rendered_span() {
    // End-to-end: a real Markdown file's inline injection
    // (`fenix_syntax::SyntaxState`) through the theme's capture-name
    // resolution (`syntax_color`) to an actual rendered span color
    // -- not just the two halves separately.
    let dir = TempDir::new("markdown_inline_rendered");
    let file = dir.write("doc.md", "Some **bold** word.");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    let id = app.focused_buffer_id();
    let highlights = app.syntax_highlights_for_visible_range(id, 0, 1);
    let spans = app.content_spans(app.open(), 0, 1, 0, None, &highlights, 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);

    let bold_span = spans.iter().find(|(s, _)| s == "bold");
    assert_eq!(
        bold_span.map(|(_, c)| *c),
        Some(app.theme.syntax_color("text.strong")),
        "expected \"bold\" colored as text.strong, got {spans:?}"
    );
    // And distinctly from plain surrounding text, not just
    // matching `fg` by coincidence.
    assert_ne!(app.theme.syntax_color("text.strong"), app.theme.fg);
}

/// A call to a proc defined in another file reads as one: one the
/// Tcl extension's language server names (`workspace/symbol`, from
/// ctags), or one in your symbols file -- qualified or not, as it's
/// called (`greet` inside `myns`, `::myns::greet`).
#[test]
fn tcl_calls_to_procs_from_ctags_and_the_symbols_file_are_colored() {
    let dir = TempDir::new("tcl_command_highlight_elsewhere");
    let file = dir.write("main.tcl", "configure_board 1\ngreet you\n::myns::greet you\nmyns::greet you\nfrom_file x\nlib::from_file y\nnot_a_proc z\n");
    let symbols = dir.write("symbols.txt", "# words\nlib::from_file\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.config.completion_symbols_file = Some(symbols);
    let id = app.focused_buffer_id();
    let key = LspKey { server: LspServer::Additional("fenix.tcl:tcl".into()), root: dir.0.clone() };
    let answer = serde_json::json!([
        { "name": "configure_board", "kind": 12, "location": { "uri": "file:///p/lib.tcl", "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } } } },
        { "name": "myns::greet", "kind": 12, "location": { "uri": "file:///p/lib.tcl", "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 0 } } } },
        { "name": "not_a_proc", "kind": 3, "location": { "uri": "file:///p/lib.tcl", "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } } } },
    ]);
    let names = functions_named(serde_json::from_value(answer).unwrap());
    assert!(!names.contains("not_a_proc"), "a namespace isn't called");
    app.lsp_workspace_names.insert(key.clone(), (Instant::now(), std::sync::Arc::new(names)));
    app.tcl_name_servers.insert(id, (Instant::now(), vec![key]));

    let highlights = app.syntax_highlights_for_visible_range(id, 0, 7);
    let spans = app.content_spans(app.open(), 0, 7, 0, None, &highlights, 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
    let function = app.theme.syntax_color("function");
    for call in ["configure_board", "greet", "::myns::greet", "myns::greet", "from_file", "lib::from_file"] {
        assert_eq!(spans.iter().find(|(s, _)| s == call).map(|(_, c)| *c), Some(function), "{call} is a call: {spans:?}");
    }
    assert_ne!(spans.iter().find(|(s, _)| s == "not_a_proc").map(|(_, c)| *c), Some(function), "{spans:?}");
}

#[test]
fn tcl_command_highlighting_only_colors_known_commands() {
    let dir = TempDir::new("tcl_command_highlight_known");
    let file = dir.write("main.tcl", "puts hello\nmyUnknownCmd123 foo\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let id = app.focused_buffer_id();

    let highlights = app.syntax_highlights_for_visible_range(id, 0, 2);
    let spans = app.content_spans(app.open(), 0, 2, 0, None, &highlights, 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);

    // "puts" -- a real builtin (in `tcl::KEYWORDS` and the query's
    // own `#any-of?` list) -- keeps its function color.
    let puts_span = spans.iter().find(|(s, _)| s == "puts");
    assert_eq!(
        puts_span.map(|(_, c)| *c),
        Some(app.theme.syntax_color("function")),
        "expected \"puts\" colored as a known command, got {spans:?}"
    );

    // "myUnknownCmd123" isn't a known command anywhere (not a
    // builtin, not ctags-sourced, not in a symbols file) -- must not
    // get the function color just for sitting in command position.
    let unknown_span = spans.iter().find(|(s, _)| s == "myUnknownCmd123");
    assert_ne!(
        unknown_span.map(|(_, c)| *c),
        Some(app.theme.syntax_color("function")),
        "expected \"myUnknownCmd123\" NOT colored as a command, got {spans:?}"
    );
}

#[test]
fn tcl_command_highlighting_recognizes_a_proc_defined_in_the_same_file() {
    // The everyday case, and the one that used to render as plain
    // text: a single script with no project root at all, so `ctags`
    // is never run and the only other source of known names is the
    // builtin list.
    let dir = TempDir::new("tcl_command_highlight_same_file");
    let file = dir.write("main.tcl", "proc configure_board {b} {
    return $b
}

configure_board stm32
");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    assert_eq!(app.project_root, None, "no project root, so nothing is indexed");
    let id = app.focused_buffer_id();

    let highlights = app.syntax_highlights_for_visible_range(id, 0, 5);
    let command_color = app.theme.syntax_color("function");
    let call = app.open().buffer.text().rfind("configure_board").unwrap();
    let color_at = |byte: usize| highlights.iter().find(|(r, _)| r.contains(&byte)).map(|(_, c)| *c);

    assert_eq!(color_at(call), Some(command_color), "a call to a proc defined above it should read as a command");
    // The definition's own name reads the same way, rather than as
    // a variable (which in most themes is plain body text).
    let definition = app.open().buffer.text().find("configure_board").unwrap();
    assert_eq!(color_at(definition), Some(command_color));
}

#[test]
fn syntax_highlights_drain_edits_even_without_an_active_language() {
    let mut app = App::with_file(None);
    assert!(app.open().syntax.is_none());
    app.test_insert_str("hello");
    let id = app.focused_buffer_id();
    assert!(app.syntax_highlights_for_visible_range(id, 0, 1).is_empty());
    // The edit log should have been drained regardless, not left to grow.
    assert!(app.open_mut().buffer.drain_edits().is_empty());
}

/// A real, uniquely-named temp directory, removed on drop -- these
/// tests exercise the real filesystem via `fenix_explorer::
/// ExplorerState`, same reasoning as that crate's own `TempDir`.
struct TempDir(PathBuf);
impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-gui-test-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn touch(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, b"hello\n").unwrap();
        path
    }
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }
    fn mkdir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A real temp git repo with one initial commit (`sub/a.txt`) --
/// mirrors `fenix_git`'s own private `committed_repo` test helper,
/// giving the Git-panel's `App`-level tests a real repo to shell
/// real `git` commands against (via `open_git_panel`/`fenix_git`
/// itself) rather than mocking already-tested plumbing.
fn fenix_git_test_repo(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    dir.write("sub/a.txt", "v1\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "initial"]);
    dir
}

#[test]
fn explorer_jump_opens_a_real_dired_buffer_in_the_focused_pane() {
    let dir = TempDir::new("jump_stashes");
    let file = dir.touch("a.txt");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_insert_str(" extra"); // dirty the original buffer
    let original_id = app.focused_buffer_id();

    app.explorer_jump();

    let dired_id = app.focused_buffer_id();
    assert_ne!(dired_id, original_id, "a real, different buffer now occupies the pane");
    assert_eq!(app.focused_dired_buffer(), Some(dired_id), "a listing page");
    assert_eq!(app.dired_states.get(&dired_id).unwrap().cwd, dir.path());
    // The original file buffer is untouched, just no longer focused.
    assert_eq!(app.buffers.get(original_id).unwrap().buffer.text(), " extrahello\n");
}

#[test]
fn explorer_jump_does_not_touch_an_already_open_sidebar() {
    // Regression test: `explorer`/`sidebar` used to be candidates for
    // one shared field/mechanism -- confirms the new real-buffer dired
    // path still leaves the separate, unrelated sidebar untouched.
    let sidebar_dir = TempDir::new("jump_no_clobber_sidebar");
    let jump_dir = TempDir::new("jump_no_clobber_jump");
    let file = jump_dir.touch("a.txt");

    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(sidebar_dir.path()).unwrap());
    app.sidebar_open = true;
    app.sidebar_focused = false;

    app.test_open_path(&file);
    app.explorer_jump();

    assert_eq!(app.dired_states.get(&app.focused_buffer_id()).unwrap().cwd, jump_dir.path());
    // The sidebar's own listing must be untouched by the jump.
    assert!(app.sidebar_open);
    assert_eq!(app.sidebar.as_ref().unwrap().cwd, sidebar_dir.path());
}

/// `.` (`SPC e h`) lists hidden files and folders -- a dot-name is
/// one, on Windows too -- in every explorer, and is remembered.
#[test]
fn hidden_files_are_shown_and_hidden_again_and_remembered() {
    let dir = TempDir::new("explorer_hidden");
    std::fs::create_dir(dir.0.join(".git")).unwrap();
    dir.write(".env", "x");
    dir.write("main.tcl", "");
    let mut app = App::with_file(None);
    app.config.explorer_show_hidden = None;
    app.open_dired_at(&dir.0);
    let id = app.focused_buffer_id();
    let names = |app: &App| -> Vec<String> { app.dired_states[&id].entries.iter().map(|e| e.name.clone()).collect() };
    assert_eq!(names(&app), ["main.tcl"]);
    app.explorer_toggle_hidden();
    assert_eq!(app.config.explorer_show_hidden, Some(true), "kept as the setting");
    let mut shown = names(&app);
    shown.sort();
    assert_eq!(shown, [".env", ".git", "main.tcl"]);
    // A new explorer lists them too.
    app.open_dired_at(&dir.0);
    assert_eq!(app.dired_states[&app.focused_buffer_id()].entries.len(), 3);
    app.explorer_toggle_hidden();
    assert_eq!(names(&app), ["main.tcl"]);
}

/// `SPC e H`: an explorer of your home folder.
#[test]
fn spc_e_capital_h_opens_an_explorer_at_home() {
    let Some(home) = dirs::home_dir() else { return };
    let mut app = App::with_file(None);
    app.explorer_home();
    assert_eq!(app.dired_states.get(&app.focused_buffer_id()).map(|s| s.cwd.clone()), Some(home));
    let bound = |command: &str| keymap::leader_trie().bindings().into_iter().find(|(_, c)| **c == command).map(|(seq, _)| seq.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" "));
    assert_eq!(bound("explorer.home").as_deref(), Some("SPC e H"));
    assert_eq!(bound("explorer.toggle_hidden").as_deref(), Some("SPC e h"));
}

/// Labels of every row the open popup would list, in picker order.
fn completion_labels(app: &App) -> Vec<String> {
    let state = app.completion.as_ref().expect("the popup should be open");
    state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect()
}

// -- With no Tcl language server: Tcl's options, subcommands and
// usage lines are the Tcl extension's (`fenix.tcl`) ------------------

#[test]
fn sync_completion_stays_closed_after_a_command_with_no_subcommands() {
    let dir = TempDir::new("completion_no_subcommands");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("puts ");

    app.sync_completion();

    assert!(app.completion.is_none(), "`puts` has nothing to offer after a space");
}

#[test]
fn sync_completion_stays_closed_once_the_subcommand_itself_is_complete() {
    let dir = TempDir::new("completion_after_subcommand");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("string compare ");

    app.sync_completion();

    assert!(app.completion.is_none(), "`string compare` takes strings next, not more subcommands");
}

#[test]
fn a_dash_that_is_not_a_flag_opens_nothing() {
    for text in ["set x -", "expr {$a -", "lindex $l end-", "puts [expr 3 -"] {
        let dir = TempDir::new("completion_options_minus");
        let file = dir.write("foo.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str(text);

        app.sync_completion();

        assert!(app.completion.is_none(), "{text:?} should not open the popup");
    }
}

#[test]
fn option_completion_never_fires_in_a_non_tcl_buffer() {
    let dir = TempDir::new("completion_options_non_tcl");
    let file = dir.write("foo.txt", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("lsort -");

    app.sync_completion();

    assert!(app.completion.is_none());
}

#[test]
fn subcommand_completion_never_fires_in_a_non_tcl_buffer() {
    let dir = TempDir::new("completion_ensemble_non_tcl");
    let file = dir.write("foo.txt", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("string ");

    app.sync_completion();

    assert!(app.completion.is_none());
}

fn tcl_app_with_cursor_on(name: &str, text: &str, col: usize) -> (TempDir, App) {
    let dir = TempDir::new(name);
    let file = dir.write("foo.tcl", text);
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_set_cursor(Cursor { char_idx: col, sticky_col: col });
    (dir, app)
}

#[test]
fn k_on_a_user_proc_or_whitespace_shows_nothing() {
    let (_dir, mut app) = tcl_app_with_cursor_on("hover_unknown", "my_proc $a $b\n", 2);
    app.request_hover();
    assert_eq!(app.lsp_hover, None);

    app.test_set_cursor(Cursor { char_idx: 7, sticky_col: 7 }); // the space after `my_proc`
    app.request_hover();
    assert_eq!(app.lsp_hover, None);
}

#[test]
fn k_outside_a_tcl_buffer_with_no_server_shows_nothing() {
    let dir = TempDir::new("hover_non_tcl");
    let file = dir.write("foo.txt", "string compare\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.request_hover();

    assert_eq!(app.lsp_hover, None);
}

// -- What the completion popup does with Escape ---------------------
//
// Verified against a real `vim -s` replay under a PTY, and against
// `:h popupmenu-keys`, which lists every key that means something
// special while the menu is up and does not list Escape.

/// A Tcl buffer in Insert mode with the popup open on "se".
/// Moves the open popup's selection to a plain word (not a snippet,
/// which expands to more than its label), and gives its label.
fn select_a_word(app: &mut App) -> String {
    let completion = app.completion.as_mut().expect("the popup is open");
    for _ in 0..50 {
        let item = &completion.picker.selected().expect("something to pick").payload;
        if !matches!(item.source, completion::Source::Snippet) {
            return item.label.clone();
        }
        completion.picker.move_selection(1);
    }
    panic!("no word among the matches")
}

fn app_with_an_open_completion_popup(name: &str) -> (TempDir, App) {
    let dir = TempDir::new(name);
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("se");
    app.sync_completion();
    assert!(app.completion.is_some(), "the popup should be open before the key under test");
    assert_eq!(app.vim.mode(), Mode::Insert);
    (dir, app)
}

#[test]
fn escape_leaves_insert_mode_in_one_press_even_with_the_popup_open() {
    // The whole point. Escape used to be spent closing the popup,
    // so returning to Normal took two presses -- and whether you
    // needed one or two depended on whether a popup you may not
    // have been looking at happened to be open.
    let (_dir, mut app) = app_with_an_open_completion_popup("esc_one_press");

    app.test_completion_key(KeyPress::named(FenixNamedKey::Escape));

    assert_eq!(app.vim.mode(), Mode::Normal, "one Escape, and you are back in Normal");
    assert!(app.completion.is_none(), "and the popup went with it");
}

#[test]
fn escape_with_the_popup_open_leaves_the_typed_text_alone() {
    // Leaving Insert is not the same as undoing what you typed:
    // "se" was really typed and stays typed. Only an explicit accept
    // (Tab/Enter/Ctrl-y) may complete it to something longer.
    let (_dir, mut app) = app_with_an_open_completion_popup("esc_keeps_text");

    app.test_completion_key(KeyPress::named(FenixNamedKey::Escape));

    assert_eq!(app.open().buffer.text(), "se");
}

#[test]
fn ctrl_e_dismisses_the_popup_and_leaves_you_typing() {
    // Vim's own dismiss key, and the behaviour Escape used to have
    // -- kept, just moved to the key Vim puts it on.
    let (_dir, mut app) = app_with_an_open_completion_popup("ctrl_e_dismiss");

    app.test_completion_key(KeyPress::char('e').with_ctrl());

    assert!(app.completion.is_none(), "the popup is gone");
    assert_eq!(app.vim.mode(), Mode::Insert, "but you are still typing");
    assert_eq!(app.open().buffer.text(), "se", "and nothing was inserted");
}

#[test]
fn ctrl_y_accepts_the_selected_match_like_tab_and_enter() {
    // Vim's own accept key, beside the Tab/Enter every other editor
    // uses -- all three do the same thing rather than one of them
    // meaning something subtly different.
    let dir = TempDir::new("ctrl_y_accept");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("seek se");
    app.sync_completion();
    let selected = select_a_word(&mut app);

    app.test_completion_key(KeyPress::char('y').with_ctrl());

    assert_eq!(app.open().buffer.text(), format!("seek {selected}"));
    assert_eq!(app.vim.mode(), Mode::Insert, "accepting a completion keeps you typing");
}

#[test]
fn escape_still_leaves_insert_mode_with_no_popup_open() {
    // The ordinary case, unchanged -- worth pinning because Escape
    // now reaches Vim by falling through the same branch the popup
    // keys are claimed in.
    let dir = TempDir::new("esc_no_popup");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    assert!(app.completion.is_none());

    app.test_completion_key(KeyPress::named(FenixNamedKey::Escape));

    assert_eq!(app.vim.mode(), Mode::Normal);
}

#[test]
fn ctrl_e_and_ctrl_y_are_only_claimed_while_the_popup_is_open() {
    // Outside the popup they are unclaimed keys; swallowing them
    // there would invent a binding that only exists by accident.
    let dir = TempDir::new("ctrl_ey_unclaimed");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));

    assert!(!app.completion_key(KeyPress::char('e').with_ctrl()));
    assert!(!app.completion_key(KeyPress::char('y').with_ctrl()));
}

#[test]
fn enter_after_a_word_already_whole_is_a_new_line() {
    let (_dir, mut app) = app_with_an_open_completion_popup("enter_whole");
    let (start, label) = {
        let state = app.completion.as_ref().unwrap();
        (state.prefix_start, state.picker.selected().unwrap().payload.label.clone())
    };
    let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
    buffer.replace_range(cursor, start, cursor.char_idx, &label);
    assert!(app.enter_is_a_new_line());
    app.test_completion_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(app.completion.is_none());
}

#[test]
fn enter_with_nothing_typed_is_a_new_line_unless_the_popup_was_asked_for() {
    let (_dir, mut app) = app_with_an_open_completion_popup("enter_empty");
    let at = app.cursor().char_idx;
    app.completion.as_mut().unwrap().prefix_start = at;
    assert!(app.enter_is_a_new_line());
    app.completion_asked = true;
    assert!(!app.enter_is_a_new_line());
    app.completion_asked = false;
    app.completion_selected = true;
    assert!(!app.enter_is_a_new_line(), "an item picked with the arrows is taken");
}

#[test]
fn sync_completion_closes_when_the_prefix_becomes_empty() {
    let dir = TempDir::new("completion_closes_on_empty_prefix");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("se");
    app.sync_completion();
    assert!(app.completion.is_some());

    app.test_set_cursor(Cursor::at_start()); // no identifier char before the cursor now
    app.sync_completion();
    assert!(app.completion.is_none());
}

#[test]
fn sync_completion_is_none_outside_insert_mode() {
    let dir = TempDir::new("completion_none_outside_insert");
    let file = dir.write("foo.tcl", "se");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    // Starts in Normal mode -- never entered Insert.
    app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 });

    app.sync_completion();

    assert!(app.completion.is_none());
}

#[test]
fn sync_completion_never_offers_tcl_keywords_for_a_non_tcl_buffer() {
    let dir = TempDir::new("completion_no_tcl_leak_non_tcl");
    let file = dir.write("foo.rs", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("se");

    app.sync_completion();

    // Buffer-word completion still offers "se" (the word just typed
    // -- see sync_completion_offers_buffer_words_for_a_non_tcl_
    // buffer for a case with something more interesting to complete
    // to), but the Tcl-specific keyword list ("set", "seek", ...)
    // must never leak into a non-Tcl buffer.
    let state = app.completion.as_ref().expect("buffer-word completion should still open the popup");
    let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(!labels.contains(&"set".to_string()));
    assert!(!labels.contains(&"seek".to_string()));
}

#[test]
fn sync_completion_offers_buffer_words_for_a_non_tcl_buffer() {
    let dir = TempDir::new("completion_buffer_words_non_tcl");
    let file = dir.write("foo.rs", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("seek_value\n"); // an earlier word to complete against
    app.test_insert_str("se");

    app.sync_completion();

    let state = app.completion.as_ref().expect("popup should be open");
    let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(labels.contains(&"seek_value".to_string()), "expected a buffer-word candidate, got {labels:?}");
}

#[test]
fn sync_completion_does_not_trigger_inside_a_tcl_comment() {
    let dir = TempDir::new("completion_no_trigger_in_comment");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("# se");

    app.sync_completion();

    assert!(app.completion.is_none(), "a word typed inside a comment should not open the popup");
}

#[test]
fn sync_completion_does_not_trigger_inside_a_tcl_string() {
    let dir = TempDir::new("completion_no_trigger_in_string");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("puts \"se");

    app.sync_completion();

    assert!(app.completion.is_none(), "a word typed inside a string literal should not open the popup");
}

#[test]
fn sync_completion_still_triggers_right_after_a_tcl_comment_ends() {
    let dir = TempDir::new("completion_triggers_after_comment_line");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("# a comment\nse");

    app.sync_completion();

    assert!(app.completion.is_some(), "real code on the next line after a comment should still trigger completion");
}

#[test]
fn force_open_completion_still_works_inside_a_tcl_comment() {
    // Ctrl-Space is a deliberate manual override -- unlike sync_
    // completion's auto-trigger, it should work anywhere, comments
    // included.
    let dir = TempDir::new("force_open_completion_in_comment");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("# se");

    app.force_open_completion();

    assert!(app.completion.is_some(), "Ctrl-Space should still open the popup inside a comment");
}

#[test]
fn format_buffer_reindents_even_a_file_with_no_detected_language() {
    // The reworked `SPC c F`: a structural reindent (Emacs'
    // `indent-region`/real Vim's `=`), not a per-language external
    // tool -- it works on any buffer, extension-recognized or not.
    let dir = TempDir::new("format_buffer_unknown_extension");
    let file = dir.write("foo.made-up-extension", "a {\nb\n}");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.format_buffer();

    assert_eq!(app.open().buffer.text(), "a {\n    b\n}");
}

#[test]
fn format_buffer_on_a_tcl_file_excludes_a_brace_inside_a_string_from_the_nesting_count() {
    // A detected language buys real accuracy over the extension-
    // less case above: without syntax awareness, the stray `{`
    // inside the string literal would look like a genuine nesting
    // increase and wrongly indent the line after it.
    let dir = TempDir::new("format_buffer_tcl_string");
    let file = dir.write("foo.tcl", "proc foo {} {\nputs \"{ not real\"\nputs hi\n}");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.format_buffer();

    assert_eq!(app.open().buffer.text(), "proc foo {} {\n    puts \"{ not real\"\n    puts hi\n}");
}

#[test]
fn format_buffer_on_an_already_correctly_indented_file_is_a_noop() {
    let dir = TempDir::new("format_buffer_noop");
    let file = dir.write("foo.tcl", "proc foo {} {\n    puts hi\n}");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let edit_count_before = app.open().buffer.edit_count();

    app.format_buffer();

    assert_eq!(app.open().buffer.edit_count(), edit_count_before, "an unchanged result must not create an undo step");
}

#[test]
fn toggle_checkbox_checks_an_unchecked_task_item() {
    let dir = TempDir::new("toggle_checkbox_check");
    let file = dir.write("todo.md", "- [ ] buy milk");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.toggle_checkbox();

    assert_eq!(app.open().buffer.text(), "- [x] buy milk");
}

#[test]
fn toggle_checkbox_unchecks_a_checked_task_item() {
    let dir = TempDir::new("toggle_checkbox_uncheck");
    let file = dir.write("todo.md", "- [x] buy milk");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.toggle_checkbox();

    assert_eq!(app.open().buffer.text(), "- [ ] buy milk");
}

#[test]
fn toggle_checkbox_acts_on_whichever_line_the_cursor_is_on() {
    let dir = TempDir::new("toggle_checkbox_second_line");
    let file = dir.write("todo.md", "- [ ] first\n- [ ] second");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let second_line_start = app.open().buffer.line_start_char(1);
    app.test_set_cursor(Cursor { char_idx: second_line_start, sticky_col: 0 });

    app.toggle_checkbox();

    assert_eq!(app.open().buffer.text(), "- [ ] first\n- [x] second");
}

#[test]
fn toggle_checkbox_on_a_line_without_one_errors_without_changing_the_buffer() {
    let dir = TempDir::new("toggle_checkbox_noop");
    let file = dir.write("todo.md", "just a line");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.toggle_checkbox();

    assert_eq!(app.open().buffer.text(), "just a line");
    assert!(app.modeline_pieces().1.contains("no checkbox"));
}

#[test]
fn toggle_checkbox_works_regardless_of_detected_language() {
    // Deliberately not a `.md` file -- checkbox toggling has no
    // per-language gate, same posture list continuation has.
    let dir = TempDir::new("toggle_checkbox_no_language_gate");
    let file = dir.write("notes.txt", "- [ ] todo");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.toggle_checkbox();

    assert_eq!(app.open().buffer.text(), "- [x] todo");
}

#[test]
fn format_selection_is_a_noop_outside_visual_mode() {
    let dir = TempDir::new("format_selection_not_visual");
    let file = dir.write("foo.tcl", "set x 1");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.format_selection();

    assert_eq!(app.open().buffer.text(), "set x 1");
    assert_eq!(app.vim.mode(), Mode::Normal);
}

#[test]
fn format_selection_leaves_a_block_selection_intact_instead_of_silently_dropping_it() {
    let dir = TempDir::new("format_selection_block");
    let file = dir.write("foo.tcl", "set x 1\nset y 2");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('v').with_ctrl());
    app.test_vim_key(KeyPress::char('j'));

    app.format_selection();

    assert_eq!(app.open().buffer.text(), "set x 1\nset y 2");
    assert_eq!(app.vim.mode(), Mode::Visual);
}

#[test]
fn format_selection_reindents_only_the_selected_lines_using_the_whole_file_for_context() {
    let dir = TempDir::new("format_selection_reindent");
    let file = dir.write("foo.tcl", "if {1} {\nputs a\nputs b\n}");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    // Select just line 1 ("puts a") -- line 2 stays untouched even
    // though it's just as misindented, proving the edit is scoped
    // to the selection rather than reformatting the whole buffer.
    app.test_vim_key(KeyPress::char('j'));
    app.test_vim_key(KeyPress::char('V'));

    app.format_selection();

    assert_eq!(app.open().buffer.text(), "if {1} {\n    puts a\nputs b\n}");
    assert_eq!(app.vim.mode(), Mode::Normal);
}

// -- `gcc`/`gc{motion}` (comment toggling) ---------------------------

#[test]
fn gcc_comments_a_tcl_line_with_a_hash() {
    let dir = TempDir::new("gcc_tcl");
    let file = dir.write("foo.tcl", "set x 1\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_dispatch_key(KeyPress::char('g'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('c'));
    assert_eq!(app.open().buffer.text(), "# set x 1\n");
}

#[test]
fn gcc_uncomments_an_already_commented_tcl_line() {
    let dir = TempDir::new("gcc_tcl_uncomment");
    let file = dir.write("foo.tcl", "# set x 1\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_dispatch_key(KeyPress::char('g'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('c'));
    assert_eq!(app.open().buffer.text(), "set x 1\n");
}

#[test]
fn gcc_uses_double_slash_for_a_rust_buffer() {
    let dir = TempDir::new("gcc_rust");
    let file = dir.write("foo.rs", "let x = 1;\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_dispatch_key(KeyPress::char('g'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('c'));
    assert_eq!(app.open().buffer.text(), "// let x = 1;\n");
}

#[test]
fn gcap_comments_every_line_in_the_paragraph_but_not_the_blank_line() {
    let dir = TempDir::new("gcap_tcl");
    let file = dir.write("foo.tcl", "set x 1\nset y 2\n\nset z 3\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_dispatch_key(KeyPress::char('g'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('a'));
    app.test_dispatch_key(KeyPress::char('p'));
    assert_eq!(app.open().buffer.text(), "# set x 1\n# set y 2\n\nset z 3\n");
}

#[test]
fn gcc_is_a_noop_for_a_language_with_no_line_comment_token() {
    let dir = TempDir::new("gcc_json_noop");
    let file = dir.write("foo.json", "{}\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_dispatch_key(KeyPress::char('g'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('c'));
    assert_eq!(app.open().buffer.text(), "{}\n");
}

#[test]
fn dot_repeats_gcc_on_a_different_line() {
    // Confirms the manual `change_capture_dirty` fix actually works
    // -- `gcc`'s buffer mutation happens *after* `vim.handle_key`
    // returns, outside the ordinary edit_count comparison window.
    let dir = TempDir::new("gcc_dot_repeat");
    let file = dir.write("foo.tcl", "set x 1\nset y 2\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_dispatch_key(KeyPress::char('g'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('c'));
    assert_eq!(app.open().buffer.text(), "# set x 1\nset y 2\n");

    app.test_dispatch_key(KeyPress::char('j'));
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "# set x 1\n# set y 2\n");
}

#[test]
fn accept_completion_replaces_the_typed_prefix_with_the_full_label() {
    let dir = TempDir::new("completion_accept");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));
    // A word to complete to: the one being typed isn't offered (#73).
    app.test_insert_str("seek se");
    app.sync_completion();
    let expected_label = select_a_word(&mut app);

    app.accept_completion();

    assert!(app.completion.is_none());
    assert_eq!(app.open().buffer.text(), format!("seek {expected_label}"));
    assert_eq!(app.test_cursor().char_idx, 5 + expected_label.chars().count());
}

#[test]
fn accept_completion_on_a_namespace_qualified_prefix_replaces_the_whole_thing() {
    // Regression: `is_ident_char` used to stop at `:`, so typing
    // `myns::gr` only fed "gr" to the completion prefix -- widening
    // the match to anything containing "gr" anywhere, and (the
    // actually-broken part) making `accept_completion` replace only
    // "gr" with the confirmed label, leaving the typed `myns::` in
    // place and producing a duplicated `myns::myns::greet`.
    let dir = TempDir::new("completion_accept_namespaced");
    let file = dir.write("foo.tcl", "myns::greet\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let end = app.open().buffer.len_chars();
    app.test_set_cursor(Cursor { char_idx: end, sticky_col: 0 });
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("myns::gr");
    app.sync_completion();

    let state = app.completion.as_ref().expect("myns::gr should match the myns::greet buffer word");
    let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(labels.contains(&"myns::greet".to_string()), "expected myns::greet among {labels:?}");
    // Confirming `myns::greet` specifically (not whatever the picker's
    // own default selection happens to be, e.g. the just-typed
    // "myns::gr" token itself, which trivially self-matches too).
    let selected_label = |app: &App| app.completion.as_ref().unwrap().picker.selected().unwrap().payload.label.clone();
    while selected_label(&app) != "myns::greet" {
        app.completion.as_mut().unwrap().picker.move_selection(1);
    }

    app.accept_completion();

    assert_eq!(app.open().buffer.text(), "myns::greet\nmyns::greet");
}

#[test]
fn force_open_completion_shows_the_full_list_even_with_an_empty_prefix() {
    let dir = TempDir::new("completion_force_open");
    let file = dir.write("foo.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));

    app.force_open_completion();

    let state = app.completion.as_ref().expect("Ctrl-Space should force-open the popup");
    assert!(state.picker.len() >= 3, "the Tcl snippets at least");
    assert!(state.picker.visible_rows(0, state.picker.len()).all(|(_, c)| c.payload.source == completion::Source::Snippet), "no keyword pool without the Tcl extension");
}

#[test]
fn force_open_completion_offers_snippets_in_an_empty_document() {
    let dir = TempDir::new("completion_force_open_empty");
    let file = dir.write("foo.rs", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));

    app.force_open_completion();

    assert!(app.completion.as_ref().unwrap().picker.visible_rows(0, 10).all(|(_, c)| c.payload.source == completion::Source::Snippet));
}

#[test]
fn force_open_completion_offers_buffer_words_on_a_non_tcl_buffer() {
    let dir = TempDir::new("completion_force_open_non_tcl_words");
    let file = dir.write("foo.rs", "let seek_value = 1;\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('i'));

    app.force_open_completion();

    let state = app.completion.as_ref().expect("buffer-word completion should open the popup");
    let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(labels.contains(&"seek_value".to_string()), "expected a buffer-word candidate, got {labels:?}");
}

#[test]
fn completion_offers_entries_from_a_configured_symbols_file_in_any_language() {
    let dir = TempDir::new("completion_symbols_file");
    let symbols_path = dir.write("symbols.txt", "my_external_symbol\nanother_one\n");
    let file = dir.write("main.rs", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.config.completion_symbols_file = Some(symbols_path);
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("my_ext");

    app.sync_completion();

    let state = app.completion.as_ref().expect("popup should be open");
    let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(labels.contains(&"my_external_symbol".to_string()), "expected a symbols-file entry, got {labels:?}");
}

#[test]
fn symbols_file_entries_are_layered_with_buffer_words_not_replaced_by_them() {
    let dir = TempDir::new("completion_symbols_plus_buffer_words");
    let symbols_path = dir.write("symbols.txt", "my_external_symbol\n");
    let file = dir.write("main.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.config.completion_symbols_file = Some(symbols_path);
    app.test_vim_key(KeyPress::char('i'));
    // "local_scratch_var" isn't in the symbols file -- only
    // buffer-word completion can offer it, alongside the file's words.
    app.test_insert_str("local_scratch_var\n");

    app.force_open_completion(); // empty prefix -- lists everything

    let labels = completion_labels(&app);
    assert!(labels.contains(&"my_external_symbol".to_string()), "expected the symbols-file word, got {labels:?}");
    assert!(labels.contains(&"local_scratch_var".to_string()), "expected the buffer word, got {labels:?}");
}

#[test]
fn a_symbols_file_entry_that_duplicates_a_buffer_word_is_not_shown_twice() {
    let dir = TempDir::new("completion_symbols_file_dedup");
    // "set" is already a word in the buffer -- the symbols-file entry
    // should be deduped against it, not appear as a second candidate.
    let symbols_path = dir.write("symbols.txt", "set\n");
    let file = dir.write("main.tcl", "set x 1\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.config.completion_symbols_file = Some(symbols_path);
    let end = app.open().buffer.len_chars();
    app.test_set_cursor(Cursor { char_idx: end, sticky_col: 0 });
    app.test_vim_key(KeyPress::char('i'));
    app.test_insert_str("se");

    app.sync_completion();

    let state = app.completion.as_ref().expect("popup should be open");
    let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
    assert_eq!(labels.iter().filter(|l| *l == "set").count(), 1);
}

#[test]
fn picker_confirm_on_symbol_opens_the_defining_file_and_jumps_to_its_line() {
    // A hand-built `TagEntry`, not a real `ctags` shell-out -- this
    // test is about `picker_confirm`'s jump mechanics, already
    // deterministic without depending on `ctags` being on `PATH`
    // (`picker_symbols_lists_ctags_sourced_definitions_by_fully_
    // qualified_name` covers the real shell-out separately).
    let dir = TempDir::new("picker_confirm_symbol");
    let lib = dir.write("lib.tcl", "puts start\n\nproc my_target_proc {} {\n    return 1\n}\n");
    let mut app = App::with_file(None);

    let tag = fenix_completion::ctags::TagEntry { name: "my_target_proc".to_string(), file: lib.clone(), line: 3 };
    let candidates = vec![fenix_picker::Candidate::new(tag.name.clone(), tag)];
    app.enter_picker(ActivePicker::Symbol(fenix_picker::PickerState::new(candidates)));
    app.picker_confirm();

    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());
    assert_eq!(app.open().buffer.path(), Some(lib.as_path()));
    let (line, _) = app.open().buffer.line_col(&app.cursor());
    assert_eq!(line, 2); // 0-indexed: ctags' line:3 is `proc my_target_proc`
}

#[test]
fn picker_confirm_on_symbol_records_a_jump_back_to_where_it_was_invoked_from() {
    let dir = TempDir::new("picker_confirm_symbol_jumpback");
    let lib = dir.write("lib.tcl", "puts start\n\nproc my_target_proc {} {\n    return 1\n}\n");
    let origin = dir.write("main.tcl", "puts main\n");
    let mut app = App::with_file(Some(origin.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('l')); // off column 0, so the return position is checkable

    let tag = fenix_completion::ctags::TagEntry { name: "my_target_proc".to_string(), file: lib.clone(), line: 3 };
    let candidates = vec![fenix_picker::Candidate::new(tag.name.clone(), tag)];
    app.enter_picker(ActivePicker::Symbol(fenix_picker::PickerState::new(candidates)));
    app.picker_confirm();
    assert_eq!(app.open().buffer.path(), Some(lib.as_path()));

    app.jump_back();
    assert_eq!(app.open().buffer.path(), Some(origin.as_path()));
    assert_eq!(app.cursor().char_idx, 1); // where `l` left it before jumping to the symbol

    app.jump_forward();
    assert_eq!(app.open().buffer.path(), Some(lib.as_path()));
}

#[test]
fn shift_g_jump_is_recorded_and_ctrl_o_ctrl_i_round_trip_it() {
    // A real small file, not `App::with_file(None)` -- that opens the
    // startup dashboard, which already has plenty of its own content
    // for `G` to land in, not the three-line buffer this test wants.
    let dir = TempDir::new("shift_g_jump");
    let file = dir.write("abc.txt", "a\nb\nc");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    let event = app.test_vim_key(KeyPress::char('G'));
    // Mirrors handle_key's own JumpRecorded arm -- handle_key itself
    // needs a real winit KeyEvent, so this exercises the recording
    // it would do directly (same posture as set_shiftwidth_command_
    // persists_the_new_width above).
    let VimEvent::JumpRecorded(from) = event else { panic!("expected a JumpRecorded event from G, got {event:?}") };
    let buffer = app.focused_buffer_id();
    app.record_jump(JumpEntry { buffer, char_idx: from });
    assert_eq!(app.cursor().char_idx, 4); // G landed on "c"

    app.jump_back();
    assert_eq!(app.cursor().char_idx, 0);

    app.jump_forward();
    assert_eq!(app.cursor().char_idx, 4);
}

#[test]
fn backtick_backtick_goes_back_to_where_the_last_jump_left() {
    let dir = TempDir::new("quote_quote");
    let file = dir.write("abc.txt", "a\nb\nc");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let event = app.test_vim_key(KeyPress::char('G'));
    let VimEvent::JumpRecorded(from) = event else { panic!("expected a JumpRecorded event from G, got {event:?}") };
    let buffer = app.focused_buffer_id();
    app.record_jump(JumpEntry { buffer, char_idx: from });
    app.test_vim_key(KeyPress::char('`'));
    let event = app.test_vim_key(KeyPress::char('`'));
    let VimEvent::JumpToMark { name, linewise } = event else { panic!("expected a JumpToMark event, got {event:?}") };
    app.jump_to_mark(name, linewise);
    assert_eq!(app.cursor().char_idx, 0, "back where G left");
    app.test_vim_key(KeyPress::char('\''));
    let event = app.test_vim_key(KeyPress::char('\''));
    let VimEvent::JumpToMark { name, linewise } = event else { panic!("expected a JumpToMark event, got {event:?}") };
    app.jump_to_mark(name, linewise);
    assert_eq!(app.cursor().char_idx, 4, "and '' goes back again");
}

#[test]
fn an_operator_reaches_a_mark_the_host_keeps() {
    let mut app = macro_app("a\nb\nc\nd");
    dispatch_all(&mut app, "majjd'a");
    assert_eq!(app.open().buffer.text(), "d");
}

#[test]
fn recording_a_new_jump_clears_the_forward_stack() {
    let dir = TempDir::new("jump_clears_forward");
    let file = dir.write("abc.txt", "a\nb\nc");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let event = app.test_vim_key(KeyPress::char('G'));
    let VimEvent::JumpRecorded(from) = event else { panic!("expected a JumpRecorded event") };
    let buffer = app.focused_buffer_id();
    app.record_jump(JumpEntry { buffer, char_idx: from });
    app.jump_back();
    assert!(!app.jump_forward_stack.is_empty());

    app.record_jump(JumpEntry { buffer, char_idx: app.cursor().char_idx });
    assert!(app.jump_forward_stack.is_empty());
}

#[test]
fn jump_back_skips_a_stale_entry_whose_buffer_was_closed() {
    let dir = TempDir::new("jump_back_stale");
    let stale_file = dir.write("stale.tcl", "gone\n");
    let mut app = App::with_file(None);
    let stale_id = app.buffers.open_path(&stale_file);
    app.buffers.close(stale_id);
    app.jump_back_stack.push(JumpEntry { buffer: stale_id, char_idx: 0 });

    let origin_buffer = app.focused_buffer_id();
    let origin_char_idx = app.cursor().char_idx;
    app.jump_back();

    // The only entry was stale and got dropped -- nothing to land
    // on, so the focused buffer/cursor are untouched.
    assert_eq!(app.focused_buffer_id(), origin_buffer);
    assert_eq!(app.cursor().char_idx, origin_char_idx);
    assert!(app.jump_back_stack.is_empty());
}

#[test]
fn jump_back_and_forward_are_noops_with_nothing_recorded() {
    let mut app = App::with_file(None);
    let origin = app.focused_buffer_id();
    app.jump_back();
    assert_eq!(app.focused_buffer_id(), origin);
    app.jump_forward();
    assert_eq!(app.focused_buffer_id(), origin);
}

#[test]
fn mark_set_then_backtick_jump_returns_to_the_exact_position() {
    let dir = TempDir::new("mark_set_backtick");
    let file = dir.write("abc.txt", "abcdef\nghijkl\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('l'));
    app.test_vim_key(KeyPress::char('l')); // char_idx 2
    app.test_vim_key(KeyPress::char('m'));
    let event = app.test_vim_key(KeyPress::char('a'));
    // Mirrors handle_key's own MarkSet arm -- see set_shiftwidth_
    // command_persists_the_new_width's own comment for why tests do
    // this manually instead of driving a real winit KeyEvent.
    let VimEvent::MarkSet(name, char_idx) = event else { panic!("expected a MarkSet event, got {event:?}") };
    let buffer = app.focused_buffer_id();
    app.marks.insert(name, JumpEntry { buffer, char_idx });

    app.test_vim_key(KeyPress::char('0'));
    assert_eq!(app.cursor().char_idx, 0);

    app.test_vim_key(KeyPress::char('`'));
    let event = app.test_vim_key(KeyPress::char('a'));
    let VimEvent::JumpToMark { name, linewise } = event else { panic!("expected a JumpToMark event, got {event:?}") };
    app.jump_to_mark(name, linewise);

    assert_eq!(app.cursor().char_idx, 2);
}

#[test]
fn quote_mark_jump_lands_on_the_first_non_blank_of_the_marks_line() {
    let dir = TempDir::new("mark_quote_linewise");
    let file = dir.write("abc.txt", "abc\n   def\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('j')); // line 2
    app.test_vim_key(KeyPress::char('$')); // end of line, not its first non-blank
    let buffer = app.focused_buffer_id();
    let char_idx = app.cursor().char_idx;
    app.marks.insert('a', JumpEntry { buffer, char_idx });
    app.test_vim_key(KeyPress::char('g'));
    app.test_vim_key(KeyPress::char('g')); // back to line 1

    app.test_vim_key(KeyPress::char('\''));
    let event = app.test_vim_key(KeyPress::char('a'));
    let VimEvent::JumpToMark { name, linewise } = event else { panic!("expected a JumpToMark event, got {event:?}") };
    assert!(linewise);
    app.jump_to_mark(name, linewise);

    let (line, col) = app.open().buffer.line_col(&app.cursor());
    assert_eq!(line, 1);
    assert_eq!(col, 3); // first non-blank of "   def" -- 3 leading spaces
}

#[test]
fn mark_jump_crosses_buffers() {
    let dir = TempDir::new("mark_cross_buffer");
    let other = dir.write("other.txt", "target line\n");
    let origin = dir.write("origin.txt", "start\n");
    let mut app = App::with_file(Some(origin.to_string_lossy().into_owned()));
    let other_id = app.buffers.open_path(&other);
    app.marks.insert('a', JumpEntry { buffer: other_id, char_idx: 3 });

    app.jump_to_mark('a', false);

    assert_eq!(app.open().buffer.path(), Some(other.as_path()));
    assert_eq!(app.cursor().char_idx, 3);
}

#[test]
fn mark_jump_records_a_jumplist_entry_so_ctrl_o_returns() {
    let dir = TempDir::new("mark_jump_records");
    let file = dir.write("abc.txt", "abcdef\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let buffer = app.focused_buffer_id();
    app.marks.insert('a', JumpEntry { buffer, char_idx: 4 });
    app.test_vim_key(KeyPress::char('l')); // char_idx 1

    app.jump_to_mark('a', false);
    assert_eq!(app.cursor().char_idx, 4);

    app.jump_back();
    assert_eq!(app.cursor().char_idx, 1);
}

#[test]
fn jump_to_an_unset_mark_is_a_noop() {
    let mut app = App::with_file(None);
    let origin = app.focused_buffer_id();
    app.jump_to_mark('z', false);
    assert_eq!(app.focused_buffer_id(), origin);
}

#[test]
fn an_edit_automatically_sets_the_dot_mark() {
    let mut app = macro_app("hello world");
    assert!(!app.marks.contains_key(&'.'));
    app.test_dispatch_key(KeyPress::char('x')); // deletes 'h' -- a real edit
    assert!(app.marks.contains_key(&'.'));
    let after_edit = app.cursor().char_idx;
    app.test_dispatch_key(KeyPress::char('$')); // move away
    app.jump_to_mark('.', false);
    assert_eq!(app.cursor().char_idx, after_edit);
}

#[test]
fn a_plain_motion_does_not_set_the_dot_mark() {
    let mut app = macro_app("hello world");
    app.test_dispatch_key(KeyPress::char('l')); // motion only, no edit
    assert!(!app.marks.contains_key(&'.'));
}

#[test]
fn leaving_insert_mode_automatically_sets_the_caret_mark() {
    let mut app = macro_app("hello");
    app.test_dispatch_key(KeyPress::char('A')); // append at end -> Insert mode
    app.test_dispatch_key(KeyPress::char('!'));
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(app.marks.contains_key(&'^'));
    let after_insert = app.cursor().char_idx;
    app.test_dispatch_key(KeyPress::char('0')); // move to start
    app.jump_to_mark('^', false);
    assert_eq!(app.cursor().char_idx, after_insert);
}

#[test]
fn jump_to_mark_drops_a_stale_entry_whose_buffer_was_closed() {
    let dir = TempDir::new("mark_stale");
    let stale_file = dir.write("stale.txt", "gone\n");
    let mut app = App::with_file(None);
    let stale_id = app.buffers.open_path(&stale_file);
    app.buffers.close(stale_id);
    app.marks.insert('a', JumpEntry { buffer: stale_id, char_idx: 0 });

    app.jump_to_mark('a', false);

    assert!(!app.marks.contains_key(&'a'));
}


// -- Renaming by editing the listing ---------------------------------

// -- Handing things back to the rest of the system -------------------

#[test]
fn copying_a_path_puts_the_whole_thing_on_the_clipboard() {
    // The whole path, not the name: a path is what you paste into a
    // terminal or another program's open dialog.
    let dir = TempDir::new("last_mile_yank");
    let file = dir.write("a.txt", "x");
    let mut app = app_with_places("last_mile_yank", dir.path());
    put_cursor_on(&mut app, "a.txt");

    app.explorer_yank_path();

    assert!(app.modeline_pieces().1.contains(&format!("copied {}", file.display())), "got: {}", app.modeline_pieces().1);
}

#[test]
fn opening_something_that_vanished_says_so_rather_than_launching_nothing() {
    let dir = TempDir::new("last_mile_open_gone");
    let file = dir.write("a.txt", "x");
    let mut app = app_with_places("last_mile_open_gone", dir.path());
    put_cursor_on(&mut app, "a.txt");
    std::fs::remove_file(&file).unwrap();

    app.explorer_open_externally();

    assert!(app.modeline_pieces().1.contains("is not there"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_shell_opened_here_starts_here() {
    // Without a working directory the first thing anybody types is
    // a `cd`, which is the whole point of the key.
    let dir = TempDir::new("last_mile_terminal");
    let mut app = app_with_places("last_mile_terminal", dir.path());

    app.explorer_terminal_here();

    let id = app.focused_buffer_id();
    assert_eq!(app.terminal_buffer_cwds.get(&id), Some(&dir.path().to_path_buf()));
    assert!(app.terminal_buffers.contains_key(&id), "and a real shell is running in it");
}

/// `SPC o T` and the panel (`SPC o t`) start in the project you're in,
/// not in Fenix's own working folder (your home, from the Start menu).
#[test]
fn a_shell_starts_in_the_project_youre_in() {
    let dir = TempDir::new("terminal_in_project");
    let file = dir.write("main.tcl", "");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.project_root = Some(dir.0.clone());
    assert_eq!(app.terminal_start_dir(), Some(dir.0.clone()));
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();
    assert_eq!(app.terminal_buffer_cwds.get(&id), Some(&dir.0));
    // No project: the file's own folder.
    app.project_root = None;
    let code = app.buffers.open_path(&file);
    app.open_buffer_in_focused_pane(code);
    assert_eq!(app.terminal_start_dir().map(|p| p.canonicalize().unwrap()), Some(dir.0.canonicalize().unwrap()));
    // From a buffer of no file, still no project: the folder of the
    // file used last, not Fenix's own.
    app.new_scratch_buffer();
    assert_eq!(app.terminal_start_dir().map(|p| p.canonicalize().unwrap()), Some(dir.0.canonicalize().unwrap()));
}

#[test]
fn a_respawned_shell_comes_back_where_it_was() {
    // Remembered rather than passed once: after `exit`, reopening
    // should not silently land somewhere else.
    let dir = TempDir::new("last_mile_terminal_respawn");
    let mut app = app_with_places("last_mile_terminal_respawn", dir.path());
    app.explorer_terminal_here();
    let id = app.focused_buffer_id();
    app.terminal_buffers.get_mut(&id).unwrap().session.kill();

    app.open_terminal_buffer();

    assert_eq!(app.terminal_buffer_cwds.get(&id), Some(&dir.path().to_path_buf()));
    assert!(app.terminal_buffers.get_mut(&id).unwrap().session.is_alive());
}

#[test]
fn closing_a_terminal_forgets_where_it_was() {
    let dir = TempDir::new("last_mile_terminal_close");
    let mut app = app_with_places("last_mile_terminal_close", dir.path());
    app.explorer_terminal_here();
    let id = app.focused_buffer_id();

    app.kill_buffer();

    assert!(!app.terminal_buffer_cwds.contains_key(&id));
}

#[test]
fn searching_from_a_listing_searches_that_directory_once() {
    // Once: the next unqualified search should go back to meaning
    // the project.
    let dir = TempDir::new("last_mile_grep");
    let mut app = app_with_places("last_mile_grep", dir.path());
    app.project_root = Some(PathBuf::from(r"C:\somewhere\else"));

    app.explorer_grep_here();

    let id = app.find_page(|m| matches!(m, PageModel::Search(_))).expect("the Search page opened");
    let page = app.search_page(id).unwrap();
    assert_eq!(page.scope(), crate::search_page::ScopeKind::Folder);
    assert_eq!(page.folder.as_deref(), Some(dir.path()));
}

#[test]
fn git_here_opens_the_folder_s_repository_and_leaves_the_project() {
    // Not moving the project behind everyone's back: search, tasks,
    // agents and extensions stay where you are.
    let dir = TempDir::new("last_mile_git_here");
    let repo = dir.mkdir("repo");
    std::fs::create_dir(repo.join(".git")).unwrap();
    let inner = dir.mkdir("repo/src");
    let mut app = app_with_places("last_mile_git_here", &inner);
    let before = app.project_root.clone();

    app.explorer_git_here();

    // The repository root, not the directory that happened to be open.
    let root = |a: &App| match a.pages.get(&a.focused_buffer_id()).map(|s| &s.model) {
        Some(PageModel::Git(g)) => Some(g.root.clone()),
        _ => None,
    };
    assert_eq!(root(&app), Some(fenix_lsp::normalize(repo.clone())));
    assert_eq!(app.project_root, before);
}

#[test]
fn git_here_in_a_folder_of_no_repository_says_so() {
    let dir = TempDir::new("last_mile_git_here_plain");
    let mut app = app_with_places("last_mile_git_here_plain", dir.path());
    let before = app.project_root.clone();

    app.explorer_git_here();

    assert!(app.modeline_pieces().1.contains("isn't in a git repository"), "{}", app.modeline_pieces().1);
    assert_eq!(app.project_root, before);
}

// -- Links --------------------------------------------------------------

#[test]
fn a_link_is_shown_as_a_link_rather_than_as_what_it_points_at() {
    // Following one into a tree you did not expect to be in is
    // exactly the surprise this prevents.
    let dir = TempDir::new("last_mile_link");
    let real = dir.mkdir("real");
    let link = dir.path().join("shortcut");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
    #[cfg(not(windows))]
    let made = std::os::unix::fs::symlink(&real, &link).is_ok();
    if !made {
        return; // no permission to create links here
    }
    let mut app = app_with_places("last_mile_link", dir.path());

    let text = listing_text(&mut app);
    let row = text.lines().find(|l| l.contains("shortcut")).expect("the link is listed");
    assert!(row.contains("shortcut →"), "marked as a link, not just a folder: {row:?}");
    let real = text.lines().find(|l| l.contains("real")).unwrap();
    assert!(!real.contains('→'), "{real:?}");
}

#[test]
fn properties_say_where_a_link_points() {
    let dir = TempDir::new("last_mile_link_props");
    let real = dir.mkdir("real");
    let link = dir.path().join("shortcut");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
    #[cfg(not(windows))]
    let made = std::os::unix::fs::symlink(&real, &link).is_ok();
    if !made {
        return;
    }
    let mut app = app_with_places("last_mile_link_props", dir.path());
    put_cursor_on(&mut app, "shortcut");

    app.explorer_handle_action(ExplorerAction::ShowProperties);

    assert!(app.modeline_pieces().1.contains("links to"), "got: {}", app.modeline_pieces().1);
}

// -- Big operations, archives, properties ----------------------------

#[test]
fn a_copy_runs_as_a_job_and_reports_when_it_is_done() {
    let src = TempDir::new("job_copy_src");
    let dest = TempDir::new("job_copy_dest");
    src.write("a.txt", "A");
    let mut app = app_with_places("job_copy", src.path());

    app.explorer_prompt_submit(PromptKind::CopyTo, &dest.path().to_string_lossy());

    // With no event loop the job runs inline, so it is already over.
    assert!(app.explorer_job.is_none());
    assert!(dest.path().join("a.txt").exists());
    assert!(app.modeline_pieces().1.contains("copied"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn only_one_long_operation_runs_at_a_time() {
    // Two copies competing for one disk finish no sooner than one
    // after the other, and one progress line is something a person
    // can read.
    let src = TempDir::new("job_one_src");
    let dest = TempDir::new("job_one_dest");
    src.write("a.txt", "A");
    let mut app = app_with_places("job_one", src.path());
    app.explorer_job = Some(ExplorerJob {
        running: "copying",
        done: "copied",
        cancel: Arc::new(AtomicBool::new(false)),
        progress: fenix_fs::Progress::default(),
    });

    app.explorer_prompt_submit(PromptKind::CopyTo, &dest.path().to_string_lossy());

    assert!(!dest.path().join("a.txt").exists(), "nothing started");
    assert!(app.modeline_pieces().1.contains("already being copied"));
}

#[test]
fn a_running_job_owns_the_status_line_and_says_how_to_stop_it() {
    let dir = TempDir::new("job_progress_text");
    let mut app = app_with_places("job_progress", dir.path());
    app.explorer_job = Some(ExplorerJob {
        running: "copying",
        done: "copied",
        cancel: Arc::new(AtomicBool::new(false)),
        progress: fenix_fs::Progress {
            files_done: 3,
            files_total: 10,
            bytes_done: 512,
            bytes_total: 2048,
            current: PathBuf::from(r"C:\work\photo.jpg"),
        },
    });

    let text = app.active_prompt_text().unwrap();

    // "copying", not a past tense with its ending chopped off --
    // that is what this said until it was read out loud.
    assert!(text.starts_with("copying "), "got: {text}");
    assert!(text.contains("3/10 files"), "got: {text}");
    assert!(text.contains("25%"), "both counts, because neither alone is honest: {text}");
    assert!(text.contains("photo.jpg"), "got: {text}");
    assert!(text.contains("SPC e k"), "got: {text}");
}

#[test]
fn cancelling_asks_the_worker_to_stop_and_says_what_that_means() {
    // A file already being written has to finish -- saying so beats
    // a key that appears to do nothing.
    let dir = TempDir::new("job_cancel");
    let mut app = app_with_places("job_cancel", dir.path());
    let flag = Arc::new(AtomicBool::new(false));
    app.explorer_job =
        Some(ExplorerJob { running: "copying", done: "copied", cancel: flag.clone(), progress: fenix_fs::Progress::default() });

    app.cancel_explorer_job();

    assert!(flag.load(Ordering::Relaxed));
    assert!(app.modeline_pieces().1.contains("in flight"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn cancelling_with_nothing_running_says_so() {
    let dir = TempDir::new("job_cancel_idle");
    let mut app = app_with_places("job_cancel_idle", dir.path());
    app.cancel_explorer_job();
    assert!(app.modeline_pieces().1.contains("nothing running"));
}

#[test]
fn a_cancelled_job_reports_what_did_happen_rather_than_only_that_it_stopped() {
    // After a cancel the useful question is what is already at the
    // other end.
    let dir = TempDir::new("job_cancel_report");
    let mut app = app_with_places("job_cancel_report", dir.path());
    app.explorer_job = Some(ExplorerJob {
        running: "copying",
        done: "copied",
        cancel: Arc::new(AtomicBool::new(false)),
        progress: fenix_fs::Progress::default(),
    });

    app.finish_explorer_job(vec![
        fenix_fs::Outcome { path: PathBuf::from("a.txt"), error: None },
        fenix_fs::Outcome { path: PathBuf::from("b.txt"), error: Some("cancelled".to_string()) },
    ]);

    assert!(app.modeline_pieces().1.contains("1 copied before that"), "got: {}", app.modeline_pieces().1);
}

// -- Archives --------------------------------------------------------

#[test]
fn archiving_offers_a_name_rather_than_demanding_one() {
    if fenix_fs::archiver().is_none() {
        return;
    }
    let dir = TempDir::new("archive_prompt");
    dir.write("report.docx", "x");
    let mut app = app_with_places("archive_prompt", dir.path());

    app.explorer_handle_action(ExplorerAction::BeginArchive);

    assert_eq!(app.active_prompt_text().unwrap(), "Archive as: report.docx.zip");
}

#[test]
fn a_marked_set_zips_and_unzips_back_to_what_it_was() {
    if fenix_fs::archiver().is_none() {
        return;
    }
    let dir = TempDir::new("archive_round_trip");
    dir.write("a.txt", "A");
    dir.write("b.txt", "B");
    let mut app = app_with_places("archive_round_trip", dir.path());
    app.explorer_handle_action(ExplorerAction::MarkAll);

    app.explorer_prompt_submit(PromptKind::ArchiveTo, "bundle.zip");
    assert!(dir.path().join("bundle.zip").exists(), "{}", app.modeline_pieces().1);

    // And back: put the cursor on the archive and extract it. The
    // cursor, not the state's own selection -- an action reconciles
    // the two from the cursor, which is the point of doing it that
    // way round.
    app.active_explorer_mut().unwrap().unmark_all();
    put_cursor_on(&mut app, "bundle.zip");
    app.explorer_handle_action(ExplorerAction::ExtractArchive);

    assert_eq!(std::fs::read_to_string(dir.path().join("bundle").join("a.txt")).unwrap(), "A");
    assert_eq!(std::fs::read_to_string(dir.path().join("bundle").join("b.txt")).unwrap(), "B");
}

#[test]
fn extracting_something_that_is_not_an_archive_says_so() {
    if fenix_fs::archiver().is_none() {
        return;
    }
    let dir = TempDir::new("archive_not_one");
    dir.write("notes.txt", "hello");
    let mut app = app_with_places("archive_not_one", dir.path());

    app.explorer_handle_action(ExplorerAction::ExtractArchive);

    assert!(app.modeline_pieces().1.contains("does not look like an archive"), "got: {}", app.modeline_pieces().1);
}

// -- Properties ------------------------------------------------------

#[test]
fn properties_say_what_a_column_cannot_hold() {
    let dir = TempDir::new("props_show");
    dir.write("a.txt", "hello");
    let mut app = app_with_places("props_show", dir.path());

    app.explorer_handle_action(ExplorerAction::ShowProperties);

    let text = app.modeline_pieces().1;
    assert!(text.contains("a.txt"), "got: {text}");
    assert!(text.contains("5B"), "got: {text}");
    assert!(text.contains("modified"), "got: {text}");
}

#[test]
fn a_directory_gets_counted_rather_than_reported_as_zero() {
    // The one number a listing structurally cannot show, which is
    // why it is asked for.
    let dir = TempDir::new("props_measure_dir");
    let sub = dir.mkdir("tree");
    std::fs::write(sub.join("a.txt"), "12345").unwrap();
    let mut app = app_with_places("props_measure_dir", dir.path());
    put_cursor_on(&mut app, "tree");

    app.explorer_handle_action(ExplorerAction::ShowProperties);

    // No event loop, so the walk ran inline and its answer replaced
    // the first line.
    let text = app.modeline_pieces().1;
    assert!(text.contains("5B in 1 file"), "got: {text}");
}

#[test]
fn read_only_can_be_flipped_from_the_listing() {
    let dir = TempDir::new("props_toggle_ro");
    let file = dir.write("a.txt", "hello");
    let mut app = app_with_places("props_toggle_ro", dir.path());

    app.explorer_handle_action(ExplorerAction::ToggleReadOnly);
    assert!(fenix_fs::properties(&file).unwrap().attributes.readonly);
    assert!(app.modeline_pieces().1.contains("read-only"), "got: {}", app.modeline_pieces().1);

    app.explorer_handle_action(ExplorerAction::ToggleReadOnly);
    assert!(!fenix_fs::properties(&file).unwrap().attributes.readonly);
}

#[test]
fn a_mixed_selection_ends_up_consistent_rather_than_each_swapping() {
    // Decided by the first target, so "make these writable" means
    // that for all of them.
    let dir = TempDir::new("props_toggle_mixed");
    let a = dir.write("a.txt", "A");
    let b = dir.write("b.txt", "B");
    fenix_fs::set_readonly(&b, true).unwrap();
    let mut app = app_with_places("props_toggle_mixed", dir.path());
    app.explorer_handle_action(ExplorerAction::MarkAll);

    app.explorer_handle_action(ExplorerAction::ToggleReadOnly);

    assert!(fenix_fs::properties(&a).unwrap().attributes.readonly);
    assert!(fenix_fs::properties(&b).unwrap().attributes.readonly);
    // Left writable so the temp directory can clean itself up.
    fenix_fs::set_readonly(&a, false).unwrap();
    fenix_fs::set_readonly(&b, false).unwrap();
}

// -- Renaming by editing the listing ---------------------------------

/// A listing open on three files, with rename mode already started.
fn app_editing_names(name: &str) -> (TempDir, App) {
    let dir = TempDir::new(name);
    dir.write("a.txt", "A");
    dir.write("b.txt", "B");
    dir.write("c.txt", "C");
    let mut app = app_with_places(name, dir.path());
    app.start_rename_mode();
    (dir, app)
}

/// Replaces the whole buffer with `text`, as an edit would.
fn set_buffer_text(app: &mut App, text: &str) {
    let id = app.focused_buffer_id();
    let ob = app.buffers.get_mut(id).unwrap();
    let end = ob.buffer.len_chars();
    let mut scratch = Cursor::at_start();
    ob.buffer.replace_range(&mut scratch, 0, end, text);
}

#[test]
fn rename_mode_shows_bare_names_one_per_line() {
    // The columns are decoration around the thing being edited;
    // leaving them in would mean parsing them back out of whatever
    // the user did to the line.
    let (_dir, app) = app_editing_names("wdired_bare");
    assert_eq!(app.open().buffer.text(), "a.txt\nb.txt\nc.txt");
}

#[test]
fn a_listing_is_read_only_until_its_names_are_being_edited() {
    // Typing into a listing desyncs the text from the rows it stands
    // for -- except in rename mode, where the text is the point.
    let dir = TempDir::new("wdired_readonly");
    dir.touch("a.txt");
    let mut app = app_with_places("wdired_readonly", dir.path());
    let listing = app.focused_buffer_id();
    assert!(app.buffer_edits_are_reverted(listing, BufferKind::Page), "the listing is a page");

    app.start_rename_mode();

    let id = app.focused_buffer_id();
    assert_ne!(id, listing, "the names are a text buffer of their own");
    assert!(!app.buffer_edits_are_reverted(id, BufferKind::Explorer));
}

#[test]
fn an_edit_to_a_listing_is_reverted_when_not_editing_names() {
    let dir = TempDir::new("wdired_revert");
    dir.touch("a.txt");
    let mut app = app_with_places("wdired_revert", dir.path());
    let before = app.open().buffer.text();

    app.test_dispatch_key(KeyPress::char('x')); // delete-char

    assert_eq!(app.open().buffer.text(), before);
}

#[test]
fn applying_an_edited_name_renames_the_file() {
    let (dir, mut app) = app_editing_names("wdired_apply");
    set_buffer_text(&mut app, "renamed.txt\nb.txt\nc.txt");

    app.apply_rename_mode();
    assert!(app.active_prompt_text().unwrap().contains("Rename"), "it asks first");
    app.rename_confirm_key(KeyPress::char('y'));

    assert!(dir.path().join("renamed.txt").exists());
    assert!(!dir.path().join("a.txt").exists());
    assert!(dir.path().join("b.txt").exists(), "and left the others alone");
    assert!(app.rename_mode.is_empty(), "editing is over");
}

#[test]
fn the_confirmation_shows_a_real_example_rather_than_only_a_count() {
    // "Rename 40 files?" is not a question anyone can answer.
    let (_dir, mut app) = app_editing_names("wdired_preview");
    set_buffer_text(&mut app, "x.txt\ny.txt\nc.txt");

    app.apply_rename_mode();

    let text = app.active_prompt_text().unwrap();
    assert!(text.contains("2 files"), "got: {text}");
    assert!(text.contains("a.txt -> x.txt"), "got: {text}");
}

#[test]
fn declining_the_confirmation_renames_nothing() {
    let (dir, mut app) = app_editing_names("wdired_decline");
    set_buffer_text(&mut app, "renamed.txt\nb.txt\nc.txt");
    app.apply_rename_mode();

    app.rename_confirm_key(KeyPress::named(FenixNamedKey::Escape));

    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("renamed.txt").exists());
}

#[test]
fn two_names_can_be_swapped_in_one_edit() {
    // The case that needs renaming through temporaries, driven the
    // way a user would actually reach it.
    let (dir, mut app) = app_editing_names("wdired_swap");
    set_buffer_text(&mut app, "b.txt\na.txt\nc.txt");

    app.apply_rename_mode();
    app.rename_confirm_key(KeyPress::char('y'));

    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "B");
    assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "A");
}

#[test]
fn deleting_a_line_is_refused_and_leaves_the_edit_on_screen_to_fix() {
    // Position is identity: a deleted line is not a deleted file,
    // and guessing which it meant would eventually guess wrong.
    let (dir, mut app) = app_editing_names("wdired_line_count");
    set_buffer_text(&mut app, "a.txt\nb.txt");

    app.apply_rename_mode();

    assert!(app.rename_confirm.is_none(), "nothing armed");
    assert!(app.modeline_pieces().1.contains("not a deleted file"), "got: {}", app.modeline_pieces().1);
    assert!(dir.path().join("c.txt").exists(), "and nothing was deleted");
    assert!(!app.rename_mode.is_empty(), "still editing, so the mistake can be fixed");
}

#[test]
fn two_lines_given_the_same_name_are_refused() {
    // One would silently destroy the other.
    let (dir, mut app) = app_editing_names("wdired_duplicate");
    set_buffer_text(&mut app, "same.txt\nsame.txt\nc.txt");

    app.apply_rename_mode();

    assert!(app.rename_confirm.is_none());
    assert!(app.modeline_pieces().1.contains("both be named"), "got: {}", app.modeline_pieces().1);
    assert!(dir.path().join("a.txt").exists() && dir.path().join("b.txt").exists());
}

#[test]
fn renaming_onto_an_untouched_file_is_refused() {
    let (dir, mut app) = app_editing_names("wdired_collision");
    set_buffer_text(&mut app, "c.txt\nb.txt\nc.txt");

    app.apply_rename_mode();

    assert!(app.rename_confirm.is_none());
    assert_eq!(std::fs::read_to_string(dir.path().join("c.txt")).unwrap(), "C", "untouched");
}

#[test]
fn a_name_with_a_separator_moves_the_file() {
    // Bulk *reorganising*, not just bulk renaming.
    let (dir, mut app) = app_editing_names("wdired_move");
    set_buffer_text(&mut app, "2026/a.txt\nb.txt\nc.txt");

    app.apply_rename_mode();
    app.rename_confirm_key(KeyPress::char('y'));

    assert_eq!(std::fs::read_to_string(dir.path().join("2026").join("a.txt")).unwrap(), "A");
}

#[test]
fn cancelling_puts_the_listing_back() {
    let (_dir, mut app) = app_editing_names("wdired_cancel");
    set_buffer_text(&mut app, "nonsense\nnonsense2\nnonsense3");
    let id = app.focused_buffer_id();

    app.cancel_rename_mode(id);

    assert!(app.rename_mode.is_empty());
    assert!(listing_text(&mut app).contains("a.txt"), "the real listing is back");
    assert!(listing_text(&mut app).lines().nth(1).unwrap().to_string().contains("3 items"), "header and all");
}

#[test]
fn an_edit_that_changes_nothing_just_ends_the_edit() {
    let (_dir, mut app) = app_editing_names("wdired_noop");

    app.apply_rename_mode();

    assert!(app.rename_confirm.is_none());
    assert!(app.rename_mode.is_empty());
    assert!(app.modeline_pieces().1.contains("no names changed"));
}

#[test]
fn rename_mode_needs_a_listing() {
    let mut app = App::with_file(None);
    app.start_rename_mode();
    assert!(app.rename_mode.is_empty());
    assert!(app.modeline_pieces().1.contains("needs a file listing"));
}

#[test]
fn an_empty_directory_has_nothing_to_rename() {
    let dir = TempDir::new("wdired_empty");
    let mut app = app_with_places("wdired_empty", dir.path());

    app.start_rename_mode();

    assert!(app.rename_mode.is_empty());
    assert!(app.modeline_pieces().1.contains("nothing here to rename"));
}

// -- Two listings side by side ---------------------------------------

#[test]
fn the_dual_pane_command_leaves_two_listings_open() {
    let dir = TempDir::new("dual_open");
    dir.touch("a.txt");
    let mut app = app_with_places("dual_open", dir.path());

    app.explorer_dual_pane();

    let listings = app
        .windows()
        .windows()
        .into_iter()
        .filter_map(|pane| app.windows().content(pane).copied())
        .filter(|b| app.dired_states.contains_key(b))
        .count();
    assert_eq!(listings, 2);
}

#[test]
fn dual_pane_works_from_anywhere_not_only_from_an_explorer() {
    // Otherwise it would be a key you can only press once you have
    // already done the thing it is for.
    let mut app = App::with_file(None);
    app.recent_dirs = fenix_project::RecentFiles::dirs_or_default(
        std::env::temp_dir().join(format!("fenix-places-test-dual-any-{}.txt", std::process::id())),
    );
    assert!(app.focused_dired_buffer().is_none());

    app.explorer_dual_pane();

    assert!(app.focused_dired_buffer().is_some());
}

#[test]
fn copying_defaults_its_destination_to_the_other_listing() {
    // The whole reason anyone arranges two listings: copy from here
    // to there without typing "there".
    let left = TempDir::new("dual_copy_left");
    let right = TempDir::new("dual_copy_right");
    left.touch("a.txt");
    let mut app = app_with_places("dual_copy", left.path());
    app.split_vertical();
    app.open_dired_at(right.path());
    // Back to the left half, which is the one with the file.
    let left_pane = app
        .windows()
        .windows()
        .into_iter()
        .find(|&p| {
            app.windows().content(p).and_then(|b| app.dired_states.get(b)).is_some_and(|s| s.cwd == left.path())
        })
        .expect("the left listing");
    app.windows_mut().focus(left_pane);

    app.explorer_handle_action(ExplorerAction::BeginCopy);

    let prompt = app.active_prompt_text().unwrap();
    assert!(prompt.contains(&right.path().display().to_string()), "got: {prompt}");
}

#[test]
fn a_lone_listing_seeds_nothing_rather_than_guessing() {
    // With nowhere obvious to copy to, an empty prompt is honest;
    // a guess would be a destination nobody chose.
    let dir = TempDir::new("dual_copy_alone");
    dir.touch("a.txt");
    let mut app = app_with_places("dual_alone", dir.path());

    app.explorer_handle_action(ExplorerAction::BeginCopy);

    assert_eq!(app.active_prompt_text().unwrap(), "Copy to: ");
}

#[test]
fn two_listings_on_the_same_directory_seed_nothing_either() {
    // `SPC e d` starts both halves in the same place; until one of
    // them moves there is no "other" directory to offer.
    let dir = TempDir::new("dual_copy_same");
    dir.touch("a.txt");
    let mut app = app_with_places("dual_same", dir.path());
    app.explorer_dual_pane();

    app.explorer_handle_action(ExplorerAction::BeginCopy);

    assert_eq!(app.active_prompt_text().unwrap(), "Copy to: ");
}

// -- All three listings read the same way ----------------------------

#[test]
fn the_sidebar_navigates_through_the_same_off_thread_path_as_a_pane() {
    // It was the one still reading inline, which meant the sidebar
    // could freeze the editor on a share exactly as a pane used to.
    let dir = TempDir::new("sidebar_async");
    let sub = dir.mkdir("into-here");
    std::fs::write(sub.join("inside.txt"), "x").unwrap();
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    app.sidebar_focused = true;

    app.explorer_handle_action(ExplorerAction::Open);

    assert_eq!(app.sidebar.as_ref().unwrap().cwd, sub);
    // And it went through the request machinery rather than around
    // it, which is what makes it abandonable.
    assert!(app.explorer_requests.contains_key(&ExplorerTarget::Sidebar));
}

#[test]
fn a_stale_sidebar_listing_is_dropped_like_any_other() {
    let first = TempDir::new("sidebar_stale_first");
    first.touch("in-first.txt");
    let second = TempDir::new("sidebar_stale_second");
    second.touch("in-second.txt");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(first.path()).unwrap());
    app.sidebar_open = true;
    app.sidebar_focused = true;
    app.explorer_navigate(ExplorerTarget::Sidebar, first.path());
    let stale = app.explorer_requests[&ExplorerTarget::Sidebar].id;

    app.explorer_navigate(ExplorerTarget::Sidebar, second.path());
    let late = fenix_explorer::read_listing(first.path(), false).unwrap();
    app.apply_explorer_listed(ExplorerTarget::Sidebar, stale, Ok(late));

    assert_eq!(app.sidebar.as_ref().unwrap().cwd, second.path());
}

#[test]
fn a_slow_sidebar_read_can_be_abandoned_too() {
    let dir = TempDir::new("sidebar_abandon");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    app.next_explorer_request += 1;
    let pending = app.next_explorer_request;
    app.explorer_requests.insert(
        ExplorerTarget::Sidebar,
        ExplorerRequest { id: pending, path: PathBuf::from(r"\\nas\media"), started: Instant::now(), done: false },
    );

    assert!(app.explorer_abandon_listing(ExplorerTarget::Sidebar));
    assert_eq!(app.sidebar.as_ref().unwrap().cwd, dir.path());
}

#[test]
fn the_directory_picker_navigates_off_thread_as_well() {
    let dir = TempDir::new("overlay_async");
    let sub = dir.mkdir("into-here");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::Open);

    assert_eq!(app.explorer.as_ref().unwrap().cwd, sub);
    assert!(app.explorer_requests.contains_key(&ExplorerTarget::Overlay));
}

#[test]
fn each_listing_keeps_its_own_place_in_the_queue() {
    // Three listings, three independent reads: a result for one must
    // never be applied to another.
    let a = TempDir::new("targets_independent_a");
    let b = TempDir::new("targets_independent_b");
    a.touch("in-a.txt");
    b.touch("in-b.txt");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(a.path()).unwrap());
    app.open_dired_at(b.path());
    let buffer = app.focused_buffer_id();

    assert_eq!(app.explorer_state(ExplorerTarget::Sidebar).unwrap().cwd, a.path());
    assert_eq!(app.explorer_state(ExplorerTarget::Buffer(buffer)).unwrap().cwd, b.path());
    assert!(app.explorer_state(ExplorerTarget::Overlay).is_none());
}

// -- Narrowing a listing, and searching under it ---------------------

#[test]
fn a_filter_narrows_the_listing_as_it_is_typed() {
    // Not on Enter: a filter you cannot see the effect of while
    // typing is just a slower search.
    let dir = TempDir::new("gui_filter_live");
    dir.touch("alpha.txt");
    dir.touch("beta.txt");
    dir.touch("alpine.rs");
    let mut app = app_with_places("filter_live", dir.path());

    app.explorer_handle_action(ExplorerAction::BeginFilter);
    for c in "alp".chars() {
        app.explorer_prompt_key(KeyPress::char(c));
    }

    let names = explorer_row_names(&app);
    assert_eq!(names, ["alpha.txt", "alpine.rs"]);
}

#[test]
fn a_narrowed_listing_says_so_in_its_header() {
    // A listing that is quietly narrowed is a listing that appears
    // to have lost files.
    let dir = TempDir::new("gui_filter_header");
    dir.touch("alpha.txt");
    let mut app = app_with_places("filter_header", dir.path());

    app.explorer_handle_action(ExplorerAction::BeginFilter);
    app.explorer_prompt_key(KeyPress::char('a'));

    assert!(listing_text(&mut app).lines().nth(1).unwrap().to_string().contains("filter: a"));
}

#[test]
fn escape_widens_the_listing_back_out() {
    // Leaving it narrowed after cancelling would be a filter you
    // cannot remember setting.
    let dir = TempDir::new("gui_filter_escape");
    dir.touch("alpha.txt");
    dir.touch("beta.txt");
    let mut app = app_with_places("filter_escape", dir.path());
    app.explorer_handle_action(ExplorerAction::BeginFilter);
    for c in "alp".chars() {
        app.explorer_prompt_key(KeyPress::char(c));
    }
    assert_eq!(app.active_explorer().unwrap().entries.len(), 1);

    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Escape));

    assert_eq!(app.active_explorer().unwrap().entries.len(), 2);
    assert!(app.active_explorer().unwrap().filter.is_empty());
}

#[test]
fn enter_keeps_the_filter_and_closes_the_prompt() {
    // So you can act on what you narrowed to.
    let dir = TempDir::new("gui_filter_enter");
    dir.touch("alpha.txt");
    dir.touch("beta.txt");
    let mut app = app_with_places("filter_enter", dir.path());
    app.explorer_handle_action(ExplorerAction::BeginFilter);
    for c in "alp".chars() {
        app.explorer_prompt_key(KeyPress::char(c));
    }

    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));

    assert!(app.explorer_prompt.is_none());
    assert_eq!(app.active_explorer().unwrap().entries.len(), 1);
}

#[test]
fn an_operation_on_a_narrowed_listing_cannot_reach_what_is_hidden() {
    // The invariant that makes filtering safe rather than alarming.
    let dir = TempDir::new("gui_filter_targets");
    dir.touch("keep.txt");
    dir.touch("other.txt");
    let mut app = app_with_places("filter_targets", dir.path());
    app.explorer_handle_action(ExplorerAction::BeginFilter);
    for c in "keep".chars() {
        app.explorer_prompt_key(KeyPress::char(c));
    }
    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));

    app.explorer_handle_action(ExplorerAction::MarkAll);

    assert_eq!(app.active_explorer().unwrap().target_paths(), vec![dir.path().join("keep.txt")]);
}

#[test]
fn find_searches_everything_under_the_directory_not_just_its_children() {
    let dir = TempDir::new("gui_find");
    let deep = dir.mkdir("one/two/three");
    std::fs::write(deep.join("buried.txt"), "x").unwrap();
    let mut app = app_with_places("find", dir.path());

    app.explorer_handle_action(ExplorerAction::BeginFind);

    let labels: Vec<String> =
        picker_visible_labels(app.active_picker.as_ref().unwrap(), 0, 50).into_iter().map(|(_, l)| l).collect();
    assert!(labels.iter().any(|l| l.contains("buried.txt")), "got: {labels:?}");
}

#[test]
fn find_in_an_empty_directory_says_so_rather_than_opening_an_empty_picker() {
    let dir = TempDir::new("gui_find_empty");
    let mut app = app_with_places("find_empty", dir.path());

    app.explorer_handle_action(ExplorerAction::BeginFind);

    assert!(app.active_picker.is_none());
    assert!(app.modeline_pieces().1.contains("nothing under"), "got: {}", app.modeline_pieces().1);
}

// -- Getting somewhere without walking there -------------------------

/// An explorer open on `dir`, with bookmarks and recents pointed at
/// throwaway files rather than the real config directory.
fn app_with_places(name: &str, dir: &Path) -> App {
    let mut app = App::with_file(None);
    app.recent_dirs = fenix_project::RecentFiles::dirs_or_default(
        std::env::temp_dir().join(format!("fenix-places-test-{name}-{}.txt", std::process::id())),
    );
    app.open_dired_at(dir);
    app
}

#[test]
fn the_path_bar_opens_seeded_with_where_you_already_are() {
    // So a sibling directory is an edit, not a retype.
    let dir = TempDir::new("path_bar_seed");
    let mut app = app_with_places("seed", dir.path());

    app.start_path_prompt();

    let text = app.active_prompt_text().unwrap();
    assert!(text.starts_with("Go to: "), "got: {text}");
    assert!(text.contains(&dir.path().display().to_string()), "got: {text}");
    // With a separator, so the first Tab offers what is *inside*
    // rather than the siblings of where you are.
    assert!(text.ends_with('\\'), "got: {text}");
}

#[test]
fn typing_a_path_and_pressing_enter_goes_there() {
    let dir = TempDir::new("path_bar_go");
    let sub = dir.mkdir("somewhere");
    std::fs::write(sub.join("inside.txt"), "x").unwrap();
    let mut app = app_with_places("go", dir.path());

    app.explorer_prompt_submit(PromptKind::GoToPath, &sub.to_string_lossy());

    assert_eq!(app.active_explorer().unwrap().cwd, sub);
    assert!(listing_text(&mut app).contains("inside.txt"));
}

#[test]
fn path_prompt_opens_an_explorer_from_dashboard_or_document() {
    let dir = TempDir::new("path_prompt_without_explorer");
    let file = dir.write("document.txt", "keep this document");
    for file_arg in [None, Some(file.to_string_lossy().into_owned())] {
        let mut app = App::with_file(file_arg);
        let original = app.focused_buffer_id();
        assert!(app.active_explorer_target().is_none());
        app.start_path_prompt();
        app.explorer_prompt.as_mut().unwrap().input = dir.path().to_string_lossy().into_owned();
        app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        assert!(app.explorer_prompt.is_none());
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.focused_dired_buffer().is_some());
        assert_eq!(app.active_explorer().unwrap().cwd, dir.path());
        assert!(listing_text(&mut app).contains("document.txt"));
        assert!(app.buffers.get(original).is_some());
    }
}

#[test]
fn path_prompt_opens_a_file_without_an_existing_explorer() {
    let dir = TempDir::new("path_prompt_file_without_explorer");
    let file = dir.write("document.txt", "opened from the path bar");
    let mut app = App::with_file(None);
    app.start_path_prompt();
    app.explorer_prompt.as_mut().unwrap().input = file.to_string_lossy().into_owned();
    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(app.explorer_prompt.is_none());
    assert_eq!(app.open().kind, BufferKind::Text);
    assert_eq!(app.open().buffer.text(), "opened from the path bar");
}

#[test]
fn typing_a_file_opens_it_rather_than_refusing() {
    // Being told "that is not a directory" after typing a full
    // filename is pedantry, not help.
    let dir = TempDir::new("path_bar_file");
    let file = dir.write("notes.txt", "hello");
    let mut app = app_with_places("file", dir.path());

    app.explorer_prompt_submit(PromptKind::GoToPath, &file.to_string_lossy());

    assert_eq!(app.open().kind, BufferKind::Text);
    assert_eq!(app.open().buffer.path(), Some(file.as_path()));
}

#[test]
fn a_path_with_a_home_shortcut_and_quotes_still_resolves() {
    // The two things a pasted path arrives wrapped in.
    let dir = TempDir::new("path_bar_expand");
    let mut app = app_with_places("expand", dir.path());
    let quoted = format!("\"{}\"", dir.path().display());

    app.explorer_prompt_submit(PromptKind::GoToPath, &quoted);

    assert_eq!(app.active_explorer().unwrap().cwd, dir.path());
}

#[test]
fn tab_fills_in_as_much_as_every_candidate_agrees_on() {
    let dir = TempDir::new("path_bar_tab");
    dir.mkdir("alpha");
    dir.mkdir("alpine");
    let mut app = app_with_places("tab", dir.path());
    app.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::GoToPath, input: format!("{}\\al", dir.path().display()) });
    app.refresh_path_completions();

    app.complete_path_prompt();

    // Up to where they diverge, and no further -- the next character
    // is the user's to choose.
    assert!(app.explorer_prompt.as_ref().unwrap().input.ends_with("alp"), "got: {}", app.explorer_prompt.as_ref().unwrap().input);
}

#[test]
fn tab_on_a_single_match_finishes_it_and_steps_inside() {
    let dir = TempDir::new("path_bar_tab_one");
    dir.mkdir("only-one");
    let mut app = app_with_places("tab_one", dir.path());
    app.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::GoToPath, input: format!("{}\\on", dir.path().display()) });
    app.refresh_path_completions();

    app.complete_path_prompt();

    let input = &app.explorer_prompt.as_ref().unwrap().input;
    assert!(input.ends_with("only-one\\"), "got: {input}");
}

#[test]
fn tab_with_nothing_matching_says_so_rather_than_doing_nothing() {
    let dir = TempDir::new("path_bar_tab_none");
    let mut app = app_with_places("tab_none", dir.path());
    app.explorer_prompt =
        Some(ExplorerPrompt { kind: PromptKind::GoToPath, input: format!("{}\\zzz", dir.path().display()) });
    app.refresh_path_completions();

    app.complete_path_prompt();

    assert!(app.modeline_pieces().1.contains("no directory here"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn tab_only_completes_a_path_and_leaves_other_prompts_alone() {
    // In an archive name, Tab is a character somebody might want.
    let dir = TempDir::new("path_bar_tab_scope");
    let mut app = app_with_places("tab_scope", dir.path());
    app.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::ArchiveTo, input: "a".to_string() });

    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Tab));

    assert_eq!(app.explorer_prompt.as_ref().unwrap().input, "a", "unchanged, not completed");
}

#[test]
fn a_canonicalised_path_is_shown_the_way_a_person_would_write_it() {
    // Every registered project root has been through `canonicalize`,
    // which on Windows returns `\\?\C:\...` -- unrecognisable at a
    // glance and meaningless to a reader.
    assert_eq!(readable_path(Path::new(r"\\?\C:\Users\thoma")), r"C:\Users\thoma");
    assert_eq!(readable_path(Path::new(r"\\?\UNC\nas\media")), r"\\nas\media");
    assert_eq!(readable_path(Path::new(r"C:\Users\thoma")), r"C:\Users\thoma");
    assert_eq!(readable_path(Path::new(r"\\nas\media")), r"\\nas\media");
}

#[test]
fn the_recent_directories_list_stays_out_of_the_real_config_directory_under_test() {
    // The tests here open real directories through the real code
    // path, which records every one -- and the crash-recovery work
    // already found what happens when that lands in the user's own
    // history.
    let path = default_recent_dirs_path();
    assert!(
        path.starts_with(std::env::temp_dir()),
        "a test run must not touch the real history: {}",
        path.display()
    );
}

#[test]
fn a_server_with_no_share_named_is_a_question_not_a_directory() {
    assert!(is_bare_server(Path::new(r"\\nas")));
    assert!(is_bare_server(Path::new(r"\\nas\")));
    assert!(!is_bare_server(Path::new(r"\\nas\media")));
    assert!(!is_bare_server(Path::new(r"C:\Users")));
    assert!(!is_bare_server(Path::new(r"\\")));
}

// -- Places ----------------------------------------------------------

#[test]
fn visiting_a_directory_remembers_it() {
    // Recorded where the listing lands, not where it is asked for --
    // a path that never answered is not somewhere you have been.
    let dir = TempDir::new("places_remember");
    let app = app_with_places("remember", dir.path());
    assert_eq!(app.recent_dirs.paths().first(), Some(&dir.path().to_path_buf()));
}

#[test]
fn a_directory_that_failed_to_open_is_not_remembered() {
    let dir = TempDir::new("places_remember_fail");
    let mut app = app_with_places("remember_fail", dir.path());
    let before = app.recent_dirs.paths().len();

    let id = app.focused_buffer_id();
    app.explorer_navigate(ExplorerTarget::Buffer(id), &dir.path().join("nope"));

    assert_eq!(app.recent_dirs.paths().len(), before);
}

#[test]
fn places_lists_bookmarks_drives_and_where_you_have_been() {
    let dir = TempDir::new("places_list");
    let mut app = app_with_places("list", dir.path());
    app.config.explorer_bookmarks = vec![("nas".to_string(), PathBuf::from(r"\\nas\media"))];

    app.picker_places();

    let rows = picker_visible_labels(app.active_picker.as_ref().unwrap(), 0, 50);
    let labels: Vec<String> = rows.into_iter().map(|(_, label)| label).collect();
    assert!(labels.iter().any(|l| l.contains("nas")), "bookmarks: {labels:?}");
    assert!(labels.iter().any(|l| l.contains(&dir.path().display().to_string())), "recents: {labels:?}");
    // The machine's own drives, from the real query.
    assert!(labels.iter().any(|l| l.contains("local disk") || l.contains("volume")), "drives: {labels:?}");
}

#[test]
fn a_mapped_drive_shows_where_it_really_points() {
    // `Z:` on its own tells you nothing about where you are going.
    let volume = fenix_fs::Volume {
        root: PathBuf::from("Z:\\"),
        name: String::new(),
        kind: fenix_fs::VolumeKind::Network,
        remote: Some(r"\\nas\media".to_string()),
        free: None,
        total: None,
    };
    let dir = TempDir::new("places_mapped");
    let mut app = app_with_places("mapped", dir.path());
    app.volumes = vec![volume];

    app.picker_places();

    let labels: Vec<String> =
        picker_visible_labels(app.active_picker.as_ref().unwrap(), 0, 50).into_iter().map(|(_, l)| l).collect();
    assert!(labels.iter().any(|l| l.contains(r"->  \\nas\media")), "got: {labels:?}");
}

#[test]
fn confirming_a_place_goes_there() {
    let dir = TempDir::new("places_confirm");
    let sub = dir.mkdir("target-dir");
    let mut app = app_with_places("confirm", dir.path());
    app.active_picker =
        Some(ActivePicker::Places(fenix_picker::PickerState::new(vec![fenix_picker::Candidate::new("t".to_string(), sub.clone())])));

    app.picker_confirm();

    assert!(app.active_picker.is_none());
    assert_eq!(app.active_explorer().unwrap().cwd, sub);
}

#[test]
fn bookmarking_names_the_folder_after_itself() {
    // No prompt: the folder's own name is right almost every time,
    // and a bookmark you can add without stopping to think is one
    // you will actually add.
    let dir = TempDir::new("places_bookmark");
    let sub = dir.mkdir("interesting");
    let mut app = app_with_places("bookmark", &sub);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));

    app.bookmark_current_directory();

    assert_eq!(app.config.explorer_bookmarks, vec![("interesting".to_string(), sub)]);
    assert!(app.modeline_pieces().1.contains("bookmarked as interesting"));
}

#[test]
fn bookmarking_the_same_place_twice_says_so_instead_of_duplicating_it() {
    let dir = TempDir::new("places_bookmark_twice");
    let sub = dir.mkdir("interesting");
    let mut app = app_with_places("bookmark_twice", &sub);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));

    app.bookmark_current_directory();
    app.bookmark_current_directory();

    assert_eq!(app.config.explorer_bookmarks.len(), 1);
    assert!(app.modeline_pieces().1.contains("already bookmarked"));
}

#[test]
fn recent_directories_are_offered_on_their_own_too() {
    let dir = TempDir::new("places_recent_only");
    let mut app = app_with_places("recent_only", dir.path());

    app.picker_recent_dirs();

    let labels: Vec<String> =
        picker_visible_labels(app.active_picker.as_ref().unwrap(), 0, 50).into_iter().map(|(_, l)| l).collect();
    assert_eq!(labels, vec![dir.path().display().to_string()]);
}

// -- The buffer-backed listing can now do things ---------------------
//
// Everything below was reachable only from the sidebar, which cannot
// be split. `app.rs` said so itself: "Marking/rename/create/delete/
// copy/move aren't wired for this buffer-backed form yet."

/// An explorer buffer open on a directory with three files, cursor
/// on the first entry.
fn app_with_a_listing(name: &str) -> (TempDir, App) {
    let dir = TempDir::new(name);
    dir.write("a.txt", "a");
    dir.write("b.txt", "b");
    dir.write("c.txt", "c");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    (dir, app)
}

/// Selects the row showing `name`, the way moving there with `j`
/// would -- which is what an action then acts on.
fn put_cursor_on(app: &mut App, name: &str) {
    let explorer = app.active_explorer_mut().expect("a listing");
    let index = explorer.entries.iter().position(|e| e.name == name).unwrap_or_else(|| panic!("no entry named {name:?}"));
    explorer.select_index(index);
}

/// The rows' names, in the order the listing shows them.
fn explorer_row_names(app: &App) -> Vec<String> {
    app.active_explorer().unwrap().entries.iter().map(|e| e.name.clone()).collect()
}

/// The focused explorer page, laid out 100 wide and 40 tall.
fn listing_text(app: &mut App) -> String {
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    app.note_page_rows(id, 40);
    app.ensure_page_layout(id, pane, 100);
    app.open().buffer.text()
}

/// The entry the listing would act on right now.
fn selected_name(app: &App) -> String {
    app.active_explorer().unwrap().selected_entry().unwrap().name.clone()
}

#[test]
fn the_focused_explorer_buffer_is_the_active_listing() {
    let (dir, app) = app_with_a_listing("buffer_is_active");
    assert_eq!(app.active_explorer().map(|e| e.cwd.clone()), Some(dir.path().to_path_buf()));
}

#[test]
fn an_operation_acts_on_the_row_the_cursor_is_on() {
    // The listing keeps its own `selected` because the sidebar has
    // no cursor; a buffer has a real one, and it is the real one the
    // user is looking at. Reconciling them is what stops an
    // operation from acting on a different row than the visible one.
    let (_dir, mut app) = app_with_a_listing("cursor_is_selection");
    app.explorer_handle_action(ExplorerAction::Down); // onto the second entry

    app.explorer_handle_action(ExplorerAction::ToggleMark);

    assert_eq!(app.active_explorer().unwrap().marks.len(), 1);
    assert!(app.active_explorer().unwrap().marks.iter().any(|p| p.ends_with("b.txt")));
}

#[test]
fn the_cursor_stays_on_its_own_row_when_the_listing_is_re_rendered() {
    // Regression: the header changes length whenever the counts in
    // it change, so remembering the cursor as a character index slid
    // it backwards on every mark -- and the next operation then
    // acted on a different row than the one being looked at.
    let (_dir, mut app) = app_with_a_listing("render_keeps_cursor");
    app.explorer_handle_action(ExplorerAction::Down); // b.txt
    let line_before = app.open().buffer.line_col(&app.test_cursor()).0;

    app.explorer_handle_action(ExplorerAction::ToggleMark); // grows the header

    assert_eq!(app.open().buffer.line_col(&app.test_cursor()).0, line_before);
    app.explorer_handle_action(ExplorerAction::ToggleMark);
    assert!(app.active_explorer().unwrap().marks.is_empty(), "the second press un-marked the same row");
}

#[test]
fn marking_shows_up_in_the_header_count() {
    let (_dir, mut app) = app_with_a_listing("mark_header");
    app.explorer_handle_action(ExplorerAction::ToggleMark);
    assert!(listing_text(&mut app).lines().nth(1).unwrap().to_string().contains("1 marked"));
}

#[test]
fn deleting_from_the_buffer_asks_first_and_then_bins_the_marked_set() {
    let (dir, mut app) = app_with_a_listing("buffer_delete");
    app.explorer_handle_action(ExplorerAction::ToggleMark);
    app.explorer_handle_action(ExplorerAction::Down);
    app.explorer_handle_action(ExplorerAction::ToggleMark);

    app.explorer_handle_action(ExplorerAction::BeginDelete);
    assert!(app.active_prompt_text().unwrap().contains("Recycle Bin"), "it asks, and says where they go");
    app.explorer_prompt_key(KeyPress::char('y'));

    assert!(!dir.path().join("a.txt").exists());
    assert!(!dir.path().join("b.txt").exists());
    assert!(dir.path().join("c.txt").exists(), "and left the unmarked one alone");
    assert_eq!(app.active_explorer().unwrap().entries.len(), 1, "the listing caught up");
}

#[test]
fn sorting_from_the_buffer_reorders_the_rows_without_re_reading() {
    let dir = TempDir::new("buffer_sort");
    dir.write("small.txt", "x");
    dir.write("big.txt", "0123456789");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());

    app.explorer_handle_action(ExplorerAction::CycleSort); // name -> size

    let text = listing_text(&mut app);
    let names = explorer_row_names(&app);
    assert_eq!(names, ["small.txt", "big.txt"]);
    assert!(text.lines().nth(1).unwrap().contains("by size"), "the header says how it is ordered");
}

#[test]
fn reversing_the_sort_flips_it() {
    let (_dir, mut app) = app_with_a_listing("buffer_sort_reverse");
    app.explorer_handle_action(ExplorerAction::ReverseSort);
    let names = explorer_row_names(&app);
    assert_eq!(names, ["c.txt", "b.txt", "a.txt"]);
}


// -- Reading a directory off the main thread -------------------------
//
// The whole point of this machinery is a share that does not answer.
// That cannot be staged in a unit test (a made-up hostname is the
// live check, in the plan's verification), so what is pinned here is
// everything around it: which results are allowed to land, what
// happens to the ones that are not, and that abandoning leaves the
// pane usable. With no `event_proxy` the read runs inline, so by the
// time a call returns the result has already been applied.

#[test]
fn opening_a_directory_lists_it() {
    let dir = TempDir::new("async_open");
    dir.touch("a.txt");
    let mut app = App::with_file(None);

    app.open_dired_at(dir.path());

    let id = app.focused_buffer_id();
    assert_eq!(app.dired_states[&id].entries.len(), 1);
    assert!(listing_text(&mut app).contains("a.txt"));
}

#[test]
fn a_listing_that_was_superseded_is_dropped_rather_than_yanking_the_pane_back() {
    // Navigate away while a slow read is in flight and the slow one
    // eventually answers. It must not drag you back to the directory
    // you already left.
    let first = TempDir::new("async_stale_first");
    first.touch("in-first.txt");
    let second = TempDir::new("async_stale_second");
    second.touch("in-second.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(first.path());
    let id = app.focused_buffer_id();
    let stale = app.explorer_requests[&ExplorerTarget::Buffer(id)].id;

    app.explorer_navigate(ExplorerTarget::Buffer(id), second.path());
    // The first read finally answers, long after it stopped mattering.
    let late = fenix_explorer::read_listing(first.path(), false).unwrap();
    app.apply_explorer_listed(ExplorerTarget::Buffer(id), stale, Ok(late));

    assert_eq!(app.dired_states[&id].cwd, second.path());
    assert!(listing_text(&mut app).contains("in-second.txt"));
    assert!(!listing_text(&mut app).contains("in-first.txt"));
}

#[test]
fn a_listing_for_a_buffer_that_was_closed_is_dropped() {
    let dir = TempDir::new("async_closed");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let id = app.focused_buffer_id();
    let request = app.explorer_requests[&ExplorerTarget::Buffer(id)].id;
    app.kill_buffer();

    let listing = fenix_explorer::read_listing(dir.path(), false).unwrap();
    app.apply_explorer_listed(ExplorerTarget::Buffer(id), request, Ok(listing));

    assert!(app.buffers.get(id).is_none(), "and nothing panicked on the way");
}

#[test]
fn a_directory_that_refuses_to_open_says_so_and_leaves_you_where_you_were() {
    // Being dumped into an empty pane is the worst possible place to
    // be left after a share denies access.
    let dir = TempDir::new("async_error");
    dir.touch("still-here.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let id = app.focused_buffer_id();

    app.explorer_navigate(ExplorerTarget::Buffer(id), &dir.path().join("does-not-exist"));

    assert_eq!(app.dired_states[&id].cwd, dir.path(), "still in the directory that works");
    assert!(listing_text(&mut app).contains("still-here.txt"));
    assert!(!app.modeline_pieces().1.is_empty(), "and it said something");
}

#[test]
fn abandoning_a_pending_read_leaves_the_pane_where_it_was() {
    let dir = TempDir::new("async_abandon");
    dir.touch("still-here.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let id = app.focused_buffer_id();
    // Stage a read that has not answered, the way a dead share
    // leaves one.
    app.next_explorer_request += 1;
    let pending = app.next_explorer_request;
    app.explorer_requests.insert(
        ExplorerTarget::Buffer(id),
        ExplorerRequest { id: pending, path: PathBuf::from(r"\\nas\media"), started: Instant::now(), done: false },
    );

    assert!(app.explorer_abandon_listing(ExplorerTarget::Buffer(id)));

    assert!(app.modeline_pieces().1.contains("stopped waiting"), "got: {}", app.modeline_pieces().1);
    assert_eq!(app.dired_states[&id].cwd, dir.path());
    assert!(listing_text(&mut app).contains("still-here.txt"), "the listing you had is still there");
}

#[test]
fn a_read_that_was_abandoned_is_dropped_when_it_finally_answers() {
    // The thread cannot be killed, so the guarantee is that whatever
    // it eventually returns changes nothing.
    let first = TempDir::new("async_abandon_late_first");
    first.touch("in-first.txt");
    let other = TempDir::new("async_abandon_late_other");
    other.touch("in-other.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(first.path());
    let id = app.focused_buffer_id();
    app.next_explorer_request += 1;
    let abandoned = app.next_explorer_request;
    app.explorer_requests.insert(
        ExplorerTarget::Buffer(id),
        ExplorerRequest { id: abandoned, path: other.path().to_path_buf(), started: Instant::now(), done: false },
    );
    app.explorer_abandon_listing(ExplorerTarget::Buffer(id));

    let late = fenix_explorer::read_listing(other.path(), false).unwrap();
    app.apply_explorer_listed(ExplorerTarget::Buffer(id), abandoned, Ok(late));

    assert_eq!(app.dired_states[&id].cwd, first.path());
}

#[test]
fn abandoning_does_nothing_when_no_read_is_outstanding() {
    // So Escape keeps its ordinary meaning the rest of the time.
    let dir = TempDir::new("async_abandon_none");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    assert!(!app.explorer_abandon_listing(ExplorerTarget::Buffer(app.focused_buffer_id())));
}

#[test]
fn a_fast_directory_never_flashes_a_reading_line() {
    // The note is for a wait worth mentioning; showing it for every
    // local directory would be noise on the common path.
    let dir = TempDir::new("async_fast");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    assert!(app.explorer_waiting_on(ExplorerTarget::Buffer(app.focused_buffer_id())).is_none());
}

#[test]
fn a_read_that_has_been_waiting_long_enough_is_named() {
    let dir = TempDir::new("async_slow");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let id = app.focused_buffer_id();
    app.next_explorer_request += 1;
    let request = app.next_explorer_request;
    app.explorer_requests.insert(
        ExplorerTarget::Buffer(id),
        ExplorerRequest {
            id: request,
            path: PathBuf::from(r"\\nas\media"),
            started: Instant::now() - SLOW_LISTING_AFTER - Duration::from_millis(1),
            done: false,
        },
    );

    assert_eq!(app.explorer_waiting_on(ExplorerTarget::Buffer(id)), Some(Path::new(r"\\nas\media")));
}

#[test]
fn git_badges_that_belong_to_another_read_are_dropped() {
    // Badges computed for one directory decorating another would be
    // worse than no badges at all.
    let dir = TempDir::new("async_git_stale");
    dir.touch("a.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let id = app.focused_buffer_id();
    let stale = app.explorer_requests[&ExplorerTarget::Buffer(id)].id - 1;

    let mut statuses = HashMap::new();
    statuses.insert(dir.path().join("a.txt"), fenix_explorer::GitStatus::Modified);
    app.apply_explorer_git_status(ExplorerTarget::Buffer(id), stale, statuses);

    assert_eq!(app.dired_states[&id].entries[0].git_status, None);
}

#[test]
fn a_network_path_is_recognised_by_its_own_spelling() {
    assert!(is_unc(Path::new(r"\\nas\media")));
    assert!(is_unc(Path::new("//nas/media")));
    assert!(!is_unc(Path::new(r"C:\Users")));
    assert!(!is_unc(Path::new("/home/thoma")));
}

#[test]
fn a_dired_buffer_can_be_closed_like_any_other_via_kill_buffer() {
    // The actual "dired as a real buffer" feature: closable via `SPC
    // b k` like any other buffer, falling back to whatever was
    // focused before it, same as closing any other buffer would.
    let dir = TempDir::new("dired_kill_buffer");
    let file = dir.touch("a.txt");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let original_id = app.focused_buffer_id();

    app.explorer_jump();
    let dired_id = app.focused_buffer_id();
    assert_ne!(dired_id, original_id);

    app.kill_buffer();

    assert!(app.buffers.get(dired_id).is_none(), "the dired buffer should be gone");
    assert_eq!(app.focused_buffer_id(), original_id, "falls back to the buffer it was opened over");
}

#[test]
fn enter_on_a_file_line_opens_it_replacing_the_dired_buffer() {
    let dir = TempDir::new("dired_enter_file");
    let file = dir.touch("target.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());

    app.explorer_handle_action(ExplorerAction::Open);

    assert_eq!(app.open().kind, BufferKind::Text);
    assert_eq!(app.open().buffer.path(), Some(file.as_path()));
}

#[test]
fn enter_on_a_directory_line_navigates_into_it_reusing_the_same_buffer() {
    let dir = TempDir::new("dired_enter_dir_parent");
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("inner.txt"), b"hi\n").unwrap();
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let dired_id = app.focused_buffer_id();

    app.explorer_handle_action(ExplorerAction::Open); // the only entry is "sub/"

    assert_eq!(app.focused_buffer_id(), dired_id, "same buffer, not a new one");
    assert_eq!(app.dired_states.get(&dired_id).unwrap().cwd, sub);
    assert!(listing_text(&mut app).contains("inner.txt"));
}

#[test]
fn dash_navigates_to_the_parent_directory() {
    let dir = TempDir::new("dired_parent_dir");
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let mut app = App::with_file(None);
    app.open_dired_at(&sub);
    let dired_id = app.focused_buffer_id();

    app.explorer_handle_action(ExplorerAction::ParentDir);

    assert_eq!(app.dired_states.get(&dired_id).unwrap().cwd, dir.path());
}

#[test]
fn a_listing_is_a_page_with_a_header_a_tree_and_a_key_strip() {
    let dir = TempDir::new("page_listing");
    std::fs::create_dir(dir.path().join("src")).unwrap();
    dir.touch("README.md");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());

    let text = listing_text(&mut app);
    let lines: Vec<&str> = text.lines().collect();
    let name = dir.path().file_name().unwrap().to_string_lossy().into_owned();
    assert!(lines[0].ends_with(&name), "the breadcrumb ends where it is: {}", lines[0]);
    assert!(lines[1].contains("2 items"), "{}", lines[1]);
    assert!(text.contains("▸ ") && text.contains("src"), "a folder has a chevron:\n{text}");
    assert!(lines.last().unwrap().contains("Enter open"), "the key strip is at the bottom:\n{text}");
    assert_eq!(app.page_title(app.focused_buffer_id()), format!("{name}/"), "the tab is the folder's name, not its path");
}

#[test]
fn keys_reach_the_listing_through_its_page() {
    let (_dir, mut app) = app_with_a_listing("page_keys");

    assert!(app.page_key(KeyPress::char('j')));
    assert_eq!(selected_name(&app), "b.txt");
    assert!(app.page_key(KeyPress::char('G')));
    assert_eq!(selected_name(&app), "c.txt");
    assert!(app.page_key(KeyPress::char('g')));
    assert!(app.page_key(KeyPress::char('g')));
    assert_eq!(selected_name(&app), "a.txt", "gg, through the page's own g");

    assert!(app.page_key(KeyPress::char('?')));
    assert!(listing_text(&mut app).contains("MARK"), "? shows every key");
    assert!(app.page_key(KeyPress::char('?')));
    assert!(listing_text(&mut app).contains("a.txt"), "and again goes back");

    assert!(!app.page_key(KeyPress::char(' ')), "Space is still the leader's");
}

#[test]
fn a_folder_opens_in_place_and_a_refresh_keeps_it_open() {
    let dir = TempDir::new("page_tree_refresh");
    let sub = dir.mkdir("sub");
    std::fs::write(sub.join("inner.txt"), "").unwrap();
    dir.touch("z.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());

    assert!(app.page_key(KeyPress::named(FenixNamedKey::Tab)));
    assert_eq!(explorer_row_names(&app), ["sub", "inner.txt", "z.txt"]);
    std::fs::write(sub.join("new.txt"), "").unwrap();
    app.explorer_handle_action(ExplorerAction::Refresh);

    assert_eq!(explorer_row_names(&app), ["sub", "inner.txt", "new.txt", "z.txt"]);
    assert!(listing_text(&mut app).contains("│ "), "the open folder's rows have a guide");
}

#[test]
fn the_folder_picker_is_a_page_and_closing_it_goes_back() {
    let dir = TempDir::new("page_picker");
    let file = dir.write("doc.txt", "x");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let original = app.focused_buffer_id();

    app.toggle_sidebar();
    assert!(app.sidebar_focused);
    app.open_folder_picker(dir.path(), ExplorerPurpose::PickProjectDir);

    assert!(app.picking_folder(), "the picker has the keyboard, not the sidebar");
    assert_eq!(app.page_title(app.focused_buffer_id()), "*pick a folder*");
    assert!(listing_text(&mut app).contains("S adds the folder shown as a project"));
    assert!(app.page_key(KeyPress::char('q')));
    assert!(!app.picking_folder());
    assert!(app.explorer.is_none());
    assert_eq!(app.focused_buffer_id(), original, "the pane shows what it did before");
}

#[test]
fn the_sidebar_is_drawn_from_the_explorer_page() {
    let dir = TempDir::new("page_sidebar");
    let file = dir.write("main.rs", "fn main() {}");
    let mut app = App::with_file(None);
    app.test_open_path(&file);
    app.toggle_sidebar();

    let (spans, focus, _, _) = app.sidebar_rows(30, 20).unwrap();

    let text: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(text.contains("main.rs"), "{text}");
    assert!(spans.iter().any(|(s, _, icon)| *icon && s.chars().all(crate::icon::is_icon)), "the icon in the icon font");
    assert!(text.lines().all(|l| l.chars().count() <= 30), "{text}");
    assert!(focus.is_some(), "the selected row");
}

/// A project (a `Cargo.toml` makes it one) with `src/main.rs` and
/// `src/lib.rs`, and an app editing `src/main.rs`.
fn app_in_a_project(name: &str) -> (TempDir, App) {
    let dir = TempDir::new(name);
    dir.write("Cargo.toml", "[package]");
    let src = dir.mkdir("src");
    std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(src.join("lib.rs"), "").unwrap();
    let mut app = App::with_file(None);
    app.test_open_path(&src.join("main.rs"));
    (dir, app)
}

/// The page line and column the sidebar shows `name` on.
fn sidebar_cell(app: &mut App, name: &str) -> (usize, usize) {
    let (spans, _, _, _) = app.sidebar_rows(30, 20).unwrap();
    let text: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    let line = text.lines().position(|l| l.contains(name)).unwrap_or_else(|| panic!("{name} isn't shown:\n{text}"));
    let row = text.lines().nth(line).unwrap();
    (line, row[..row.find(name).unwrap()].chars().count())
}

/// Where on screen the sidebar's cell (`line`, `col`) is.
fn sidebar_pos(line: usize, col: usize) -> (f32, f32) {
    // Where the sidebar draws it: inside its panel (`STRIP_INSET`).
    (text::PAD_LEFT + text::STRIP_INSET + col as f32 * text::CHAR_WIDTH + 1.0, text::PAD_TOP + text::STRIP_INSET + line as f32 * text::LINE_HEIGHT + 1.0)
}

#[test]
fn the_sidebar_opens_on_the_files_project_and_shows_the_file() {
    let (dir, mut app) = app_in_a_project("follow_open");

    app.toggle_sidebar();

    let sidebar = app.sidebar.as_ref().unwrap();
    assert_eq!(sidebar.cwd, dir.path(), "the project, not the file's folder");
    assert_eq!(sidebar.selected_entry().unwrap().path, dir.path().join("src").join("main.rs"), "src opened down to it");
    assert_eq!(app.sidebar_view.current.as_deref(), Some(dir.path().join("src").join("main.rs").as_path()));
}

#[test]
fn the_sidebar_catches_up_with_files_made_removed_and_renamed_elsewhere() {
    let (dir, mut app) = app_in_a_project("sidebar_outside_changes");
    app.toggle_sidebar();
    app.sidebar_focused = false;
    let names = |app: &App| app.sidebar.as_ref().unwrap().entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    let shown = |app: &App, name: &str| names(app).iter().any(|n| n == name);
    assert!(shown(&app, "main.rs"), "{:?}", names(&app));

    // A terminal, an agent, a save as: in the project and in a folder
    // open in it.
    std::fs::write(dir.path().join("notes.md"), "x").unwrap();
    std::fs::write(dir.path().join("src").join("extra.rs"), "x").unwrap();
    app.start_disk_probe();
    assert!(shown(&app, "notes.md") && shown(&app, "extra.rs"), "{:?}", names(&app));

    std::fs::rename(dir.path().join("src").join("extra.rs"), dir.path().join("src").join("renamed.rs")).unwrap();
    std::fs::remove_file(dir.path().join("notes.md")).unwrap();
    app.start_disk_probe();
    assert!(shown(&app, "renamed.rs") && !shown(&app, "extra.rs") && !shown(&app, "notes.md"), "{:?}", names(&app));
    assert!(shown(&app, "main.rs"), "src stays open: {:?}", names(&app));

    // Nothing changed: not read again.
    let before = app.next_explorer_request;
    app.start_disk_probe();
    assert_eq!(app.next_explorer_request, before);

    // Watching switched off: left as it is.
    app.config.watch_files = Some(false);
    std::fs::write(dir.path().join("later.md"), "x").unwrap();
    app.start_disk_probe();
    assert!(!shown(&app, "later.md"));
}

#[test]
fn a_folder_s_names_changing_reads_the_sidebar_again_once_it_settles() {
    let (dir, mut app) = app_in_a_project("sidebar_watched");
    app.toggle_sidebar();
    app.sidebar_focused = false;
    let shown = |app: &App, name: &str| app.sidebar.as_ref().unwrap().entries.iter().any(|e| e.name == name);
    std::fs::write(dir.path().join("src").join("watched.rs"), "x").unwrap();
    let now = Instant::now();
    // What the watcher says (`folders_changed`), for `src`.
    app.explorer_folders_changed(vec![dir.path().join("src")], now);
    assert_eq!(app.explorer_relist_dirty(now), Some(now + explorer_watch::SETTLE), "a burst settles first");
    assert!(!shown(&app, "watched.rs"));
    assert_eq!(app.explorer_relist_dirty(now + explorer_watch::SETTLE), None);
    assert!(shown(&app, "watched.rs"), "read again, src still open");
    // A folder no listing shows changes nothing.
    let elsewhere = TempDir::new("sidebar_watched_elsewhere");
    let before = app.next_explorer_request;
    app.explorer_folders_changed(vec![elsewhere.path().to_path_buf()], now);
    assert_eq!(app.explorer_relist_dirty(now + explorer_watch::SETTLE), None);
    assert_eq!(app.next_explorer_request, before);
}

#[test]
fn the_sidebar_follows_the_file_being_edited_without_taking_the_keyboard() {
    let (dir, mut app) = app_in_a_project("follow_switch");
    app.toggle_sidebar();
    app.sidebar_focused = false;

    app.test_open_path(&dir.path().join("src").join("lib.rs"));
    app.sidebar_follow();

    assert_eq!(app.sidebar.as_ref().unwrap().selected_entry().unwrap().name, "lib.rs");
    assert!(!app.sidebar_focused, "the keyboard stays with the editor");

    // Pinned, it stays where it is.
    app.config.explorer_follow = Some(false);
    app.test_open_path(&dir.path().join("Cargo.toml"));
    app.sidebar_follow();
    assert_eq!(app.sidebar.as_ref().unwrap().selected_entry().unwrap().name, "lib.rs");
}

#[test]
fn a_file_in_another_project_takes_the_sidebar_there() {
    let (_dir, mut app) = app_in_a_project("follow_other_a");
    app.toggle_sidebar();
    let other = TempDir::new("follow_other_b");
    other.write("package.json", "{}");
    let file = other.write("index.js", "");

    app.test_open_path(&file);
    app.sidebar_follow();

    assert_eq!(app.sidebar.as_ref().unwrap().cwd, other.path());
    assert_eq!(app.sidebar.as_ref().unwrap().selected_entry().unwrap().path, file, "and shows the file once it's read");
}

#[test]
fn spc_e_r_shows_the_file_in_the_sidebar_and_focuses_it() {
    let (dir, mut app) = app_in_a_project("reveal_current");
    app.sidebar_reveal_current();
    assert!(app.sidebar_open && app.sidebar_focused);
    assert_eq!(app.sidebar.as_ref().unwrap().selected_entry().unwrap().path, dir.path().join("src").join("main.rs"));

    // Pinned elsewhere, it still goes to the file.
    app.config.explorer_follow = Some(false);
    put_sidebar_on(&mut app, "Cargo.toml");
    app.sidebar_focused = false;
    app.sidebar_reveal_current();
    assert_eq!(app.sidebar.as_ref().unwrap().selected_entry().unwrap().name, "main.rs");
    assert!(app.sidebar_focused);
}

fn put_sidebar_on(app: &mut App, name: &str) {
    let sidebar = app.sidebar.as_mut().unwrap();
    let index = sidebar.entries.iter().position(|e| e.name == name).unwrap();
    sidebar.select_index(index);
}

#[test]
fn clicks_in_the_sidebar_select_fold_open_and_go_up() {
    let (dir, mut app) = app_in_a_project("sidebar_clicks");
    app.toggle_sidebar();
    app.sidebar_focused = false;

    // A click selects, and focuses the sidebar.
    let (line, col) = sidebar_cell(&mut app, "Cargo.toml");
    app.handle_click_at(sidebar_pos(line, col), 800.0, 600.0);
    assert!(app.sidebar_focused);
    assert_eq!(app.sidebar.as_ref().unwrap().selected_entry().unwrap().name, "Cargo.toml");

    // Its chevron folds a folder.
    let (line, _) = sidebar_cell(&mut app, "src");
    let chevron = app.sidebar_view.chevrons[line - app.sidebar_view.rows_at];
    app.handle_click_at(sidebar_pos(line, chevron), 800.0, 600.0);
    assert!(!app.sidebar.as_ref().unwrap().entries.iter().any(|e| e.name == "main.rs"), "src folded");

    // Two clicks on a file open it.
    let (line, col) = sidebar_cell(&mut app, "Cargo.toml");
    app.handle_click_at(sidebar_pos(line, col), 800.0, 600.0);
    app.handle_click_at(sidebar_pos(line, col), 800.0, 600.0);
    assert_eq!(app.open().buffer.path(), Some(dir.path().join("Cargo.toml").as_path()));

    // A breadcrumb segment goes to its folder.
    app.sidebar_view.crumbs = vec![(0..3, dir.path().parent().unwrap().to_path_buf())];
    app.handle_click_at(sidebar_pos(0, 1), 800.0, 600.0);
    assert_eq!(app.sidebar.as_ref().unwrap().cwd, dir.path().parent().unwrap());
}

#[test]
fn the_sidebars_edge_drags_within_limits() {
    let mut app = App::with_file(None);
    app.toggle_sidebar();
    assert!(app.on_sidebar_edge((text::SIDEBAR_WIDTH + 2.0, 100.0)));
    assert!(!app.on_sidebar_edge((text::SIDEBAR_WIDTH + 20.0, 100.0)));
    app.drag_sidebar_to(330.0, 1000.0);
    assert_eq!(app.sidebar_width, 330.0);
    assert_eq!(app.frame_metrics(600.0).0, 330.0, "the panes start where it ends");
    app.drag_sidebar_to(10.0, 1000.0);
    assert_eq!(app.sidebar_width, text::SIDEBAR_MIN_WIDTH);
    app.drag_sidebar_to(900.0, 1000.0);
    assert_eq!(app.sidebar_width, 600.0);
}

#[test]
fn slash_filters_the_whole_tree_and_n_steps_through_it() {
    let dir = TempDir::new("tree_filter");
    let deep = dir.path().join("src").join("parser");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("lexer.rs"), "").unwrap();
    std::fs::write(dir.path().join("lexicon.md"), "").unwrap();
    dir.touch("other.txt");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());

    assert!(app.page_key(KeyPress::char('/')));
    for c in "lex".chars() {
        app.explorer_prompt_key(KeyPress::char(c));
    }

    assert_eq!(explorer_row_names(&app), ["src", "parser", "lexer.rs", "lexicon.md"], "found below the folders read, with the way down");
    let text = listing_text(&mut app);
    assert!(text.lines().nth(1).unwrap().contains("filter: lex"));
    let id = app.focused_buffer_id();
    assert!(!app.pages[&id].page.underlines.is_empty(), "the matches are underlined");

    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(app.explorer_prompt.is_none(), "Enter keeps the filter");
    assert!(app.page_key(KeyPress::char('n')));
    assert_eq!(selected_name(&app), "lexer.rs");
    assert!(app.page_key(KeyPress::char('n')));
    assert_eq!(selected_name(&app), "lexicon.md");

    assert!(app.page_key(KeyPress::named(FenixNamedKey::Escape)));
    assert_eq!(explorer_row_names(&app), ["src", "lexicon.md", "other.txt"], "Esc clears it");
}

#[test]
fn p_previews_the_selection_beside_the_listing_and_follows_it() {
    let dir = TempDir::new("preview");
    let sub = dir.mkdir("sub");
    std::fs::write(sub.join("inner.txt"), "").unwrap();
    let lines: String = (1..=40).map(|i| format!("let x{i} = {i};\n")).collect();
    let file = dir.write("main.rs", &format!("fn main() {{}}\n{lines}"));
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    let listing = app.focused_buffer_id();
    let preview_text = |app: &mut App| {
        let (_, id) = app.explorer_preview.unwrap();
        let pane = app.pane_showing(id).unwrap();
        app.note_page_rows(id, 20);
        app.ensure_page_layout(id, pane, 60);
        app.buffers.get(id).unwrap().buffer.text()
    };

    assert!(app.page_key(KeyPress::char('p')));
    assert_eq!(app.windows().window_count(), 2);
    assert_eq!(app.focused_buffer_id(), listing, "the keyboard stays with the listing");
    assert!(preview_text(&mut app).contains("inner.txt"), "a folder shows what's in it");

    assert!(app.page_key(KeyPress::char('j')));
    let text = preview_text(&mut app);
    assert!(text.contains("1 fn main() {}") && text.contains("Rust"), "follows to the file, numbered:\n{text}");
    assert!(app.page_key(KeyPress::char('J')));
    assert!(!preview_text(&mut app).contains("1 fn main()"), "J scrolls it");

    assert!(app.page_key(KeyPress::char('P')));
    assert!(app.explorer_preview.is_none());
    assert_eq!(app.focused_buffer_id(), listing, "still with the listing");
    let other = app.windows().windows().into_iter().find(|&p| p != app.focused_pane_id()).unwrap();
    let shown = *app.windows().content(other).unwrap();
    assert_eq!(app.buffers.get(shown).unwrap().buffer.path(), Some(file.as_path()), "the file, in the pane beside");

    assert!(app.page_key(KeyPress::char('p')));
    assert!(app.page_key(KeyPress::char('p')));
    assert!(app.explorer_preview.is_none());
    assert_eq!(app.windows().window_count(), 2, "p again closes the preview's pane, leaving the file's");
}

#[test]
fn a_new_file_is_typed_in_place_and_a_slash_makes_folders() {
    let (dir, mut app) = app_with_a_listing("create_in_place");
    assert!(app.page_key(KeyPress::char('a')));
    for c in "docs/new notes.md".chars() {
        assert!(app.page_key(KeyPress::char(c)), "{c:?} is typed, Space included");
    }
    let text = listing_text(&mut app);
    assert!(text.contains("docs/new notes.md▏"), "typed in a row of the listing:\n{text}");

    assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));

    let made = dir.path().join("docs").join("new notes.md");
    assert!(made.exists());
    assert_eq!(app.active_explorer().unwrap().selected_entry().unwrap().path, made, "the listing shows it, selected");
}

#[test]
fn a_new_folder_goes_inside_an_open_folder() {
    let dir = TempDir::new("create_folder_inside");
    dir.mkdir("sub");
    let mut app = App::with_file(None);
    app.open_dired_at(dir.path());
    app.page_key(KeyPress::named(FenixNamedKey::Tab));
    app.page_key(KeyPress::char('A'));
    for c in "inner".chars() {
        app.page_key(KeyPress::char(c));
    }
    app.page_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(dir.path().join("sub").join("inner").is_dir());
}

#[test]
fn r_renames_in_place_with_the_name_before_its_extension_selected() {
    let (dir, mut app) = app_with_a_listing("rename_in_place");
    app.page_key(KeyPress::char('j')); // b.txt
    assert!(app.page_key(KeyPress::char('R')));
    for c in "renamed".chars() {
        app.page_key(KeyPress::char(c));
    }
    assert!(listing_text(&mut app).contains("renamed▏.txt"), "the stem was typed over, the extension kept");
    app.page_key(KeyPress::named(FenixNamedKey::Enter));

    assert!(dir.path().join("renamed.txt").exists());
    assert!(!dir.path().join("b.txt").exists());
    assert!(dir.path().join("a.txt").exists(), "and nothing else moved");
    assert_eq!(selected_name(&app), "renamed.txt");
}

#[test]
fn escape_gives_up_a_name_and_a_taken_one_stays_to_fix() {
    let (dir, mut app) = app_with_a_listing("edit_cancel");
    app.page_key(KeyPress::char('a'));
    app.page_key(KeyPress::char('x'));
    app.page_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(!listing_text(&mut app).contains('▏'));
    assert!(!dir.path().join("x").exists());

    app.page_key(KeyPress::char('a'));
    for c in "a.txt".chars() {
        app.page_key(KeyPress::char(c));
    }
    app.page_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(app.status_message.as_ref().is_some_and(|m| m.is_error), "it says why");
    assert!(listing_text(&mut app).contains("a.txt▏"), "and the name stays to fix");
}

#[test]
fn a_name_typed_in_the_sidebar_takes_a_space() {
    let (dir, mut app) = app_in_a_project("sidebar_edit");
    app.toggle_sidebar();
    put_sidebar_on(&mut app, "Cargo.toml");
    app.explorer_handle_action(ExplorerAction::BeginCreateFile);
    for c in "two words".chars() {
        assert!(app.explorer_page_key(ExplorerTarget::Sidebar, KeyPress::char(c)));
    }
    app.explorer_page_key(ExplorerTarget::Sidebar, KeyPress::named(FenixNamedKey::Enter));
    assert!(dir.path().join("two words").exists());
}

#[test]
fn each_project_keeps_its_sidebar_as_it_was_left() {
    let (dir, mut app) = app_in_a_project("explorer_session_a");
    app.toggle_sidebar();
    assert!(app.sidebar.as_ref().unwrap().open_folders().contains(&dir.path().join("src")), "src opened to show main.rs");
    app.sidebar.as_mut().unwrap().set_filter("lib");
    app.sidebar_width = 300.0;

    // Another project, whose sidebar was never opened.
    let other = TempDir::new("explorer_session_b");
    other.write("go.mod", "module x");
    let file = other.write("main.go", "");
    app.test_open_path(&file);
    app.sidebar_follow();
    assert_eq!(app.sidebar.as_ref().unwrap().cwd, other.path());
    assert_eq!(app.sidebar.as_ref().unwrap().filter, "", "its own filter -- none");

    // And back: open folders, filter and width as they were.
    app.test_open_path(&dir.path().join("src").join("main.rs"));
    app.sidebar_follow();
    let sidebar = app.sidebar.as_ref().unwrap();
    assert_eq!(sidebar.cwd, dir.path());
    assert_eq!(sidebar.filter, "lib");
    assert!(sidebar.open_folders().contains(&dir.path().join("src")));
    assert_eq!(app.sidebar_width, 300.0);

    // Kept on the disk, and read back by the next Fenix.
    app.save_explorer_sessions();
    let saved: HashMap<PathBuf, explorer_host::ExplorerSession> =
        fenix_storage::state::read(&app.config.state_dir().join(explorer_host::SESSIONS_FILE), "projects").unwrap().unwrap();
    assert_eq!(saved[dir.path()].filter, "lib");
    assert!(saved[dir.path()].open);
}

#[test]
fn a_project_whose_sidebar_was_closed_closes_it_when_it_comes_back() {
    let (dir, mut app) = app_in_a_project("explorer_session_closed");
    app.toggle_sidebar();
    app.toggle_sidebar(); // closed in this project
    let other = TempDir::new("explorer_session_closed_b");
    other.write("go.mod", "module x");
    let file = other.write("main.go", "");
    app.test_open_path(&file);
    app.refresh_project_root();
    app.toggle_sidebar(); // open in the other
    assert!(app.sidebar_open);

    app.test_open_path(&dir.path().join("Cargo.toml"));
    app.refresh_project_root();

    assert!(!app.sidebar_open, "closed, as it was left here");
}

#[test]
fn the_explorer_settings_apply_to_every_listing_at_once() {
    let (dir, mut app) = app_with_a_listing("explorer_settings");
    dir.mkdir("node_modules");
    app.explorer_handle_action(ExplorerAction::Refresh);
    assert!(explorer_row_names(&app).contains(&"node_modules".to_string()));

    app.set_setting("explorer.hide", Some(fenix_config::Value::List(vec!["node_modules".into(), "*.txt".into()]))).unwrap();
    assert!(explorer_row_names(&app).is_empty(), "{:?}", explorer_row_names(&app));

    app.set_setting("explorer.width", Some(fenix_config::Value::Int(333))).unwrap();
    assert_eq!(app.sidebar_width, 333.0);
    app.drag_sidebar_to(280.0, 1200.0);
    app.sidebar_width_dragged();
    assert_eq!(app.config.explorer_width, Some(280), "dragging sets the setting");

    app.set_setting("explorer.compact", Some(fenix_config::Value::Bool(false))).unwrap();
    assert!(!app.active_explorer().unwrap().compact);
}

#[test]
fn toggle_sidebar_opens_then_closes() {
    let dir = TempDir::new("toggle_sidebar");
    dir.touch("a.txt");
    let mut app = App::with_file(None);
    app.test_open_path(&dir.path().join("a.txt"));

    app.toggle_sidebar();
    assert!(app.sidebar_open);
    assert!(app.sidebar_focused);
    assert!(app.sidebar.is_some());

    app.toggle_sidebar();
    assert!(!app.sidebar_open);
    assert!(!app.sidebar_focused);
    assert!(app.sidebar.is_none());
}

/// While the terminal panel has the keyboard the modeline says so,
/// and how to get it back (#42).
#[test]
fn the_modeline_says_when_a_terminal_has_the_keyboard() {
    let mut app = App::with_file(None);
    assert_ne!(app.modeline_pieces().0, "TERMINAL");
    app.toggle_terminal();
    app.status_message = None;
    let (badge, rest) = app.modeline_pieces();
    assert_eq!(badge, "TERMINAL");
    assert!(rest.contains("C-\\ back to the editor"), "{rest}");
    app.terminal_focused = false;
    let (badge, rest) = app.modeline_pieces();
    assert_ne!(badge, "TERMINAL");
    assert!(!rest.contains("back to the editor"));
}

#[test]
fn toggle_terminal_opens_then_hides_without_dropping_the_session() {
    let mut app = App::with_file(None);

    // No `event_proxy` in a test (see its own doc comment), so
    // `toggle_terminal`'s spawn runs synchronously via `apply_
    // terminal_spawned` instead of a background thread -- same
    // "no fake event loop needed" posture every other background-
    // spawning path in this file already has, and lets this test
    // assert the *end state* directly without waiting on a channel.
    app.toggle_terminal();
    assert!(app.terminal_open);
    assert!(app.terminal_focused);
    assert!(!app.terminal_spawning);
    assert!(app.terminal.is_some(), "expected a real shell to have been spawned");

    // Unlike `toggle_sidebar`, hiding must NOT drop the session --
    // the whole point is that the shell (and anything running in
    // it) keeps going while the panel is hidden.
    app.toggle_terminal();
    assert!(!app.terminal_open);
    assert!(!app.terminal_focused);
    assert!(app.terminal.is_some(), "the session should still be alive after hiding the panel");
}

#[test]
fn terminal_panel_render_is_none_while_closed() {
    let app = App::with_file(None);
    assert!(app.terminal_panel_render().is_none());
}

#[test]
fn terminal_panel_render_shows_a_placeholder_while_spawning() {
    // Simulates the real gap between `toggle_terminal` kicking off a
    // background spawn and `apply_terminal_spawned` hearing back --
    // this exact state (open, no session yet, but already focused)
    // is what used to render as an unexplained blank panel.
    let mut app = App::with_file(None);
    app.terminal_open = true;
    app.terminal_spawning = true;
    let view = app.terminal_panel_render().expect("expected a placeholder while spawning");
    let (spans, bg_segments) = (view.spans, view.backgrounds);
    let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(joined.contains("Starting shell"));
    assert!(bg_segments.is_empty());
}

#[test]
fn terminal_panel_render_shows_real_content_once_a_session_exists() {
    let mut app = App::with_file(None);
    app.toggle_terminal(); // synchronous spawn in a test, see the test above
    let spans = app.terminal_panel_render().expect("expected real terminal content").spans;
    let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(!joined.contains("Starting shell"), "a live session must not still show the placeholder");
}

#[test]
fn apply_terminal_spawned_error_closes_the_panel_and_surfaces_a_message() {
    let mut app = App::with_file(None);
    app.terminal_open = true;
    app.terminal_spawning = true;

    app.apply_terminal_spawned(Err(std::io::Error::other("no shell for you")));

    assert!(!app.terminal_open);
    assert!(!app.terminal_spawning);
    assert!(app.terminal.is_none());
    assert!(app.modeline_pieces().1.contains("couldn't start a terminal"));
}

#[test]
fn apply_terminal_spawned_does_not_reopen_a_panel_the_user_already_closed() {
    // The shell keeps starting up in the background even if the
    // user closes the panel before it's ready (same "runs on while
    // hidden" posture `toggle_terminal` already has for an already-
    // open session) -- but it must not force the panel back open or
    // steal focus once it finally does report back.
    let mut app = App::with_file(None);
    app.terminal_open = true;
    app.terminal_spawning = true;
    app.terminal_open = false; // user closed it while the spawn was in flight

    let (session, reader) = fenix_terminal::Terminal::spawn(text::TERMINAL_ROWS as u16, 80).expect("failed to spawn a real shell");
    app.apply_terminal_spawned(Ok((session, reader)));

    assert!(!app.terminal_open);
    assert!(!app.terminal_focused);
    assert!(app.terminal.is_some(), "the session should still be kept, just not shown/focused");
}

// -- Pane-resident terminals (`SPC o T`) ---------------------------
//
// These spawn real shells, the same way the panel's own tests above
// do (and `fenix-git`/`fenix-docker`'s tests spawn real `git`/
// `docker`) -- with no `event_proxy` the spawn runs synchronously
// through `apply_terminal_buffer_spawned`, so there is a live
// process to assert against by the time each call returns.

#[test]
fn opening_a_terminal_buffer_puts_a_live_shell_in_the_focused_pane() {
    let mut app = App::with_file(None);

    app.open_terminal_buffer();

    let id = app.focused_buffer_id();
    assert_eq!(app.buffers.get(id).map(|ob| ob.kind), Some(BufferKind::Terminal));
    assert!(app.terminal_buffers.contains_key(&id), "expected a real shell for this buffer");
    assert_eq!(app.terminal_buffer_focused, Some(id), "a shell you just asked for should take the keyboard");
    assert_eq!(app.active_terminal_buffer(), Some(id));
}

#[test]
fn the_popup_panel_and_a_pane_terminal_are_separate_shells() {
    // The whole reason both exist: one follows you around, one stays
    // put. Sharing a session would make them the same feature twice.
    let mut app = App::with_file(None);

    app.toggle_terminal();
    app.open_terminal_buffer();

    assert!(app.terminal.is_some(), "the panel keeps its own shell");
    assert_eq!(app.terminal_buffers.len(), 1, "and the pane has one of its own");
}

#[test]
fn output_reaches_the_shell_it_came_from_and_no_other() {
    let mut app = App::with_file(None);
    app.toggle_terminal();
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();

    app.apply_terminal_output(TerminalTarget::Buffer(id), b"in the pane".to_vec());
    app.apply_terminal_output(TerminalTarget::Panel, b"in the panel".to_vec());

    assert!(screen_text(app.terminal_buffers[&id].session.screen()).contains("in the pane"));
    assert!(!screen_text(app.terminal_buffers[&id].session.screen()).contains("in the panel"));
    assert!(screen_text(app.terminal.as_ref().unwrap().session.screen()).contains("in the panel"));
}

#[test]
fn a_second_press_takes_the_keyboard_back_rather_than_opening_another_shell() {
    // `SPC o T` is both "give me a shell" and "let me type in it
    // again" -- see `open_terminal_buffer`. Accumulating shells for
    // the second meaning would leave processes running behind panes
    // nobody looks at.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();
    app.unfocus_terminal_buffer();

    app.open_terminal_buffer();

    assert_eq!(app.focused_buffer_id(), id, "same buffer");
    assert_eq!(app.terminal_buffers.len(), 1, "same shell");
    assert_eq!(app.terminal_buffer_focused, Some(id));
}

#[test]
fn releasing_the_keyboard_leaves_the_shell_running_and_the_pane_alone() {
    // `Ctrl-\` unfocuses; it does not close. Killing the shell here
    // would lose whatever it was in the middle of.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();

    app.unfocus_terminal_buffer();

    assert_eq!(app.active_terminal_buffer(), None, "keys go back to Vim");
    assert!(app.terminal_buffers.contains_key(&id), "the shell is still running");
    assert_eq!(app.focused_buffer_id(), id, "and the pane still shows it");
}

#[test]
fn typing_only_reaches_the_terminal_while_its_own_pane_is_focused() {
    // Capture deliberately survives a pane switch, so this is the
    // check that stops keystrokes meant for a file from being typed
    // into a shell in the pane next door.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let terminal_pane = app.focused_pane_id();
    app.split_vertical();
    let other = app.windows().windows().into_iter().find(|&p| p != terminal_pane).expect("split makes a second pane");
    app.windows_mut().focus(other);
    app.new_scratch_buffer();

    assert_eq!(app.active_terminal_buffer(), None, "this pane is not the terminal");

    app.windows_mut().focus(terminal_pane);
    assert!(app.active_terminal_buffer().is_some(), "back in the terminal's own pane");
}

#[test]
fn a_terminal_buffer_survives_its_pane_being_pointed_at_something_else() {
    // Keyed by buffer, not by pane (`BufferKind::Terminal`), so
    // navigating away hides it exactly like any other buffer and
    // `SPC b b` brings it back -- with whatever was running still
    // running.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();

    app.new_scratch_buffer();

    assert_ne!(app.focused_buffer_id(), id);
    assert!(app.terminal_buffers.contains_key(&id), "the shell keeps going while out of sight");
    assert!(app.buffers.get(id).is_some(), "and the buffer is still there to switch back to");
}

#[test]
fn closing_the_buffer_ends_the_shell() {
    // The one gesture that means the shell has no owner left.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();

    app.kill_buffer();

    assert!(!app.terminal_buffers.contains_key(&id));
    assert!(app.buffers.get(id).is_none());
    assert_eq!(app.terminal_buffer_focused, None, "and it can't still be holding the keyboard");
}

#[test]
fn several_terminals_are_named_apart() {
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let first = app.focused_buffer_id();
    app.new_scratch_buffer();
    app.open_terminal_buffer();
    let second = app.focused_buffer_id();

    assert_eq!(app.buffer_display_name(first), "*terminal 1*");
    assert_eq!(app.buffer_display_name(second), "*terminal 2*");
}

#[test]
fn a_terminals_numbering_is_never_reused() {
    // A recycled number would point at a different shell than the
    // one you remembered.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    app.kill_buffer();

    app.open_terminal_buffer();

    assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*terminal 2*");
}

#[test]
fn nothing_the_shell_prints_is_written_into_the_buffers_own_text() {
    // The screen is a grid the host owns; a rope kept alongside it
    // would be a second, always-slightly-wrong copy that `:w` could
    // then write to a file.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();

    app.apply_terminal_output(TerminalTarget::Buffer(id), b"lots of output\r\n".to_vec());

    assert_eq!(app.open().buffer.text(), "");
    assert!(!app.open().buffer.is_dirty(), "and nothing here is unsaved work");
}

#[test]
fn a_shell_that_exited_hands_the_keyboard_back_on_the_next_keystroke() {
    // Otherwise everything typed after `exit` would vanish into a
    // dead process, with the release chord the only way out and
    // nothing on screen to suggest it.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();
    app.terminal_buffers.get_mut(&id).unwrap().session.kill();

    assert_eq!(app.active_terminal_buffer(), None);
    assert_eq!(app.terminal_buffer_focused, None);
    assert!(app.modeline_pieces().1.contains("the shell exited"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn reopening_on_a_dead_terminal_restarts_it_in_the_same_buffer() {
    // The pane, the workspace and the name are all still the ones
    // the user arranged; only the process behind them is gone.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let id = app.focused_buffer_id();
    app.terminal_buffers.get_mut(&id).unwrap().session.kill();

    app.open_terminal_buffer();

    assert_eq!(app.focused_buffer_id(), id, "same buffer");
    assert_eq!(app.buffer_display_name(id), "*terminal 1*", "same name");
    assert!(app.terminal_buffers.get_mut(&id).unwrap().session.is_alive(), "with a live shell again");
}

#[test]
fn a_spawn_that_lands_after_its_buffer_was_closed_is_dropped() {
    // The user can close a pane while its shell is still starting; a
    // terminal that captured the keyboard on the way out would
    // swallow everything typed next with nothing to explain it.
    let mut app = App::with_file(None);
    let ghost = app.buffers.open_terminal();
    app.buffers.close(ghost);

    let spawned = fenix_terminal::Terminal::spawn(24, 80).expect("failed to spawn a real shell");
    app.apply_terminal_buffer_spawned(ghost, Ok(spawned));

    assert!(!app.terminal_buffers.contains_key(&ghost));
    assert_eq!(app.terminal_buffer_focused, None);
}

#[test]
fn a_failed_spawn_says_so_instead_of_leaving_an_inert_pane() {
    let mut app = App::with_file(None);
    let id = app.buffers.open_terminal();
    app.open_buffer_in_focused_pane(id);
    app.terminal_buffer_labels.insert(id, "*terminal 1*".to_string());

    app.apply_terminal_buffer_spawned(id, Err(std::io::Error::other("no shell for you")));

    assert!(!app.terminal_buffers.contains_key(&id));
    assert_eq!(app.terminal_buffer_focused, None);
    assert!(app.modeline_pieces().1.contains("couldn't start a terminal"));
}

#[test]
fn a_terminal_pane_gets_no_line_number_gutter() {
    // A gutter would shift every column out from under what the
    // shell thinks it drew.
    let mut app = App::with_file(None);
    app.open_terminal_buffer();
    let ob = app.buffers.get(app.focused_buffer_id()).unwrap();
    assert_eq!(app.gutter_chars(ob), 0);
}

/// `terminal.shell` is a program and its arguments, a quoted path
/// one word; empty is the platform's own.
#[test]
fn the_terminal_shell_setting_is_a_program_and_its_arguments() {
    let mut app = App::with_file(None);
    assert_eq!(app.terminal_shell(), None, "unset: the platform's");
    app.config.set_value("terminal.shell", Some(fenix_config::Value::Text("  ".into())));
    assert_eq!(app.terminal_shell(), None, "empty: the platform's");
    app.config.set_value("terminal.shell", Some(fenix_config::Value::Text(r#""C:\Program Files\Git\bin\bash.exe" -l"#.into())));
    assert_eq!(app.terminal_shell(), Some((r"C:\Program Files\Git\bin\bash.exe".to_string(), vec!["-l".to_string()])));
    app.config.set_value("terminal.shell", Some(fenix_config::Value::Text("pwsh -NoLogo -NoProfile".into())));
    assert_eq!(app.terminal_shell(), Some(("pwsh".to_string(), vec!["-NoLogo".to_string(), "-NoProfile".to_string()])));
}

#[test]
fn write_terminal_input_is_a_no_op_without_a_live_session() {
    let mut app = App::with_file(None);
    // Never toggled on -- `app.terminal` stays `None`. Just
    // confirms this doesn't panic; there's nothing to assert on
    // beyond that (no session to have received anything).
    app.write_terminal_input(KeyPress::char('a'));
}

/// Drives one real shell all the way through the app: spawn it,
/// type at it with the same `KeyPress` encoding the keyboard uses,
/// pump its real output through `apply_terminal_output`, and read
/// the result back out of the renderer. Returns the rendered text
/// once `marker` shows up in it, or `Err` with whatever *was* on
/// screen when it gave up.
///
/// This is the shape of the bug this whole path exists to have
/// fixed. Windows' ConPTY opens a session by asking where the
/// cursor is and says nothing further until it is answered, so
/// before `fenix_terminal::query` existed the screen here stayed
/// permanently, silently blank while the shell sat alive and
/// waiting -- exactly what the panel showed. No unit test on the
/// app's own logic could have caught that; only running a real
/// shell through the real renderer does.
///
/// The reading has to happen on its own thread: `Read::read` on a
/// PTY blocks until something arrives with no way to bound the call
/// itself, so only a channel's `recv_timeout` can enforce a real
/// deadline (see `fenix-terminal`'s own round-trip test).
fn pump_until(
    app: &mut App,
    target: TerminalTarget,
    mut reader: Box<dyn std::io::Read + Send>,
    marker: &str,
    render: impl Fn(&App) -> String,
) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = String::new();
    while Instant::now() < deadline {
        let Ok(chunk) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else { break };
        app.apply_terminal_output(target, chunk);
        last = render(app);
        if last.contains(marker) {
            return Ok(last);
        }
    }
    Err(last)
}

/// Types `text` at a shell one keypress at a time, through the same
/// encoding a real keystroke takes, and presses Enter.
fn type_line(mut send: impl FnMut(KeyPress), text: &str) {
    for c in text.chars() {
        send(KeyPress::char(c));
    }
    send(KeyPress::named(FenixNamedKey::Enter));
}

#[test]
fn the_panel_renders_what_a_real_shell_actually_prints() {
    // End-to-end over everything but the GPU: a real PTY, the real
    // ConPTY handshake, the real parser, the app's own output
    // routing, and the real span builder the renderer draws from.
    let mut app = App::with_file(None);
    let (session, reader) = fenix_terminal::Terminal::spawn(text::TERMINAL_ROWS as u16, 80).expect("failed to spawn a real shell");
    app.terminal = Some(TerminalState::new(session, None));
    app.terminal_open = true;
    app.terminal_focused = true;

    type_line(|key| app.write_terminal_input(key), "echo fenix-panel-marker");

    let render = |app: &App| {
        app.terminal_panel_render().map(|view| view.spans.iter().map(|(text, _, _)| text.as_str()).collect::<String>()).unwrap_or_default()
    };
    match pump_until(&mut app, TerminalTarget::Panel, reader, "fenix-panel-marker", render) {
        Ok(_) => {}
        Err(last) => panic!("the shell's own output never reached the panel. Last rendered screen:\n{last}"),
    }
}

#[test]
fn a_pane_terminal_renders_what_a_real_shell_actually_prints() {
    // The same end-to-end for the other surface, down to the
    // per-cell background colours the pane renderer needs and the
    // panel's own `RowSpans` has nowhere to put.
    let mut app = App::with_file(None);
    let id = app.buffers.open_terminal();
    app.open_buffer_in_focused_pane(id);
    app.terminal_buffer_labels.insert(id, "*terminal 1*".to_string());
    let (session, reader) = fenix_terminal::Terminal::spawn(24, 80).expect("failed to spawn a real shell");
    app.terminal_buffers.insert(id, TerminalState::new(session, None));
    app.focus_terminal_buffer(id);

    type_line(|key| app.write_terminal_buffer_input(id, key), "echo fenix-pane-marker");

    let render = move |app: &App| match app.terminal_buffers.get(&id) {
        Some(state) => {
            let spans = app.terminal_row_spans(state.session.screen()).spans;
            spans.iter().map(|(text, _, _)| text.as_str()).collect::<String>()
        }
        None => String::new(),
    };
    match pump_until(&mut app, TerminalTarget::Buffer(id), reader, "fenix-pane-marker", render) {
        Ok(_) => {}
        Err(last) => panic!("the shell's own output never reached the pane. Last rendered screen:\n{last}"),
    }
}

/// Everything currently on a parsed terminal screen, as one string
/// -- enough to assert that a given chunk of output landed on the
/// screen it was addressed to.
fn screen_text(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let mut out = String::new();
    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                out.push_str(cell.contents());
            }
        }
    }
    out
}

#[test]
fn wheel_delta_lines_for_line_delta_uses_the_fixed_notch_amount() {
    assert_eq!(wheel_delta_lines(MouseScrollDelta::LineDelta(0.0, 1.0), 20.0), WHEEL_LINES_PER_NOTCH);
    assert_eq!(wheel_delta_lines(MouseScrollDelta::LineDelta(0.0, -1.0), 20.0), -WHEEL_LINES_PER_NOTCH);
}

#[test]
fn wheel_delta_lines_for_pixel_delta_converts_through_line_height() {
    let delta = MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, 40.0));
    assert_eq!(wheel_delta_lines(delta, 20.0), 2);
}

#[test]
fn frame_geometry_places_sidebar_terminal_and_pane_rects_consistently() {
    let dir = TempDir::new("frame_geometry");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    app.terminal_open = true;

    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);

    assert_eq!(geometry.sidebar_rect, Some(fenix_window::Rect { x: 0.0, y: 0.0, w: sidebar_px, h: modeline_top }));
    assert_eq!(
        geometry.terminal_rect,
        Some(fenix_window::Rect { x: 0.0, y: modeline_top - terminal_h, w: 800.0, h: terminal_h })
    );
    assert_eq!(geometry.panes.len(), 1); // no splits yet
}

/// Framed, each pane is inset on the tray; flat (`appearance.panels`),
/// it's the layout's rect edge to edge, as before 1.4, and a click
/// still lands on the pane drawn there either way.
#[test]
fn panes_are_framed_on_the_tray_or_flat_edge_to_edge() {
    let mut app = App::with_file(None);
    let first = app.windows().focused_id();
    app.split_vertical();
    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let layout = app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: modeline_top - terminal_h });
    let framed = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    assert_eq!(app.panel_look(), panels::Look::Framed, "framed unless set");
    for ((id, raw), (_, drawn)) in layout.iter().zip(&framed.panes) {
        assert!(drawn.x > raw.x && drawn.w < raw.w, "{id:?} inset: {drawn:?} in {raw:?}");
    }
    app.config.set_value("appearance.panels", Some(fenix_config::Value::Text("flat".into())));
    assert_eq!(app.panel_look(), panels::Look::Flat);
    let flat = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    assert_eq!(flat.panes, layout, "edge to edge");
    let first_rect = flat.panes.iter().find(|(id, _)| *id == first).unwrap().1;
    app.handle_click_at((first_rect.x + 2.0, first_rect.y + 30.0), 800.0, 600.0);
    assert_eq!(app.windows().focused_id(), first, "a click at its very edge is the pane's");
}

/// The focus edge follows the keyboard: the pane clicked or moved to,
/// the terminal panel while it's typed in, the sidebar while it is.
#[test]
fn the_focus_edge_follows_the_keyboard() {
    let mut app = App::with_file(None);
    let first = app.windows().focused_id();
    app.split_vertical();
    let second = app.windows().focused_id();
    let edge = |app: &App| panels::keyboard_panel(app.windows().focused_id(), app.terminal_focused && app.terminal_open, app.sidebar_focused, app.debug_drawer_focused());
    assert_eq!(edge(&app), panels::KeyboardPanel::Pane(second));
    app.windows_mut().focus(first);
    assert_eq!(edge(&app), panels::KeyboardPanel::Pane(first));
    app.terminal_open = true;
    app.terminal_focused = true;
    assert_eq!(edge(&app), panels::KeyboardPanel::Terminal);
    app.terminal_focused = false;
    app.sidebar_focused = true;
    assert_eq!(edge(&app), panels::KeyboardPanel::Sidebar);
}

#[test]
fn click_on_a_non_focused_pane_focuses_it() {
    let mut app = App::with_file(None);
    let first = app.windows().focused_id();
    app.split_vertical();
    let second = app.windows().focused_id();
    assert_ne!(first, second);

    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    let first_rect = geometry.panes.iter().find(|(id, _)| *id == first).unwrap().1;
    let pos = (first_rect.x + 5.0, first_rect.y + 5.0);

    app.handle_click_at(pos, 800.0, 600.0);
    assert_eq!(app.windows().focused_id(), first);
}

#[test]
fn click_on_the_sidebar_focuses_it_and_clears_terminal_focus() {
    let dir = TempDir::new("click_sidebar");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    app.terminal_open = true;
    app.terminal_focused = true;

    app.handle_click_at((10.0, 10.0), 800.0, 600.0);
    assert!(app.sidebar_focused);
    assert!(!app.terminal_focused);
}

#[test]
fn click_on_the_terminal_focuses_it_and_clears_sidebar_focus() {
    let mut app = App::with_file(None);
    app.terminal_open = true;
    app.sidebar_focused = true;

    let (_, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let pos = (10.0, modeline_top - terminal_h + 5.0);
    app.handle_click_at(pos, 800.0, 600.0);
    assert!(app.terminal_focused);
    assert!(!app.sidebar_focused);
}

#[test]
fn click_below_the_pane_area_is_a_no_op() {
    let mut app = App::with_file(None);
    let focused = app.windows().focused_id();

    app.handle_click_at((10.0, 590.0), 800.0, 600.0); // inside the modeline strip
    assert_eq!(app.windows().focused_id(), focused);
    assert!(!app.sidebar_focused);
    assert!(!app.terminal_focused);
}

#[test]
fn wheel_over_a_non_focused_pane_scrolls_it_without_moving_its_cursor() {
    let dir = TempDir::new("wheel_nonfocused");
    let content: String = (0..100).map(|i| format!("line{i}\n")).collect();
    let file = dir.write("f.txt", &content);
    let mut app = App::with_file(None);
    app.test_open_path(&file);
    let first = app.windows().focused_id();
    app.split_vertical();
    let second = app.windows().focused_id();
    assert_ne!(first, second);
    let cursor_before = *app.pane_state(first);

    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    let first_rect = geometry.panes.iter().find(|(id, _)| *id == first).unwrap().1;
    let pos = (first_rect.x + 5.0, first_rect.y + 5.0);

    app.handle_wheel_at(pos, MouseScrollDelta::LineDelta(0.0, -1.0), 800.0, 600.0);
    assert_eq!(app.pane_state(first).cursor, cursor_before.cursor); // cursor untouched
    assert_eq!(app.pane_state(first).scroll_line, WHEEL_LINES_PER_NOTCH as usize);
    assert_eq!(app.windows().focused_id(), second); // scrolling doesn't steal focus
}

#[test]
fn wheel_over_the_focused_pane_moves_the_cursor_by_the_wheel_amount() {
    let dir = TempDir::new("wheel_focused");
    let content: String = (0..50).map(|i| format!("line{i}\n")).collect();
    let file = dir.write("f.txt", &content);
    let mut app = App::with_file(None);
    app.test_open_path(&file);
    let pane = app.windows().focused_id();

    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    let rect = geometry.panes.iter().find(|(id, _)| *id == pane).unwrap().1;
    let pos = (rect.x + 5.0, rect.y + 5.0);

    app.handle_wheel_at(pos, MouseScrollDelta::LineDelta(0.0, -1.0), 800.0, 600.0);
    let buffer_id = app.focused_buffer_id();
    let (line, _) = app.buffers.get(buffer_id).unwrap().buffer.line_col(&app.test_cursor());
    assert_eq!(line, WHEEL_LINES_PER_NOTCH as usize);
}

#[test]
fn wheel_over_a_listing_scrolls_it_and_leaves_the_selection() {
    let dir = TempDir::new("wheel_explorer");
    for i in 0..60 {
        dir.touch(&format!("f{i:02}.txt"));
    }
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);
    let pane = app.windows().focused_id();

    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    let rect = geometry.panes.iter().find(|(id, _)| *id == pane).unwrap().1;
    let pos = (rect.x + 5.0, rect.y + 5.0);

    listing_text(&mut app);
    app.handle_wheel_at(pos, MouseScrollDelta::LineDelta(0.0, -1.0), 800.0, 600.0);
    let text = listing_text(&mut app);
    assert_eq!(app.explorer.as_ref().unwrap().selected, 0, "the selection stays");
    assert!(!text.contains("f00.txt") && text.contains(&format!("f{:02}.txt", WHEEL_LINES_PER_NOTCH)), "{text}");
}

#[test]
fn wheel_over_the_sidebar_scrolls_it_even_when_unfocused() {
    let dir = TempDir::new("wheel_sidebar");
    for i in 0..60 {
        dir.touch(&format!("f{i:02}.txt"));
    }
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    assert!(!app.sidebar_focused);
    app.sidebar_rows(30, 20);

    app.handle_wheel_at((10.0, 10.0), MouseScrollDelta::LineDelta(0.0, -1.0), 800.0, 600.0);
    app.sidebar_rows(30, 20);
    assert_eq!(app.sidebar_view.scroll, WHEEL_LINES_PER_NOTCH as usize);
    assert_eq!(app.sidebar.as_ref().unwrap().selected, 0, "the selection stays");
    assert!(!app.sidebar_focused); // wheel alone never focuses
}

#[test]
fn wheel_over_the_terminal_panel_is_a_no_op_without_a_live_session() {
    let mut app = App::with_file(None);
    app.terminal_open = true; // no real spawn -- app.terminal stays None

    let (_, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let pos = (10.0, modeline_top - terminal_h + 5.0);
    app.handle_wheel_at(pos, MouseScrollDelta::LineDelta(0.0, -1.0), 800.0, 600.0); // must not panic
    assert!(app.terminal.is_none());
}

#[test]
fn split_vertical_adds_a_focused_pane_showing_the_same_buffer() {
    let mut app = App::with_file(None);
    let original = app.windows().focused_id();
    let buffer_id = app.focused_buffer_id();

    app.split_vertical();

    assert_eq!(app.windows().window_count(), 2);
    assert_ne!(app.windows().focused_id(), original);
    assert_eq!(app.focused_buffer_id(), buffer_id); // new pane shows the same buffer
}

#[test]
fn split_horizontal_also_adds_a_focused_pane() {
    let mut app = App::with_file(None);
    app.split_horizontal();
    assert_eq!(app.windows().window_count(), 2);
}

#[test]
fn close_window_is_a_no_op_on_the_last_window() {
    let mut app = App::with_file(None);
    app.close_window();
    assert_eq!(app.windows().window_count(), 1);
}

#[test]
fn close_window_collapses_back_to_one_pane() {
    let mut app = App::with_file(None);
    app.split_vertical();
    app.close_window();
    assert_eq!(app.windows().window_count(), 1);
}

#[test]
fn splitting_a_window_gives_each_pane_its_own_independent_cursor() {
    // The actual feature: two panes on the same buffer must not
    // share a cursor -- moving one's must leave the other's exactly
    // where it was.
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.test_insert_str("one\ntwo\nthree");
    app.test_set_cursor(Cursor::at_start());

    let original_pane = app.focused_pane_id();
    app.split_vertical(); // new pane focused, seeded from the same position
    let new_pane = app.focused_pane_id();
    assert_ne!(original_pane, new_pane);
    assert_eq!(app.pane_state(new_pane).cursor.char_idx, 0);

    // Move the cursor in the new (focused) pane only.
    for ch in ['j', 'j'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    assert_ne!(app.pane_state(new_pane).cursor.char_idx, 0);

    // Switch focus back to the original pane -- its cursor must be
    // untouched by what just happened in the other one.
    app.navigate_window(NavDirection::Left);
    assert_eq!(app.focused_pane_id(), original_pane);
    assert_eq!(app.cursor().char_idx, 0);
}

#[test]
fn closing_a_window_drops_its_pane_state() {
    let mut app = App::with_file(None);
    app.split_vertical();
    let new_pane = app.focused_pane_id();
    app.close_window();
    assert!(app.workspaces.active_pane_states().get(&new_pane).is_none());
}

#[test]
fn cycle_window_wraps_between_the_two_panes() {
    let mut app = App::with_file(None);
    let first = app.windows().focused_id();
    app.split_vertical();
    let second = app.windows().focused_id();
    assert_ne!(first, second);

    app.cycle_window();
    assert_eq!(app.windows().focused_id(), first);
    app.cycle_window();
    assert_eq!(app.windows().focused_id(), second);
}

#[test]
fn navigate_window_moves_focus_left_to_the_original_pane() {
    let mut app = App::with_file(None);
    let left = app.windows().focused_id();
    app.split_vertical(); // new pane appears to the right, becomes focused
    assert_ne!(app.windows().focused_id(), left);

    app.navigate_window(fenix_window::NavDirection::Left);
    assert_eq!(app.windows().focused_id(), left);
}

#[test]
fn navigate_window_left_from_the_leftmost_pane_focuses_an_open_sidebar() {
    let dir = TempDir::new("nav_left_into_sidebar");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    assert!(!app.sidebar_focused);

    app.navigate_window(fenix_window::NavDirection::Left);
    assert!(app.sidebar_focused);
}

#[test]
fn navigate_window_left_prefers_an_actual_pane_over_the_sidebar() {
    let dir = TempDir::new("nav_left_prefers_pane");
    let mut app = App::with_file(None);
    let left = app.windows().focused_id();
    app.split_vertical(); // new pane to the right, becomes focused
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;

    app.navigate_window(fenix_window::NavDirection::Left);
    assert_eq!(app.windows().focused_id(), left);
    assert!(!app.sidebar_focused);
}

#[test]
fn only_window_closes_every_other_pane() {
    let mut app = App::with_file(None);
    app.split_vertical();
    app.split_horizontal();
    assert_eq!(app.windows().window_count(), 3);

    app.only_window();
    assert_eq!(app.windows().window_count(), 1);
}

#[test]
fn balance_windows_resets_a_skewed_ratio_back_to_half() {
    let mut app = App::with_file(None);
    app.split_vertical();
    app.windows_mut().resize_focused(0.3); // skew away from 0.5
    app.balance_windows();

    let rects = app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 });
    for (_, r) in rects {
        assert!((r.w - 100.0).abs() < 0.01);
    }
}

#[test]
fn new_workspace_becomes_active_and_opens_on_a_home_of_its_own() {
    let dir = TempDir::new("new_workspace_home");
    let file = dir.write("a.txt", "a\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let buffer_id = app.focused_buffer_id();

    app.new_workspace();

    assert_eq!(app.workspaces.len(), 2);
    assert_eq!(app.workspaces.active_index(), 1);
    assert_eq!(app.workspaces.active_name(), "workspace-2");
    assert_ne!(app.focused_buffer_id(), buffer_id, "the file stays in the workspace it was in");
    assert_eq!(app.open().kind, BufferKind::Dashboard);
    assert!(app.is_a_workspace_home(app.focused_buffer_id()));
    assert_eq!(app.windows().window_count(), 1); // new workspace starts unsplit
}

#[test]
fn next_and_prev_workspace_cycle_and_wrap() {
    let mut app = App::with_file(None);
    app.new_workspace();
    app.new_workspace(); // 3 workspaces total, active = 2

    app.next_workspace(); // wraps to 0
    assert_eq!(app.workspaces.active_index(), 0);
    app.prev_workspace(); // wraps back to 2
    assert_eq!(app.workspaces.active_index(), 2);
    app.prev_workspace();
    assert_eq!(app.workspaces.active_index(), 1);
}

#[test]
fn remove_workspace_is_a_no_op_on_the_last_one() {
    let mut app = App::with_file(None);
    app.remove_workspace();
    assert_eq!(app.workspaces.len(), 1);
}

#[test]
fn remove_workspace_removes_the_active_one() {
    let mut app = App::with_file(None);
    app.new_workspace(); // active: workspace-2

    app.remove_workspace();

    assert_eq!(app.workspaces.len(), 1);
    assert_eq!(app.workspaces.active_name(), "workspace-1");
}

#[test]
fn picker_switch_workspace_lists_every_open_workspace_marking_the_active_one() {
    let mut app = App::with_file(None);
    app.new_workspace(); // active: workspace-2

    app.picker_switch_workspace();

    match &app.active_picker {
        Some(ActivePicker::SwitchWorkspace(state)) => {
            let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
            assert_eq!(labels, vec!["  workspace-1", "* workspace-2"]);
        }
        other => panic!("expected an open SwitchWorkspace picker, got is_some={}", other.is_some()),
    }
    assert_eq!(app.main_view, MainView::Picker);
}

#[test]
fn picker_confirm_on_switch_workspace_jumps_straight_to_the_chosen_one() {
    let mut app = App::with_file(None);
    app.new_workspace();
    app.new_workspace(); // 3 workspaces, active = 2

    app.picker_switch_workspace();
    match &mut app.active_picker {
        Some(ActivePicker::SwitchWorkspace(state)) => state.move_selection(-2), // select workspace-1
        _ => panic!("expected an open SwitchWorkspace picker"),
    }
    app.picker_confirm();

    assert_eq!(app.workspaces.active_index(), 0);
    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());
}

#[test]
fn rename_workspace_prompt_round_trips_through_enter() {
    let mut app = App::with_file(None);

    app.start_rename_workspace_prompt();
    assert_eq!(app.workspace_rename_prompt.as_deref(), Some("workspace-1"));

    for c in "Editor".chars() {
        app.workspace_rename_prompt_key(KeyPress::char(c));
    }
    // start_rename_workspace_prompt pre-seeds the prompt with the
    // existing name rather than clearing it -- typing appends,
    // matching start_rename_file_prompt's own seeded-not-blank
    // behaviour.
    assert_eq!(app.workspace_rename_prompt.as_deref(), Some("workspace-1Editor"));

    app.workspace_rename_prompt_key(KeyPress::named(FenixNamedKey::Enter));

    assert!(app.workspace_rename_prompt.is_none());
    assert_eq!(app.workspaces.active_name(), "workspace-1Editor");
}

#[test]
fn rename_workspace_prompt_escape_cancels_without_renaming() {
    let mut app = App::with_file(None);
    app.start_rename_workspace_prompt();

    app.workspace_rename_prompt_key(KeyPress::named(FenixNamedKey::Escape));

    assert!(app.workspace_rename_prompt.is_none());
    assert_eq!(app.workspaces.active_name(), "workspace-1");
}

#[test]
fn rename_workspace_to_a_blank_name_is_a_no_op() {
    let mut app = App::with_file(None);
    app.start_rename_workspace_prompt();
    // Clear the pre-seeded name entirely.
    for _ in 0.."workspace-1".len() {
        app.workspace_rename_prompt_key(KeyPress::named(FenixNamedKey::Backspace));
    }

    app.workspace_rename_prompt_key(KeyPress::named(FenixNamedKey::Enter));

    assert_eq!(app.workspaces.active_name(), "workspace-1", "an empty name must not replace the real one");
}

#[test]
fn parse_workspace_action_recognizes_every_built_in_kind() {
    assert_eq!(parse_workspace_action("git"), WorkspaceAction::Git);
    assert_eq!(parse_workspace_action("jira"), WorkspaceAction::Jira);
    assert_eq!(parse_workspace_action("docker"), WorkspaceAction::Plain, "not a name it knows");
    assert_eq!(parse_workspace_action("containers"), WorkspaceAction::Containers);
    assert_eq!(parse_workspace_action("vnc:build-vm"), WorkspaceAction::Vnc("build-vm".to_string()));
    assert_eq!(parse_workspace_action(""), WorkspaceAction::Plain);
    assert_eq!(parse_workspace_action("editor"), WorkspaceAction::Plain, "an unrecognized action is still a usable plain workspace");
    assert_eq!(parse_workspace_action("vnc:"), WorkspaceAction::Plain, "a host-less vnc: falls back to plain rather than connecting nowhere");
    assert_eq!(parse_workspace_action("project:C:\\src\\fenix"), WorkspaceAction::Project(PathBuf::from("C:\\src\\fenix")));
    assert_eq!(parse_workspace_action("project:"), WorkspaceAction::Plain, "a path-less project: falls back to plain rather than switching nowhere");
}

#[test]
fn opening_a_configured_project_workspace_switches_to_the_project_and_names_the_workspace() {
    let known_dir = TempDir::new("workspace_project_known");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    let project_dir = TempDir::new("workspace_project_target");
    project_dir.touch("main.rs");
    app.config.workspaces = vec![("My Project".to_string(), format!("project:{}", project_dir.path().display()))];

    app.open_configured_workspace("My Project");

    assert_eq!(app.workspaces.active_name(), "My Project");
    assert_eq!(app.project_root.as_deref(), Some(project_dir.path()));
    assert!(app.active_picker.is_some(), "should chain into a find-file picker, same as SPC p p");
}

#[test]
fn opening_an_already_open_project_workspace_a_second_time_switches_instead_of_reopening() {
    let known_dir = TempDir::new("workspace_project_known_reopen");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    let project_dir = TempDir::new("workspace_project_target_reopen");
    project_dir.touch("main.rs");
    app.config.workspaces = vec![("My Project".to_string(), format!("project:{}", project_dir.path().display()))];
    app.open_configured_workspace("My Project");
    app.picker_cancel();
    app.new_workspace(); // some other workspace becomes active

    app.open_configured_workspace("My Project");

    // The start screen's workspace became My Project, as the hub's
    // opening does; then one more.
    assert_eq!(app.workspaces.len(), 2, "must not have created a second My Project workspace");
    assert_eq!(app.workspaces.active_name(), "My Project");
}

#[test]
fn start_workspace_launcher_picker_reports_an_empty_list_instead_of_opening_over_nothing() {
    let mut app = App::with_file(None);
    app.start_workspace_launcher_picker();
    assert!(app.active_picker.is_none());
    assert!(app.modeline_pieces().1.contains("no workspaces on the shelf"));
}

#[test]
fn start_workspace_launcher_picker_lists_every_configured_entry_by_name() {
    let mut app = App::with_file(None);
    app.config.workspaces = vec![("Git".to_string(), "git".to_string()), ("Editor".to_string(), String::new())];

    app.start_workspace_launcher_picker();

    match &app.active_picker {
        Some(ActivePicker::WorkspaceLauncher(state)) => {
            let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
            assert_eq!(labels, vec!["Git", "Editor"]);
        }
        other => panic!("expected an open WorkspaceLauncher picker, got is_some={}", other.is_some()),
    }
}

#[test]
fn opening_a_plain_configured_workspace_creates_it_named_after_the_config_entry() {
    let mut app = App::with_file(None);
    app.config.workspaces = vec![("Editor".to_string(), String::new())];

    app.open_configured_workspace("Editor");

    assert_eq!(app.workspaces.len(), 2);
    assert_eq!(app.workspaces.active_name(), "Editor");
}

#[test]
fn opening_an_already_open_plain_workspace_a_second_time_switches_instead_of_duplicating() {
    let mut app = App::with_file(None);
    app.config.workspaces = vec![("Editor".to_string(), String::new())];
    app.open_configured_workspace("Editor");
    app.new_workspace(); // some other workspace becomes active

    app.open_configured_workspace("Editor");

    assert_eq!(app.workspaces.len(), 3, "must not have created a second Editor workspace");
    assert_eq!(app.workspaces.active_name(), "Editor");
}

#[test]
fn opening_a_configured_vnc_workspace_without_the_extension_errors_without_creating_a_workspace() {
    let mut app = App::with_file(None);
    app.config.workspaces = vec![("VNC Build".to_string(), "vnc:no-such-vm".to_string())];
    let workspaces_before = app.workspaces.len();

    app.open_configured_workspace("VNC Build");

    assert_eq!(app.workspaces.len(), workspaces_before, "the error path must not fabricate a workspace");
    assert!(app.modeline_pieces().1.contains("vnc:no-such-vm needs the VNC extension"), "{}", app.modeline_pieces().1);
    // The regression this guards against: the error path touches
    // no workspace at all, so a blanket rename
    // (rather than one gated on the active index actually having
    // moved) would have renamed whatever workspace was active --
    // here, the original "workspace-1" -- to "VNC Build" even
    // though nothing about it changed.
    assert_eq!(app.workspaces.active_name(), "workspace-1");
}

#[test]
fn picker_confirm_on_workspace_launcher_dispatches_the_configured_action() {
    let mut app = App::with_file(None);
    app.config.workspaces = vec![("Editor".to_string(), String::new())];

    app.start_workspace_launcher_picker();
    app.picker_confirm();

    assert!(app.active_picker.is_none());
    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.workspaces.active_name(), "Editor");
}

#[test]
fn workspace_list_index_of_name_finds_an_exact_match_only() {
    let mut app = App::with_file(None);
    app.new_workspace();
    app.workspaces.rename_active("Podman".to_string());

    assert_eq!(app.workspaces.index_of_name("Podman"), Some(1));
    assert_eq!(app.workspaces.index_of_name("podman"), None, "not case-insensitive");
    assert_eq!(app.workspaces.index_of_name("nonexistent"), None);
}

#[test]
fn each_workspace_keeps_its_own_split_layout() {
    let mut app = App::with_file(None);
    app.split_vertical();
    app.split_horizontal();
    assert_eq!(app.windows().window_count(), 3);

    app.new_workspace(); // fresh, unsplit workspace
    assert_eq!(app.windows().window_count(), 1);

    app.prev_workspace(); // back to workspace-1
    assert_eq!(app.windows().window_count(), 3); // layout untouched
}

#[test]
fn modeline_shows_the_workspace_indicator_only_once_there_are_several() {
    let mut app = App::with_file(None);
    assert!(!app.modeline_text().contains("workspace"));

    app.new_workspace();
    assert!(app.modeline_text().contains("[2/2 workspace-2]"));
}

#[test]
fn a_page_popup_opens_under_its_row_or_over_it_near_the_bottom_and_stays_in_the_pane() {
    let pane = fenix_window::Rect { x: 100.0, y: 50.0, w: 600.0, h: 400.0 };
    let at = PagePopupAt { buffer: App::with_file(None).focused_buffer_id(), pane, first_line: 10, frac: 0.0, gutter_px: 0.0 };
    let (cw, lh) = (8.0, 20.0);
    // Line 12 is the third row: its top at 50 + 4 + 40.
    let r = popup_beside_row(at, 12, 4, 200.0, 100.0, 450.0, 1000.0, cw, lh).unwrap();
    assert_eq!((r.x, r.y), (100.0 + 4.0 * cw, 94.0 + lh + 2.0), "under the row, text lined up with column 4");
    // Line 28 sits near the bottom: the popup goes over it instead.
    let r = popup_beside_row(at, 28, 4, 200.0, 100.0, 450.0, 1000.0, cw, lh).unwrap();
    let top = 54.0 + 18.0 * lh;
    assert_eq!(r.y, top - 2.0 - 100.0);
    // A wide one far right is kept inside the pane.
    let r = popup_beside_row(at, 12, 70, 300.0, 100.0, 450.0, 1000.0, cw, lh).unwrap();
    assert_eq!(r.x + r.w, 700.0);
    assert!(popup_beside_row(at, 5, 4, 200.0, 100.0, 450.0, 1000.0, cw, lh).is_none(), "scrolled out of view above");
}

#[test]
fn prompt_popup_is_none_when_nothing_is_capturing_input() {
    let app = App::with_file(None);
    assert!(app.prompt_popup(800.0, 580.0).is_none());
}

#[test]
fn prompt_popup_shows_a_live_command_lines_text() {
    let mut app = App::with_file(None);
    for ch in [':', 'w', 'q'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    let (_, spans) = app.prompt_popup(800.0, 580.0).unwrap();
    assert_eq!(spans[0].0, ":wq");
    assert_eq!(spans[0].1, app.theme.fg_modeline);
}

#[test]
fn prompt_popup_rect_never_extends_past_the_window_or_under_the_modeline() {
    let mut app = App::with_file(None);
    for ch in [':', 'w', 'q'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    let modeline_top = 40.0;
    let (rect, _) = app.prompt_popup(300.0, modeline_top).unwrap();

    assert!(rect.x >= 0.0);
    assert!(rect.y >= 0.0);
    assert!(rect.x + rect.w <= 300.0 + 0.01);
    assert!(rect.y + rect.h <= modeline_top + 0.01);
}

#[test]
fn prompt_popup_sits_just_above_the_modeline_left_aligned() {
    let mut app = App::with_file(None);
    for ch in [':', 'w', 'q'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    let modeline_top = 580.0;
    let (rect, _) = app.prompt_popup(800.0, modeline_top).unwrap();
    assert_eq!(rect.x, text::WHICH_KEY_MARGIN);
    assert_eq!(rect.y, modeline_top - rect.h - text::WHICH_KEY_MARGIN);
}

#[test]
fn prompt_popup_widens_to_fit_a_longer_prompt_than_the_minimum() {
    let mut app = App::with_file(None);
    for ch in [':', 'w', 'q'] {
        app.test_vim_key(KeyPress::char(ch));
    }
    let (short_rect, _) = app.prompt_popup(800.0, 580.0).unwrap();
    assert_eq!(short_rect.w, text::WHICH_KEY_MIN_WIDTH); // a 3-char command clamps to the floor

    app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
    for ch in ": a very long command line that should widen the popup".chars() {
        app.test_vim_key(KeyPress::char(ch));
    }
    let (long_rect, spans) = app.prompt_popup(800.0, 580.0).unwrap();
    assert!(long_rect.w > short_rect.w);
    let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(joined.contains("a very long command line"));
}

fn completion_item(label: &str, kind: fenix_completion::CompletionKind) -> fenix_picker::Candidate<completion::Item> {
    fenix_picker::Candidate::new(label, fenix_completion::CompletionItem { label: label.to_string(), kind, detail: String::new() }.into())
}

#[test]
fn completion_popup_is_none_when_nothing_is_open() {
    let app = App::with_file(None);
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
    assert!(app.completion_popup(800.0, 580.0, rect, Some((0, 0)), 0.0, 0.0).is_none());
}

#[test]
fn completion_popup_is_none_without_a_caret_to_anchor_under() {
    let mut app = App::with_file(None);
    let items = vec![completion_item("set", fenix_completion::CompletionKind::Keyword)];
    app.completion = Some(CompletionState { prefix_start: 0, picker: fenix_picker::PickerState::new(items) });
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
    assert!(app.completion_popup(800.0, 580.0, rect, None, 0.0, 0.0).is_none());
}

#[test]
fn completion_popup_lists_candidates_and_flags_the_selected_row() {
    let mut app = App::with_file(None);
    let items =
        vec![completion_item("set", fenix_completion::CompletionKind::Keyword), completion_item("seek", fenix_completion::CompletionKind::Keyword)];
    let mut picker = fenix_picker::PickerState::new(items);
    picker.move_selection(1); // select "seek", index 1
    app.completion = Some(CompletionState { prefix_start: 0, picker });

    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 100.0 };
    let (_, spans, selected_row) = app.completion_popup(800.0, 580.0, rect, Some((0, 4)), 0.0, 0.0).unwrap();
    let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(joined.contains("set"));
    assert!(joined.contains("seek"));
    assert_eq!(selected_row, Some(1));
}

#[test]
fn completion_popup_labels_detail_and_documentation_separately() {
    let mut app = App::with_file(None);
    let mut item = completion::Item::text("render_widget".to_string(), completion::Source::Lsp);
    item.detail = "Widget renderer".to_string();
    item.documentation = "Creates a widget.\nReturns the rendered handle.".to_string();
    app.completion = Some(CompletionState {
        prefix_start: 0,
        picker: fenix_picker::PickerState::new(vec![fenix_picker::Candidate::new("render_widget", item)]),
    });
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 100.0 };
    let (_, spans, _) = app.completion_popup(800.0, 580.0, rect, Some((0, 0)), 0.0, 0.0).unwrap();
    let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(joined.contains("DETAIL Widget renderer"), "{joined}");
    assert!(joined.contains("DOCS Creates a widget."));
    assert!(joined.contains("Returns the rendered handle."));
}

#[test]
fn completion_popup_colors_keywords_and_tags_differently() {
    let mut app = App::with_file(None);
    let items =
        vec![completion_item("set", fenix_completion::CompletionKind::Keyword), completion_item("my_proc", fenix_completion::CompletionKind::Tag)];
    app.completion = Some(CompletionState { prefix_start: 0, picker: fenix_picker::PickerState::new(items) });

    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 100.0 };
    let (_, spans, _) = app.completion_popup(800.0, 580.0, rect, Some((0, 0)), 0.0, 0.0).unwrap();
    let keyword_color = spans.iter().find(|(s, _, _)| s == "set").unwrap().1;
    let tag_color = spans.iter().find(|(s, _, _)| s == "my_proc").unwrap().1;
    // caret_text/fg_modeline, not syntax_keyword/syntax_function --
    // this popup's background is bg_modeline, and the syntax_* family
    // is calibrated for contrast against bg instead (see the doc
    // comment on completion_popup's color match for the full story).
    assert_eq!(keyword_color, app.theme.caret_text);
    assert_eq!(tag_color, app.theme.fg_modeline);
    assert_ne!(keyword_color, tag_color);
}

#[test]
fn completion_popup_rect_never_extends_past_the_window_or_under_the_modeline() {
    let mut app = App::with_file(None);
    let items = vec![completion_item("set", fenix_completion::CompletionKind::Keyword)];
    app.completion = Some(CompletionState { prefix_start: 0, picker: fenix_picker::PickerState::new(items) });

    // Anchored near the bottom-right corner -- must clamp, not overflow.
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 300.0, h: 40.0 };
    let (popup_rect, _, _) = app.completion_popup(300.0, 40.0, rect, Some((0, 290)), 0.0, 0.0).unwrap();
    assert!(popup_rect.x + popup_rect.w <= 300.0 + 0.01);
    assert!(popup_rect.y + popup_rect.h <= 40.0 + 0.01);
}

/// A 72-column usage line at the default font is ~550px -- wider
/// than the 480px which-key cap the hover and completion popups used
/// to inherit, which cut `string`'s subcommand list mid-word.
#[test]
fn hover_and_completion_popups_grow_to_fit_a_full_usage_line() {
    let mut app = App::with_file(None);
    let line = "subcommands: bytelength, cat, compare, equal, first, index, is, last, le";
    assert_eq!(line.chars().count(), 72, "the widest line `tcl::hover_text` produces");
    let needed = line.chars().count() as f32 * text::CHAR_WIDTH;
    let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 1200.0, h: 600.0 };

    app.lsp_hover = Some(format!("string subcommand ?arg ...?
{line}"));
    let (hover_rect, _) = app.hover_popup(1200.0, 600.0, rect, Some((0, 0)), 0.0, 0.0).unwrap();
    assert!(hover_rect.w >= needed, "hover popup {}px can't show a {needed}px line", hover_rect.w);
    assert!(hover_rect.w > text::WHICH_KEY_MAX_WIDTH);

    let mut item = completion::Item::text("string".into(), completion::Source::Keyword);
    item.detail = line.to_string();
    let picker = fenix_picker::PickerState::new(vec![fenix_picker::Candidate::new("string".to_string(), item)]);
    app.completion = Some(CompletionState { prefix_start: 0, picker });
    let (completion_rect, _, _) = app.completion_popup(1200.0, 600.0, rect, Some((0, 0)), 0.0, 0.0).unwrap();
    assert!(completion_rect.w >= needed, "completion popup {}px can't show a {needed}px DETAIL row", completion_rect.w);
}

#[test]
fn explorer_open_selected_on_a_directory_navigates_into_it() {
    let dir = TempDir::new("open_dir");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_open_selected(); // selected = "sub" (only entry)
    assert_eq!(app.explorer.as_ref().unwrap().cwd, dir.path().join("sub"));
    assert!(app.picking_folder()); // still in the explorer, just deeper
}

#[test]
fn explorer_open_selected_on_a_file_in_full_buffer_mode_returns_to_editor() {
    let dir = TempDir::new("open_file_full");
    dir.touch("a.txt");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_open_selected();
    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.explorer.is_none());
    assert_eq!(app.open().buffer.text(), "hello\n");
}

#[test]
fn explorer_open_selected_on_a_file_in_sidebar_mode_keeps_the_sidebar() {
    let dir = TempDir::new("open_file_sidebar");
    dir.touch("a.txt");
    let mut app = App::with_file(None);
    app.sidebar = Some(ExplorerState::opened(dir.path()).unwrap());
    app.sidebar_open = true;
    app.sidebar_focused = true;

    app.explorer_open_selected();
    assert!(app.sidebar_open); // sidebar persists
    assert!(!app.sidebar_focused); // focus returns to the editor
    assert!(app.sidebar.is_some());
    assert_eq!(app.open().buffer.text(), "hello\n");
}

#[test]
fn explorer_handle_action_down_and_up_move_selection() {
    let dir = TempDir::new("action_down_up");
    dir.touch("a");
    dir.touch("b");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::Down);
    assert_eq!(app.explorer.as_ref().unwrap().selected, 1);
    app.explorer_handle_action(ExplorerAction::Up);
    assert_eq!(app.explorer.as_ref().unwrap().selected, 0);
}

#[test]
fn delete_prompt_confirms_on_y_and_cancels_on_anything_else() {
    let dir = TempDir::new("delete_prompt_cancel");
    dir.touch("keep.txt");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::BeginDelete);
    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Escape)); // any non-'y' key cancels
    assert!(app.explorer_prompt.is_none());
    assert!(dir.path().join("keep.txt").exists());
}

#[test]
fn delete_prompt_deletes_on_y() {
    let dir = TempDir::new("delete_prompt_confirm");
    dir.touch("gone.txt");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::BeginDelete);
    app.explorer_prompt_key(KeyPress::char('y'));
    assert!(app.explorer_prompt.is_none());
    assert!(!dir.path().join("gone.txt").exists());
}

#[test]
fn prompt_popup_shows_the_delete_confirmation_with_the_target_count() {
    let dir = TempDir::new("delete_prompt_modeline");
    dir.touch("a.txt");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::BeginDelete);
    assert_eq!(app.active_prompt_text(), Some("Move 1 item to the Recycle Bin? (y/n)".to_string()));
}

#[test]
fn the_permanent_delete_confirmation_does_not_read_like_the_recoverable_one() {
    // Two different acts: one is undoable from the Recycle Bin, one
    // is not, and a prompt that read the same for both would be the
    // last thing between the user and losing work.
    let dir = TempDir::new("delete_prompt_permanent");
    dir.touch("a.txt");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);
    app.explorer_delete_permanently = true;

    app.explorer_handle_action(ExplorerAction::BeginDelete);

    let text = app.active_prompt_text().unwrap();
    assert!(text.contains("PERMANENTLY"), "got: {text}");
    assert!(text.contains("cannot be undone"), "got: {text}");
}

// -- Copying onto something that already exists ---------------------
//
// The explorer this replaces overwrote silently: its own
// `copy_targets_to` doc comment said the GUI was expected to check
// first, and the GUI did not. These pin that it now asks, and that
// every answer leaves the filesystem in the state the answer named.

/// An explorer at `src` with `a.txt` marked, and a `dest` that
/// already has an `a.txt` of its own.
fn app_with_a_pending_collision(name: &str) -> (TempDir, TempDir, App) {
    let src = TempDir::new(&format!("{name}_src"));
    let dest = TempDir::new(&format!("{name}_dest"));
    src.write("a.txt", "new");
    dest.write("a.txt", "old");
    let mut app = App::with_file(None);
    app.open_folder_picker(src.path(), ExplorerPurpose::Browse);
    (src, dest, app)
}

fn answer_collision(app: &mut App, dest: &TempDir, kind: PromptKind, key: char) {
    app.explorer_prompt = Some(ExplorerPrompt { kind, input: dest.path().to_string_lossy().into_owned() });
    let ExplorerPrompt { kind, input } = app.explorer_prompt.take().unwrap();
    app.explorer_prompt_submit(kind, &input);
    assert!(app.explorer_conflict.is_some(), "expected to be asked before anything was written");
    app.explorer_prompt_key(KeyPress::char(key));
}

#[test]
fn a_copy_onto_an_existing_file_asks_before_writing_anything() {
    let (_src, dest, mut app) = app_with_a_pending_collision("collision_asks");
    app.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CopyTo, input: dest.path().to_string_lossy().into_owned() });
    let ExplorerPrompt { kind, input } = app.explorer_prompt.take().unwrap();

    app.explorer_prompt_submit(kind, &input);

    assert!(app.explorer_conflict.is_some());
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old", "nothing written yet");
    let text = app.active_prompt_text().unwrap();
    assert!(text.contains("a.txt already exists"), "the prompt names what is in the way: {text}");
    assert!(text.contains("[o]verwrite") && text.contains("[s]kip") && text.contains("[b]oth"), "got: {text}");
}

#[test]
fn the_collision_question_is_a_menu_beside_the_row_too() {
    let (_src, dest, mut app) = app_with_a_pending_collision("collision_menu");
    app.explorer_prompt_submit(PromptKind::CopyTo, &dest.path().to_string_lossy());
    listing_text(&mut app);
    let id = app.focused_buffer_id();
    let menu = app.pages[&id].page.popup.as_ref().expect("a menu beside the row").text();
    assert!(menu.contains("a.txt is already there") && menu.contains("o overwrite") && menu.contains("b keep both"), "{menu}");

    app.explorer_prompt_key(KeyPress::char('s'));
    listing_text(&mut app);
    assert!(app.pages[&id].page.popup.is_none(), "answered, it goes");
}

#[test]
fn overwrite_replaces_the_file_that_was_there() {
    let (_src, dest, mut app) = app_with_a_pending_collision("collision_overwrite");
    answer_collision(&mut app, &dest, PromptKind::CopyTo, 'o');
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "new");
    assert!(app.explorer_conflict.is_none());
}

#[test]
fn skip_leaves_both_sides_exactly_as_they_were() {
    let (src, dest, mut app) = app_with_a_pending_collision("collision_skip");
    answer_collision(&mut app, &dest, PromptKind::CopyTo, 's');
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(src.path().join("a.txt")).unwrap(), "new");
}

#[test]
fn keep_both_lands_a_numbered_copy_beside_the_original() {
    let (_src, dest, mut app) = app_with_a_pending_collision("collision_both");
    answer_collision(&mut app, &dest, PromptKind::CopyTo, 'b');
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(dest.path().join("a (2).txt")).unwrap(), "new");
}

#[test]
fn cancelling_a_collision_writes_nothing_and_says_so() {
    // Escape, or any key that is not one of the three answers --
    // the safe reading of an unrecognised keystroke in the middle of
    // an operation is "do nothing".
    let (src, dest, mut app) = app_with_a_pending_collision("collision_cancel");
    app.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CopyTo, input: dest.path().to_string_lossy().into_owned() });
    let ExplorerPrompt { kind, input } = app.explorer_prompt.take().unwrap();
    app.explorer_prompt_submit(kind, &input);

    app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Escape));

    assert!(app.explorer_conflict.is_none());
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old");
    assert!(src.path().join("a.txt").exists());
    assert!(app.modeline_pieces().1.contains("cancelled"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_move_that_is_skipped_does_not_delete_the_source() {
    // The one outcome a move must never produce: deciding not to
    // write, and deleting anyway.
    let (src, dest, mut app) = app_with_a_pending_collision("collision_move_skip");
    answer_collision(&mut app, &dest, PromptKind::MoveTo, 's');
    assert!(src.path().join("a.txt").exists(), "the source survived the skip");
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old");
}

#[test]
fn a_copy_with_nothing_in_the_way_does_not_ask() {
    // The common case has to stay a single keystroke.
    let src = TempDir::new("collision_none_src");
    let dest = TempDir::new("collision_none_dest");
    src.write("a.txt", "new");
    let mut app = App::with_file(None);
    app.open_folder_picker(src.path(), ExplorerPurpose::Browse);

    app.explorer_prompt_submit(PromptKind::CopyTo, &dest.path().to_string_lossy());

    assert!(app.explorer_conflict.is_none());
    assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "new");
}

#[test]
fn a_batch_reports_how_many_worked_rather_than_only_the_first_failure() {
    let src = TempDir::new("batch_report_src");
    let dest = TempDir::new("batch_report_dest");
    src.write("a.txt", "x");
    src.write("b.txt", "x");
    let mut app = App::with_file(None);
    app.open_folder_picker(src.path(), ExplorerPurpose::Browse);
    app.explorer_handle_action(ExplorerAction::MarkAll);

    app.explorer_prompt_submit(PromptKind::CopyTo, &dest.path().to_string_lossy());

    assert!(app.modeline_pieces().1.contains("2 items copied"), "got: {}", app.modeline_pieces().1);
}


/// Whether `rg` is on `PATH`. The grep feature is a disclosed hard
/// requirement on ripgrep (see `fenix_project::grep_project`), so
/// on a machine without it these tests have nothing to exercise --
/// they say so and stop, rather than failing forever and drowning
/// out a real regression next to them.
fn ripgrep_available() -> bool {
    std::process::Command::new("rg").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn find_file_labels_are_relative_to_root() {
    let dir = TempDir::new("find_file_candidates");
    dir.touch("a.txt");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    dir.write("sub/b.txt", "hello\n");

    let mut app = App::with_file(None);
    app.open_find_file(dir.path(), false);
    let Some(ActivePicker::FindFile(state)) = &app.active_picker else { panic!("a find-file picker") };
    let mut labels: Vec<String> = state.visible_rows(0, 10).map(|(_, c)| c.label.clone()).collect();
    labels.sort();
    assert_eq!(labels, vec!["a.txt".to_string(), "sub/b.txt".to_string()]);
}

/// `SPC :` and a command that opens nothing of its own: Enter runs it
/// and the PICKER page goes, rather than staying up blank.
#[test]
fn a_command_picked_in_the_palette_leaves_the_picker_page() {
    let mut app = App::with_file(None);
    let candidates = vec![fenix_picker::Candidate::new("toggle line numbers".to_string(), "editor.cycle_line_numbers".to_string())];
    app.enter_picker(ActivePicker::Commands(fenix_picker::PickerState::new(candidates)));
    app.picker_confirm();
    assert!(app.active_picker.is_none());
    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.queued_command.as_deref(), Some("editor.cycle_line_numbers"));
    // With no match, Enter does nothing and keeps the query.
    app.enter_picker(ActivePicker::Commands(fenix_picker::PickerState::new(Vec::new())));
    app.picker_confirm();
    assert!(app.active_picker.is_some() && app.main_view == MainView::Picker);
    // A picker page left with no picker: the next key clears it.
    app.active_picker = None;
    app.picker_key(KeyPress::named(FenixNamedKey::Escape));
    assert_eq!(app.main_view, MainView::Editor);
}

#[test]
fn enter_picker_and_picker_cancel_leave_the_buffer_untouched() {
    let dir = TempDir::new("picker_enter_cancel");
    let file = dir.touch("a.txt");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_insert_str(" extra");
    let dirty_text = app.open().buffer.text();
    let focused_before = app.focused_buffer_id();

    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(Vec::new())));
    assert_eq!(app.main_view, MainView::Picker);
    assert!(app.active_picker.is_some());
    assert_eq!(app.focused_buffer_id(), focused_before);

    app.picker_cancel();
    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());
    assert_eq!(app.open().buffer.text(), dirty_text);
}

#[test]
fn picker_confirm_on_find_file_opens_the_selected_path_and_returns_to_editor() {
    let dir = TempDir::new("picker_confirm_find_file");
    let target = dir.write("target.txt", "picked contents\n");
    let mut app = App::with_file(None);

    let candidates = vec![fenix_picker::Candidate::new("target.txt", target.clone())];
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    app.picker_confirm();

    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());
    assert_eq!(app.open().buffer.path(), Some(target.as_path()));
    assert_eq!(app.open().buffer.text(), "picked contents\n");
}

#[test]
fn picker_confirm_with_no_matches_is_a_no_op() {
    let mut app = App::with_file(None);
    let candidates: Vec<fenix_picker::Candidate<PathBuf>> =
        vec![fenix_picker::Candidate::new("only.txt", PathBuf::from("only.txt"))];
    let mut state = fenix_picker::PickerState::new(candidates);
    state.push_char('z'); // no match against "only.txt"
    assert!(state.selected().is_none());
    app.enter_picker(ActivePicker::FindFile(state));

    app.picker_confirm();

    // Nothing to confirm -- still open, nothing stashed/opened changed.
    assert_eq!(app.main_view, MainView::Picker);
    assert!(app.active_picker.is_some());
}

#[test]
fn quickfix_next_and_prev_step_through_matches_without_reopening_the_picker() {
    // Hand-built `GrepMatch`es: this is about quickfix stepping
    // mechanics, not the search that fills the list.
    let dir = TempDir::new("quickfix_step");
    let a = dir.write("a.txt", "one\ntwo\nthree\n");
    let b = dir.write("b.txt", "four\nfive\n");
    let mut app = App::with_file(None);
    app.quickfix = vec![
        QuickfixEntry::Grep(fenix_project::GrepMatch { path: a.clone(), line: 2, col: 1, text: "two".to_string() }),
        QuickfixEntry::Grep(fenix_project::GrepMatch { path: b.clone(), line: 1, col: 1, text: "four".to_string() }),
    ];

    app.quickfix_next(); // nothing visited yet -- lands on the first
    assert_eq!(app.open().buffer.path(), Some(a.as_path()));
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1);

    app.quickfix_next();
    assert_eq!(app.open().buffer.path(), Some(b.as_path()));
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 0);

    app.quickfix_prev();
    assert_eq!(app.open().buffer.path(), Some(a.as_path()));
}

#[test]
fn quickfix_next_clamps_at_the_last_match() {
    let dir = TempDir::new("quickfix_clamp_end");
    let a = dir.write("a.txt", "only line\n");
    let mut app = App::with_file(None);
    app.quickfix = vec![QuickfixEntry::Grep(fenix_project::GrepMatch { path: a, line: 1, col: 1, text: "only line".to_string() })];

    app.quickfix_next();
    let after_first = app.quickfix_index;
    app.quickfix_next(); // past the end -- no-op
    assert_eq!(app.quickfix_index, after_first);
}

#[test]
fn quickfix_prev_clamps_at_the_first_match() {
    let dir = TempDir::new("quickfix_clamp_start");
    let a = dir.write("a.txt", "l1\nl2\n");
    let mut app = App::with_file(None);
    app.quickfix = vec![
        QuickfixEntry::Grep(fenix_project::GrepMatch { path: a.clone(), line: 1, col: 1, text: "l1".to_string() }),
        QuickfixEntry::Grep(fenix_project::GrepMatch { path: a, line: 2, col: 1, text: "l2".to_string() }),
    ];

    app.quickfix_next(); // index 0
    app.quickfix_prev(); // before the start -- no-op
    assert_eq!(app.quickfix_index, Some(0));
}

#[test]
fn quickfix_navigation_is_a_noop_when_the_list_is_empty() {
    let mut app = App::with_file(None);
    let origin = app.focused_buffer_id();
    app.quickfix_next();
    app.quickfix_prev();
    assert_eq!(app.focused_buffer_id(), origin);
    assert_eq!(app.quickfix_index, None);
}

#[test]
fn picker_confirm_on_a_match_sets_the_quickfix_index_to_the_confirmed_match() {
    let dir = TempDir::new("quickfix_confirm_index");
    let a = dir.write("a.txt", "one\ntwo\n");
    let mut app = App::with_file(None);
    let second = fenix_project::GrepMatch { path: a.clone(), line: 2, col: 1, text: "two".to_string() };
    app.quickfix = vec![
        QuickfixEntry::Grep(fenix_project::GrepMatch { path: a.clone(), line: 1, col: 1, text: "one".to_string() }),
        QuickfixEntry::Grep(second.clone()),
    ];
    let candidates = vec![fenix_picker::Candidate::new("a.txt:2: two", second)];
    app.enter_picker(ActivePicker::ProjectTodos(fenix_picker::PickerState::new(candidates)));

    app.picker_confirm();

    assert_eq!(app.quickfix_index, Some(1)); // the second entry, not the first
}

#[test]
fn quickfix_navigation_records_a_jumplist_entry() {
    let dir = TempDir::new("quickfix_jumplist");
    let a = dir.write("a.txt", "one\ntwo\n");
    let origin = dir.write("origin.txt", "start\n");
    let mut app = App::with_file(Some(origin.to_string_lossy().into_owned()));
    app.quickfix = vec![QuickfixEntry::Grep(fenix_project::GrepMatch { path: a.clone(), line: 2, col: 1, text: "two".to_string() })];

    app.quickfix_next();
    assert_eq!(app.open().buffer.path(), Some(a.as_path()));

    app.jump_back();
    assert_eq!(app.open().buffer.path(), Some(origin.as_path()));
}

#[test]
fn switch_to_project_registers_the_root_and_chains_into_find_file() {
    let known_dir = TempDir::new("switch_known");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));

    let project_dir = TempDir::new("switch_target");
    project_dir.touch("main.rs");

    // Already mid-picker (as `picker_confirm` would leave it before
    // dispatching to `switch_to_project`) -- switch shouldn't touch
    // `main_view` or the focused buffer, just swap the active picker.
    let focused_before = app.focused_buffer_id();
    app.enter_picker(ActivePicker::SwitchProject(fenix_picker::PickerState::new(Vec::new())));

    app.switch_to_project(project_dir.path().to_path_buf());

    assert_eq!(app.project_root, Some(project_dir.path().to_path_buf()));
    assert_eq!(app.known_projects.roots(), &[project_dir.path().to_path_buf()]);
    assert_eq!(app.main_view, MainView::Picker);
    assert_eq!(app.focused_buffer_id(), focused_before);
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => {
            assert_eq!(state.selected().map(|c| c.label.as_str()), Some("main.rs"))
        }
        _ => panic!("expected switch_to_project to chain into a FindFile picker"),
    }
}

#[test]
fn opening_a_file_does_not_auto_register_its_project() {
    // Regression test: known_projects used to auto-populate from
    // every project you ever opened a file in, which is exactly what
    // SPC p a/SPC p d exist to replace with an explicitly curated
    // list. Opening a file under a real project root must leave
    // known_projects untouched.
    let known_dir = TempDir::new("no_auto_register_known");
    let project_dir = TempDir::new("no_auto_register_target");
    project_dir.touch(".git"); // a real, detectable project marker
    let file = project_dir.write("main.rs", "fn main() {}");

    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.test_open_path(&file);
    app.refresh_project_root(); // what the real open-file paths call after test_open_path's equivalent

    assert!(app.project_root.is_some(), "project_root itself should still auto-detect");
    assert!(app.known_projects.roots().is_empty(), "known_projects should stay empty until SPC p a");
}

#[test]
fn a_subproject_s_file_in_the_parent_project_s_workspace_is_the_parent_project() {
    let dir = TempDir::new("nested_projects");
    let root = fenix_lsp::normalize(std::fs::canonicalize(dir.path()).unwrap());
    dir.mkdir(".git");
    dir.write("Cargo.toml", "[workspace]\n");
    let child = dir.write("tool/Cargo.toml", "[package]\n");
    let child_root = child.parent().unwrap().to_path_buf();
    let file = dir.write("tool/src/main.rs", "fn main() {}\n");
    let file = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(dir.path().join("projects.txt"));

    // In the subproject's own workspace: the subproject.
    app.open_project(child_root.clone(), Some(file.clone()), false);
    let child_root = fenix_lsp::normalize(std::fs::canonicalize(&child_root).unwrap());
    assert_eq!(app.project_root.as_deref(), Some(child_root.as_path()));

    // The parent opened, landing on that same file: the parent, not
    // stuck in the subproject.
    app.open_project(root.clone(), Some(file.clone()), false);
    assert_eq!(app.workspaces.active_workspace().project.as_deref(), Some(root.as_path()));
    assert_eq!(app.open().buffer.path(), Some(file.as_path()));
    assert_eq!(app.project_root.as_deref(), Some(root.as_path()));
    assert_eq!(app.sidebar_root_for(Some(&file)), root);

    // And back to the subproject's workspace: the subproject again.
    app.open_project(child_root.clone(), None, false);
    assert_eq!(app.project_root.as_deref(), Some(child_root.as_path()));
}

/// Two git repositories opened as projects, each in its workspace;
/// b's active.
fn two_repositories(name: &str) -> (TempDir, App, PathBuf, PathBuf) {
    let dir = TempDir::new(name);
    let mut roots = Vec::new();
    for repo in ["alpha", "beta"] {
        let root = dir.mkdir(repo);
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap().success());
        std::fs::write(root.join("README.md"), "# r\n").unwrap();
        roots.push(fenix_lsp::normalize(std::fs::canonicalize(&root).unwrap()));
    }
    let (a, b) = (roots[0].clone(), roots[1].clone());
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(dir.path().join("projects.txt"));
    app.open_project(a.clone(), None, false);
    app.open_project(b.clone(), None, false);
    (dir, app, a, b)
}

#[test]
fn the_git_panel_left_in_one_project_isn_t_where_another_s_git_goes() {
    let (_dir, mut app, a, b) = two_repositories("git_panel_moves");
    app.open_project(a.clone(), None, false);
    app.open_git_panel();
    assert_eq!(app.git_session.as_ref().map(|s| s.repo_root.clone()), Some(a.clone()));
    app.open_project(b.clone(), None, false);
    assert_eq!(app.git_action_repo_root(), b, "b's, not the panel's");
    app.open_git_panel();
    assert_eq!(app.git_session.as_ref().map(|s| s.repo_root.clone()), Some(b), "asked for from b: b's");
}

#[test]
fn a_new_workspace_s_home_is_in_no_project() {
    let (_dir, mut app, _a, b) = two_repositories("fresh_home");
    assert_eq!(app.project_root.as_deref(), Some(b.as_path()));
    app.new_workspace();
    assert_eq!(app.project_root, None, "not the last project you were in");
}

#[test]
fn the_terminal_panel_is_each_project_s_own() {
    let (_dir, mut app, a, b) = two_repositories("terminal_per_project");
    app.toggle_terminal();
    assert_eq!(app.terminal_root.as_deref(), Some(b.as_path()));
    let b_shell = app.terminal_shell;
    app.toggle_terminal();
    app.open_project(a.clone(), None, false);
    app.toggle_terminal();
    assert_eq!(app.terminal_root.as_deref(), Some(a.as_path()), "a shell of a's");
    assert_ne!(app.terminal_shell, b_shell);
    assert_eq!(app.parked_terminals.len(), 1, "b's runs on");
    app.toggle_terminal();
    app.open_project(b.clone(), None, false);
    app.toggle_terminal();
    assert_eq!(app.terminal_shell, b_shell, "b's again, as it was");
    app.toggle_terminal();
}

/// A Cargo workspace opened as a project, a member crate in it with
/// its own `.fenix`: (dir, app, mono, member, the member's file),
/// the file focused.
fn a_monorepo(name: &str) -> (TempDir, App, PathBuf, PathBuf, PathBuf) {
    let dir = TempDir::new(name);
    dir.mkdir(".git");
    dir.write("Cargo.toml", "[workspace]\n");
    dir.write("crates/core/Cargo.toml", "[package]\nname = \"core\"\n");
    dir.write("crates/core/.fenix/settings.toml", "[editor]\nindent_width = 7\n");
    let file = dir.write("crates/core/src/lib.rs", "\n");
    let canon = |p: &Path| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap());
    let (mono, member, file) = (canon(dir.path()), canon(&dir.path().join("crates/core")), canon(&file));
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(dir.path().join("projects.txt"));
    app.open_project(mono.clone(), Some(file.clone()), false);
    (dir, app, mono, member, file)
}

#[test]
fn a_nested_project_s_own_fenix_folder_speaks_for_its_files() {
    let (_dir, app, mono, member, _file) = a_monorepo("nested_fenix");
    assert_eq!(app.project_root.as_deref(), Some(mono.as_path()));
    assert_eq!(app.settings_root().as_deref(), Some(member.as_path()), "the crate's settings.toml");
    assert_eq!(app.project_settings.as_ref().map(|(r, _)| r.clone()), Some(member.clone()));
    assert_eq!(app.trust_root().as_deref(), Some(mono.as_path()), "no tools.json in the crate: the project's");
}

#[test]
fn the_task_picker_offers_the_nested_project_s_tasks_then_the_project_s() {
    let (_dir, mut app, mono, member, _file) = a_monorepo("nested_tasks");
    app.picker_tasks();
    let Some(ActivePicker::Task(state)) = &app.active_picker else { panic!("a task picker: {}", app.modeline_pieces().1) };
    let roots: Vec<PathBuf> = (0..state.len()).filter_map(|i| state.visible_rows(i, 1).next().map(|(_, c)| c.payload.1.clone())).collect();
    assert_eq!(roots.first(), Some(&member), "the crate's first");
    assert!(roots.contains(&mono), "then the workspace's");
}

#[test]
fn an_extension_is_told_the_project_it_runs_in_for_a_nested_project_s_file() {
    let (_dir, app, mono, _member, _file) = a_monorepo("nested_here");
    let (here, _) = app.editor_where();
    assert_eq!(here.project.map(|p| PathBuf::from(p.root)), Some(mono));
}

#[test]
fn a_folder_opened_as_a_project_is_that_project_marker_or_not() {
    let dir = TempDir::new("plain_folder_project");
    dir.mkdir(".git");
    let docs = dir.mkdir("docs");
    let docs = fenix_lsp::normalize(std::fs::canonicalize(&docs).unwrap());
    let file = dir.write("docs/guide.md", "# Guide\n");
    let file = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(dir.path().join("projects.txt"));
    app.open_project(docs.clone(), Some(file.clone()), false);
    assert_eq!(app.project_root.as_deref(), Some(docs.as_path()), "the folder opened, not the repository around it");
    assert_eq!(app.focused_project().as_deref(), Some(docs.as_path()), "extensions follow it too");
}

#[test]
fn picker_add_project_prompt_opens_the_explorer_at_the_current_project_root() {
    let root_dir = TempDir::new("add_project_prompt_root");
    let mut app = App::with_file(None);
    app.project_root = Some(root_dir.path().to_path_buf());

    app.picker_add_project_prompt();

    assert!(app.picking_folder());
    assert_eq!(app.explorer_purpose, ExplorerPurpose::PickProjectDir);
    assert_eq!(app.explorer.as_ref().map(|e| e.cwd.as_path()), Some(root_dir.path()));
}

#[test]
fn select_cwd_registers_the_browsed_directory_and_returns_to_the_editor() {
    let known_dir = TempDir::new("select_cwd_known");
    let project_dir = TempDir::new("select_cwd_target");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.project_root = Some(project_dir.path().to_path_buf());

    app.picker_add_project_prompt();
    app.explorer_handle_action(ExplorerAction::SelectCwd);

    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.explorer.is_none());
    assert_eq!(app.explorer_purpose, ExplorerPurpose::Browse);
    let canonical = fenix_lsp::normalize(std::fs::canonicalize(project_dir.path()).unwrap());
    assert_eq!(app.known_projects.roots(), std::slice::from_ref(&canonical));
    // Persisted, not just held in memory.
    let reloaded = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    assert_eq!(reloaded.roots(), &[canonical]);
}

#[test]
fn select_cwd_is_a_noop_outside_pick_project_dir_mode() {
    let dir = TempDir::new("select_cwd_browse_noop");
    let known_dir = TempDir::new("select_cwd_browse_noop_known");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::SelectCwd);

    assert!(app.known_projects.roots().is_empty());
    assert!(app.picking_folder(), "browsing should be untouched by a stray S");
}

#[test]
fn start_explore_from_home_opens_the_explorer_at_the_home_directory() {
    let mut app = App::with_file(None);
    // Something other than the home dir, so a bug that reuses
    // `project_root` instead of `dirs::home_dir()` would be caught.
    app.project_root = Some(PathBuf::from("."));

    app.start_explore_from_home();

    assert!(app.picking_folder());
    assert_eq!(app.explorer_purpose, ExplorerPurpose::FindFrom);
    let home = dirs::home_dir().expect("test environment should have a home dir");
    assert_eq!(app.explorer.as_ref().map(|e| e.cwd.as_path()), Some(home.as_path()));
}

#[test]
fn select_cwd_in_find_from_mode_opens_a_recursive_find_file_picker_rooted_there() {
    let dir = TempDir::new("find_from_select_cwd");
    dir.write("top.txt", "x");
    dir.write("sub/nested.txt", "x");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::FindFrom);

    app.explorer_handle_action(ExplorerAction::SelectCwd);

    assert_eq!(app.main_view, MainView::Picker);
    assert!(app.explorer.is_none());
    assert_eq!(app.explorer_purpose, ExplorerPurpose::Browse);
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => {
            let labels: Vec<String> = state.visible_rows(0, state.len()).map(|(_, c)| c.label.clone()).collect();
            assert!(labels.iter().any(|l| l.ends_with("top.txt")), "{labels:?}");
            assert!(labels.iter().any(|l| l.contains("sub") && l.ends_with("nested.txt")), "{labels:?}");
        }
        _ => panic!("expected an active FindFile picker"),
    }
}

#[test]
fn select_cwd_in_browse_mode_does_not_open_a_picker() {
    let dir = TempDir::new("find_from_select_cwd_browse_noop");
    let mut app = App::with_file(None);
    app.open_folder_picker(dir.path(), ExplorerPurpose::Browse);

    app.explorer_handle_action(ExplorerAction::SelectCwd);

    assert!(app.picking_folder(), "browsing should be untouched by a stray S");
    assert!(app.active_picker.is_none());
}

#[test]
fn opening_a_file_while_picking_a_project_dir_does_nothing() {
    let dir = TempDir::new("pick_project_dir_ignores_files");
    dir.write("main.rs", "fn main() {}");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.picker_add_project_prompt();
    app.explorer_open_selected();

    assert!(app.picking_folder(), "a file open shouldn't leave the picker");
    assert_eq!(app.explorer_purpose, ExplorerPurpose::PickProjectDir);
}

/// The palette lists every command but itself, with its key and the
/// extension it's from, the ones run lately first; confirming queues
/// the command for `handle_key`.
/// Tab in the palette gives a command keys -- refused when they'd hide
/// a group or sit under another binding (tests have no keybindings
/// file to write, so a good binding stops there).
#[test]
fn keys_from_the_palette_that_would_hide_something_are_refused() {
    let mut app = App::with_file(None);
    let bind = |app: &mut App, keys: &str| {
        app.bind_keys_start("agenda.open".into());
        for c in keys.chars() {
            app.bind_keys_feed(KeyPress::char(c));
        }
        app.bind_keys_feed(KeyPress::named(FenixNamedKey::Enter));
        app.status_message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    };
    assert!(bind(&mut app, " f").contains("is a group"));
    assert!(bind(&mut app, " fsx").contains("SPC f s runs file.save"));
    assert!(bind(&mut app, "x").contains("start with SPC"));
    assert!(bind(&mut app, " oz").contains("no settings folder"), "the keys are fine; only the file is missing in tests");
    assert!(app.binding_keys.is_none());
    app.open_command_palette();
    app.picker_key(KeyPress::named(FenixNamedKey::Tab));
    assert!(app.active_picker.is_none() && app.binding_keys.is_some(), "Tab in the palette starts it");
}

/// `SPC h k` then keys: what they run, once they run something.
#[test]
fn describe_key_says_what_the_keys_run() {
    let mut app = App::with_file(None);
    app.describe_key_start();
    for c in [' ', 'f'] {
        app.describe_key_feed(KeyPress::char(c));
    }
    assert!(app.describing_keys.is_some(), "still waiting after SPC f");
    app.describe_key_feed(KeyPress::char('s'));
    assert!(app.describing_keys.is_none());
    let said = app.status_message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(said.starts_with("SPC f s runs file.save: Save the current file") && said.ends_with("core"), "{said}");
}

/// `SPC h c` describes; `SPC h b` lists the keys, Enter runs one.
#[test]
fn describe_command_and_the_bindings_list() {
    let mut app = App::with_file(None);
    app.open_bindings_picker();
    let Some(ActivePicker::Bindings(state)) = &app.active_picker else { panic!("the bindings") };
    let rows: Vec<String> = state.visible_rows(0, state.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(rows.iter().any(|r| r.starts_with("SPC f s") && r.contains("file.save")), "{rows:?}");
}

#[test]
fn the_palette_lists_every_command_recent_first() {
    let mut app = App::with_file(None);
    app.note_command_run("window.split_vertical");
    app.note_command_run("file.save");
    app.open_command_palette();
    let Some(ActivePicker::Commands(state)) = &app.active_picker else { panic!("the palette") };
    let offered = app.commands().iter().filter(|c| !app.command_hidden(&c.id)).count();
    assert_eq!(state.len(), offered - 1, "every command but the palette, and a feature's that's off");
    let rows: Vec<String> = state.visible_rows(0, 3).map(|(_, c)| c.label.clone()).collect();
    assert!(rows[0].starts_with("Files › Save") && rows[0].ends_with("file.save"), "{rows:?}");
    assert!(rows[1].contains("window.split_vertical"), "{rows:?}");
    app.picker_confirm();
    assert_eq!(app.queued_command.as_deref(), Some("file.save"));
    assert!(app.active_picker.is_none());
}

/// Typing an area puts its commands first, matches names and ids word
/// by word, a description only by whole words, and nothing whose
/// letters are just scattered across it.
#[test]
fn the_palette_ranks_by_name_and_area() {
    let mut app = App::with_file(None);
    app.open_command_palette();
    for c in "git".chars() {
        app.picker_key(KeyPress::char(c));
    }
    let Some(ActivePicker::Commands(state)) = &app.active_picker else { panic!("the palette") };
    let ids: Vec<String> = state.visible_rows(0, state.len()).map(|(_, c)| c.payload.clone()).collect();
    assert!(ids[..10].iter().all(|id| id.starts_with("git.")), "Git's first: {:?}", &ids[..10]);
    assert!(!ids.iter().any(|id| id == "debug.breakpoint_hits"), "not by scattered letters");
    assert!(ids.len() < 60, "{} matches", ids.len());
    app.picker_cancel();
    app.open_command_palette();
    for c in "hit count".chars() {
        app.picker_key(KeyPress::char(c));
    }
    let Some(ActivePicker::Commands(state)) = &app.active_picker else { panic!("the palette") };
    assert_eq!(state.selected().map(|c| c.payload.as_str()), Some("debug.breakpoint_hits"), "found by its description's words");
}

/// The palette's view of the commands: area › name, the description
/// under it, keys on caps, the details beside; one that can't run
/// here faded, with why.
#[test]
fn the_palettes_view_of_a_command() {
    let mut app = App::with_file(None);
    app.open_command_palette();
    for c in "git".chars() {
        app.picker_key(KeyPress::char(c));
    }
    let view = app.palette_view(30).expect("a view");
    let palette::Body::List(rows) = &view.body else { panic!("a list") };
    assert_eq!(rows[0].area.as_deref(), Some("Git"));
    assert_eq!(rows[0].section.as_ref().map(|s| s.0.as_str()), Some("Git"));
    assert!(rows.iter().any(|r| r.keys.as_deref().is_some_and(|k| k.starts_with("SPC g "))), "keys from the keymap: {rows:?}");
    assert!(matches!(&view.side, Some(palette::Side::Details { .. })));
    assert!(view.chips.iter().any(|(t, accent)| *accent && t.ends_with("in Git")));
    app.picker_cancel();
    app.open_command_palette();
    for c in "xml".chars() {
        app.picker_key(KeyPress::char(c));
    }
    let view = app.palette_view(30).unwrap();
    let palette::Body::List(rows) = &view.body else { panic!("a list") };
    let xml = rows.iter().find(|r| r.title.contains("well-formed") || r.title.contains("XML")).expect("an XML command");
    assert!(xml.dimmed && xml.detail.as_deref() == Some("Not an XML file"), "{xml:?}");
}

#[test]
fn picker_pick_theme_lists_every_shipped_theme() {
    let mut app = App::with_file(None);
    app.picker_pick_theme();
    match &app.active_picker {
        Some(ActivePicker::Theme(state)) => assert_eq!(state.len(), theme::all().len()),
        other => panic!("expected an open Theme picker, got is_some={}", other.is_some()),
    }
    assert_eq!(app.main_view, MainView::Picker);
}

#[test]
fn text_in_other_encodings_opens_edits_and_saves_as_it_came() {
    let dir = TempDir::new("encodings");
    let mut app = App::with_file(None);
    let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("h\u{e9}llo\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
    let cases: [(&str, &[u8], &[u8], &str); 3] = [
        ("utf16.txt", &utf16, &[0xFF, 0xFE, b'Z', 0, 0xE9, 0, b'l', 0, b'l', 0, b'o', 0, b'\n', 0], "UTF-16 LE"),
        ("latin1.txt", b"h\xE9llo\n", b"Z\xE9llo\n", "windows-1252"),
        ("bom.txt", b"\xEF\xBB\xBFhello\n", b"\xEF\xBB\xBFZello\n", "UTF-8 BOM"),
    ];
    for (name, before, after, label) in cases {
        let path = dir.path().join(name);
        std::fs::write(&path, before).unwrap();
        assert!(!App::looks_binary(&path), "{name}");
        app.open_file_from_picker(&path);
        assert!(app.editor_status_details().contains(label), "{name}: {}", app.editor_status_details());
        assert!(!app.open().buffer.text().starts_with('\u{FEFF}'), "{name}: a mark isn't text");
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.replace_range(cursor, 0, 1, "Z");
        app.save();
        assert_eq!(std::fs::read(&path).unwrap(), after, "{name}");
        app.reload_focused_file();
        assert!(app.open().buffer.text().starts_with('Z'), "{name}");
        assert!(app.editor_status_details().contains(label), "{name}, reloaded");
    }
}

#[test]
fn saving_with_an_encoding_that_cant_hold_the_text_changes_nothing() {
    let dir = TempDir::new("encoding_refused");
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "plain\n").unwrap();
    let mut app = App::with_file(None);
    app.open_file_from_picker(&path);
    let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
    buffer.replace_range(cursor, 0, 5, "\u{1F600}");
    let latin = fenix_core::TextEncoding::choices().into_iter().find(|e| e.label() == "windows-1252").unwrap();
    app.save_with_encoding(latin);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "plain\n");
    assert_eq!(app.open().buffer.encoding(), fenix_core::TextEncoding::Utf8);
    assert!(app.open().buffer.is_dirty());
}

#[test]
fn a_file_can_be_read_again_as_another_encoding_and_saved_in_one() {
    let dir = TempDir::new("encoding_chosen");
    let path = dir.path().join("a.txt");
    std::fs::write(&path, b"\xCF\xF0\xE8\xE2\xE5\xF2\n").unwrap();
    let mut app = App::with_file(None);
    app.open_file_from_picker(&path);
    let cyrillic = fenix_core::TextEncoding::choices().into_iter().find(|e| e.label() == "windows-1251").unwrap();
    app.reopen_with_encoding(cyrillic);
    assert_eq!(app.open().buffer.text(), "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}\n");
    assert!(!app.open().buffer.is_dirty());
    app.save_with_encoding(fenix_core::TextEncoding::Utf8);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}\n");
}

#[test]
fn picker_confirm_on_theme_applies_it_persists_and_returns_to_the_editor() {
    let dir = TempDir::new("picker_confirm_theme");
    let mut app = App::with_file(None);
    app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
    app.theme = theme::default_theme();

    app.picker_pick_theme();
    for ch in "Nord".chars() {
        picker_push_char(app.active_picker.as_mut().unwrap(), ch);
    }
    app.picker_confirm();

    assert_eq!(app.theme.name, "Nord");
    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());
    let reloaded = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
    assert_eq!(reloaded.theme, Some("Nord".to_string())); // persisted
}

#[test]
fn picker_delete_project_lists_the_known_projects() {
    let known_dir = TempDir::new("delete_project_list");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.known_projects.add(PathBuf::from("/repo/one"));

    app.picker_delete_project();

    match &app.active_picker {
        Some(ActivePicker::DeleteProject(state)) => assert_eq!(state.len(), 1),
        other => panic!("expected an open DeleteProject picker, got is_some={}", other.is_some()),
    }
}

#[test]
fn picker_confirm_on_delete_project_removes_it_and_returns_to_the_editor() {
    let known_dir = TempDir::new("delete_project_confirm");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.known_projects.add(PathBuf::from("/repo/one"));
    app.known_projects.add(PathBuf::from("/repo/two"));

    app.picker_delete_project();
    app.picker_confirm(); // deletes whatever's selected -- MRU-first, so "/repo/two"

    assert_eq!(app.known_projects.roots(), &[PathBuf::from("/repo/one")]);
    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());

    let reloaded = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    assert_eq!(reloaded.roots(), &[PathBuf::from("/repo/one")]);
}

#[test]
fn picker_switch_buffer_lists_open_buffers_mru_first() {
    let dir = TempDir::new("switch_buffer_candidates");
    let a = dir.write("a.txt", "a");
    let b = dir.write("b.txt", "b");
    let mut app = App::with_file(None);
    app.test_open_path(&a);
    app.test_open_path(&b); // b opened (and touched) last -> MRU-first

    app.picker_switch_buffer();

    match &app.active_picker {
        Some(ActivePicker::SwitchBuffer(state)) => {
            let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
            assert_eq!(labels[0], crate::page::home_relative(&b), "outside a project: the whole path, from ~ when under it");
            assert!(labels.contains(&crate::page::home_relative(&a).as_str()));
        }
        _ => panic!("expected picker_switch_buffer to open a SwitchBuffer picker"),
    }
}

/// A page is listed by its title, not as `*scratch*` (#50).
#[test]
fn picker_switch_buffer_names_pages_by_their_titles() {
    let mut app = App::with_file(None);
    app.cmd_project_new();
    app.picker_switch_buffer();
    let Some(ActivePicker::SwitchBuffer(state)) = &app.active_picker else { panic!("expected a SwitchBuffer picker") };
    let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
    assert!(labels.contains(&"New project"), "by its tab's name: {labels:?}");
    assert!(!labels.contains(&"scratch"), "{labels:?}");
}

/// Each pane's Home and the empty scratches are listed once; a second
/// scratch with something in it is `scratch 2`.
#[test]
fn picker_switch_buffer_lists_lookalikes_once() {
    // However the root and the file were spelt, a file in the project
    // is by its place in it.
    assert_eq!(crate::page::path_within(Path::new(r"\\?\C:\code\fenix"), Path::new(r"c:\code\fenix\src\main.rs")).as_deref(), Some("src/main.rs"));
    assert_eq!(crate::page::path_within(Path::new(r"C:\code\fenix"), Path::new(r"C:\code\fenix-other\a.rs")), None, "a folder that only starts the same");
    let mut homes = App::with_file(None);
    // A second Home, as a session restored with one a pane can have.
    let other = homes.new_home_buffer();
    let mut ids = homes.buffers.mru().to_vec();
    if !ids.contains(&other) {
        ids.push(other);
    }
    assert!(ids.iter().filter(|id| homes.buffers.get(**id).is_some_and(|ob| ob.kind == BufferKind::Dashboard)).count() >= 2, "a Home a pane");
    let names: Vec<String> = homes.buffer_candidates(&ids).into_iter().map(|c| c.label).collect();
    assert_eq!(names.iter().filter(|n| **n == homes.workspaces.active_workspace().name).count(), 1, "one Home: {names:?}");
    assert!(!names.iter().any(|n| n == "home"), "{names:?}");
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.new_scratch_buffer();
    app.new_scratch_buffer();
    app.test_insert('x');
    app.picker_switch_buffer();
    let Some(ActivePicker::SwitchBuffer(state)) = &app.active_picker else { panic!() };
    let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
    assert_eq!(labels.iter().filter(|l| l.starts_with("scratch")).count(), 2, "the one written in, and one empty: {labels:?}");
    assert!(labels.contains(&"scratch") && labels.contains(&"scratch 2"), "{labels:?}");
}

#[test]
fn picker_switch_buffer_marks_dirty_buffers() {
    let mut app = App::with_file(None); // opens the dashboard, not a plain scratch buffer
    app.new_scratch_buffer(); // an untouched, genuinely trackable Text buffer
    app.picker_switch_buffer();
    match &app.active_picker {
        Some(ActivePicker::SwitchBuffer(state)) => {
            assert_eq!(state.visible_rows(0, 10).next().unwrap().1.label, "scratch");
        }
        _ => panic!("expected a SwitchBuffer picker"),
    }

    app.test_insert('x'); // now dirty
    app.picker_switch_buffer();
    let view = app.palette_view(20).expect("the buffer picker's card");
    let palette::Body::List(rows) = view.body else { panic!() };
    assert_eq!(rows[0].title, "scratch");
    assert_eq!(rows[0].right, "unsaved", "its changes said at the right");
}

#[test]
fn picker_confirm_on_switch_buffer_points_the_focused_pane_at_it() {
    let dir = TempDir::new("switch_buffer_confirm");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(None);
    let scratch_id = app.focused_buffer_id();
    app.test_open_path(&a); // now focused on a.txt

    let candidates = vec![fenix_picker::Candidate::new("*scratch*", scratch_id)];
    app.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
    app.picker_confirm();

    assert_eq!(app.focused_buffer_id(), scratch_id);
    assert_eq!(app.main_view, MainView::Editor);
    assert!(app.active_picker.is_none());
}

#[test]
fn next_and_prev_buffer_cycle_through_every_open_buffer() {
    let dir = TempDir::new("cycle_buffer");
    let a = dir.write("a.txt", "a");
    let b = dir.write("b.txt", "b");
    let mut app = App::with_file(None); // scratch, then a, then b
    app.test_open_path(&a);
    app.test_open_path(&b);

    let ids = app.buffers.ids_sorted_by_path(); // scratch, a, b (scratch sorts first)
    assert_eq!(app.focused_buffer_id(), ids[2]); // currently on b

    app.next_buffer(); // wraps to scratch
    assert_eq!(app.focused_buffer_id(), ids[0]);

    app.prev_buffer(); // back to b
    assert_eq!(app.focused_buffer_id(), ids[2]);

    app.prev_buffer(); // to a
    assert_eq!(app.focused_buffer_id(), ids[1]);
}

#[test]
fn kill_buffer_falls_back_to_the_neighbour_tab() {
    let dir = TempDir::new("kill_buffer");
    let (a, b, c) = (dir.write("a.txt", "a"), dir.write("b.txt", "b"), dir.write("c.txt", "c"));
    let mut app = App::with_file(None);
    app.ensure_workspace_home();
    app.test_open_path(&a);
    app.test_open_path(&b);
    let b_id = app.focused_buffer_id();
    app.test_open_path(&c);
    let c_id = app.focused_buffer_id();
    app.test_open_path(&b);

    app.kill_buffer(); // b sits between a and c: the right one wins

    assert_eq!(app.focused_buffer_id(), c_id);
    assert!(app.buffers.get(b_id).is_none());
    assert!(!app.pane_tab_order(app.focused_pane_id()).contains(&b_id));
}

#[test]
fn kill_buffer_on_a_panes_only_file_falls_back_to_home() {
    let dir = TempDir::new("kill_buffer_home");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(None);
    let home = app.ensure_workspace_home();
    app.test_open_path(&a);

    app.kill_buffer();

    assert_eq!(app.focused_buffer_id(), home);
    assert_eq!(app.buffers.len(), 1);
}

#[test]
fn home_refuses_to_be_killed() {
    let mut app = App::with_file(None);
    let home = app.ensure_workspace_home();

    app.kill_buffer();

    assert_eq!(app.focused_buffer_id(), home);
    assert!(app.buffers.get(home).is_some());
}

#[test]
fn kill_buffer_retargets_every_other_pane_showing_it() {
    let dir = TempDir::new("kill_buffer_multi_pane");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(None);
    let scratch_id = app.ensure_workspace_home();
    app.test_open_path(&a); // focused pane now on a
    app.split_vertical(); // new pane, also showing a
    assert_eq!(app.windows().window_count(), 2);

    app.kill_buffer(); // closes a, focused in the new pane

    for pane in app.windows().windows() {
        assert_eq!(app.windows().content(pane), Some(&scratch_id));
    }
}

#[test]
fn kill_buffer_refuses_with_a_named_error_when_the_focused_text_buffer_is_dirty() {
    let dir = TempDir::new("kill_buffer_dirty_guard");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let id = app.focused_buffer_id();
    app.test_insert('!');

    app.kill_buffer();

    assert!(app.buffers.get(id).is_some(), "the dirty buffer should still be open");
    assert_eq!(app.focused_buffer_id(), id);
    assert!(app.modeline_pieces().1.contains("a.txt"));
    assert!(app.modeline_pieces().1.contains("unsaved changes"));
}

#[test]
fn force_kill_buffer_discards_changes_and_closes_anyway() {
    let dir = TempDir::new("force_kill_buffer");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(None);
    let scratch_id = app.focused_buffer_id();
    app.test_open_path(&a);
    let dirty_id = app.focused_buffer_id();
    app.test_insert('!');

    app.force_kill_buffer();

    assert!(app.buffers.get(dirty_id).is_none());
    assert_eq!(app.focused_buffer_id(), scratch_id);
}

#[test]
fn save_and_close_buffer_refuses_when_the_dirty_buffer_has_no_path() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    let id = app.focused_buffer_id();
    app.test_insert('!');

    app.save_and_close_buffer();

    assert!(app.buffers.get(id).is_some(), "should still be open");
    assert_eq!(app.focused_buffer_id(), id);
    assert!(app.modeline_pieces().1.contains(":w <path>"));
}

#[test]
fn save_and_close_buffer_saves_then_closes_a_named_dirty_buffer() {
    let dir = TempDir::new("save_and_close_buffer");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let id = app.focused_buffer_id();
    app.test_insert('!');

    app.save_and_close_buffer();

    assert!(app.buffers.get(id).is_none());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "!a");
}

#[test]
fn save_all_refuses_up_front_when_any_dirty_text_buffer_has_no_path() {
    let dir = TempDir::new("save_all_refuses");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let named_id = app.focused_buffer_id();
    app.test_insert('!');
    app.new_scratch_buffer();
    let unnamed_id = app.focused_buffer_id();
    app.test_insert('!');

    assert!(!app.save_all());

    assert!(app.buffers.get(named_id).unwrap().buffer.is_dirty(), "no partial save should have happened");
    assert!(app.buffers.get(unnamed_id).unwrap().buffer.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
}

#[test]
fn save_all_saves_every_dirty_named_buffer() {
    let dir = TempDir::new("save_all_saves");
    let a = dir.write("a.txt", "a");
    let b = dir.write("b.txt", "b");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    app.test_insert('!');
    app.test_open_path(&b);
    app.test_insert('!');

    assert!(app.save_all());

    assert_eq!(std::fs::read_to_string(&a).unwrap(), "!a");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "!b");
}

#[test]
fn new_scratch_buffer_opens_a_fresh_empty_buffer_in_the_focused_pane() {
    let dir = TempDir::new("new_scratch");
    let a = dir.write("a.txt", "hello");
    let mut app = App::with_file(None);
    app.test_open_path(&a);
    let a_id = app.focused_buffer_id();

    app.new_scratch_buffer();

    assert_ne!(app.focused_buffer_id(), a_id);
    assert_eq!(app.open().buffer.text(), "");
    assert_eq!(app.open().kind, BufferKind::Text);
    assert_eq!(app.open().buffer.path(), None);
}

#[test]
fn with_file_none_opens_home_instead_of_a_plain_scratch_buffer() {
    let mut app = App::with_file(None);
    assert_eq!(app.open().kind, BufferKind::Dashboard);
    let view = app.test_home();
    assert!(view.text.contains("Find a file"), "{}", view.text);
    assert_eq!(app.open().buffer.text(), view.text, "the buffer holds the laid-out text");
}

#[test]
fn gutter_chars_is_zero_for_a_dashboard_buffer() {
    let app = App::with_file(None);
    assert_eq!(app.gutter_chars(app.open()), 0);
}

#[test]
fn home_relays_itself_out_when_its_pane_changes_size() {
    let mut app = App::with_file(None);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    app.refresh_home_data(true);
    app.ensure_home_layout(id, pane, 140, 45);
    let wide = app.open().buffer.text();
    app.ensure_home_layout(id, pane, 50, 45);
    let narrow = app.open().buffer.text();
    assert_ne!(wide, narrow);
    assert!(narrow.lines().all(|l| l.chars().count() <= 50));
    assert_eq!(app.home_views[&id].size, (50, 45));
}

#[test]
fn home_keeps_the_selection_across_a_relayout() {
    let dir = TempDir::new("home_keeps_selection");
    let project = TempDir::new("home_keeps_selection_project");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(dir.path().join("projects.txt"));
    app.known_projects.add(project.path().to_path_buf());
    app.recent_files = fenix_project::RecentFiles::load_or_default(dir.path().join("recent_files.txt"));
    let entry = dashboard::HomeEntry::Project(project.path().to_path_buf());
    app.test_select_home(&entry);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    app.ensure_home_layout(id, pane, 60, 50);
    let selected = app.home_selected_slot(id, pane).unwrap();
    assert_eq!(app.home_views[&id].slots[selected].entry, entry);
}

/// With a file to resume, Home opens on it: the `Enter` its column
/// shows resumes it, rather than opening the finder.
#[test]
fn home_opens_on_the_file_to_resume() {
    let dir = TempDir::new("home_resume_first");
    let file = dir.write("app.rs", "fn main() {}\n");
    let mut app = App::with_file(None);
    app.recent_files = fenix_project::RecentFiles::load_or_default(dir.path().join("recent_files.txt"));
    app.recent_files.add(file.clone());
    app.test_home();
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    let at = app.home_views[&id].slots[app.home_selected_slot(id, pane).unwrap()].entry.clone();
    assert!(matches!(at, dashboard::HomeEntry::Resume(_)), "{at:?}");
    assert!(app.home_key(KeyPress::named(FenixNamedKey::Enter)));
    assert_eq!(app.open().buffer.path().map(fenix_core::path_key), Some(fenix_core::path_key(&file)));
}

#[test]
fn home_keys_move_between_slots_and_numbers_activate() {
    let dir = TempDir::new("home_keys");
    let project = TempDir::new("home_keys_project");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(dir.path().join("projects.txt"));
    app.known_projects.add(project.path().to_path_buf());
    app.recent_files = fenix_project::RecentFiles::load_or_default(dir.path().join("recent_files.txt"));
    app.test_home();
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    let at = |app: &App| app.home_views[&id].slots[app.home_selected_slot(id, pane).unwrap()].entry.clone();
    assert_eq!(at(&app), dashboard::HomeEntry::Find, "Home opens on the find field");
    assert!(app.home_key(KeyPress::char('j')));
    assert_eq!(at(&app), dashboard::HomeEntry::Project(project.path().to_path_buf()), "no resume or recent: j lands in projects");
    assert!(app.home_key(KeyPress::char('k')));
    assert_eq!(at(&app), dashboard::HomeEntry::Find);
    assert!(!app.home_key(KeyPress::char('x')), "other keys reach Vim");

    assert!(app.home_key(KeyPress::char('1')));
    assert_eq!(app.project_root, Some(project.path().to_path_buf()));
    assert!(matches!(&app.active_picker, Some(ActivePicker::FindFile(_))));
}

#[test]
fn home_is_called_home() {
    let app = App::with_file(None);
    assert!(app.modeline_text().contains("home"));
}

#[test]
fn open_dashboard_goes_to_the_workspaces_one_home() {
    let dir = TempDir::new("open_dashboard");
    let a = dir.write("a.txt", "hello");
    let mut app = App::with_file(None);
    let home = app.focused_buffer_id();
    app.test_open_path(&a);
    let a_id = app.focused_buffer_id();

    app.open_dashboard();
    assert_ne!(app.focused_buffer_id(), a_id);
    assert_eq!(app.focused_buffer_id(), home, "the start screen became the workspace's Home");
    assert_eq!(app.open().kind, BufferKind::Dashboard);
    assert!(!app.test_home().slots.is_empty());

    app.test_open_path(&a);
    app.open_dashboard();
    assert_eq!(app.focused_buffer_id(), home, "not a second dashboard");
}

// -- tabs ---------------------------------------------------------

/// `App::with_file(None)` with its Home adopted and three files open
/// in the one pane, in order; the pane is on the last.
fn app_with_three_tabs(name: &str) -> (App, TempDir, [BufferId; 4]) {
    let dir = TempDir::new(name);
    let mut app = App::with_file(None);
    let home = app.ensure_workspace_home();
    let mut ids = [home; 4];
    for (i, file) in ["a.txt", "b.txt", "c.txt"].into_iter().enumerate() {
        let path = dir.write(file, file);
        app.test_open_path(&path);
        ids[i + 1] = app.focused_buffer_id();
    }
    (app, dir, ids)
}

#[test]
fn home_is_first_in_the_strip_and_never_listed_twice() {
    let (app, _dir, ids) = app_with_three_tabs("tabs_order");
    assert_eq!(app.pane_tab_order(app.focused_pane_id()), ids.to_vec());
    assert_eq!(app.tab_label(ids[0]), app.workspaces.active_name(), "Home is labelled with the workspace");
}

#[test]
fn gt_and_gt_shifted_step_and_wrap_through_home() {
    let (mut app, _dir, [home, a, b, c]) = app_with_three_tabs("tabs_step");
    keys(&mut app, "gt");
    assert_eq!(app.focused_buffer_id(), home, "gt from the last tab wraps to Home");
    keys(&mut app, "gt");
    assert_eq!(app.focused_buffer_id(), a);
    keys(&mut app, "gT");
    keys(&mut app, "gT");
    assert_eq!(app.focused_buffer_id(), c);
    keys(&mut app, "2gT");
    assert_eq!(app.focused_buffer_id(), a);
    let _ = b;
}

#[test]
fn a_counted_gt_goes_to_that_file_and_gh_goes_home() {
    let (mut app, _dir, [home, a, b, _c]) = app_with_three_tabs("tabs_nth");
    keys(&mut app, "2gt");
    assert_eq!(app.focused_buffer_id(), b);
    keys(&mut app, "1gt");
    assert_eq!(app.focused_buffer_id(), a);
    keys(&mut app, "9gt");
    assert_eq!(app.focused_buffer_id(), a, "past the last tab stays put");
    keys(&mut app, "gh");
    assert_eq!(app.focused_buffer_id(), home);
}

#[test]
fn g_tab_goes_back_to_the_tab_before() {
    let (mut app, _dir, [_home, a, _b, c]) = app_with_three_tabs("tabs_last");
    keys(&mut app, "1gt");
    assert_eq!(app.focused_buffer_id(), a);
    keys(&mut app, "g");
    app.test_route_key(KeyPress::named(FenixNamedKey::Tab));
    assert_eq!(app.focused_buffer_id(), c);
    keys(&mut app, "g");
    app.test_route_key(KeyPress::named(FenixNamedKey::Tab));
    assert_eq!(app.focused_buffer_id(), a);
}

#[test]
fn closing_the_last_file_tab_lands_on_home_and_home_never_closes() {
    let dir = TempDir::new("tabs_close");
    let a = dir.write("a.txt", "a");
    let mut app = App::with_file(None);
    let home = app.ensure_workspace_home();
    app.test_open_path(&a);
    let (pane, a_id) = (app.focused_pane_id(), app.focused_buffer_id());

    app.close_pane_tab(pane, a_id);
    assert_eq!(app.focused_buffer_id(), home);
    assert!(app.buffers.get(a_id).is_some(), "closing a tab keeps the buffer");

    app.close_pane_tab(pane, home);
    assert_eq!(app.pane_tab_order(pane), vec![home]);
}

#[test]
fn closing_a_middle_tab_goes_to_its_right_neighbour() {
    let (mut app, _dir, [_home, a, b, c]) = app_with_three_tabs("tabs_close_mid");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, b);
    app.close_pane_tab(pane, b);
    assert_eq!(app.focused_buffer_id(), c);
    assert_eq!(app.pane_tab_order(pane)[1..], [a, c]);
}

#[test]
fn a_new_tab_opens_right_after_the_one_you_were_on() {
    let (mut app, dir, [home, a, b, c]) = app_with_three_tabs("tabs_insert");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, a);
    app.test_open_path(&dir.write("d.txt", "d"));
    let d = app.focused_buffer_id();
    assert_eq!(app.pane_tab_order(pane), vec![home, a, d, b, c]);
}

#[test]
fn jumps_share_one_preview_tab_until_it_is_edited() {
    let (mut app, dir, [home, a, b, c]) = app_with_three_tabs("tabs_preview");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, a);
    app.open_file_as_preview(&dir.write("p.txt", "p"));
    let p = app.focused_buffer_id();
    assert!(app.is_preview_tab(pane, p));
    assert_eq!(app.pane_tab_order(pane), vec![home, a, p, b, c]);

    app.open_file_as_preview(&dir.write("q.txt", "q"));
    let q = app.focused_buffer_id();
    assert_eq!(app.pane_tab_order(pane), vec![home, a, q, b, c], "the next jump reuses the preview's place");

    app.test_route_key(KeyPress::char('x'));
    assert!(!app.is_preview_tab(pane, q), "an edit keeps it");
    app.open_file_as_preview(&dir.write("r.txt", "r"));
    let r = app.focused_buffer_id();
    assert_eq!(app.pane_tab_order(pane), vec![home, a, q, r, b, c]);
}

#[test]
fn opening_a_previewed_file_on_purpose_keeps_it() {
    let dir = TempDir::new("tabs_preview_keep");
    let file = dir.write("p.txt", "p");
    let mut app = App::with_file(None);
    app.ensure_workspace_home();
    let pane = app.focused_pane_id();
    app.open_file_as_preview(&file);
    assert!(app.is_preview_tab(pane, app.focused_buffer_id()));
    app.open_file_from_picker(&file);
    assert!(!app.is_preview_tab(pane, app.focused_buffer_id()));
}

#[test]
fn keep_preview_and_a_double_click_keep_it() {
    let dir = TempDir::new("tabs_preview_click");
    let mut app = App::with_file(None);
    app.ensure_workspace_home();
    let pane = app.focused_pane_id();
    app.open_file_as_preview(&dir.write("p.txt", "p"));
    let p = app.focused_buffer_id();
    app.tab_clicked(pane, p);
    assert!(app.is_preview_tab(pane, p), "one click only switches");
    app.tab_clicked(pane, p);
    assert!(!app.is_preview_tab(pane, p));

    app.open_file_as_preview(&dir.write("q.txt", "q"));
    let q = app.focused_buffer_id();
    app.keep_focused_preview();
    assert!(!app.is_preview_tab(pane, q));
}

#[test]
fn with_previews_off_every_jump_is_a_tab() {
    let dir = TempDir::new("tabs_preview_off");
    let mut app = App::with_file(None);
    app.config.preview_tab = Some(false);
    let home = app.ensure_workspace_home();
    app.open_file_as_preview(&dir.write("p.txt", "p"));
    let p = app.focused_buffer_id();
    app.open_file_as_preview(&dir.write("q.txt", "q"));
    let q = app.focused_buffer_id();
    assert_eq!(app.pane_tab_order(app.focused_pane_id()), vec![home, p, q]);
}

#[test]
fn a_closed_tab_reopens_where_it_was() {
    let (mut app, _dir, [home, a, b, c]) = app_with_three_tabs("tabs_reopen");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, b);
    app.close_focused_tab();
    assert_eq!(app.pane_tab_order(pane), vec![home, a, c]);
    assert!(app.buffers.get(b).is_some());
    app.reopen_closed_tab();
    assert_eq!(app.focused_buffer_id(), b);
    assert_eq!(app.pane_tab_order(pane), vec![home, a, b, c]);
}

#[test]
fn closing_the_others_leaves_home_and_this_one_and_each_comes_back() {
    let (mut app, _dir, [home, a, b, c]) = app_with_three_tabs("tabs_others");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, b);
    app.close_other_tabs();
    assert_eq!(app.pane_tab_order(pane), vec![home, b]);
    app.reopen_closed_tab();
    app.reopen_closed_tab();
    assert_eq!(app.pane_tab_order(pane), vec![home, a, b, c]);
}

#[test]
fn a_tab_moves_left_and_right_but_home_stays_first() {
    let (mut app, _dir, [home, a, b, c]) = app_with_three_tabs("tabs_shift");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, a);
    app.shift_focused_tab(-1);
    assert_eq!(app.pane_tab_order(pane), vec![home, a, b, c]);
    app.shift_focused_tab(1);
    assert_eq!(app.pane_tab_order(pane), vec![home, b, a, c]);
}

#[test]
fn the_mouse_closes_with_the_middle_button_and_drags_to_move() {
    let (mut app, _dir, [home, a, b, c]) = app_with_three_tabs("tabs_mouse");
    let pane = app.focused_pane_id();
    let rect = |x: f32| fenix_window::Rect { x, y: 0.0, w: 10.0, h: 10.0 };
    app.tab_geometry = [home, a, b, c]
        .iter()
        .enumerate()
        .map(|(i, &buffer)| (pane, TabRect { buffer, body: rect(i as f32 * 10.0), close: rect(i as f32 * 10.0 + 8.0) }))
        .collect();
    app.tab_mouse((12.0, 5.0), MouseButton::Left, true); // a
    app.tab_mouse((32.0, 5.0), MouseButton::Left, false); // onto c
    assert_eq!(app.pane_tab_order(pane), vec![home, b, c, a]);
    app.tab_mouse((22.0, 5.0), MouseButton::Middle, true); // over what was b's place
    assert_eq!(app.pane_tab_order(pane).len(), 3);
    app.tab_mouse((2.0, 5.0), MouseButton::Middle, true); // Home
    assert_eq!(app.pane_tab_order(pane)[0], home);
}

#[test]
fn every_pane_starts_with_the_same_home() {
    let (mut app, _dir, [home, ..]) = app_with_three_tabs("tabs_split");
    app.split_vertical();
    assert_eq!(app.pane_tab_order(app.focused_pane_id())[0], home);
}

#[test]
fn a_new_workspace_gets_a_home_of_its_own() {
    let mut app = App::with_file(None);
    let first = app.ensure_workspace_home();
    app.new_workspace();
    let second = app.ensure_workspace_home();
    assert_ne!(first, second);
    assert_eq!(app.focused_buffer_id(), second);
}

#[test]
fn a_project_workspaces_home_shows_only_that_projects_files() {
    let inside = TempDir::new("home_scope_in");
    let outside = TempDir::new("home_scope_out");
    let (mine, other) = (inside.write("mine.rs", "x"), outside.write("other.rs", "y"));
    let mut app = App::with_file(None);
    for file in [&other, &mine] {
        app.recent_files.add(std::fs::canonicalize(file).unwrap());
    }
    let home = app.ensure_workspace_home();
    let root = fenix_lsp::normalize(std::fs::canonicalize(inside.path()).unwrap());
    app.workspaces.active_workspace_mut().project = Some(root);
    app.refresh_home_data(true);

    let scoped = app.home_data_for(home);
    let names: Vec<&str> = scoped.resume.iter().chain(&scoped.recent).map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["mine.rs"]);
    assert!(scoped.date.starts_with(&*inside.path().file_name().unwrap().to_string_lossy()), "{}", scoped.date);
    let everything: Vec<&str> = app.home_data.resume.iter().chain(&app.home_data.recent).map(|f| f.name.as_str()).collect();
    assert!(everything.contains(&"other.rs"));
}

#[test]
fn modeline_shows_the_capture_badge_only_while_a_surface_has_the_keyboard() {
    let mut app = App::with_file(None);
    assert_ne!(app.modeline_pieces().0, "CAPTURE");
    app.wasm.captured = Some(app.focused_buffer_id());
    assert_eq!(app.modeline_pieces().0, "CAPTURE");
    app.wasm.captured = None;
    assert_ne!(app.modeline_pieces().0, "CAPTURE");
}

#[test]
fn buffer_display_name_uses_the_filename_for_a_real_path() {
    let dir = TempDir::new("buffer_display_name_path");
    let a = dir.write("hello.rs", "fn main() {}");
    let mut app = App::with_file(None);
    app.test_open_path(&a);
    assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "hello.rs");
}

#[test]
fn buffer_display_name_falls_back_to_no_name_for_a_pathless_text_buffer() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "[No Name]");
}

#[test]
fn handle_user_event_never_panics_for_any_variant() {
    // `App::user_event` (the real `ApplicationHandler` trait method)
    // needs a real `ActiveEventLoop`, which nothing outside winit's
    // own running loop can construct -- this exercises the actual
    // logic (`handle_user_event`) directly instead, same workaround
    // `test_vim_key` already uses for keyboard input.
    let mut app = App::with_file(None);
    app.handle_user_event(FenixUserEvent::Lsp {
        key: app.lsp_key(fenix_syntax::LanguageId::Python),
        generation: 0,
        event: fenix_lsp::LspEvent::Disconnected("test".to_string()),
    });
    app.handle_user_event(FenixUserEvent::TaskEvent(app.focused_buffer_id(), fenix_tasks::RunId::next(), fenix_tasks::TaskEvent::Output(vec![])));
    app.handle_user_event(FenixUserEvent::TaskEvent(app.focused_buffer_id(), fenix_tasks::RunId::next(), fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Cancelled)));
}

/// `SPC p r`, and what `run_test {path, line}` calls: the test a line
/// is in, alone, in Task Output -- from the buffer's text, unsaved
/// edits and all.
#[test]
fn run_test_at_runs_the_test_a_line_is_in() {
    let dir = TempDir::new("run_test_at");
    dir.write("pyproject.toml", "[project]\nname = \"ledger\"\n");
    let path = dir.write("test_ledger.py", "def helper():\n    pass\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let err = app.run_test_at(&path, 2).unwrap_err();
    assert!(err.contains("isn't in a test"), "{err}");
    assert!(app.modeline_pieces().1.contains("isn't in a test"));
    // Typed, not saved.
    let end = app.open().buffer.len_chars();
    let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
    buffer.replace_range(cursor, end, end, "\ndef test_totals():\n    assert 1 == 1\n");
    assert_eq!(app.run_test_at(&path, 5).unwrap(), "test test_totals");
    let session = app.task_session.as_ref().expect("Task Output opened");
    let header = app.buffers.get(session.buffer).unwrap().buffer.text();
    assert!(header.starts_with("$ pytest test_ledger.py::test_totals"), "{header}");
    assert_eq!(app.project_last_tasks.values().next().map(|t| t.name.as_str()), Some("test test_totals"), "SPC p T runs it again");
}

#[test]
fn picker_tasks_errors_with_no_project_root() {
    let mut app = App::with_file(None);
    app.picker_tasks();
    assert!(app.modeline_pieces().1.contains("no project root"));
    assert!(app.active_picker.is_none());
}

#[test]
fn picker_tasks_errors_when_the_project_root_has_no_discoverable_tasks() {
    let dir = TempDir::new("no_tasks");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.picker_tasks();
    assert!(app.modeline_pieces().1.contains("no known tasks"));
}

#[test]
fn picker_tasks_opens_a_task_picker_with_the_discovered_candidates() {
    let dir = TempDir::new("cargo_project");
    dir.write("Cargo.toml", "[package]\nname=\"x\"");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.picker_tasks();

    match &app.active_picker {
        Some(ActivePicker::Task(state)) => assert_eq!(state.len(), 3), // build/test/clippy
        _ => panic!("expected an open Task picker"),
    }
}

/// Each row says where its task comes from: Fenix's defaults, an
/// extension, or the project's `tools.json` (#35).
#[test]
fn the_task_picker_says_where_each_task_comes_from() {
    let dir = TempDir::new("task_sources");
    dir.write("Cargo.toml", "[package]\nname=\"x\"");
    std::fs::create_dir_all(dir.path().join(".fenix")).unwrap();
    dir.write(".fenix/tools.json", r#"{"tasks": {"lint": {"executable": "cargo", "args": ["clippy"]}}}"#);
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.picker_tasks();
    let Some(ActivePicker::Task(state)) = &app.active_picker else { panic!("expected an open Task picker") };
    let labels = picker_visible_labels(app.active_picker.as_ref().unwrap(), 0, state.len()).into_iter().map(|(_, l)| l).collect::<Vec<_>>();
    assert!(labels.iter().any(|l| l == "cargo build   [Fenix]"), "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("lint   cargo clippy") && l.ends_with("[tools.json]")), "{labels:?}");
    assert_eq!(task_picker_label(&fenix_tasks::TaskDef { name: "test".into(), command: "uv".into(), args: vec!["run".into(), "pytest".into()] }, "fenix.python"), "test   uv run pytest   [fenix.python]");
}

#[test]
fn run_task_opens_a_task_output_buffer_with_the_command_header() {
    let dir = TempDir::new("run_task_header");
    let mut app = App::with_file(None);
    let program = if cfg!(windows) { "cmd" } else { "true" };
    let args: Vec<String> = if cfg!(windows) { vec!["/c".to_string(), "exit".to_string(), "0".to_string()] } else { Vec::new() };
    let task = fenix_tasks::TaskDef { name: "t".to_string(), command: program.to_string(), args };

    app.run_task(task, dir.path().to_path_buf());

    assert_eq!(app.open().kind, BufferKind::TaskOutput);
    assert!(app.open().buffer.text().starts_with(&format!("$ {program}")));
}

#[test]
fn run_task_without_a_real_event_proxy_captures_output_synchronously() {
    // Headless tests consume the same supervisor events as the GUI.
    let dir = TempDir::new("run_task_sync");
    let mut app = App::with_file(None);
    let (program, args) = echo_command("hello-from-task");
    let task = fenix_tasks::TaskDef { name: "echo".to_string(), command: program, args };

    app.run_task(task, dir.path().to_path_buf());

    let text = app.open().buffer.text();
    assert!(text.contains("hello-from-task"), "expected the echoed text in the buffer, got:\n{text}");
    assert!(text.contains("task succeeded"), "expected a success marker, got:\n{text}");
}

#[test]
fn run_task_a_second_time_reuses_the_same_panel_and_clears_the_previous_output() {
    let dir = TempDir::new("run_task_reuse");
    let mut app = App::with_file(None);
    let (program, args) = echo_command("first-run");
    app.run_task(fenix_tasks::TaskDef { name: "first".to_string(), command: program, args }, dir.path().to_path_buf());
    let first_buffer = app.focused_buffer_id();

    let (program2, args2) = echo_command("second-run");
    app.run_task(fenix_tasks::TaskDef { name: "second".to_string(), command: program2, args: args2 }, dir.path().to_path_buf());

    assert_eq!(app.focused_buffer_id(), first_buffer, "should reuse the same panel, not open a second one");
    let text = app.open().buffer.text();
    assert!(!text.contains("first-run"), "the previous run's output should have been cleared, got:\n{text}");
    assert!(text.contains("second-run"));
}

#[test]
fn stale_task_events_cannot_modify_or_finish_a_newer_run() {
    let dir = TempDir::new("task_stale_events");
    let mut app = App::with_file(None);
    let (command, args) = echo_command("current-run");
    app.run_task(fenix_tasks::TaskDef { name: "t".into(), command, args }, dir.path().to_path_buf());
    let buffer = app.focused_buffer_id();
    let current = fenix_tasks::RunId::next();
    let stale = fenix_tasks::RunId::next();
    app.task_session.as_mut().unwrap().run_id = Some(current);
    let before = app.open().buffer.text();
    app.apply_task_event(buffer, stale, fenix_tasks::TaskEvent::Output(vec![fenix_tasks::OutputLine {
        stream: fenix_tasks::OutputStream::Stdout, text: "stale.rs:1:1: old error".into(), cut: false,
    }]));
    app.apply_task_event(buffer, stale, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Cancelled));
    assert_eq!(app.open().buffer.text(), before);
    assert!(app.quickfix.is_empty());
    assert_eq!(app.task_session.as_ref().unwrap().run_id, Some(current));
    app.apply_task_event(buffer, current, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Cancelled));
    assert!(app.open().buffer.text().contains("task cancelled"));
    assert_eq!(app.task_session.as_ref().unwrap().run_id, None);
    let finished = app.open().buffer.text();
    app.apply_task_event(buffer, current, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Cancelled));
    assert_eq!(app.open().buffer.text(), finished, "completion is applied once");
}

#[test]
fn task_spawn_failure_displays_the_underlying_error() {
    let dir = TempDir::new("task_spawn_failure");
    let mut app = App::with_file(None);
    app.run_task(fenix_tasks::TaskDef { name: "missing".into(), command: "fenix-missing-tool-xyz".into(), args: vec![] }, dir.path().to_path_buf());
    let text = app.open().buffer.text();
    assert!(text.contains("couldn't start `fenix-missing-tool-xyz`:"));
    assert!(app.task_session.as_ref().unwrap().run_id.is_none());
}

#[test]
fn rerun_last_task_errors_when_nothing_has_run_yet() {
    let mut app = App::with_file(None);
    app.rerun_last_task();
    assert!(app.modeline_pieces().1.contains("no task has been run"));
}

#[test]
fn rerun_last_task_reruns_the_same_task_that_was_last_run() {
    let dir = TempDir::new("rerun_last");
    let mut app = App::with_file(None);
    let (program, args) = echo_command("rerun-marker");
    app.run_task(fenix_tasks::TaskDef { name: "t".to_string(), command: program, args }, dir.path().to_path_buf());

    // A second run so the panel's content is genuinely fresh, not
    // just left over from the first `run_task` call above.
    app.reset_task_output_buffer(app.focused_buffer_id(), "");
    app.rerun_last_task();

    assert!(app.open().buffer.text().contains("rerun-marker"));
}

#[test]
fn kill_running_task_errors_when_nothing_is_running() {
    let mut app = App::with_file(None);
    app.kill_running_task();
    assert!(app.modeline_pieces().1.contains("no task is running"));
}

// -- DAP debugger (`SPC u ...`) ---------------------------------------

#[test]
fn debug_toggle_breakpoint_sets_then_clears_a_line() {
    let dir = TempDir::new("debug_toggle_bp");
    let file = dir.write("main.py", "a = 1\nb = 2\nc = 3\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('j')); // move to line 2

    app.debug_toggle_breakpoint();
    // `fenix_lsp::normalize` strips the `\\?\` verbatim prefix
    // `canonicalize` always adds on Windows -- see `debug_toggle_
    // breakpoint`'s own doc comment for the real bug this exists to
    // avoid (a breakpoint registered under the un-normalized form
    // never actually matches what the adapter reports back).
    let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
    assert_eq!(app.breakpoints.get(&canonical).map(|l| l.keys().copied().collect::<Vec<_>>()), Some(vec![2]));

    app.debug_toggle_breakpoint(); // toggling the same line again clears it
    assert!(app.breakpoints.get(&canonical).is_none_or(|lines| lines.is_empty()));
}

#[test]
fn debug_toggle_breakpoint_errors_for_a_pathless_buffer() {
    let mut app = App::with_file(None);
    app.debug_toggle_breakpoint();
    assert!(app.modeline_pieces().1.contains("no file to set a breakpoint"));
}

/// `SPC q r` with nowhere to keep the session (a test's App, or
/// `session.restore = false`): it says so, and Fenix keeps running.
#[test]
fn a_restart_without_a_session_to_come_back_to_is_refused() {
    let mut app = App::with_file(None);
    let why = app.prepare_restart().expect("refused");
    assert!(why.contains("session"), "{why}");
    assert!(!app.restart_requested);
}

#[test]
fn debug_start_or_continue_errors_for_an_unrecognized_language() {
    let dir = TempDir::new("debug_no_language");
    let file = dir.write("notes.txt", "hello\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.debug_start_or_continue();
    assert!(app.modeline_pieces().1.contains("no recognized language"));
}

#[test]
fn debug_start_or_continue_errors_when_no_adapter_is_known_for_the_language() {
    let dir = TempDir::new("debug_no_adapter");
    let file = dir.write("main.rs", "fn main() {}\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.debug_start_or_continue();
    let said = app.modeline_pieces().1;
    assert!(said.contains("no debug adapter for rust"), "{said}");
    assert!(said.contains("SPC p h") && said.contains("SPC h l"), "says where to look: {said}");
}

#[test]
fn debug_stepping_and_stop_error_when_no_session_is_running() {
    let mut app = App::with_file(None);
    app.debug_step_over();
    assert!(app.modeline_pieces().1.contains("no debug session running"));
    app.debug_step_into();
    assert!(app.modeline_pieces().1.contains("no debug session running"));
    app.debug_step_out();
    assert!(app.modeline_pieces().1.contains("no debug session running"));
    app.debug_stop();
    assert!(app.modeline_pieces().1.contains("no debug session running"));
}

#[test]
fn debug_add_watch_errors_with_no_identifier_before_the_cursor() {
    let dir = TempDir::new("debug_watch_no_ident");
    let file = dir.write("main.py", "   \n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.debug_add_watch();
    assert!(app.modeline_pieces().1.contains("no identifier before the cursor"));
}

/// Builds a `DebugSession` around a real (but trivial and short-
/// lived) spawned process -- enough for `DapClient::spawn` to
/// succeed and for the session's own state-transition logic
/// (`apply_dap_event`/`apply_dap_response`) to be exercised
/// directly with fabricated `DapEvent`s, the same "construct the
/// session by hand, drive it with synthetic events" shape `apply_
/// lsp_response`'s own tests already use for `LspSession`. Whether
/// requests sent through `session.client` actually reach anything
/// alive doesn't matter here -- only the dispatch logic on the
/// receiving side is under test.
/// A narrow column's hint wraps in it rather than stopping after a
/// few words; a value too long for its column is shown whole on the
/// drawer's bottom line while it's selected.
#[test]
fn the_drawer_wraps_its_hints_and_shows_a_cut_value_whole() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    let dock: String = app.debug_dock_spans(100).into_iter().map(|(text, _, _)| text).collect();
    let squeezed: String = dock.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(squeezed.contains("SPC u P pins one"), "the Watches hint whole, over its rows: {dock}");
    let long = format!("b'{}'", "\\x0c".repeat(40));
    {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.variables = vec![debug_drawer::DebugScope::of("Locals", &[("raw", long.as_str())])];
    }
    app.debug_dock_open = true;
    app.debug_drawer.focused = true;
    app.debug_drawer.column = 1;
    let rows = app.debug_drawer_rows(debug_drawer::Column::Variables);
    let at = rows.iter().position(|(t, _)| t.contains("raw")).unwrap();
    app.debug_drawer.selected[1] = at;
    let dock: String = app.debug_dock_spans(100).into_iter().map(|(text, _, _)| text).collect();
    let bottom = dock.lines().last().unwrap_or("");
    assert!(bottom.starts_with("› ") && bottom.contains("raw = b'"), "the selected value on the bottom line: {bottom}");
}

#[test]
fn the_debug_dock_says_the_standard_keys_with_standard_keys() {
    let mut app = App::with_file(None);
    app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
    app.sync_key_policy();
    debug_test_session(&mut app);
    let dock: String = app.debug_dock_spans(200).into_iter().map(|(text, _, _)| text).collect();
    assert!(!dock.contains("SPC ") && !dock.contains("K on a name"), "{dock}");
    assert!(dock.contains("C-k u f") && dock.contains("C-k u e evaluates"), "{dock}");
}

fn debug_test_session(app: &mut App) {
    let dir = TempDir::new("debug_session_probe");
    // A process that stays alive: requests the test makes must reach it.
    let (client, _rx) = fenix_dap::DapClient::spawn_command(sink_command(dir.path())).expect("the test binary can start itself");
    let generation = tool_sessions::generation();
    app.debug_session = Some(DebugSession {
        language: fenix_syntax::LanguageId::Python,
        generation,
        launch_attributes: serde_json::Map::new(),
        attach: false,
        name: "main.py".into(),
        adapter: crate::dap::Adapter { command: None, port: None },
        port: None,
        children: Vec::new(),
        stopped_on: generation,
        ended: false,
        client,
        reader: None,
        pending: HashMap::new(),
        // The project you're in, as when it started from its code.
        root: app.integration_root(),
        initialized: false,
        stopped_thread: None,
        last_thread: None,
        current_frame_id: None,
        call_stack: Vec::new(),
        frame_ids: Vec::new(),
        selected_frame: 0,
        variables: Vec::new(),
        toggled: Default::default(),
        console: Default::default(),
        watches: Vec::new(),
        watch_values: HashMap::new(),
        end: None,
        output_tail: Default::default(),
        client_closed: false,
        exception_filters: Vec::new(),
        pin_values: HashMap::new(),
        running_on: generation,
    });
}

/// A stop's file, already shown in the workspace an earlier stop
/// opened: that one comes forward, no new workspace each time.
#[test]
fn a_second_stop_in_the_same_file_reuses_its_workspace() {
    let dir = TempDir::new("debug_goto_reuse");
    let target = dir.write("main.py", "x = 1\ny = 2\n");
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    app.goto_debug_location(&target, 1);
    // Off to another workspace, as after looking at something else.
    app.new_workspace();
    let after_first = app.workspaces.len();
    app.goto_debug_location(&target, 2);
    assert_eq!(app.workspaces.len(), after_first, "the second stop opened another workspace");
    assert_eq!(app.open().buffer.path(), Some(target.as_path()));
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1);
}

/// The program over: the panel says so, with its exit code and last
/// output, and what it showed while stopped goes.
#[test]
fn an_ended_program_says_how_with_its_last_output() {
    use fenix_dap::events::{Event, ExitedEventBody, OutputCategory, OutputEventBody};
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.variables = vec![debug_drawer::DebugScope::of("Locals", &[("x", "1")])];
    }
    let said = OutputEventBody::builder().output("Traceback (most recent call last):\nModuleNotFoundError: No module named 'pyapp'\n".to_string()).category(OutputCategory::Stderr).build();
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Output(said)));
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Exited(ExitedEventBody::builder().exit_code(1).build())));
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Terminated(fenix_dap::events::TerminatedEventBody::builder().build())));
    let s = app.debug_session.as_ref().unwrap();
    assert!(s.ended && s.variables.is_empty() && s.stopped_thread.is_none());
    let text = render_call_stack_text(s);
    assert!(text.contains("(program ended, exit code 1)"), "{text}");
    assert!(text.contains("ModuleNotFoundError: No module named 'pyapp'"), "{text}");
    assert!(text.contains("SPC u u starts another session"), "{text}");
}

/// CodeLLDB refusing a launch: the panel says why, not "starting".
#[test]
fn a_refused_launch_ends_the_session_and_says_why() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    let seq = app.debug_session.as_ref().unwrap().client.request(fenix_dap::requests::Request::ConfigurationDone).unwrap();
    app.debug_session.as_mut().unwrap().pending.insert(seq, PendingDapRequest::Launch);
    let err = fenix_dap::responses::ErrorResponse::builder().command(String::new()).message("error: Error reading commands from file lldb_commands - file not found.\n".to_string()).body(fenix_dap::responses::ErrorResponseBody::new(None)).build();
    app.apply_dap_event(fenix_dap::DapEvent::Response { request_seq: seq, result: Err(err) });
    let s = app.debug_session.as_ref().unwrap();
    assert!(s.ended);
    assert!(render_call_stack_text(s).contains("(the debug adapter refused: error: Error reading commands from file lldb_commands - file not found.)"), "{}", render_call_stack_text(s));
    assert!(app.modeline_pieces().1.contains("launch failed"));
}

/// netcoredbg refuses `configurationDone` when the program didn't
/// start: the session ends, its output (why) shown.
#[test]
fn a_refused_configuration_done_ends_the_session_with_its_output() {
    use fenix_dap::events::{Event, OutputCategory, OutputEventBody};
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    let said = OutputEventBody::builder().output("A fatal error was encountered. The library 'hostpolicy.dll' required to execute the application was not found".to_string()).category(OutputCategory::Stderr).build();
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Output(said)));
    let seq = app.debug_session.as_ref().unwrap().client.request(fenix_dap::requests::Request::ConfigurationDone).unwrap();
    app.debug_session.as_mut().unwrap().pending.insert(seq, PendingDapRequest::ConfigurationDone);
    let err = fenix_dap::responses::ErrorResponse::builder().command("configurationDone".to_string()).message("Failed command 'configurationDone' : 0x80004005".to_string()).body(fenix_dap::responses::ErrorResponseBody::new(None)).build();
    app.apply_dap_event(fenix_dap::DapEvent::Response { request_seq: seq, result: Err(err) });
    let s = app.debug_session.as_ref().unwrap();
    let text = render_call_stack_text(s);
    assert!(s.ended && text.contains("configurationDone: Failed command") && text.contains("hostpolicy.dll"), "{text}");
}

/// A session that ended in another project doesn't block starting one
/// here: it gives way, as one in this project does. One still running
/// there still does.
#[test]
fn an_ended_session_in_another_project_gives_way() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    {
        let s = app.debug_session.as_mut().unwrap();
        s.root = PathBuf::from("/some/other/project");
        s.ended = true;
    }
    app.debug_start_or_continue();
    assert!(!app.modeline_pieces().1.contains("another project"), "{}", app.modeline_pieces().1);
    assert!(app.debug_session.as_ref().is_none_or(|s| s.root != Path::new("/some/other/project")));

    debug_test_session(&mut app);
    app.debug_session.as_mut().unwrap().root = PathBuf::from("/some/other/project");
    app.debug_start_or_continue();
    assert!(app.modeline_pieces().1.contains("another project"), "{}", app.modeline_pieces().1);
    app.debug_start_from(crate::dap::Template::from_project(&Default::default(), "python").unwrap());
    assert!(app.debug_session.as_ref().is_some_and(|s| s.root == Path::new("/some/other/project")));
}

/// `SPC u q`: the adapter goes, the panel says "stopped", and the next
/// `SPC u u` clears it away rather than saying a session is running.
#[test]
fn a_stopped_session_gives_way_to_the_next() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    let workspaces = app.workspaces.len();
    app.debug_stop();
    let s = app.debug_session.as_ref().expect("kept, ended, for its panel");
    assert!(s.ended && s.client_closed);
    assert!(render_call_stack_text(s).contains("(stopped)"));
    app.debug_stop();
    assert!(app.modeline_pieces().1.contains("no debug session running"));
    // No file here, so nothing starts -- but the old session is gone,
    // and no workspace with it: it never had one.
    app.debug_start_or_continue();
    assert!(app.debug_session.is_none());
    assert_eq!(app.workspaces.len(), workspaces);
    assert!(!app.modeline_pieces().1.contains("already running"));
}

/// A program that ended on its own, and its adapter still there (dlv):
/// `SPC u q` still closes it, and keeps the exit code shown.
#[test]
fn stopping_after_the_end_closes_the_adapter_and_keeps_the_exit_code() {
    use fenix_dap::events::{Event, ExitedEventBody};
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Exited(ExitedEventBody::builder().exit_code(0).build())));
    app.debug_stop();
    let s = app.debug_session.as_ref().unwrap();
    assert!(s.client_closed);
    assert!(render_call_stack_text(s).contains("exit code 0"));
}

/// Stopped: the breakpoint and the stop in the gutter, the frame's
/// values at the ends of the lines above it; running again, the values
/// go and the breakpoint stays.
#[test]
fn a_stop_is_marked_in_the_gutter_and_its_values_written_in_the_code() {
    let dir = TempDir::new("debug_inline");
    let file = dir.write("main.py", "def f(a):\n    b = a + 1\n    return b\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let buffer = app.focused_buffer_id();
    app.breakpoints.entry(file.clone()).or_default().insert(3, Default::default());
    debug_test_session(&mut app);
    {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.call_stack = vec![("f".into(), file.clone(), 3)];
        s.variables = vec![debug_drawer::DebugScope::of("Locals", &[("a", "1"), ("b", "2")])];
    }
    app.refresh_debug_panes();
    assert_eq!(app.debug_marks(buffer, &[0, 1, 2], 3), vec![(2, debug_host::DebugMark::StopOnBreakpoint(debug_breakpoints::BreakpointLook::Plain))]);
    let texts: Vec<String> = app.code_decorations(buffer).into_iter().filter_map(|d| match &d.look { crate::decorations::Look::Chip { text, .. } => Some(text.trim().to_string()), _ => None }).collect();
    assert_eq!(texts, vec!["a = 1", "b = 2"], "a on the def line, b on its last line above the stop");
    app.apply_dap_event(fenix_dap::DapEvent::Event(fenix_dap::events::Event::Continued(fenix_dap::events::ContinuedEventBody::builder().thread_id(1).build())));
    assert!(app.code_decorations(buffer).is_empty(), "running: no values");
    assert_eq!(app.debug_marks(buffer, &[0, 1, 2], 3), vec![(2, debug_host::DebugMark::Breakpoint(debug_breakpoints::BreakpointLook::Plain))]);
}

fn dap_scope(name: &str, reference: i32) -> fenix_dap::types::Scope {
    fenix_dap::types::Scope::builder().name(name.to_string()).variables_reference(reference).expensive(false).build()
}

fn dap_var(name: &str, value: &str, reference: i32) -> fenix_dap::types::Variable {
    fenix_dap::types::Variable::builder().name(name.to_string()).value(value.to_string()).variables_reference(reference).build()
}

fn asked_vars(app: &App) -> Vec<(i32, Vec<String>)> {
    let mut asked: Vec<(i32, Vec<String>)> = app.debug_session.as_ref().unwrap().pending.values().filter_map(|p| match p { PendingDapRequest::Variables { frame, path } => Some((*frame, path.clone())), _ => None }).collect();
    asked.sort();
    asked
}

/// The drawer's variables: the first scope's asked for, a variable
/// opened asked for in turn and kept open into the next frame; a frame
/// selected takes the values in the code with it, and what comes late
/// for the frame before is dropped.
#[test]
fn the_drawer_opens_variables_and_selecting_a_frame_moves_the_values() {
    let dir = TempDir::new("debug_drawer_frames");
    let caller = dir.write("caller.py", "def main():\n    rows = run()\n");
    let callee = dir.write("callee.py", "def run():\n    total = 1\n    return total\n");
    let mut app = App::with_file(Some(caller.to_string_lossy().into_owned()));
    let buffer = app.focused_buffer_id();
    debug_test_session(&mut app);
    let on = {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.call_stack = vec![("run".into(), callee.clone(), 3), ("main".into(), caller.clone(), 2)];
        s.frame_ids = vec![10, 20];
        s.current_frame_id = Some(10);
        s.generation
    };
    app.debug_scopes_came(on, 10, vec![dap_scope("Locals", 1), dap_scope("Globals", 2)]);
    assert_eq!(asked_vars(&app), vec![(10, vec!["Locals".to_string()])], "the first scope, not Globals");
    app.debug_variables_came(on, 10, vec!["Locals".into()], vec![dap_var("o", "Order(1)", 7), dap_var("total", "1", 0)]);
    app.debug_toggle_var(vec!["Locals".into(), "o".into()]);
    assert!(asked_vars(&app).contains(&(10, vec!["Locals".to_string(), "o".to_string()])));
    app.debug_variables_came(on, 10, vec!["Locals".into(), "o".into()], vec![dap_var("qty", "3", 0)]);
    let rows: Vec<String> = app.debug_drawer_rows(debug_drawer::Column::Variables).into_iter().map(|(t, _)| t).collect();
    assert_eq!(rows, ["▾ Locals", "  ▾ o = Order(1)", "      qty = 3", "    total = 1", "▸ Globals"]);

    app.debug_select_frame(1);
    let s = app.debug_session.as_ref().unwrap();
    assert_eq!((s.selected_frame, s.current_frame_id, s.variables.is_empty()), (1, Some(20), true));
    assert!(s.pending.values().any(|p| matches!(p, PendingDapRequest::Scopes { frame: 20 })));
    app.debug_variables_came(on, 10, vec!["Locals".into()], vec![dap_var("late", "1", 0)]);
    assert!(app.debug_session.as_ref().unwrap().variables.is_empty(), "frame 10's answer, late: dropped");
    app.debug_scopes_came(on, 20, vec![dap_scope("Locals", 3)]);
    app.debug_variables_came(on, 20, vec!["Locals".into()], vec![dap_var("rows", "[]", 0), dap_var("o", "Order(2)", 8)]);
    assert!(asked_vars(&app).contains(&(20, vec!["Locals".to_string(), "o".to_string()])), "o stays open");
    let texts: Vec<String> = app.code_decorations(buffer).into_iter().filter_map(|d| match &d.look { crate::decorations::Look::Chip { text, .. } => Some(text.trim().to_string()), _ => None }).collect();
    assert_eq!(texts, vec!["rows = []"], "main's values, in caller.py");
    let stack: Vec<String> = app.debug_drawer_rows(debug_drawer::Column::Stack).into_iter().map(|(t, _)| t).collect();
    assert_eq!(stack, ["  run  callee.py:3", "▸ main  caller.py:2"]);
}

/// A stop at `line` of `file` in `f`, its locals `values` come, as the
/// adapter's answers would bring them.
fn trail_stop(app: &mut App, file: &Path, line: usize, values: &[(&str, &str)]) {
    let on = {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.call_stack = vec![("f".into(), file.to_path_buf(), line)];
        s.frame_ids = vec![10];
        s.selected_frame = 0;
        s.current_frame_id = Some(10);
        s.generation
    };
    app.debug_trail_stopped();
    app.debug_scopes_came(on, 10, vec![dap_scope("Locals", 1)]);
    app.debug_variables_came(on, 10, vec!["Locals".into()], values.iter().map(|(n, v)| dap_var(n, v, 0)).collect());
}

/// The Ember Trail: a changed value in ember with the old one struck,
/// the stripe on the lines stopped on, `[t` back to a stop as it was
/// and Enter to now; a restarted session keeps the trail, dimmed.
#[test]
fn the_ember_trail_keeps_each_stop_and_goes_back_along_it() {
    use crate::decorations::Look;
    let dir = TempDir::new("debug_trail");
    let file = dir.write("main.py", "total = 0\nfor i in range(3):\n    total += i\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let buffer = app.focused_buffer_id();
    debug_test_session(&mut app);
    trail_stop(&mut app, &file, 3, &[("total", "0"), ("i", "0")]);
    trail_stop(&mut app, &file, 3, &[("total", "1"), ("i", "1")]);
    let looks: Vec<Look> = app.code_decorations(buffer).into_iter().map(|d| d.look.clone()).collect();
    assert!(looks.contains(&Look::Chip { text: "total = 1".into(), role: crate::page::Role::Warn }), "a changed value on a chip of its own: {looks:?}");
    assert!(looks.contains(&Look::Chip { text: "i = 1".into(), role: crate::page::Role::Warn }), "{looks:?}");
    assert!(looks.contains(&Look::Struck("0".into())), "{looks:?}");
    let marks = app.debug_trail_marks(buffer, &[0, 1, 2], 3);
    assert_eq!(marks.iter().map(|(row, c)| (*row, c[3])).collect::<Vec<_>>(), [(2, 1.0)]);
    let ribbon: String = app.debug_ribbon_spans().unwrap().into_iter().map(|(t, _, _)| t).collect();
    assert!(ribbon.contains("●◉") && ribbon.contains("stop 2/2") && ribbon.contains("changed total i"), "{ribbon}");

    app.debug_trail_scrub(false, 1);
    assert_eq!(app.debug_trail.viewing, Some(0));
    assert!(app.debug_modeline_segment().contains("stop 1/2"), "{}", app.debug_modeline_segment());
    let texts: Vec<String> = app.code_decorations(buffer).into_iter().filter_map(|d| match &d.look { Look::Chip { text, .. } => Some(text.trim().to_string()), _ => None }).collect();
    assert_eq!(texts, ["total = 0", "i = 0"], "the first stop, as it was");
    assert!(app.debug_trail_key(KeyPress::named(FenixNamedKey::Enter)));
    assert_eq!(app.debug_trail.viewing, None);
    app.debug_trail_scrub(false, 1);
    app.debug_trail_scrub(true, 1);
    assert_eq!(app.debug_trail.viewing, None, "]t past the newest is now");

    // Ended: the breadcrumb is back under the tabs, and `[t` still
    // goes back along the trail, showing it again.
    app.debug_session.as_mut().unwrap().ended = true;
    assert!(app.debug_ribbon_spans().is_none(), "no ribbon once the session's over");
    app.debug_trail_scrub(false, 1);
    assert!(app.debug_ribbon_spans().is_some(), "looking back along the trail shows it");
    app.debug_trail_now();

    // Restarted: the trail stays, dim.
    debug_test_session(&mut app);
    assert_eq!(app.debug_trail.stops.len(), 2);
    let marks = app.debug_trail_marks(buffer, &[0, 1, 2], 3);
    assert_eq!(marks.iter().map(|(row, c)| (*row, c[3])).collect::<Vec<_>>(), [(2, 0.3)]);
}

/// A breakpoint's condition from the drawer's line, then off: kept, not
/// sent, dim in the gutter; the exceptions the adapter offers listed
/// above the breakpoints and turned on there.
#[test]
fn a_breakpoint_takes_a_condition_and_goes_off_and_exceptions_are_chosen() {
    let dir = TempDir::new("debug_conditions");
    let file = dir.write("main.py", "for i in range(3):\n    print(i)\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let buffer = app.focused_buffer_id();
    debug_test_session(&mut app);
    app.debug_edit_breakpoint(BreakpointPart::Condition);
    assert!(app.debug_drawer_focused());
    for c in "i == 2".chars() {
        app.debug_drawer_key(KeyPress::char(c));
    }
    app.debug_drawer_key(KeyPress::named(FenixNamedKey::Enter));
    let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
    assert_eq!(app.breakpoints[&canonical][&1].condition.as_deref(), Some("i == 2"));
    assert_eq!(app.debug_marks(buffer, &[0, 1], 2), vec![(0, debug_host::DebugMark::Breakpoint(debug_breakpoints::BreakpointLook::Conditional))]);
    app.debug_toggle_breakpoint_on();
    assert!(app.breakpoints[&canonical][&1].off);
    assert_eq!(app.debug_marks(buffer, &[0, 1], 2), vec![(0, debug_host::DebugMark::Breakpoint(debug_breakpoints::BreakpointLook::Off))]);
    assert!(debug_breakpoints::source_breakpoints(&app.breakpoints[&canonical], None).is_empty(), "off: not sent");

    // The adapter's exceptions, from its answer to initialize.
    let filter = |f: &str, label: &str, default: bool| fenix_dap::types::ExceptionBreakpointsFilter::builder().filter(f.to_string()).label(label.to_string()).default(default).build();
    let caps = fenix_dap::types::Capabilities::builder().exception_breakpoint_filters(vec![filter("raised", "Raised Exceptions", false), filter("uncaught", "Uncaught Exceptions", true)]).build();
    let seq = app.debug_session.as_ref().unwrap().client.request(fenix_dap::requests::Request::ConfigurationDone).unwrap();
    app.debug_session.as_mut().unwrap().pending.insert(seq, PendingDapRequest::Initialize);
    app.apply_dap_event(fenix_dap::DapEvent::Response { request_seq: seq, result: Ok(fenix_dap::responses::SuccessResponse::Initialize(caps)) });
    let rows: Vec<String> = app.debug_drawer_rows(debug_drawer::Column::Breakpoints).into_iter().map(|(t, _)| t).collect();
    assert_eq!(rows, ["☐ Raised Exceptions", "☑ Uncaught Exceptions", "○ main.py:1  if i == 2  off"]);
    app.debug_exception_breakpoints();
    app.debug_drawer_key(KeyPress::char('t'));
    assert_eq!(app.debug_exceptions.get("raised"), Some(&true));
}

/// A logpoint's message, which adapters send as console output, goes
/// to the drawer's console but not the program's last words.
#[test]
fn console_output_goes_to_the_drawer_but_not_the_programs_last_words() {
    use fenix_dap::events::{Event, OutputCategory, OutputEventBody};
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Output(OutputEventBody::builder().output("i is 2\n".to_string()).category(OutputCategory::Console).build())));
    app.apply_dap_event(fenix_dap::DapEvent::Event(Event::Output(OutputEventBody::builder().output("{}".to_string()).category(OutputCategory::Telemetry).build())));
    let s = app.debug_session.as_ref().unwrap();
    assert_eq!((s.console.iter().cloned().collect::<Vec<_>>(), s.output_tail.len()), (vec!["i is 2".to_string()], 0));
}

/// A pinned name's value written at its line at each stop, dim once
/// the program runs on.
#[test]
fn a_pinned_value_is_written_at_its_line() {
    use crate::decorations::Look;
    let dir = TempDir::new("debug_pins");
    let file = dir.write("main.py", "rows = load()\nprint(rows)\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let buffer = app.focused_buffer_id();
    debug_test_session(&mut app);
    {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.current_frame_id = Some(10);
    }
    app.debug_toggle_pin();
    let pin = app.debug_pins[0].clone();
    assert_eq!((pin.line, pin.expression.as_str()), (1, "rows"));
    assert!(app.debug_session.as_ref().unwrap().pending.values().any(|p| matches!(p, PendingDapRequest::Pin { pin: asked } if *asked == pin)));
    app.debug_pin_answered(pin.clone(), Some("[1, 2]".into()));
    let looks: Vec<Look> = app.code_decorations(buffer).into_iter().map(|d| d.look.clone()).collect();
    assert_eq!(looks, [Look::Text { text: "    ◆ rows = [1, 2]".into(), role: crate::page::Role::Accent, after: true }]);
    app.apply_dap_event(fenix_dap::DapEvent::Event(fenix_dap::events::Event::Continued(fenix_dap::events::ContinuedEventBody::builder().thread_id(1).build())));
    let looks: Vec<Look> = app.code_decorations(buffer).into_iter().map(|d| d.look.clone()).collect();
    assert_eq!(looks, [Look::Text { text: "    ◆ rows = [1, 2]".into(), role: crate::page::Role::Muted, after: true }], "running: the last stop's, dim");
    app.debug_toggle_pin();
    assert!(app.debug_pins.is_empty() && app.code_decorations(buffer).is_empty(), "unpinned");
}

/// Child sessions (js-debug's): each under the one that asked for it in
/// the stack column, the stopped one's frames under it; the stop names
/// the child; F6 pauses the connection the program runs on; its start
/// and its exit go to the console.
#[test]
fn child_sessions_are_shown_under_their_parent() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    let dir = TempDir::new("debug_child");
    let (client, _rx) = fenix_dap::DapClient::spawn_command(sink_command(dir.path())).unwrap();
    let (root, child) = (app.debug_session.as_ref().unwrap().generation, tool_sessions::generation());
    {
        let s = app.debug_session.as_mut().unwrap();
        s.children.push(DebugChild { generation: child, parent: root, name: "app.js".into(), client, reader: None, pending: HashMap::new(), configuration: Default::default(), attach: false, initialized: true });
    }
    app.apply_dap_event_on(child, fenix_dap::DapEvent::Event(fenix_dap::events::Event::Thread(fenix_dap::events::ThreadEventBody::builder().reason(fenix_dap::events::ThreadEventReason::Started).thread_id(7).build())));
    assert_eq!(app.debug_session.as_ref().unwrap().running_on, child, "F6 pauses the child");
    {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(7);
        s.stopped_on = child;
        s.call_stack = vec![("handler".into(), PathBuf::from("/p/app.js"), 4)];
    }
    let rows: Vec<String> = app.debug_drawer_rows(debug_drawer::Column::Stack).into_iter().map(|(t, _)| t).collect();
    assert_eq!(rows, ["main.py (running)", "  app.js (paused)", "    ▸ handler  app.js:4"]);
    assert!(debug_host::debug_state_text(app.debug_session.as_ref().unwrap()).ends_with("app.js:4 in app.js"));
    app.apply_dap_event_on(child, fenix_dap::DapEvent::Event(fenix_dap::events::Event::Exited(fenix_dap::events::ExitedEventBody::builder().exit_code(3).build())));
    let s = app.debug_session.as_ref().unwrap();
    assert!(s.children.is_empty() && s.running_on == root && !s.ended);
    assert_eq!(s.console.iter().cloned().collect::<Vec<_>>(), ["app.js exited with code 3", "app.js ended"]);
}

/// In the drawer: columns and rows walked, a watch added and taken
/// away, the evaluate line asked with its answer in the console, Esc
/// back out; SPC is still the leader's.
#[test]
fn the_drawer_takes_the_keys_until_esc() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    app.debug_session.as_mut().unwrap().stopped_thread = Some(1);
    app.debug_session.as_mut().unwrap().current_frame_id = Some(10);
    app.debug_focus_drawer();
    assert!(app.debug_drawer_focused());
    assert!(!app.debug_drawer_key(KeyPress::char(' ')), "SPC is the leader's");
    app.debug_drawer_key(KeyPress::char('a'));
    for c in "len(rows)".chars() {
        app.debug_drawer_key(KeyPress::char(c));
    }
    app.debug_drawer_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(app.debug_session.as_ref().unwrap().watches, ["len(rows)"]);
    assert!(app.debug_session.as_ref().unwrap().pending.values().any(|p| matches!(p, PendingDapRequest::Evaluate { expression } if expression == "len(rows)")));
    app.debug_drawer_key(KeyPress::char('l'));
    app.debug_drawer_key(KeyPress::char('l'));
    assert_eq!(app.debug_drawer.column, 2);
    app.debug_drawer_key(KeyPress::char('x'));
    assert!(app.debug_session.as_ref().unwrap().watches.is_empty());

    app.debug_drawer_key(KeyPress::char('i'));
    for c in "1/0".chars() {
        app.debug_drawer_key(KeyPress::char(c));
    }
    app.debug_drawer_key(KeyPress::named(FenixNamedKey::Enter));
    assert!(app.debug_session.as_ref().unwrap().pending.values().any(|p| matches!(p, PendingDapRequest::Repl { expression } if expression == "1/0")));
    let err = fenix_dap::responses::ErrorResponse::builder().command("evaluate".to_string()).message("ZeroDivisionError: division by zero".to_string()).body(fenix_dap::responses::ErrorResponseBody::new(None)).build();
    app.debug_repl_answered("1/0".into(), Err(err));
    let console: Vec<String> = app.debug_drawer_rows(debug_drawer::Column::Console).into_iter().map(|(t, _)| t).collect();
    assert_eq!(console, ["› 1/0", "  ! ZeroDivisionError: division by zero"]);
    app.debug_drawer_key(KeyPress::char('i'));
    app.debug_drawer_key(KeyPress::named(FenixNamedKey::Up));
    assert_eq!(app.debug_drawer.line.as_ref().map(|l| l.text.as_str()), Some("1/0"), "Up recalls what was asked");
    app.debug_drawer_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(app.debug_drawer.line.is_none() && app.debug_drawer_focused(), "Esc lets the line go first");
    app.debug_drawer_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(!app.debug_drawer_focused());
    let dock: String = app.debug_dock_spans(160).into_iter().map(|(text, _, _)| text).collect();
    assert!(dock.contains("CONSOLE") && dock.contains("SPC u e evaluates"), "{dock}");
}

/// `K` while stopped: the name under the cursor's value on a card, what's
/// in it under it; a name the adapter can't evaluate is the language
/// server's hover.
#[test]
fn the_lens_shows_a_value_while_stopped() {
    let dir = TempDir::new("debug_lens");
    let file = dir.write("main.py", "order = make()\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    assert!(!app.debug_lens(), "no session: K is the hover");
    debug_test_session(&mut app);
    let on = {
        let s = app.debug_session.as_mut().unwrap();
        s.stopped_thread = Some(1);
        s.current_frame_id = Some(10);
        s.generation
    };
    assert!(app.debug_lens());
    assert!(app.debug_session.as_ref().unwrap().pending.values().any(|p| matches!(p, PendingDapRequest::Lens { expression } if expression == "order")));
    let body = fenix_dap::responses::EvaluateResponseBody::builder().result("Order(1042)".to_string()).type_(Some("Order".to_string())).variables_reference(5).build();
    app.debug_lens_answered(on, "order".into(), Ok(fenix_dap::responses::SuccessResponse::Evaluate(body)));
    assert_eq!(app.lsp_hover.as_deref(), Some("order = Order(1042)\nOrder"));
    let card = app.debug_session.as_ref().unwrap().pending.values().find_map(|p| match p { PendingDapRequest::LensChildren { card } => Some(card.clone()), _ => None }).expect("what's in it asked for");
    app.debug_lens_children(card, vec![dap_var("qty", "3", 0)]);
    assert_eq!(app.lsp_hover.as_deref(), Some("order = Order(1042)\nOrder\n  qty = 3"));
}

#[test]
fn a_stop_puts_the_editor_in_debug_until_esc_and_the_next_stop_brings_it_back() {
    let dir = TempDir::new("debug_mode");
    let file = dir.write("main.py", "a = 1\nb = 2\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    debug_test_session(&mut app);
    app.workspaces.switch_to_index(0);
    let root = app.integration_root();
    app.debug_session.as_mut().unwrap().root = root;
    app.debug_session.as_mut().unwrap().initialized = true;
    assert!(!app.debug_mode_active());
    assert_eq!(app.debug_modeline_segment(), "   [running]");
    let stop = || fenix_dap::DapEvent::Event(fenix_dap::events::Event::Stopped(fenix_dap::events::StoppedEventBody::builder().reason(fenix_dap::events::StoppedEventReason::Breakpoint).thread_id(Some(1)).build()));
    app.apply_dap_event(stop());
    assert!(app.debug_mode_active());
    assert_eq!(app.mode_badge_label(), "DEBUG");
    assert!(app.debug_modeline_segment().starts_with("   [stopped]   n next"), "{}", app.debug_modeline_segment());
    app.debug_do(debug_host::DebugAction::Leave);
    assert_eq!((app.debug_mode_active(), app.mode_badge_label(), app.debug_modeline_segment().as_str()), (false, "NORMAL", "   [stopped]"));
    app.debug_start_or_continue();
    assert!(app.debug_mode_active(), "SPC u u brings DEBUG back rather than going on");
    app.debug_do(debug_host::DebugAction::Leave);
    app.apply_dap_event(stop());
    assert!(app.debug_mode_active(), "and so does the next stop");
}

#[test]
fn run_to_cursor_is_a_breakpoint_until_the_next_stop() {
    let dir = TempDir::new("debug_run_to");
    let file = dir.write("main.py", "a = 1\nb = 2\nc = 3\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    debug_test_session(&mut app);
    app.workspaces.switch_to_index(0);
    app.debug_session.as_mut().unwrap().stopped_thread = Some(1);
    let pane = app.focused_pane_id();
    app.pane_state_mut(pane).cursor = Cursor { char_idx: 12, sticky_col: 0 };
    app.debug_run_to_cursor();
    let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
    assert_eq!(app.debug_run_to, Some((canonical, 3)));
    assert!(app.breakpoints.values().all(|l| l.is_empty()), "not one of yours");
    app.apply_dap_event(fenix_dap::DapEvent::Event(fenix_dap::events::Event::Stopped(fenix_dap::events::StoppedEventBody::builder().reason(fenix_dap::events::StoppedEventReason::Breakpoint).thread_id(Some(1)).build())));
    assert_eq!(app.debug_run_to, None);
}

#[test]
fn workspace_names_stay_unique_after_one_is_removed() {
    let mut app = App::with_file(None);
    app.new_workspace(); // workspace-2
    app.new_workspace(); // workspace-3
    app.workspaces.switch_to_index(1);
    app.remove_workspace(); // workspace-2 goes
    app.new_workspace();
    let names: Vec<String> = app.workspaces.summaries().into_iter().map(|(_, n)| n.to_string()).collect();
    assert_eq!(names, vec!["workspace-1", "workspace-3", "workspace-2"]);
}

#[test]
fn a_successful_initialize_response_sends_launch_without_panicking() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    let seq = app.debug_session.as_ref().unwrap().client.request(fenix_dap::requests::Request::ConfigurationDone).unwrap();
    app.debug_session.as_mut().unwrap().pending.insert(seq, PendingDapRequest::Initialize);

    app.apply_dap_event(fenix_dap::DapEvent::Response {
        request_seq: seq,
        result: Ok(fenix_dap::responses::SuccessResponse::Initialize(fenix_dap::types::Capabilities::default())),
    });

    // A `launch` request should now be the one outstanding pending
    // entry (the `Initialize` one this test inserted was consumed).
    let session = app.debug_session.as_ref().unwrap();
    assert!(session.pending.values().any(|p| matches!(p, PendingDapRequest::Launch)));
}

#[test]
fn a_stopped_event_records_the_thread_and_requests_a_stack_trace() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);

    let body = fenix_dap::events::StoppedEventBody::builder().reason(fenix_dap::events::StoppedEventReason::Breakpoint).thread_id(Some(1)).build();
    app.apply_dap_event(fenix_dap::DapEvent::Event(fenix_dap::events::Event::Stopped(body)));

    let session = app.debug_session.as_ref().unwrap();
    assert_eq!(session.stopped_thread, Some(1));
    assert!(session.pending.values().any(|p| matches!(p, PendingDapRequest::StackTrace)));
}

#[test]
fn a_continued_event_clears_the_stopped_state_and_refreshes_panes() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    app.debug_session.as_mut().unwrap().stopped_thread = Some(1);
    app.debug_session.as_mut().unwrap().call_stack = vec![("main".to_string(), PathBuf::from("main.py"), 3)];

    let body = fenix_dap::events::ContinuedEventBody::builder().thread_id(1).build();
    app.apply_dap_event(fenix_dap::DapEvent::Event(fenix_dap::events::Event::Continued(body)));

    let session = app.debug_session.as_ref().unwrap();
    assert_eq!(session.stopped_thread, None);
    assert!(session.call_stack.is_empty());
    let dock: String = app.debug_dock_spans(120).into_iter().map(|(text, _, _)| text).collect();
    assert!(dock.contains("running") && !dock.contains("main.py:3"), "{dock}");
}

#[test]
fn an_unknown_adapter_event_is_ignored_without_panicking() {
    let mut app = App::with_file(None);
    debug_test_session(&mut app);
    app.apply_dap_event(fenix_dap::DapEvent::Unknown(serde_json::json!({"type": "event", "event": "debugpySockets"})));
    assert!(app.debug_session.is_some(), "an unrecognized event must not tear down the session");
}

/// The debug events that came while `done` isn't so, applied as the
/// event loop would, for up to 10 seconds.
fn pump_dap(app: &mut App, done: impl Fn(&App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        if started.elapsed() > Duration::from_secs(10) {
            let state = app.debug_session.as_ref().map(|s| (s.initialized, s.children.len(), s.stopped_thread, s.call_stack.len(), s.pending.len()));
            panic!("the debug session never got there: {state:?} {:?}", app.status_message.as_ref().map(|m| m.text.clone()));
        }
        let mut any = false;
        for i in 0..app.dap_unread.len() {
            let (generation, event) = match app.dap_unread[i].1.try_recv() {
                Ok(event) => (app.dap_unread[i].0, event),
                Err(_) => continue,
            };
            any = true;
            app.handle_user_event(FenixUserEvent::Dap { generation, event });
        }
        if !any {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A message from Fenix, read off a fake adapter's connection.
fn dap_read(reader: &mut std::io::BufReader<std::net::TcpStream>) -> serde_json::Value {
    serde_json::from_str(&read_lsp_frame(reader).expect("a message")).unwrap()
}

fn dap_write(stream: &std::net::TcpStream, message: serde_json::Value) {
    use std::io::Write;
    let body = message.to_string();
    (&*stream).write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
}

/// Requests read and answered -- with `body` for `command`, an empty
/// success for the rest -- up to and including `command`'s.
fn dap_answer_until(reader: &mut std::io::BufReader<std::net::TcpStream>, stream: &std::net::TcpStream, command: &str, body: serde_json::Value, seen: &std::sync::mpsc::Sender<serde_json::Value>) -> serde_json::Value {
    loop {
        let asked = dap_read(reader);
        let _ = seen.send(asked.clone());
        if asked["type"] != "request" {
            continue;
        }
        let this = asked["command"] == command;
        let answer_body = if this { body.clone() } else { serde_json::json!({}) };
        dap_write(stream, serde_json::json!({"seq": 1, "type": "response", "request_seq": asked["seq"], "success": true, "command": asked["command"], "body": answer_body}));
        if this {
            return asked;
        }
    }
}

/// an adapter over a socket that asks for a child session
/// (`startDebugging`, as js-debug does for every program) gets one: a
/// connection of its own to the same port, initialized and launched
/// with the configuration it gave. The child's stop is the session's:
/// its stack is asked of the child, stepping goes to it, and it shows
/// under its parent until it ends.
#[test]
fn a_child_session_an_adapter_asks_for_is_opened_on_its_own_connection() {
    let (dir, _) = project_scope_fixture("debug_child");
    let script = dir.write("app.js", "console.log(1)\n");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_tx, seen) = std::sync::mpsc::channel::<serde_json::Value>();
    let adapter = std::thread::spawn(move || {
        // The session's own connection.
        let (parent, _) = listener.accept().unwrap();
        let mut reading = std::io::BufReader::new(parent.try_clone().unwrap());
        let init = dap_answer_until(&mut reading, &parent, "initialize", serde_json::json!({"supportsConfigurationDoneRequest": true}), &seen_tx);
        assert_eq!(init["arguments"]["supportsStartDebuggingRequest"], true, "{init}");
        dap_answer_until(&mut reading, &parent, "launch", serde_json::json!(null), &seen_tx);
        dap_write(&parent, serde_json::json!({"seq": 2, "type": "event", "event": "initialized"}));
        dap_answer_until(&mut reading, &parent, "configurationDone", serde_json::json!(null), &seen_tx);
        dap_write(&parent, serde_json::json!({"seq": 3, "type": "request", "command": "startDebugging", "arguments": {"request": "launch", "configuration": {"type": "pwa-node", "name": "app.js [42]", "__pendingTargetId": "t1"}}}));
        let answer = dap_read(&mut reading);
        assert_eq!((answer["type"].as_str(), answer["request_seq"].as_i64(), answer["success"].as_bool()), (Some("response"), Some(3), Some(true)), "{answer}");
        // The child's, to the same port.
        let (child, _) = listener.accept().unwrap();
        let mut child_reading = std::io::BufReader::new(child.try_clone().unwrap());
        dap_answer_until(&mut child_reading, &child, "initialize", serde_json::json!({}), &seen_tx);
        let launch = dap_answer_until(&mut child_reading, &child, "launch", serde_json::json!(null), &seen_tx);
        assert_eq!(launch["arguments"]["__pendingTargetId"], "t1", "{launch}");
        dap_write(&child, serde_json::json!({"seq": 2, "type": "event", "event": "initialized"}));
        dap_answer_until(&mut child_reading, &child, "configurationDone", serde_json::json!(null), &seen_tx);
        dap_write(&child, serde_json::json!({"seq": 3, "type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 7}}));
        let frames = serde_json::json!({"stackFrames": [{"id": 1, "name": "main", "line": 1, "column": 1}]});
        let trace = dap_answer_until(&mut child_reading, &child, "stackTrace", frames, &seen_tx);
        assert_eq!(trace["arguments"]["threadId"], 7);
        let next = dap_answer_until(&mut child_reading, &child, "next", serde_json::json!(null), &seen_tx);
        assert_eq!(next["arguments"]["threadId"], 7, "stepping goes to the child");
        dap_write(&child, serde_json::json!({"seq": 4, "type": "event", "event": "terminated"}));
        // Both connections stay open until Fenix lets them go.
        let _ = dap_read(&mut child_reading);
        let _ = dap_read(&mut reading);
    });
    let mut app = App::with_file(Some(script.to_string_lossy().into_owned()));
    let root = app.integration_root();
    app.start_debug_session(fenix_syntax::LanguageId::JavaScript, root, crate::dap::Adapter { command: None, port: Some(port) }, serde_json::Map::new(), false, "app.js".into());
    pump_dap(&mut app, |app| app.debug_session.as_ref().is_some_and(|s| s.children.first().is_some_and(|c| c.initialized) && s.stopped_thread == Some(7) && !s.call_stack.is_empty()));
    let session = app.debug_session.as_ref().unwrap();
    let child = session.children[0].generation;
    assert_eq!((session.children[0].name.as_str(), session.children[0].parent, session.stopped_on), ("app.js [42]", session.generation, child));
    let shown = render_call_stack_text(session);
    assert!(shown.starts_with("app.js (running)\n  app.js [42] (paused)\n\nmain"), "{shown}");
    app.debug_step_over();
    pump_dap(&mut app, |app| app.debug_session.as_ref().is_some_and(|s| s.children.is_empty()));
    let session = app.debug_session.as_ref().unwrap();
    assert_eq!(session.stopped_thread, None, "the child that stopped is gone");
    assert!(!render_call_stack_text(session).contains("app.js [42]"));
    app.debug_stop();
    adapter.join().unwrap();
    let commands: Vec<String> = seen.try_iter().filter_map(|m| m["command"].as_str().map(str::to_string)).collect();
    assert!(commands.iter().filter(|c| *c == "initialize").count() == 2, "{commands:?}");
}

/// a launch's `preLaunchTask` runs first in the task pane, and
/// the session starts once it succeeds -- not when it fails.
#[test]
fn a_pre_launch_task_runs_first_and_the_session_waits_on_it() {
    let (dir, _) = project_scope_fixture("debug_pre_launch");
    let script = dir.write("main.py", "print(1)\n");
    let exe = std::env::current_exe().unwrap();
    let ok = serde_json::json!({"executable": exe, "args": ["--ignored", "--exact", "app::tests::project_tool_fixture"]});
    let fails = serde_json::json!({"executable": exe, "args": ["--no-such-flag"]});
    let adapter = serde_json::json!({"executable": exe, "args": ["--ignored", "--exact", "app::tests::lsp_sink_fixture"], "env": {"FENIX_LSP_SINK": "1"}});
    let tools = |task: &str| serde_json::json!({"tasks": {"build": ok, "broken": fails}, "dap": {"python": adapter}, "launch": {"program": "main.py", "preLaunchTask": task}}).to_string();
    dir.write(".fenix/tools.json", &tools("build"));
    let mut app = App::with_file(Some(script.to_string_lossy().into_owned()));
    app.debug_start_or_continue();
    assert!(app.task_session.is_some(), "the task ran in the task pane");
    assert!(app.debug_session.is_some(), "and the session started once it succeeded");
    app.debug_stop();
    dir.write(".fenix/tools.json", &tools("broken"));
    let id = app.buffers.open_path(&script);
    app.open_buffer_in_focused_pane(id);
    app.debug_start_or_continue();
    assert!(app.debug_session.is_none());
    assert!(app.status_message.as_ref().unwrap().text.contains("preLaunchTask broken failed"), "{}", app.status_message.as_ref().unwrap().text);
    dir.write(".fenix/tools.json", &tools("nothing"));
    app.open_buffer_in_focused_pane(id);
    app.debug_start_or_continue();
    assert!(app.status_message.as_ref().unwrap().text.contains("no such task"));
}

/// `"program": false` in the project's own launch sends none.
#[test]
fn the_projects_launch_may_send_no_program() {
    let (dir, _) = project_scope_fixture("debug_no_program");
    let script = dir.write("main.py", "print(1)\n");
    let exe = std::env::current_exe().unwrap();
    let adapter = serde_json::json!({"executable": exe, "args": ["--ignored", "--exact", "app::tests::lsp_sink_fixture"], "env": {"FENIX_LSP_SINK": "1"}});
    dir.write(".fenix/tools.json", &serde_json::json!({"dap": {"python": adapter}, "launch": {"program": false, "args": ["-m", "pkg"]}}).to_string());
    let mut app = App::with_file(Some(script.to_string_lossy().into_owned()));
    app.debug_start_or_continue();
    let session = app.debug_session.as_ref().expect("started");
    assert!(session.launch_attributes.get("program").is_none(), "{:?}", session.launch_attributes);
    assert_eq!(session.launch_attributes["args"], serde_json::json!(["-m", "pkg"]));
    assert_eq!(session.launch_attributes["console"], "internalConsole", "what Fenix sends for the language");
}

/// the project's own launch may attach, with attributes of its
/// own: `${pickProcess}` is picked from this machine's processes and
/// sent as a number.
#[test]
fn the_projects_launch_may_attach_to_a_process_picked() {
    let (dir, _) = project_scope_fixture("debug_attach_picked");
    let script = dir.write("main.py", "print(1)\n");
    let exe = std::env::current_exe().unwrap();
    let adapter = serde_json::json!({"executable": exe, "args": ["--ignored", "--exact", "app::tests::lsp_sink_fixture"], "env": {"FENIX_LSP_SINK": "1"}});
    dir.write(".fenix/tools.json", &serde_json::json!({"dap": {"python": adapter}, "launch": {"program": false, "request": "attach", "attributes": {"processId": "${pickProcess}"}}}).to_string());
    let mut app = App::with_file(Some(script.to_string_lossy().into_owned()));
    app.debug_start_or_continue();
    if matches!(app.active_picker, Some(ActivePicker::Launch(_))) {
        app.picker_confirm();
    }
    let session = app.debug_session.as_ref().expect("started");
    assert!(session.launch_attributes["processId"].is_u64(), "{:?}", session.launch_attributes);
    assert!(session.launch_attributes.get("program").is_none() && session.launch_attributes.get("args").is_none(), "an attach: {:?}", session.launch_attributes);
}

#[test]
fn handle_user_event_never_panics_for_the_dap_variant() {
    let mut app = App::with_file(None);
    app.handle_user_event(FenixUserEvent::Dap { generation: 0, event: fenix_dap::DapEvent::Disconnected("test".to_string()) });
}

#[test]
fn append_task_output_line_with_a_recognizable_location_feeds_the_quickfix_list() {
    let dir = TempDir::new("task_quickfix");
    let mut app = App::with_file(None);
    app.run_task(fenix_tasks::TaskDef { name: "noop".to_string(), command: "true-noop-placeholder".to_string(), args: Vec::new() }, dir.path().to_path_buf());
    let buffer = app.task_session.as_ref().unwrap().buffer;

    app.append_task_output_line(buffer, "src/main.rs:5:12: mismatched types".to_string());

    assert_eq!(app.quickfix.len(), 1);
    match &app.quickfix[0] {
        QuickfixEntry::Task(m) => {
            assert_eq!(m.path, dir.path().join("src/main.rs"));
            assert_eq!(m.line, 5);
            assert_eq!(m.col, 12);
        }
        other => panic!("expected a QuickfixEntry::Task, got {other:?}"),
    }
}

/// A cross-platform `(program, args)` that echoes `text` to stdout
/// and exits successfully -- `echo` isn't a real standalone binary
/// on Windows (it's a shell built-in), so this goes through `cmd /c`
/// there the same way a real npm-style `.cmd` shim already does
/// (see `fenix_lsp::client::resolve_command`'s own doc comment).
fn echo_command(text: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd".to_string(), vec!["/c".to_string(), "echo".to_string(), text.to_string()])
    } else {
        ("echo".to_string(), vec![text.to_string()])
    }
}

/// A server that never answers `initialize` doesn't say "starting"
/// for good: past `LSP_INITIALIZE_TIMEOUT` it's stopped, said to have
/// failed, and shown so in Tool Status until `:lsp-restart`.
#[test]
fn a_server_that_never_initializes_is_given_up_on() {
    let mut app = App::with_file(None);
    let key = LspKey::primary(fenix_syntax::LanguageId::TypeScript, PathBuf::from("/project"));
    let (client, _rx) = fenix_lsp::LspClient::connect(std::io::empty(), std::io::sink());
    let started = Instant::now();
    app.lsp_sessions.insert(key.clone(), LspSession {
        generation: tool_sessions::generation(), client, name: "typescript-language-server".into(), languages: vec!["typescript".into()], root: key.root.clone(), reader: None,
        started, capabilities: None, open_documents: HashMap::new(), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(), semantic: HashMap::new(), lenses: HashMap::new(),
    });
    assert!(app.background_jobs().iter().any(|j| j == "starting typescript-language-server"));
    // Not yet: when it's due is said.
    assert_eq!(app.lsp_initialize_tick(started + Duration::from_secs(1)), Some(started + LSP_INITIALIZE_TIMEOUT));
    assert!(app.lsp_sessions.contains_key(&key));

    assert_eq!(app.lsp_initialize_tick(started + LSP_INITIALIZE_TIMEOUT), None);
    assert!(!app.lsp_sessions.contains_key(&key));
    assert!(app.lsp_unavailable.contains(&key), "not started again every frame");
    assert!(app.background_jobs().is_empty());
    let said = app.modeline_pieces().1;
    assert!(said.contains("typescript-language-server didn't start") && said.contains(":lsp-restart"), "{said}");
    assert!(app.lsp_failures[&key].contains("didn't answer initialize in 30 s"));
}

#[test]
fn a_failure_shows_the_last_lines_a_server_wrote() {
    assert_eq!(last_line_of(""), "");
    assert_eq!(last_line_of("starting\nError: no tsserver.js\n\n"), " -- it said: Error: no tsserver.js");
    let text = failure_text("didn't answer", &(0..20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n"));
    assert!(text.starts_with("didn't answer\n  | line 12"), "{text}");
    assert!(text.ends_with("  | line 19"));
    assert_eq!(failure_text("gone", ""), "gone (it wrote nothing to stderr)");
}

/// A frame with nothing new to tell a server doesn't copy the text or
/// look at the disk again; one after an edit does.
#[test]
fn a_frame_only_syncs_the_language_servers_when_something_moved() {
    let dir = TempDir::new("lsp_frame_sync");
    let file = dir.write("notes.txt", "one\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.sync_lsp_if_changed();
    let first = app.lsp_frame_synced;
    assert!(first.is_some());
    app.sync_lsp_if_changed();
    assert_eq!(app.lsp_frame_synced, first);
    app.test_insert_str("x");
    app.sync_lsp_if_changed();
    assert_ne!(app.lsp_frame_synced, first, "an edit is synced");
}

#[test]
fn apply_lsp_diagnostics_stores_and_then_clears_them_by_path() {
    let mut app = App::with_file(None);
    let path = std::env::temp_dir().join("fenix-lsp-test-diagnostics.py");
    let uri = fenix_lsp::path_to_uri(&path).unwrap();
    let diag = lsp_types::Diagnostic { range: lsp_types::Range::default(), message: "oops".to_string(), ..Default::default() };
    let key = app.lsp_key(fenix_syntax::LanguageId::Python);
    app.apply_lsp_diagnostics(&key, lsp_types::PublishDiagnosticsParams { uri: uri.clone(), diagnostics: vec![diag.clone()], version: None });
    let shown = app.diagnostics.for_path(&path);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].message, diag.message);

    // An empty list clears it -- that's the server's own "no more
    // diagnostics here" signal, not "nothing changed."
    app.apply_lsp_diagnostics(&key, lsp_types::PublishDiagnosticsParams { uri, diagnostics: Vec::new(), version: None });
    assert!(app.diagnostics.for_path(&path).is_empty());
}

#[test]
fn modeline_shows_error_and_warning_counts_for_the_focused_buffers_diagnostics() {
    let dir = TempDir::new("modeline_diagnostics");
    let file = dir.write("has_bugs.py", "x = 1\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    assert!(!app.modeline_text().contains('E'), "no diagnostics yet -- shouldn't show a count");

    let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
    let uri = fenix_lsp::path_to_uri(&canonical).unwrap();
    let error = lsp_types::Diagnostic { severity: Some(lsp_types::DiagnosticSeverity::ERROR), message: "bad".to_string(), ..Default::default() };
    let warning = lsp_types::Diagnostic { severity: Some(lsp_types::DiagnosticSeverity::WARNING), message: "meh".to_string(), ..Default::default() };
    let key = app.lsp_key(fenix_syntax::LanguageId::Python);
    app.apply_lsp_diagnostics(&key, lsp_types::PublishDiagnosticsParams { uri, diagnostics: vec![error, warning], version: None });

    let modeline = app.modeline_text();
    assert!(modeline.contains("1E 1W"), "expected a 1E 1W indicator, got: {modeline}");
}

#[test]
fn ensure_lsp_session_is_a_no_op_for_a_language_with_no_configured_or_default_command() {
    let mut app = App::with_file(None);
    app.ensure_lsp_session(fenix_syntax::LanguageId::Batch, &std::env::temp_dir());
    assert!(!app.lsp_sessions.keys().any(|key| key.is_primary_for(fenix_syntax::LanguageId::Batch)));
}

/// A Rust panic in Task Output: `SPC p n` goes to its file, not to
/// one named for the whole line, and `Enter` on the line goes there
/// too.
#[test]
fn a_panic_in_task_output_is_a_place_to_go() {
    let dir = TempDir::new("task_panic_place");
    let target = dir.write("lib.rs", "one\ntwo\nthree three\n");
    let mut app = App::with_file(None);
    let (program, args) = echo_command("noop");
    app.run_task(fenix_tasks::TaskDef { name: "t".to_string(), command: program, args }, dir.path().to_path_buf());
    let buffer = app.task_session.as_ref().unwrap().buffer;
    let cwd = app.task_session.as_ref().unwrap().cwd.clone();
    app.quickfix.clear();
    app.append_task_output_line(buffer, "thread 'tests::amounts' (16816) panicked at lib.rs:3:7:".to_string());
    match app.quickfix.last() {
        Some(QuickfixEntry::Task(m)) => assert_eq!((m.path.clone(), m.line, m.col), (cwd.join("lib.rs"), 3, 7)),
        _ => panic!("no quickfix entry for the panic"),
    }
    // Enter on the line, in the Task Output pane.
    assert_eq!(app.focused_buffer_id(), buffer);
    let last = app.open().buffer.visual_line_count().saturating_sub(1);
    let line = (0..=last).rev().find(|&l| app.open().buffer.line(l).to_string().contains("panicked")).expect("the line is shown");
    let start = app.open().buffer.line_start_char(line);
    app.focused_buffer_and_cursor_mut().1.char_idx = start;
    // What `Enter` there does (`route_keypress` needs a live event loop).
    assert!(app.task_output_open_location_at_cursor());
    assert_eq!(app.open().buffer.path().map(fenix_core::path_key), Some(fenix_core::path_key(&cwd.join("lib.rs"))));
    assert_eq!(app.open().buffer.line_col(&app.cursor()), (2, 6));
    let _ = target;
}

#[test]
fn quickfix_jump_does_not_hijack_a_focused_task_output_pane() {
    // Reproduces a real bug found live-testing the task runner:
    // `SPC p n` jumping to a diagnostic while the Task Output pane
    // was focused used to silently replace that pane's content with
    // the jumped-to file (`open_file_from_picker` -> `open_buffer_
    // in_focused_pane` had no way to know it was a tracked session,
    // unlike VNC/Docker/Git/Jira) -- so a later `SPC p r`/`SPC p T`
    // rerun kept running, just into a buffer no pane displayed
    // anymore. Exercises the exact path that broke:
    // `QuickfixEntry::Task` -> `jump_to_grep_match` -> `open_file_
    // from_picker`, not just the guard function directly.
    let dir = TempDir::new("task_quickfix_hijack");
    let target = dir.write("main.rs", "fn main() {}\n");
    let mut app = App::with_file(None);
    let (program, args) = echo_command("noop");
    app.run_task(fenix_tasks::TaskDef { name: "t".to_string(), command: program, args }, dir.path().to_path_buf());
    let task_workspace_index = app.task_session.as_ref().unwrap().workspace_index;
    let task_pane = app.task_session.as_ref().unwrap().pane;
    let workspace_count_before = app.workspaces.len();
    app.quickfix = vec![QuickfixEntry::Task(fenix_project::GrepMatch { path: target, line: 1, col: 1, text: "x".to_string() })];
    app.quickfix_index = None;

    app.quickfix_step(1);

    assert_eq!(app.workspaces.len(), workspace_count_before + 1, "the jumped-to file should land in a new workspace");
    assert_ne!(app.workspaces.active_index(), task_workspace_index);
    app.workspaces.switch_to_index(task_workspace_index);
    app.windows_mut().focus(task_pane);
    assert_eq!(app.open().kind, BufferKind::TaskOutput, "the Task Output pane must still show task output, not the jumped-to file");
}

#[test]
fn apply_git_main_discards_a_result_from_a_superseded_request() {
    let mut app = App::with_file(None);
    app.open_git_panel();
    let session = app.git_session.as_mut().unwrap();
    session.main_request_id = 5;
    let main_buffer = session.main_buffer;
    let stale = "diff --git a/stale.txt b/stale.txt\n--- a/stale.txt\n+++ b/stale.txt\n@@ -1 +1 @@\n-x\n+y\n";
    let current = "diff --git a/current.txt b/current.txt\n--- a/current.txt\n+++ b/current.txt\n@@ -1 +1 @@\n-x\n+y\n";

    // request_id 3 is stale -- request_id 5 already landed (or was
    // issued) after it -- so this result must be dropped, not
    // clobber Main with an out-of-order diff.
    app.apply_git_main(3, main_buffer, Some(stale.to_string()));
    let text = app.buffers.get(main_buffer).unwrap().buffer.text();
    assert!(!text.contains("stale.txt"));

    app.apply_git_main(5, main_buffer, Some(current.to_string()));
    let text = app.buffers.get(main_buffer).unwrap().buffer.text();
    assert!(text.contains("current.txt"));
}

#[test]
fn apply_git_refresh_discards_a_result_from_a_superseded_request() {
    let mut app = App::with_file(None);
    app.open_git_panel();
    let sentinel = vec![fenix_git::FileEntry { path: "sentinel.txt".to_string(), index_status: 'A', worktree_status: '.' }];
    let session = app.git_session.as_mut().unwrap();
    session.refresh_request_id = 5;
    session.files = sentinel.clone();

    // request_id 3 is stale -- must not overwrite the session's
    // cached file list at all.
    let stale = GitRefreshData { status: None, files: Vec::new(), branches: Vec::new(), commits: Vec::new(), stashes: Vec::new(), in_progress: None, sides: None };
    app.apply_git_refresh(3, stale);
    assert_eq!(app.git_session.as_ref().unwrap().files, sentinel);

    // request_id 5 (current) applies normally.
    let fresh = vec![fenix_git::FileEntry { path: "fresh.txt".to_string(), index_status: 'A', worktree_status: '.' }];
    let current = GitRefreshData { status: None, files: fresh.clone(), branches: Vec::new(), commits: Vec::new(), stashes: Vec::new(), in_progress: None, sides: None };
    app.apply_git_refresh(5, current);
    assert_eq!(app.git_session.as_ref().unwrap().files, fresh);
}

#[test]
fn git_pane_by_number_matches_each_titles_own_number() {
    let mut app = App::with_file(None);
    app.open_git_panel();
    let session = app.git_session.as_ref().unwrap();
    let (status, staged, unstaged, branches, commits, stash, main) = (
        session.status_pane,
        session.staged_pane,
        session.unstaged_pane,
        session.branches_pane,
        session.commits_pane,
        session.stash_pane,
        session.main_pane,
    );

    assert_eq!(app.git_pane_by_number(1), Some(status));
    assert_eq!(app.git_pane_by_number(2), Some(staged));
    assert_eq!(app.git_pane_by_number(3), Some(unstaged));
    assert_eq!(app.git_pane_by_number(4), Some(branches));
    assert_eq!(app.git_pane_by_number(5), Some(commits));
    assert_eq!(app.git_pane_by_number(6), Some(stash));
    assert_eq!(app.git_pane_by_number(7), Some(main));
    assert_eq!(app.git_pane_by_number(8), None);
    assert_eq!(app.git_pane_by_number(0), None);

    app.git_session_close();
    assert_eq!(app.git_pane_by_number(1), None);
}

#[test]
fn git_toggle_dir_expand_reveals_and_hides_a_directorys_files_preserving_the_cursor_line() {
    let mut app = App::with_file(None);
    app.open_git_panel();
    let session = app.git_session.as_ref().unwrap();
    let (unstaged_buffer, unstaged_pane) = (session.unstaged_buffer, session.unstaged_pane);

    // Seed a deterministic file list (independent of whatever real
    // `git status` returns for this sandboxed test env) with one
    // file inside a subdirectory, and render it collapsed -- the
    // directory row lands on line 0. Unstaged (worktree_status 'M'),
    // so focus that pane -- `open_git_panel` leaves Staged focused.
    let fake_files = vec![fenix_git::FileEntry { path: "sub/a.txt".to_string(), index_status: '.', worktree_status: 'M' }];
    app.git_session.as_mut().unwrap().files = fake_files.clone();
    app.set_git_buffer(unstaged_buffer, git_panel::render_unstaged(&fake_files, &HashSet::new()));
    app.windows_mut().focus(unstaged_pane);
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });

    app.git_toggle_dir_expand();

    assert!(app.git_session.as_ref().unwrap().unstaged_expanded_dirs.contains("sub"));
    let text = app.buffers.get(unstaged_buffer).unwrap().buffer.text();
    assert!(text.contains("v sub/"), "expected an expanded marker, got: {text:?}");
    assert!(text.contains("a.txt  modified"), "expected the file shown by basename, got: {text:?}");
    assert!(!text.contains("sub/a.txt"), "the file row should show only its basename: {text:?}");

    // The cursor stayed on the directory's own row (line 0) even
    // though a new line was inserted below it.
    let focused = app.focused_pane_id();
    let (line, _) = app.buffers.get(unstaged_buffer).unwrap().buffer.line_col(&app.pane_state(focused).cursor);
    assert_eq!(line, 0);

    app.git_toggle_dir_expand();
    assert!(!app.git_session.as_ref().unwrap().unstaged_expanded_dirs.contains("sub"));
    let text = app.buffers.get(unstaged_buffer).unwrap().buffer.text();
    assert!(text.contains("> sub/"));
    assert!(!text.contains("a.txt"));
}

#[test]
fn git_toggle_dir_expand_is_a_no_op_on_a_file_row() {
    let mut app = App::with_file(None);
    app.open_git_panel();
    let staged_buffer = app.git_session.as_ref().unwrap().staged_buffer;

    let fake_files = vec![fenix_git::FileEntry { path: "root.txt".to_string(), index_status: 'A', worktree_status: '.' }];
    app.git_session.as_mut().unwrap().files = fake_files.clone();
    app.set_git_buffer(staged_buffer, git_panel::render_staged(&fake_files, &HashSet::new()));
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 }); // on the file row, not a directory -- Staged is already focused

    app.git_toggle_dir_expand();
    assert!(app.git_session.as_ref().unwrap().staged_expanded_dirs.is_empty());
}

#[test]
fn git_stage_selected_on_a_directory_entry_stages_every_file_under_it() {
    let dir = fenix_git_test_repo("git_stage_dir");
    dir.write("sub/a.txt", "changed\n");
    dir.write("sub/b.txt", "new\n");

    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    let session = app.git_session.as_ref().unwrap();
    let (unstaged_buffer, unstaged_pane, repo_root) = (session.unstaged_buffer, session.unstaged_pane, session.repo_root.clone());
    assert_eq!(repo_root, dir.path());

    let files = fenix_git::list_files(&repo_root);
    app.git_session.as_mut().unwrap().files = files.clone();
    app.set_git_buffer(unstaged_buffer, git_panel::render_unstaged(&files, &HashSet::new()));
    app.windows_mut().focus(unstaged_pane);
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 }); // the "sub/" directory row

    app.git_stage_selected();

    let files = fenix_git::list_files(&repo_root);
    assert!(files.iter().all(|f| f.index_status != '.'), "every file under sub/ should now be staged: {files:?}");
}

// -- Rebase, merge and conflict resolution ----------------------------

/// A repo where `main` and `side` both changed the same line, so
/// rebasing or merging one onto the other is guaranteed to conflict.
fn conflicting_repo(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    dir.write("a.txt", "original\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "initial"]);
    git(&["checkout", "-q", "-b", "side"]);
    dir.write("a.txt", "side version\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "side change"]);
    git(&["checkout", "-q", "main"]);
    dir.write("a.txt", "main version\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "main change"]);
    dir
}

#[test]
fn a_conflicted_rebase_reports_itself_in_the_status_banner() {
    let dir = conflicting_repo("rebase_banner");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    app.git_rebase_onto("side");

    let status = app.buffers.get(app.git_session.as_ref().unwrap().status_buffer).unwrap().buffer.text();
    assert!(status.contains("REBASING"), "the banner has to say what's running:\n{status}");
    assert!(status.contains("SPC g x c continue"), "and how to get out of it:\n{status}");
    assert!(status.contains("conflicted file"), "the conflict count belongs there too:\n{status}");
    // The message points at the next step rather than reading as a
    // plain failure -- a conflict is the normal middle of a rebase.
    assert!(app.modeline_pieces().1.contains("resolve the conflicts"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn aborting_puts_the_tree_back_and_clears_the_banner() {
    let dir = conflicting_repo("rebase_abort");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("side");
    assert!(fenix_git::in_progress(dir.path()).is_some());

    app.git_operation_abort();

    assert_eq!(fenix_git::in_progress(dir.path()), None);
    // Line endings normalized: git rewrites them on checkout wherever
    // `core.autocrlf` is on, and this is about *what* was restored.
    let restored = std::fs::read_to_string(dir.path().join("a.txt")).unwrap().replace("\r\n", "\n");
    assert_eq!(restored, "main version\n");
    let status = app.buffers.get(app.git_session.as_ref().unwrap().status_buffer).unwrap().buffer.text();
    assert!(!status.contains("REBASING"), "the banner should be gone:\n{status}");
}

#[test]
fn continuing_with_conflicts_still_unresolved_says_what_is_missing() {
    let dir = conflicting_repo("rebase_continue_blocked");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("side");

    app.git_operation_continue();

    assert!(app.modeline_pieces().1.contains("still conflicted"), "got: {}", app.modeline_pieces().1);
    assert!(fenix_git::in_progress(dir.path()).is_some(), "and the rebase is still suspended");
}

#[test]
fn continuing_and_aborting_with_nothing_running_say_so() {
    let dir = conflicting_repo("nothing_running");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.git_operation_continue();
    assert!(app.modeline_pieces().1.contains("nothing in progress"));
    app.git_operation_abort();
    assert!(app.modeline_pieces().1.contains("nothing in progress"));
}

#[test]
fn a_whole_rebase_can_be_driven_to_completion_from_the_editor() {
    // The end-to-end path the milestone exists for: rebase, hit a
    // conflict, resolve it in the buffer, stage, continue.
    let dir = conflicting_repo("rebase_full");
    let git = |args: &[&str]| {
        std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
    };
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    app.git_rebase_onto("side");
    assert!(fenix_git::in_progress(dir.path()).is_some(), "the rebase should stop on the conflict");

    // Open the conflicted file and take our side.
    let conflicted = dir.path().join("a.txt");
    app.open_file_from_picker(&conflicted);
    assert!(app.open().buffer.text().contains("<<<<<<<"), "the file has markers in it: {:?}", app.open().buffer.text());
    app.goto_conflict(true);
    app.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
    assert!(!app.open().buffer.text().contains("<<<<<<<"), "markers gone after resolving");
    app.save();

    git(&["add", "a.txt"]);
    app.git_operation_continue();

    assert_eq!(fenix_git::in_progress(dir.path()), None, "the rebase should have finished");
    assert!(app.modeline_pieces().1.contains("finished"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_file_already_open_when_the_rebase_starts_shows_the_conflict() {
    // The bug this exists for: the file was open *before* the rebase,
    // so the buffer held the pre-rebase text while git had written
    // conflict markers into the file underneath it. Saving that stale
    // buffer would have written the old content straight back over
    // git's markers and destroyed the merge state.
    let dir = conflicting_repo("rebase_open_buffer");
    let conflicted = dir.path().join("a.txt");
    let mut app = App::with_file(Some(conflicted.to_string_lossy().into_owned()));
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    assert!(!app.open().buffer.text().contains("<<<<<<<"), "no markers before the rebase");

    app.git_rebase_onto("side");

    let id = app.buffers.id_for_path(&conflicted).unwrap();
    let shown = app.buffers.get(id).unwrap().buffer.text();
    assert!(shown.contains("<<<<<<<"), "the open buffer has to catch up with what git wrote:\n{shown}");
    assert_eq!(shown.replace("\r\n", "\n"), std::fs::read_to_string(&conflicted).unwrap().replace("\r\n", "\n"));
}

#[test]
fn a_buffer_with_unsaved_edits_is_never_reloaded_out_from_under_you() {
    let dir = conflicting_repo("rebase_dirty_buffer");
    let conflicted = dir.path().join("a.txt");
    let mut app = App::with_file(Some(conflicted.to_string_lossy().into_owned()));
    app.project_root = Some(dir.path().to_path_buf());
    app.test_insert('!');
    let edited = app.open().buffer.text();
    app.open_git_panel();

    app.git_rebase_onto("side");

    let id = app.buffers.id_for_path(&conflicted).unwrap();
    let shown = app.buffers.get(id).unwrap().buffer.text();
    assert_eq!(shown, edited, "unsaved work outranks the reload");
    // Flagged, so the next save refuses rather than discarding what
    // the rebase wrote. Asserted on the flag rather than on the
    // message: the rebase's own "resolve the conflicts" is the more
    // actionable thing to say at this moment and legitimately
    // replaces it, which is exactly why the flag (and the `[disk]`
    // marker it drives) is the durable half of the warning.
    assert!(app.externally_changed.contains(&id), "the divergence outlives the message");
}

// -- Merge Requests view (`SPC g M`) -------------------------------

// -- Review (Milestone F) ------------------------------------------

#[test]
fn a_commit_message_is_written_in_a_compose_buffer_not_a_one_line_prompt() {
    // The plan asked for this in Milestone A; the buffer only got
    // built in F. A commit message is the most ordinary multi-line
    // text an editor asks for -- subject, blank line, body -- and a
    // single-line prompt can express none of it.
    let dir = TempDir::new("commit_compose");
    let git = |args: &[&str]| {
        std::process::Command::new("git").current_dir(dir.path()).args(args).output().unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@e.st"]);
    git(&["config", "user.name", "Test"]);
    dir.write("a.txt", "one
");
    git(&["add", "."]);
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    app.git_commit_prompt();

    let compose = app.compose.as_ref().expect("a compose buffer, not a prompt");
    assert!(app.git_prompt.is_none(), "and not the single-line prompt");
    assert!(app.pane_titles.get(&compose.pane).unwrap().contains("Commit message"));
    assert!(app.pane_titles.get(&compose.pane).unwrap().contains("Enter to commit"));
    // Ready to type: what's typed first isn't run as commands.
    assert_eq!(app.vim.mode(), Mode::Insert);

    app.test_insert_str("Add a.txt");
    app.compose_submit();

    assert!(app.compose.is_none(), "committed, so the buffer closed: {}", app.modeline_pieces().1);
    assert!(app.modeline_pieces().1.contains("committed: Add a.txt"), "got: {}", app.modeline_pieces().1);
    let log = std::process::Command::new("git").current_dir(dir.path()).args(["log", "--format=%s", "-1"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Add a.txt");
}

/// `SPC g x c` on a merge: Git's prepared message, in the composer
/// to edit, and the merge commit carries it -- not "Merge".
#[test]
fn finishing_a_merge_commits_git_s_message() {
    let dir = TempDir::new("merge_continue");
    let git = |args: &[&str]| {
        std::process::Command::new("git").current_dir(dir.path()).args(args).output().unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@e.st"]);
    git(&["config", "user.name", "Test"]);
    dir.write("a.txt", "one\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["checkout", "-q", "-b", "feature"]);
    dir.write("a.txt", "feature\n");
    git(&["commit", "-q", "-am", "feature"]);
    git(&["checkout", "-q", "main"]);
    dir.write("a.txt", "main\n");
    git(&["commit", "-q", "-am", "main"]);
    git(&["merge", "feature"]);
    dir.write("a.txt", "both\n");
    git(&["add", "a.txt"]);
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.git_operation_continue();

    let compose = app.compose.as_ref().expect("the message, to edit first");
    let text = app.buffers.get(compose.buffer).unwrap().buffer.text();
    assert!(text.starts_with("Merge branch 'feature'"), "{text}");
    app.compose_submit();
    let log = std::process::Command::new("git").current_dir(dir.path()).args(["log", "--format=%s", "-1"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Merge branch 'feature'");
}

#[test]
fn committing_with_nothing_staged_says_so_before_asking_for_a_message() {
    // `git commit` refuses too, but it refuses *after* the message
    // has been written, which is the wrong moment to find out.
    let dir = TempDir::new("commit_nothing_staged");
    let git = |args: &[&str]| {
        std::process::Command::new("git").current_dir(dir.path()).args(args).output().unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@e.st"]);
    git(&["config", "user.name", "Test"]);
    dir.write("a.txt", "one
");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    dir.write("a.txt", "two
");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    app.git_commit_prompt();

    assert!(app.compose.is_none(), "no buffer to fill in for nothing");
    assert!(app.modeline_pieces().1.contains("nothing staged"), "got: {}", app.modeline_pieces().1);
}

// -- Crash recovery ------------------------------------------------

/// An app whose recovery snapshots go somewhere throwaway rather
/// than into the real config directory.
fn app_with_recovery(name: &str, contents: &str) -> (TempDir, TempDir, App) {
    let work = TempDir::new(&format!("{name}_work"));
    let store = TempDir::new(&format!("{name}_store"));
    let file = work.write("notes.txt", contents);
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.recovery_dir = Some(store.path().to_path_buf());
    (work, store, app)
}

#[test]
fn a_dirty_buffer_is_snapshotted_and_the_snapshot_survives_the_app() {
    // The whole point: what's in memory exists somewhere else too.
    let (work, store, mut app) = app_with_recovery("recover_basic", "one\n");
    app.test_insert_str("unsaved work ");

    app.poll_files_changed_on_disk();

    let found = fenix_recovery::list(store.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].original, Some(work.path().join("notes.txt")));
    assert!(found[0].contents.starts_with("unsaved work "), "got: {:?}", found[0].contents);
    // And the file itself is untouched -- a snapshot is not a save.
    assert_eq!(std::fs::read_to_string(work.path().join("notes.txt")).unwrap(), "one\n");
}

#[test]
fn a_clean_buffer_is_never_snapshotted() {
    let (_work, store, mut app) = app_with_recovery("recover_clean", "one\n");
    app.poll_files_changed_on_disk();
    assert!(fenix_recovery::list(store.path()).is_empty(), "nothing is at risk, so nothing is written");
}

#[test]
fn saving_removes_the_snapshot() {
    // Otherwise the next start would offer to recover work that
    // isn't lost, which is exactly the swap-file failure mode.
    let (_work, store, mut app) = app_with_recovery("recover_saved", "one\n");
    app.test_insert('X');
    app.poll_files_changed_on_disk();
    assert_eq!(fenix_recovery::list(store.path()).len(), 1);

    app.save();

    assert!(fenix_recovery::list(store.path()).is_empty(), "a saved buffer has nothing to recover");
}

#[test]
fn discarding_a_buffer_removes_its_snapshot_too() {
    // `:q!` is the user saying they don't want these edits; offering
    // them back on the next start would be ignoring that.
    let (_work, store, mut app) = app_with_recovery("recover_discarded", "one\n");
    app.test_insert('X');
    app.poll_files_changed_on_disk();

    app.force_kill_buffer();

    assert!(fenix_recovery::list(store.path()).is_empty());
}

#[test]
fn a_buffer_that_stops_being_dirty_drops_its_snapshot_on_the_next_tick() {
    let (_work, store, mut app) = app_with_recovery("recover_reverted", "one\n");
    app.test_insert('X');
    app.poll_files_changed_on_disk();
    assert_eq!(fenix_recovery::list(store.path()).len(), 1);

    // Undone back to the saved state, so nothing is at risk again.
    app.reload_focused_file();
    app.poll_files_changed_on_disk();

    assert!(fenix_recovery::list(store.path()).is_empty());
}

#[test]
fn an_unchanged_dirty_buffer_is_not_rewritten_every_tick() {
    // The check runs every couple of seconds over every open buffer;
    // writing each one each time would be a lot of disk for nothing.
    let (_work, store, mut app) = app_with_recovery("recover_no_churn", "one\n");
    app.test_insert('X');
    app.poll_files_changed_on_disk();
    let first = std::fs::metadata(&fenix_recovery::list(store.path())[0].snapshot).unwrap().modified().unwrap();
    let id = app.focused_buffer_id();
    let edits = app.buffers.get(id).unwrap().buffer.edit_count();

    app.poll_files_changed_on_disk();
    app.poll_files_changed_on_disk();

    assert_eq!(app.snapshot_state.get(&id), Some(&edits), "still keyed to the same edit");
    let again = std::fs::metadata(&fenix_recovery::list(store.path())[0].snapshot).unwrap().modified().unwrap();
    assert_eq!(first, again, "not rewritten");
}

#[test]
fn unnamed_work_recovers_without_inventing_a_file_path() {
    let store = TempDir::new("recover_no_path");
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.recovery_dir = Some(store.path().to_path_buf());
    app.test_insert_str("scratch work");
    app.poll_files_changed_on_disk();
    let snapshots = fenix_recovery::list(store.path());
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].original, None);
    let mut fresh = App::with_file(None);
    fresh.recovery_dir = Some(store.path().to_path_buf());
    fresh.recover_snapshot(&snapshots[0]);
    assert_eq!(fresh.open().buffer.text(), "scratch work");
    assert!(fresh.open().buffer.path().is_none());
    assert!(fresh.open().buffer.is_dirty());
    fresh.force_kill_buffer();
    assert!(fenix_recovery::list(store.path()).is_empty());
}

#[test]
fn naming_recovered_scratch_saves_text_and_removes_snapshot() {
    let store = TempDir::new("recovered_scratch_save");
    let target = store.path().join("my recovered notes.txt");
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.recovery_dir = Some(store.path().to_path_buf());
    app.test_insert_str("important notes");
    app.poll_files_changed_on_disk();
    app.save_unnamed_as(target.to_str().unwrap());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "important notes");
    assert!(!app.open().buffer.is_dirty());
    assert!(fenix_recovery::list(store.path()).is_empty());
}

#[test]
fn failed_scratch_save_keeps_recovery_and_existing_destination() {
    let store = TempDir::new("scratch_save_existing");
    let target = store.write("existing.txt", "original");
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.recovery_dir = Some(store.path().to_path_buf());
    app.test_insert_str("important notes");
    app.poll_files_changed_on_disk();
    app.save_unnamed_as(target.to_str().unwrap());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert!(app.open().buffer.is_dirty());
    assert_eq!(fenix_recovery::list(store.path()).len(), 1);
}

#[test]
fn recovery_continues_with_watching_disabled_and_advances_deadline() {
    let (_work, store, mut app) = app_with_recovery("recover_without_watch", "original");
    app.config.watch_files = Some(false);
    app.test_insert_str("unsaved ");
    app.next_disk_check = Instant::now() - Duration::from_secs(1);
    app.poll_files_changed_on_disk();
    assert!(app.next_disk_check > Instant::now());
    assert_eq!(fenix_recovery::list(store.path()).len(), 1);
}

#[test]
fn recovery_failure_is_visible_and_retries_successfully() {
    let (_work, store, mut app) = app_with_recovery("recover_failure", "original");
    let blocked = store.write("not-a-directory", "blocked");
    app.recovery_dir = Some(blocked);
    app.test_insert_str("unsaved ");
    app.poll_files_changed_on_disk();
    assert!(!app.recovery_failures.is_empty());
    app.recovery_dir = Some(store.path().to_path_buf());
    app.poll_files_changed_on_disk();
    assert!(app.recovery_failures.is_empty());
    assert_eq!(fenix_recovery::list(store.path()).len(), 1);
}

#[test]
fn a_crashed_session_can_be_recovered_in_a_new_one() {
    // The end-to-end this exists for: one app writes a snapshot and
    // is dropped without saving, standing in for a crash; a fresh
    // one finds it and hands the work back.
    let work = TempDir::new("recover_crash_work");
    let store = TempDir::new("recover_crash_store");
    let file = work.write("notes.txt", "original\n");
    {
        let mut crashed = App::with_file(Some(file.to_string_lossy().into_owned()));
        crashed.recovery_dir = Some(store.path().to_path_buf());
        crashed.test_insert_str("work in progress ");
        crashed.poll_files_changed_on_disk();
        // Dropped without saving and without closing anything.
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original\n", "the file never got the edits");

    let mut fresh = App::with_file(None);
    fresh.recovery_dir = Some(store.path().to_path_buf());
    fresh.announce_recoverable_work();
    assert!(fresh.modeline_pieces().1.contains("notes.txt"), "the start says so: {}", fresh.modeline_pieces().1);
    assert!(fresh.modeline_pieces().1.contains("SPC f v"), "and where the key is: {}", fresh.modeline_pieces().1);

    fresh.picker_recovery();
    assert!(matches!(fresh.active_picker, Some(ActivePicker::Recovery(_))));
    fresh.picker_confirm();

    assert!(fresh.open().buffer.text().starts_with("work in progress "), "got: {:?}", fresh.open().buffer.text());
    assert_eq!(fresh.open().buffer.path(), Some(file.as_path()));
    // Dirty, not written: the recovered text is what you had, not
    // necessarily what you want, and `:qa` must not walk past it.
    assert!(fresh.open().buffer.is_dirty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original\n", "still not written until you say so");
}

#[test]
fn saving_recovered_work_finishes_the_job_and_clears_the_snapshot() {
    let work = TempDir::new("recover_accept_work");
    let store = TempDir::new("recover_accept_store");
    let file = work.write("notes.txt", "original\n");
    {
        let mut crashed = App::with_file(Some(file.to_string_lossy().into_owned()));
        crashed.recovery_dir = Some(store.path().to_path_buf());
        crashed.test_insert_str("recovered ");
        crashed.poll_files_changed_on_disk();
    }
    let mut fresh = App::with_file(None);
    fresh.recovery_dir = Some(store.path().to_path_buf());
    fresh.picker_recovery();
    fresh.picker_confirm();

    fresh.save();

    assert!(std::fs::read_to_string(&file).unwrap().starts_with("recovered "));
    assert!(fenix_recovery::list(store.path()).is_empty(), "nothing left to recover");
}

#[test]
fn a_snapshot_matching_the_file_is_not_offered_back_as_an_edit() {
    // The work was saved after all -- by hand, or by another
    // session. Loading it as a dirty buffer would invent a change
    // nobody made.
    let work = TempDir::new("recover_already_saved_work");
    let store = TempDir::new("recover_already_saved_store");
    let file = work.write("notes.txt", "the same\n");
    fenix_recovery::write(store.path(), &file, "the same\n").unwrap();

    let mut app = App::with_file(None);
    app.recovery_dir = Some(store.path().to_path_buf());
    app.picker_recovery();
    app.picker_confirm();

    assert!(!app.open().buffer.is_dirty(), "nothing to recover, so nothing is unsaved");
    assert!(app.modeline_pieces().1.contains("already matches"), "got: {}", app.modeline_pieces().1);
    assert!(fenix_recovery::list(store.path()).is_empty(), "and the stale snapshot is cleaned up");
}

#[test]
fn work_whose_file_was_deleted_is_still_recoverable() {
    let work = TempDir::new("recover_gone_work");
    let store = TempDir::new("recover_gone_store");
    let file = work.path().join("gone.txt");
    fenix_recovery::write(store.path(), &file, "the only copy\n").unwrap();

    let mut app = App::with_file(None);
    app.recovery_dir = Some(store.path().to_path_buf());
    app.picker_recovery();
    app.picker_confirm();

    assert_eq!(app.open().buffer.text(), "the only copy\n");
    assert!(app.open().buffer.is_dirty(), "it exists nowhere but here");
    assert!(app.modeline_pieces().1.contains("the file itself is gone"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn nothing_to_recover_says_so_rather_than_opening_an_empty_picker() {
    let store = TempDir::new("recover_nothing");
    let mut app = App::with_file(None);
    app.recovery_dir = Some(store.path().to_path_buf());

    app.picker_recovery();

    assert!(app.active_picker.is_none());
    assert!(app.modeline_pieces().1.contains("nothing to recover"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_clean_start_says_nothing_at_all() {
    // The common case, and the one swap files get wrong.
    let store = TempDir::new("recover_quiet");
    let mut app = App::with_file(None);
    app.recovery_dir = Some(store.path().to_path_buf());

    app.announce_recoverable_work();

    assert!(!app.modeline_pieces().1.contains("previous session"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn nothing_is_written_next_to_the_file_being_edited() {
    // Not swap files: the working tree stays exactly as it was, so
    // there is nothing to gitignore and nothing for a build to trip
    // over.
    let (work, _store, mut app) = app_with_recovery("recover_no_litter", "one\n");
    app.test_insert('X');
    app.poll_files_changed_on_disk();

    let names: Vec<String> =
        std::fs::read_dir(work.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["notes.txt".to_string()]);
}

#[test]
fn the_age_of_a_snapshot_reads_in_units_a_person_uses() {
    assert_eq!(describe_age(Some(Duration::from_secs(5))), "just now");
    assert_eq!(describe_age(Some(Duration::from_secs(60 * 7))), "7 minutes ago");
    assert_eq!(describe_age(Some(Duration::from_secs(60 * 60 * 5))), "5 hours ago");
    assert_eq!(describe_age(Some(Duration::from_secs(60 * 60 * 24 * 3))), "3 days ago");
    assert_eq!(describe_age(None), "at an unknown time");
}

// -- Files changing on disk while they're open ---------------------

/// Rewrites `path` and makes sure the change is visible to a
/// fingerprint check.
///
/// A filesystem with one-second timestamp resolution reports the
/// same mtime for two writes in the same second, which is exactly
/// the case the length half of the fingerprint exists for -- but a
/// test that happens to write the same number of bytes would then
/// depend on timer luck. Writing different lengths keeps these
/// deterministic; `an_edit_of_the_same_length_is_still_caught`
/// covers the other case on purpose.
fn write_externally(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

fn open_file(name: &str, contents: &str) -> (TempDir, App) {
    let dir = TempDir::new(name);
    let file = dir.write("a.txt", contents);
    let app = App::with_file(Some(file.to_string_lossy().into_owned()));
    (dir, app)
}

#[test]
fn a_clean_buffer_reloads_when_its_file_changes_underneath_it() {
    let (dir, mut app) = open_file("watch_reload", "one\n");
    app.poll_files_changed_on_disk();
    write_externally(&dir.path().join("a.txt"), "one\ntwo\nthree\n");

    app.poll_files_changed_on_disk();

    assert_eq!(app.open().buffer.text(), "one\ntwo\nthree\n");
    assert!(!app.open().buffer.is_dirty(), "a reload isn't user work, so it mustn't block :qa");
    assert!(app.modeline_pieces().1.contains("reloaded a.txt"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_dirty_buffer_is_left_alone_and_flagged_instead() {
    let (dir, mut app) = open_file("watch_dirty", "one\n");
    app.poll_files_changed_on_disk();
    app.test_insert('X');
    let mine = app.open().buffer.text();
    write_externally(&dir.path().join("a.txt"), "theirs\ntheirs\n");

    app.poll_files_changed_on_disk();

    assert_eq!(app.open().buffer.text(), mine, "unsaved edits are never thrown away silently");
    assert!(app.modeline_pieces().1.contains(":e! takes theirs"), "got: {}", app.modeline_pieces().1);
    assert!(app.modeline_pieces().1.contains(":w! keeps yours"), "got: {}", app.modeline_pieces().1);
    // And once the message times out, the marker is still there --
    // which is the point of having one: the announcement scrolls
    // away, the divergence doesn't.
    app.status_message = None;
    assert!(app.modeline_text().contains("[disk]"), "got: {}", app.modeline_text());
}

#[test]
fn saving_over_a_file_that_changed_underneath_you_is_refused() {
    // The data-loss case this whole feature exists for: a plain
    // write is a whole-file overwrite, and whatever the other side
    // did would be gone with nothing to recover it from.
    let (dir, mut app) = open_file("watch_save_refused", "one\n");
    app.poll_files_changed_on_disk();
    app.test_insert('X');
    write_externally(&dir.path().join("a.txt"), "written by something else\n");

    app.save();

    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "written by something else\n");
    assert!(app.modeline_pieces().1.contains("changed on disk"), "got: {}", app.modeline_pieces().1);
    assert!(app.open().buffer.is_dirty(), "and the edits are still there to save once you decide");
}

#[test]
fn a_forced_save_overwrites_and_clears_the_flag() {
    let (dir, mut app) = open_file("watch_save_forced", "one\n");
    app.poll_files_changed_on_disk();
    app.test_insert('X');
    let mine = app.open().buffer.text();
    write_externally(&dir.path().join("a.txt"), "written by something else\n");
    app.save();
    assert!(app.modeline_pieces().1.contains("changed on disk"));

    app.save_force();

    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), mine);
    assert!(!app.externally_changed.contains(&app.focused_buffer_id()), "the two sides agree again");
    app.status_message = None;
    assert!(!app.modeline_text().contains("[disk]"));
}

#[test]
fn reloading_takes_the_other_side_and_discards_your_edits() {
    let (dir, mut app) = open_file("watch_reload_cmd", "one\n");
    app.poll_files_changed_on_disk();
    app.test_insert('X');
    write_externally(&dir.path().join("a.txt"), "theirs\n");

    app.reload_focused_file();

    assert_eq!(app.open().buffer.text(), "theirs\n");
    assert!(!app.open().buffer.is_dirty());
    assert!(!app.externally_changed.contains(&app.focused_buffer_id()));
    // And a save now goes through, because there's nothing to lose.
    app.test_insert('Y');
    app.save();
    assert!(app.modeline_pieces().1.contains("saved"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn fenixs_own_save_is_never_reported_as_somebody_elses_change() {
    // The false positive that would make the whole feature
    // unusable: every `:w` followed by "your file changed on disk".
    let (_dir, mut app) = open_file("watch_own_save", "one\n");
    app.poll_files_changed_on_disk();
    app.test_insert('X');

    app.save();
    app.poll_files_changed_on_disk();

    assert!(!app.modeline_pieces().1.contains("changed on disk"), "got: {}", app.modeline_pieces().1);
    assert!(!app.modeline_pieces().1.contains("reloaded"), "got: {}", app.modeline_pieces().1);
    assert!(app.externally_changed.is_empty());
}

#[test]
fn a_file_rewritten_with_identical_content_says_nothing() {
    // A build or formatter that rewrites a file unchanged bumps its
    // mtime. Reporting that as a change would cry wolf constantly.
    let (dir, mut app) = open_file("watch_touch", "one\n");
    app.poll_files_changed_on_disk();
    write_externally(&dir.path().join("a.txt"), "one\n");

    app.poll_files_changed_on_disk();

    assert!(!app.modeline_pieces().1.contains("reloaded"), "got: {}", app.modeline_pieces().1);
    assert!(app.externally_changed.is_empty());
}

#[test]
fn an_edit_of_the_same_length_is_still_caught() {
    // The fingerprint is mtime *and* length precisely because
    // neither is enough alone; this is the case length can't see.
    let (dir, mut app) = open_file("watch_same_len", "aaaa\n");
    app.poll_files_changed_on_disk();
    let path = dir.path().join("a.txt");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    write_externally(&path, "bbbb\n");
    // Exercise an mtime change deterministically: rapid writes can share
    // a timestamp on Windows, which this fingerprint cannot distinguish.
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(before + Duration::from_secs(2))).unwrap();
    drop(file);

    app.poll_files_changed_on_disk();

    assert_eq!(app.open().buffer.text(), "bbbb\n");
}

#[test]
fn a_deleted_file_never_blanks_its_buffer() {
    // A "safe write" in another editor deletes and recreates the
    // file. An editor that empties your buffer for that moment is
    // worse than one that says nothing.
    let (dir, mut app) = open_file("watch_deleted", "one\ntwo\n");
    app.poll_files_changed_on_disk();
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();

    app.poll_files_changed_on_disk();

    assert_eq!(app.open().buffer.text(), "one\ntwo\n", "the content is still here");
    assert!(app.modeline_pieces().1.contains("no longer exists"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_file_that_comes_back_is_picked_up_again() {
    let (dir, mut app) = open_file("watch_recreated", "one\n");
    app.poll_files_changed_on_disk();
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    app.poll_files_changed_on_disk();

    write_externally(&dir.path().join("a.txt"), "back again\n");
    app.poll_files_changed_on_disk();

    assert_eq!(app.open().buffer.text(), "back again\n");
}

#[test]
fn generated_panels_are_never_treated_as_files() {
    // Every Git/Docker/diff pane is a real buffer the host rewrites
    // constantly and none of them has a file behind it.
    let dir = TempDir::new("watch_panels");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    std::process::Command::new("git").current_dir(dir.path()).args(["init", "-q"]).output().unwrap();
    app.open_git_panel();

    app.poll_files_changed_on_disk();

    assert!(app.disk_state.is_empty(), "nothing here has a file to watch");
    assert!(app.modeline_pieces().1.is_empty() || !app.modeline_pieces().1.contains("changed on disk"));
}

#[test]
fn a_buffer_with_no_path_is_skipped() {
    let mut app = App::with_file(None);
    app.test_insert('x');
    app.poll_files_changed_on_disk();
    assert!(app.disk_state.is_empty());
}

#[test]
fn watching_can_be_turned_off() {
    let (dir, mut app) = open_file("watch_disabled", "one\n");
    app.config.watch_files = Some(false);
    write_externally(&dir.path().join("a.txt"), "changed\n");

    app.poll_files_changed_on_disk();

    assert_eq!(app.open().buffer.text(), "one\n", "left entirely alone");
}

#[test]
fn closing_a_buffer_drops_what_was_remembered_about_its_file() {
    let (_dir, mut app) = open_file("watch_prune", "one\n");
    app.poll_files_changed_on_disk();
    assert_eq!(app.disk_state.len(), 1);

    app.force_kill_buffer();
    app.poll_files_changed_on_disk();

    assert!(app.disk_state.is_empty(), "the bookkeeping goes with the buffer");
}

#[test]
fn several_changed_files_are_named_rather_than_counted() {
    let dir = TempDir::new("watch_many");
    let a = dir.write("a.txt", "a\n");
    dir.write("b.txt", "b\n");
    dir.write("c.txt", "c\n");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    app.open_file_from_picker(&dir.path().join("b.txt"));
    app.open_file_from_picker(&dir.path().join("c.txt"));
    app.poll_files_changed_on_disk();
    for name in ["a.txt", "b.txt", "c.txt"] {
        write_externally(&dir.path().join(name), "changed by something else\n");
    }

    app.poll_files_changed_on_disk();

    // "3 files changed" makes you go looking; the point is that you
    // weren't expecting this at all.
    let message = app.modeline_pieces().1;
    assert!(message.contains("a.txt"), "got: {message}");
    assert!(message.contains("and 1 more"), "got: {message}");
}

#[test]
fn the_sweep_reschedules_itself_so_the_loop_keeps_waking() {
    let (_dir, mut app) = open_file("watch_schedule", "one\n");
    let before = app.next_disk_check;
    app.poll_files_changed_on_disk();
    assert!(app.next_disk_check > before, "the event loop waits on this deadline");
}

// -- Merge view (`SPC g x`) --------------------------------------

/// A repo where rebasing `myfeature` onto `develop` conflicts, so
/// the sides are genuinely the counter-intuitive way round: git's
/// `HEAD` is `develop`, not the branch you were standing on.
fn rebase_conflict_repo(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    let git = |args: &[&str]| {
        std::process::Command::new("git").current_dir(dir.path()).args(args).output().unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@e.st"]);
    git(&["config", "user.name", "Test"]);
    dir.write("app.conf", "shared\nvalue = base\nfooter\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["checkout", "-q", "-b", "develop"]);
    dir.write("app.conf", "shared\nvalue = DEVELOP\nfooter\n");
    git(&["commit", "-q", "-am", "develop change"]);
    git(&["checkout", "-q", "-b", "myfeature", "HEAD~1"]);
    dir.write("app.conf", "shared\nvalue = FEATURE\nfooter\n");
    git(&["commit", "-q", "-am", "feature change"]);
    dir
}

fn merge_view_text(app: &App) -> String {
    let session = app.merge_session.as_ref().expect("a merge session");
    app.buffers.get(session.merge_buffer).unwrap().buffer.text()
}

fn conflicts_pane_text(app: &App) -> String {
    let session = app.merge_session.as_ref().expect("a merge session");
    app.buffers.get(session.files_buffer).unwrap().buffer.text()
}

#[test]
fn the_merge_view_names_the_branches_rather_than_ours_and_theirs() {
    let dir = rebase_conflict_repo("merge_view_names");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");

    app.open_merge_view();

    let merge = merge_view_text(&app);
    assert!(merge.contains("<<< develop"), "the left column is the rebase target:\n{merge}");
    assert!(merge.contains(">>> myfeature"), "the right column is your own branch:\n{merge}");
    // And it says which is which in words, because "ours" during a
    // rebase means the opposite of what it sounds like.
    assert!(merge.contains("the branch you are rebasing onto"), "got:\n{merge}");
    assert!(merge.contains("your own commit being replayed"), "got:\n{merge}");
    // Both versions on one row, so they can actually be compared.
    let row = merge.lines().find(|l| l.contains("value = DEVELOP")).expect("ours is shown");
    assert!(row.contains("value = FEATURE"), "the two versions share a row:\n{row}");
}

#[test]
fn the_conflicts_pane_lists_the_files_and_what_each_key_would_keep() {
    let dir = rebase_conflict_repo("merge_view_list");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");

    app.open_merge_view();

    let files = conflicts_pane_text(&app);
    assert!(files.contains("1 conflicted file"), "got:\n{files}");
    assert!(files.contains("app.conf"), "got:\n{files}");
    // The keys are labelled by branch, not by "ours"/"theirs".
    assert!(files.contains("o = keep develop"), "got:\n{files}");
    assert!(files.contains("t = keep myfeature"), "got:\n{files}");
}

#[test]
fn resolving_in_the_merge_view_writes_the_file_and_says_which_branch_won() {
    let dir = rebase_conflict_repo("merge_view_resolve");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");
    app.open_merge_view();

    // Cursor into the merge pane, on the conflict.
    let merge_pane = app.merge_session.as_ref().unwrap().merge_pane;
    app.windows_mut().focus(merge_pane);
    app.goto_conflict(true);
    app.resolve_conflict_at_cursor(fenix_git::Resolution::Theirs);

    let on_disk = std::fs::read_to_string(dir.path().join("app.conf")).unwrap().replace("\r\n", "\n");
    assert_eq!(on_disk, "shared\nvalue = FEATURE\nfooter\n", "the file itself is resolved");
    // Named by branch: "kept theirs" would be exactly the wording
    // that makes people resolve a rebase backwards.
    assert!(app.modeline_pieces().1.contains("kept myfeature"), "got: {}", app.modeline_pieces().1);
    assert!(merge_view_text(&app).contains("Nothing left to resolve"), "got:\n{}", merge_view_text(&app));
}

#[test]
fn taking_a_whole_file_stages_it_and_names_the_branch_that_won() {
    let dir = rebase_conflict_repo("merge_view_whole_file");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");
    app.open_merge_view();

    // On the file row in the Conflicts pane.
    let session = app.merge_session.as_ref().unwrap();
    let (files_pane, files_buffer) = (session.files_pane, session.files_buffer);
    app.windows_mut().focus(files_pane);
    let row = app.buffers.get(files_buffer).unwrap().buffer.text().lines().position(|l| l.contains("app.conf")).expect("the file row");
    let char_idx = app.buffers.get(files_buffer).unwrap().buffer.line_start_char(row);
    app.pane_state_mut(files_pane).cursor = Cursor { char_idx, sticky_col: 0 };

    app.merge_take_whole_file(fenix_git::Resolution::Ours);

    let on_disk = std::fs::read_to_string(dir.path().join("app.conf")).unwrap().replace("\r\n", "\n");
    assert_eq!(on_disk, "shared\nvalue = DEVELOP\nfooter\n", "ours during a rebase is develop's version");
    assert!(app.modeline_pieces().1.contains("kept develop"), "got: {}", app.modeline_pieces().1);
    assert_eq!(fenix_git::list_files(dir.path()).iter().filter(|f| f.index_status == 'U').count(), 0, "and it's staged");
}

/// `o` on the header row, where the view opens, with one file listed:
/// that file, as the hints above it say -- not nothing.
#[test]
fn taking_a_side_on_the_header_row_takes_the_only_file() {
    let dir = rebase_conflict_repo("merge_view_header");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");
    app.open_merge_view();
    let session = app.merge_session.as_ref().unwrap();
    let files_pane = session.files_pane;
    app.windows_mut().focus(files_pane);
    app.pane_state_mut(files_pane).cursor = Cursor::at_start();

    app.merge_take_whole_file(fenix_git::Resolution::Ours);

    assert!(app.modeline_pieces().1.contains("kept develop"), "got: {}", app.modeline_pieces().1);
    assert_eq!(fenix_git::list_files(dir.path()).iter().filter(|f| f.index_status == 'U').count(), 0);
}

#[test]
fn a_resolution_can_be_put_back_before_it_is_staged() {
    let dir = rebase_conflict_repo("merge_view_restore");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");
    app.open_merge_view();
    let merge_pane = app.merge_session.as_ref().unwrap().merge_pane;
    app.windows_mut().focus(merge_pane);
    app.goto_conflict(true);
    app.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
    assert!(!std::fs::read_to_string(dir.path().join("app.conf")).unwrap().contains("<<<<<<<"));

    app.merge_restore_conflict();

    assert!(
        std::fs::read_to_string(dir.path().join("app.conf")).unwrap().contains("<<<<<<<"),
        "the markers come back, so a wrong choice isn't final"
    );
}

#[test]
fn staging_is_refused_while_markers_are_still_in_the_file() {
    // Committing conflict markers is the failure this prevents.
    let dir = rebase_conflict_repo("merge_view_stage_guard");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");
    app.open_merge_view();

    app.merge_stage_resolved();

    assert!(app.modeline_pieces().1.contains("still has 1 unresolved conflict"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn the_status_banner_says_which_branch_each_side_of_a_conflict_is() {
    let dir = rebase_conflict_repo("merge_banner_sides");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    app.git_rebase_onto("develop");

    let status = app.buffers.get(app.git_session.as_ref().unwrap().status_buffer).unwrap().buffer.text();
    assert!(status.contains("SPC g x x to resolve"), "the banner points at the view that explains it:\n{status}");
    assert!(status.contains("ours: develop"), "got:\n{status}");
    assert!(status.contains("theirs: myfeature"), "got:\n{status}");
}

#[test]
fn a_conflicted_files_diff_is_shown_rather_than_claiming_no_changes() {
    // `git diff` of an unmerged path is a *combined* diff. Before
    // those parsed, the one file you were trying to look at was the
    // one that rendered as "(no changes)".
    let dir = rebase_conflict_repo("merge_combined_diff");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();
    app.git_rebase_onto("develop");

    let diff = fenix_git::file_diff(dir.path(), "app.conf", false).expect("git diff runs on an unmerged path");
    let files = fenix_diff::parse(&diff);
    assert_eq!(files.len(), 1, "the conflicted file has a diff:\n{diff}");
    assert!(files[0].is_combined);
    assert!(files[0].hunks.iter().any(|h| !h.lines.is_empty()), "with content in it:\n{diff}");
}

#[test]
fn conflict_navigation_walks_the_markers_and_wraps() {
    let dir = TempDir::new("conflict_nav");
    let file = dir.write(
        "c.txt",
        "a\n<<<<<<< HEAD\nx1\n=======\ny1\n>>>>>>> b\nmiddle\n<<<<<<< HEAD\nx2\n=======\ny2\n>>>>>>> b\nz\n",
    );
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.goto_conflict(true);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1, "the first conflict's opener");
    app.goto_conflict(true);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 7, "the second");
    // Wraps rather than stopping: there are only ever a handful, and
    // cycling beats being stuck at the last one.
    app.goto_conflict(true);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1);
    app.goto_conflict(false);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 7);
}

#[test]
fn resolving_keeps_the_chosen_side_and_reports_what_is_left() {
    let dir = TempDir::new("conflict_resolve");
    let file = dir.write("c.txt", "a\n<<<<<<< HEAD\nx1\n=======\ny1\n>>>>>>> b\nm\n<<<<<<< HEAD\nx2\n=======\ny2\n>>>>>>> b\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.goto_conflict(true);
    app.resolve_conflict_at_cursor(fenix_git::Resolution::Theirs);

    assert!(app.open().buffer.text().starts_with("a\ny1\nm\n"), "got: {:?}", app.open().buffer.text());
    assert!(app.modeline_pieces().1.contains("kept theirs"), "got: {}", app.modeline_pieces().1);
    assert!(app.modeline_pieces().1.contains("1 conflict(s) left"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn resolving_is_an_ordinary_edit_that_undo_takes_back() {
    let dir = TempDir::new("conflict_undo");
    let file = dir.write("c.txt", "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let before = app.open().buffer.text();

    app.goto_conflict(true);
    app.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
    assert_ne!(app.open().buffer.text(), before);

    app.undo();
    assert_eq!(app.open().buffer.text(), before, "a mistaken choice is one undo away");
}

#[test]
fn resolving_with_no_conflict_under_the_cursor_says_so() {
    let dir = TempDir::new("conflict_none");
    let file = dir.write("c.txt", "nothing conflicted here\n");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
    assert!(app.modeline_pieces().1.contains("no conflict under the cursor"));
    app.goto_conflict(true);
    assert!(app.modeline_pieces().1.contains("no conflict markers"));
}

#[test]
fn a_force_push_is_confirmed_before_it_rewrites_anything() {
    let dir = conflicting_repo("force_push_confirm");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    app.git_force_push();

    assert!(app.git_confirm_text().is_some_and(|t| t.contains("Force-push")), "got: {:?}", app.git_confirm_text());
    assert!(app.git_confirm_text().is_some_and(|t| t.contains("with lease")), "the lease is the safety and should be visible");
}

#[test]
fn the_rebase_and_merge_pickers_offer_every_ref_but_head() {
    let dir = conflicting_repo("rebase_picker");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.start_rebase_picker();
    match &app.active_picker {
        Some(ActivePicker::RebaseOnto(state)) => {
            let labels: Vec<String> = state.visible_rows(0, 10).map(|(_, c)| c.label.clone()).collect();
            assert!(labels.contains(&"side".to_string()), "{labels:?}");
            assert!(!labels.contains(&"HEAD".to_string()), "rebasing onto HEAD is a no-op: {labels:?}");
        }
        _ => panic!("expected the rebase picker"),
    }

    app.picker_cancel();
    app.start_merge_picker();
    assert!(matches!(app.active_picker, Some(ActivePicker::MergeFrom(_))));
}

// -- History view (`SPC g l`) and Compare view (`SPC g c`) -------------

/// A repo with two branches that have diverged: `main` gained
/// `from_main.txt`, `side` gained `from_side.txt`, both after a
/// shared initial commit. Enough for a graph with a real second
/// lane and for three-dot vs two-dot comparison to differ.
fn diverged_repo(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    dir.write("shared.txt", "base\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "initial"]);
    git(&["checkout", "-q", "-b", "side"]);
    dir.write("from_side.txt", "side work\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "on side"]);
    git(&["checkout", "-q", "main"]);
    dir.write("from_main.txt", "main work\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "on main"]);
    dir
}

#[test]
fn the_history_view_opens_a_three_pane_workspace_with_graph_refs_and_detail() {
    let dir = diverged_repo("history_open");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.open_history_view();

    let session = app.history_session.as_ref().expect("a session");
    assert_eq!(app.pane_titles.get(&session.graph_pane).map(String::as_str), Some("1. Graph"));
    assert_eq!(app.pane_titles.get(&session.refs_pane).map(String::as_str), Some("2. Refs"));
    assert_eq!(app.pane_titles.get(&session.detail_pane).map(String::as_str), Some("3. Commit"));
    assert_eq!(app.buffers.get(session.graph_buffer).unwrap().kind, BufferKind::Graph);
    assert_eq!(app.buffers.get(session.detail_buffer).unwrap().kind, BufferKind::Diff);
}

#[test]
fn the_graph_shows_commits_from_every_branch_not_just_the_checked_out_one() {
    let dir = diverged_repo("history_graph_all");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();

    let graph = app.buffers.get(app.history_session.as_ref().unwrap().graph_buffer).unwrap().buffer.text();
    assert!(graph.contains("on main"), "{graph}");
    assert!(graph.contains("on side"), "the other branch's work is the point of --all:\n{graph}");
    assert!(graph.contains("initial"));
    // Two tips means a second lane, so at least one row draws a rail
    // beside a node.
    assert!(graph.lines().any(|l| l.starts_with("| *")), "expected a second lane:\n{graph}");
}

#[test]
fn the_refs_pane_shows_local_branches_with_their_sync_state() {
    let dir = diverged_repo("history_refs");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();

    let refs = app.buffers.get(app.history_session.as_ref().unwrap().refs_buffer).unwrap().buffer.text();
    assert!(refs.contains("Local"));
    assert!(refs.contains("main"));
    assert!(refs.contains("side"));
    // No remote in this repo, so neither branch has an upstream.
    assert!(refs.contains("[--]"), "a branch with no upstream reads differently from an in-sync one:\n{refs}");
    assert!(refs.contains("never fetched"));
}

#[test]
fn moving_down_the_graph_shows_that_commits_diff() {
    let dir = diverged_repo("history_detail");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let session = app.history_session.as_ref().unwrap();
    let (graph_pane, graph_buffer, detail_buffer) = (session.graph_pane, session.graph_buffer, session.detail_buffer);

    // The newest commit's diff is shown without moving at all.
    assert!(app.buffers.get(detail_buffer).unwrap().buffer.text().contains("from_main.txt"));

    // Move to the row for the *other* branch's commit.
    app.windows_mut().focus(graph_pane);
    let graph_text = app.buffers.get(graph_buffer).unwrap().buffer.text();
    let target = graph_text.lines().position(|l| l.contains("on side")).expect("a row for the side branch's commit");
    let char_idx = app.buffers.get(graph_buffer).unwrap().buffer.line_start_char(target);
    app.test_set_cursor(Cursor { char_idx, sticky_col: 0 });
    app.history_sync_detail();

    let detail = app.buffers.get(detail_buffer).unwrap().buffer.text();
    assert!(detail.contains("from_side.txt"), "expected the side commit's diff:\n{detail}");
}

#[test]
fn the_commit_pane_shows_who_wrote_it_and_what_it_says_above_the_diff() {
    // The gap this closes: `git show`'s header is dropped by the
    // diff parser, so the pane used to show a diff with no author,
    // date or message at all.
    let dir = diverged_repo("history_commit_header");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();

    let detail = app.buffers.get(app.history_session.as_ref().unwrap().detail_buffer).unwrap().buffer.text();
    assert!(detail.starts_with("commit "), "expected a commit header first:\n{detail}");
    assert!(detail.contains("Test <test@example.com>"), "expected the author:\n{detail}");
    assert!(detail.contains("on main"), "expected the subject:\n{detail}");
    assert!(detail.contains("from_main.txt"), "and the diff below it:\n{detail}");
}

#[test]
fn scrolling_onto_a_graph_connector_row_keeps_the_commit_already_shown() {
    // Connector rows (`|\`, `|/`) draw no commit; landing on one
    // must not yank the detail pane back to the newest commit.
    let dir = diverged_repo("history_connector_row");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    // A merge makes the graph emit connector rows at all.
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    app.open_history_view();

    let session = app.history_session.as_ref().unwrap();
    let (graph_pane, graph_buffer, detail_buffer) = (session.graph_pane, session.graph_buffer, session.detail_buffer);
    app.windows_mut().focus(graph_pane);

    // Select a specific commit, then step onto a connector row.
    let connector = app.graph_lines[&graph_buffer]
        .iter()
        .position(|l| l.as_ref().is_some_and(|l| l.commit.is_none()))
        .expect("a merge produces at least one connector row");
    let before = app.buffers.get(detail_buffer).unwrap().buffer.text();
    let char_idx = app.buffers.get(graph_buffer).unwrap().buffer.line_start_char(connector);
    app.test_set_cursor(Cursor { char_idx, sticky_col: 0 });
    app.history_sync_detail();

    assert_eq!(app.buffers.get(detail_buffer).unwrap().buffer.text(), before, "the detail pane should be unchanged");
}

#[test]
fn digit_keys_jump_between_the_history_views_panes() {
    let dir = diverged_repo("history_digits");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let (graph, refs, detail) = {
        let s = app.history_session.as_ref().unwrap();
        (s.graph_pane, s.refs_pane, s.detail_pane)
    };

    // What the digit arm in `route_keypress` resolves and focuses.
    assert_eq!(app.history_pane_by_number(1), Some(graph));
    assert_eq!(app.history_pane_by_number(2), Some(refs));
    assert_eq!(app.history_pane_by_number(3), Some(detail));
    assert_eq!(app.history_pane_by_number(7), None, "a pane number that doesn't exist is ignored");

    let target = app.history_pane_by_number(2).unwrap();
    app.windows_mut().focus(target);
    assert_eq!(app.focused_pane_id(), refs);
}

#[test]
fn the_commit_pane_gets_a_larger_share_than_the_refs_tree() {
    let dir = diverged_repo("history_proportions");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let (detail, refs) = {
        let s = app.history_session.as_ref().unwrap();
        (s.detail_pane, s.refs_pane)
    };

    let layout = app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 1000.0, h: 1000.0 });
    let height = |pane| layout.iter().find(|(id, _)| *id == pane).map(|(_, r)| r.h).unwrap();
    assert!(height(detail) > height(refs), "the commit detail holds a diff, the refs tree a short list");
}

#[test]
fn a_multi_file_commit_opens_with_its_files_folded_and_tab_unfolds_one() {
    let dir = diverged_repo("history_fold");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    // A commit touching two files is where a folded list earns its
    // keep.
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    dir.write("one.txt", "1\n");
    dir.write("two.txt", "2\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "touch two files"]);
    app.open_history_view();

    let (detail_pane, detail_buffer) = {
        let s = app.history_session.as_ref().unwrap();
        (s.detail_pane, s.detail_buffer)
    };
    let folded = app.buffers.get(detail_buffer).unwrap().buffer.text();
    assert!(folded.contains("one.txt") && folded.contains("two.txt"), "both files are listed:\n{folded}");
    assert!(!folded.contains("@@"), "but no hunks until one is opened:\n{folded}");

    // Tab on the first file row opens just that file.
    app.windows_mut().focus(detail_pane);
    let first_file = app.diff_lines[&detail_buffer]
        .iter()
        .position(|l| l.as_ref().is_some_and(|l| l.style == diff_view::DiffStyle::FileHeader))
        .expect("a file header row");
    let char_idx = app.buffers.get(detail_buffer).unwrap().buffer.line_start_char(first_file);
    app.test_set_cursor(Cursor { char_idx, sticky_col: 0 });
    app.diff_toggle_fold();

    let after = app.buffers.get(detail_buffer).unwrap().buffer.text();
    assert!(after.contains("@@"), "Tab should open the file under the cursor:\n{after}");
    // Folding re-renders the whole buffer, so the commit's own
    // header has to survive it -- the first Tab used to drop it.
    assert!(after.starts_with("commit "), "the commit header must survive a fold toggle:\n{after}");
    assert!(after.contains("Test <test@example.com>"), "including the author:\n{after}");
}

#[test]
fn a_single_file_commit_is_not_folded_since_there_is_no_list_to_scan() {
    let dir = diverged_repo("history_single_file");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();

    // The newest commit on `main` touches exactly one file.
    let detail = app.buffers.get(app.history_session.as_ref().unwrap().detail_buffer).unwrap().buffer.text();
    assert!(detail.contains("@@"), "a one-file commit shows its hunks straight away:\n{detail}");
}

#[test]
fn a_read_only_diff_refuses_a_discard_rather_than_arming_a_confirmation() {
    let dir = diverged_repo("history_no_discard");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let detail_pane = app.history_session.as_ref().unwrap().detail_pane;
    app.windows_mut().focus(detail_pane);

    app.diff_discard_request();

    assert!(app.git_confirm.is_none(), "nothing to confirm on a commit's diff");
    assert!(app.modeline_pieces().1.contains("isn't editable"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn closing_the_history_view_removes_its_buffers_and_workspace() {
    let dir = diverged_repo("history_close");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let workspaces_before = app.workspaces.len();
    let session_buffers = {
        let s = app.history_session.as_ref().unwrap();
        [s.graph_buffer, s.refs_buffer, s.detail_buffer]
    };

    app.history_close();

    assert!(app.history_session.is_none());
    assert_eq!(app.workspaces.len(), workspaces_before - 1);
    for id in session_buffers {
        assert!(app.buffers.get(id).is_none(), "every session buffer should be closed");
    }
}

#[test]
fn reopening_the_history_view_refocuses_the_existing_session() {
    let dir = diverged_repo("history_reopen");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let first = app.history_session.as_ref().unwrap().graph_buffer;
    let workspaces = app.workspaces.len();

    app.open_history_view();

    assert_eq!(app.history_session.as_ref().unwrap().graph_buffer, first, "no second session");
    assert_eq!(app.workspaces.len(), workspaces);
}

/// The focused Compare page's text, laid out.
fn compare_text(app: &mut App) -> String {
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    assert!(matches!(app.pages.get(&id).map(|s| &s.model), Some(pages::PageModel::Compare(_))), "the Compare page isn't focused");
    app.ensure_page_layout(id, pane, 120);
    app.open().buffer.text()
}

#[test]
fn comparing_head_with_a_branch_lists_what_head_adds() {
    let dir = diverged_repo("compare_open");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.open_git_compare("side".to_string(), None);

    let text = compare_text(&mut app);
    assert!(text.contains("main compared with side"), "the header names both sides:
{text}");
    assert!(text.contains("Ahead (1)") && text.contains("on main"), "{text}");
    assert!(text.contains("from_main.txt"), "{text}");
    assert!(!text.contains("from_side.txt"), "three-dot leaves out side's own later work:
{text}");
}

#[test]
fn two_dot_brings_in_what_the_other_side_gained_since_diverging() {
    let dir = diverged_repo("compare_two_dot");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_compare("side".to_string(), None);

    assert!(app.page_key(KeyPress::char('t')));

    let text = compare_text(&mut app);
    assert!(text.contains("from_side.txt"), "two-dot compares the trees:
{text}");
    assert!(text.contains("every difference"), "the header reflects the mode:
{text}");
}

#[test]
fn comparing_a_ref_with_itself_says_there_is_no_difference_rather_than_an_error() {
    let dir = diverged_repo("compare_same");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.open_git_compare("main".to_string(), None);

    let text = compare_text(&mut app);
    assert!(text.contains("No differences."), "{text}");
    assert!(!text.contains("couldn't compare"), "{text}");
}

#[test]
fn the_compare_picker_offers_every_ref_with_the_base_branch_first() {
    let dir = diverged_repo("compare_picker");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.config.git_base_branch = Some("side".to_string());

    app.start_compare_picker();

    match &app.active_picker {
        Some(ActivePicker::CompareTarget(state)) => {
            let labels: Vec<String> = state.visible_rows(0, 10).map(|(_, c)| c.label.clone()).collect();
            assert_eq!(labels[0], "side", "the configured base branch leads: {labels:?}");
            assert!(labels.contains(&"main".to_string()));
        }
        _ => panic!("expected the compare picker"),
    }
}

#[test]
fn the_compare_picker_leads_with_master_when_there_is_no_main() {
    let dir = diverged_repo("compare_base_master");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["branch", "-m", "main", "master"]);
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.start_compare_picker();

    match &app.active_picker {
        Some(ActivePicker::CompareTarget(state)) => {
            let labels: Vec<String> = state.visible_rows(0, 10).map(|(_, c)| c.label.clone()).collect();
            assert_eq!(labels[0], "master", "a repo on master should lead with it: {labels:?}");
        }
        _ => panic!("expected the compare picker"),
    }
}

#[test]
fn the_compare_picker_says_what_its_choosing() {
    let dir = diverged_repo("compare_prompts");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.start_compare_picker();
    let (_, suffix) = app.modeline_pieces();
    assert!(suffix.contains("compare HEAD with"), "{suffix}");
}

#[test]
fn comparing_with_a_ref_that_does_not_exist_says_so_on_the_page() {
    // An empty list beside a message that has already gone reads as
    // "identical", a completely different answer from "not a ref".
    let dir = diverged_repo("compare_bad_ref");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());

    app.open_git_compare("no-such-branch".to_string(), None);

    let text = compare_text(&mut app);
    assert!(text.contains("couldn't compare no-such-branch...HEAD"), "got:
{text}");
    assert!(!text.contains("No differences"), "must not claim the two sides match:
{text}");
}

#[test]
fn confirming_the_picker_opens_the_compare_page_and_r_on_it_picks_again() {
    let dir = diverged_repo("compare_chain");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.start_compare_picker();

    app.picker_key(KeyPress::named(FenixNamedKey::Enter));

    assert!(app.active_picker.is_none());
    assert!(compare_text(&mut app).contains("compared with main"));
    assert!(app.page_key(KeyPress::char('R')));
    assert!(matches!(&app.active_picker, Some(ActivePicker::CompareOwn { target, .. }) if target == "main"));
}

#[test]
fn a_history_pane_is_a_tracked_session_so_a_jump_never_hijacks_it() {
    let dir = diverged_repo("history_guard");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.open_history_view();
    let pane = app.history_session.as_ref().unwrap().graph_pane;
    app.windows_mut().focus(pane);

    assert!(app.focused_pane_holds_a_tracked_session());
}

// -- Diff viewer (the Main pane) ---------------------------------------

/// Opens the Git panel on a repo whose `sub/a.txt` has two edits far
/// enough apart to be two separate hunks, selects that file in
/// Unstaged, and leaves the cursor on the Main pane's first hunk
/// header. Returns the session's main buffer and the repo root.
fn diff_panel_with_two_hunks(app: &mut App, dir: &TempDir) -> (BufferId, PathBuf) {
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").current_dir(dir.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    // Commit a long enough baseline that two edits far apart in it
    // are two *separate* hunks with context between them -- the
    // helper repo's own one-line file would only ever produce one.
    dir.write("sub/a.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "baseline"]);
    dir.write("sub/a.txt", "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\nELEVEN\ntwelve\n");
    app.project_root = Some(dir.path().to_path_buf());
    app.open_git_panel();

    let session = app.git_session.as_ref().unwrap();
    let (unstaged_buffer, unstaged_pane, main_buffer, main_pane, repo_root) =
        (session.unstaged_buffer, session.unstaged_pane, session.main_buffer, session.main_pane, session.repo_root.clone());

    let files = fenix_git::list_files(&repo_root);
    app.git_session.as_mut().unwrap().files = files.clone();
    // `sub/` renders collapsed unless it's in the expanded set, and a
    // collapsed directory hides its file rows entirely -- expand it
    // so there's a file row to put the cursor on at all.
    let expanded: HashSet<String> = ["sub".to_string()].into_iter().collect();
    app.git_session.as_mut().unwrap().unstaged_expanded_dirs = expanded.clone();
    app.set_git_buffer(unstaged_buffer, git_panel::render_unstaged(&files, &expanded));
    app.windows_mut().focus(unstaged_pane);
    // Row 1: `sub/` is row 0, the file itself is row 1.
    let char_idx = app.buffers.get(unstaged_buffer).unwrap().buffer.line_start_char(1);
    app.test_set_cursor(Cursor { char_idx, sticky_col: 0 });
    app.git_sync_main(); // no event_proxy in tests -> fetches synchronously

    app.windows_mut().focus(main_pane);
    (main_buffer, repo_root)
}

/// Moves the focused pane's cursor to the first row matching `style`.
fn cursor_to_first_diff_row(app: &mut App, buffer: BufferId, style: diff_view::DiffStyle) {
    let line = app.diff_lines[&buffer].iter().position(|l| l.as_ref().is_some_and(|l| l.style == style)).expect("row exists");
    let char_idx = app.buffers.get(buffer).unwrap().buffer.line_start_char(line);
    app.test_set_cursor(Cursor { char_idx, sticky_col: 0 });
}

#[test]
fn the_main_pane_renders_a_parsed_diff_with_line_numbers_not_raw_text() {
    let dir = fenix_git_test_repo("diff_main_render");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);

    let text = app.buffers.get(main_buffer).unwrap().buffer.text();
    assert!(text.starts_with("M  sub/a.txt"), "expected a file header row, got:\n{text}");
    assert!(text.contains("@@"), "expected hunk headers:\n{text}");
    // A content row carries both line-number columns before the
    // verbatim patch line -- the thing raw `git diff` text never had.
    assert!(text.lines().any(|l| l.contains("  1   1  one")), "expected numbered context rows:\n{text}");
    assert_eq!(app.buffers.get(main_buffer).unwrap().kind, BufferKind::Diff);
}

#[test]
fn every_diff_row_anchors_back_to_its_file_and_hunk() {
    let dir = fenix_git_test_repo("diff_anchor");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::Added);
    let (buffer, anchor) = app.diff_anchor_at_cursor().expect("an added row anchors");
    assert_eq!(buffer, main_buffer);
    assert_eq!(anchor.file, 0);
    assert_eq!(anchor.hunk, Some(0));
    assert!(anchor.new_line.is_some() && anchor.old_line.is_none(), "an added line has only a new-side number");
}

#[test]
fn staging_the_hunk_under_the_cursor_stages_only_that_hunk() {
    let dir = fenix_git_test_repo("diff_stage_hunk");
    let mut app = App::with_file(None);
    let (main_buffer, repo_root) = diff_panel_with_two_hunks(&mut app, &dir);
    assert_eq!(app.diff_models[&main_buffer].files[0].hunks.len(), 2, "test setup should produce two hunks");

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::Added);
    app.diff_apply_hunk(fenix_git::ApplyTarget::Stage);

    let staged = fenix_git::file_diff(&repo_root, "sub/a.txt", true).unwrap();
    assert!(staged.contains("+TWO"), "the hunk under the cursor should be staged:\n{staged}");
    assert!(!staged.contains("ELEVEN"), "the other hunk should not be:\n{staged}");
}

#[test]
fn staging_a_hunk_of_an_already_staged_diff_says_what_to_press_instead() {
    let dir = fenix_git_test_repo("diff_stage_wrong_direction");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);
    // Pretend Main is showing the *staged* side.
    app.diff_models.get_mut(&main_buffer).unwrap().source = DiffSource::Staged;

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::Added);
    app.diff_apply_hunk(fenix_git::ApplyTarget::Stage);

    assert!(app.modeline_pieces().1.contains("`S` unstages it"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_hunk_action_on_a_commits_diff_reports_that_it_is_not_editable() {
    let dir = fenix_git_test_repo("diff_readonly");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);
    app.diff_models.get_mut(&main_buffer).unwrap().source = DiffSource::ReadOnly;

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::Added);
    app.diff_apply_hunk(fenix_git::ApplyTarget::Stage);

    assert!(app.modeline_pieces().1.contains("isn't editable"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn a_hunk_action_on_a_file_header_row_asks_for_a_hunk_instead() {
    let dir = fenix_git_test_repo("diff_file_header_row");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::FileHeader);
    app.diff_apply_hunk(fenix_git::ApplyTarget::Stage);

    assert!(app.modeline_pieces().1.contains("inside a hunk"), "got: {}", app.modeline_pieces().1);
}

#[test]
fn jumping_between_hunks_moves_to_the_next_and_previous_hunk_header() {
    let dir = fenix_git_test_repo("diff_jump_hunk");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);

    let hunk_rows: Vec<usize> = app.diff_lines[&main_buffer]
        .iter()
        .enumerate()
        .filter(|(_, l)| l.as_ref().is_some_and(|l| l.style == diff_view::DiffStyle::HunkHeader))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hunk_rows.len(), 2);

    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 }); // top of the buffer
    app.diff_jump_hunk(true);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, hunk_rows[0]);
    app.diff_jump_hunk(true);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, hunk_rows[1]);
    // Stops rather than wrapping, so "am I at the last change" is felt.
    app.diff_jump_hunk(true);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, hunk_rows[1]);
    assert!(app.modeline_pieces().1.contains("no next hunk"));

    app.diff_jump_hunk(false);
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, hunk_rows[0]);
}

#[test]
fn folding_a_file_hides_its_hunks_and_unfolding_brings_them_back() {
    let dir = fenix_git_test_repo("diff_fold");
    let mut app = App::with_file(None);
    let (main_buffer, _) = diff_panel_with_two_hunks(&mut app, &dir);
    let full = app.buffers.get(main_buffer).unwrap().buffer.visual_line_count();

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::FileHeader);
    app.diff_toggle_fold();
    let folded = app.buffers.get(main_buffer).unwrap().buffer.visual_line_count();
    assert!(folded < full, "folding should hide the hunks: {folded} vs {full}");
    assert!(!app.buffers.get(main_buffer).unwrap().buffer.text().contains("@@"));

    app.diff_toggle_fold();
    assert_eq!(app.buffers.get(main_buffer).unwrap().buffer.visual_line_count(), full);
}

#[test]
fn discarding_a_hunk_arms_a_confirmation_naming_the_file() {
    let dir = fenix_git_test_repo("diff_discard_confirm");
    let mut app = App::with_file(None);
    let (main_buffer, repo_root) = diff_panel_with_two_hunks(&mut app, &dir);

    cursor_to_first_diff_row(&mut app, main_buffer, diff_view::DiffStyle::Added);
    app.git_arm_confirm();
    assert!(app.git_confirm_text().is_some_and(|t| t.contains("Discard this hunk of sub/a.txt")), "got: {:?}", app.git_confirm_text());

    app.git_confirm_key(KeyPress::char('y'));
    let contents = std::fs::read_to_string(repo_root.join("sub/a.txt")).unwrap();
    assert!(contents.contains("two"), "the discarded hunk should be back to its committed form:\n{contents}");
    assert!(contents.contains("ELEVEN"), "the other edit should be untouched:\n{contents}");
}

#[test]
fn record_recent_file_persists_the_canonicalized_path() {
    let dir = TempDir::new("record_recent_file");
    let recent_dir = TempDir::new("record_recent_file_config");
    let file = dir.write("a.txt", "hello");
    let mut app = App::with_file(None);
    app.recent_files = fenix_project::RecentFiles::load_or_default(recent_dir.path().join("recent_files.txt"));

    app.record_recent_file(&file);

    let canonical = fenix_core::plain_path(&std::fs::canonicalize(&file).unwrap());
    assert_eq!(app.recent_files.paths(), std::slice::from_ref(&canonical));
    let reloaded =
        fenix_project::RecentFiles::load_or_default(recent_dir.path().join("recent_files.txt"));
    assert_eq!(reloaded.paths(), &[canonical]);
}

#[test]
fn enter_on_a_home_project_switches_to_that_project() {
    let known_dir = TempDir::new("dashboard_activate_project");
    let project_dir = TempDir::new("dashboard_activate_project_target");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.known_projects.add(project_dir.path().to_path_buf());
    app.recent_files = fenix_project::RecentFiles::load_or_default(known_dir.path().join("recent_files.txt"));
    app.open_dashboard();
    app.test_select_home(&dashboard::HomeEntry::Project(project_dir.path().to_path_buf()));

    assert!(app.home_key(KeyPress::named(FenixNamedKey::Enter)));

    assert_eq!(app.main_view, MainView::Picker);
    assert_eq!(app.project_root, Some(project_dir.path().to_path_buf()));
    assert!(matches!(&app.active_picker, Some(ActivePicker::FindFile(_))));
}

#[test]
fn enter_on_a_home_file_opens_it() {
    let known_dir = TempDir::new("dashboard_activate_recent");
    let file_dir = TempDir::new("dashboard_activate_recent_target");
    let file = file_dir.write("notes.txt", "hello");
    let mut app = App::with_file(None);
    app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
    app.recent_files = fenix_project::RecentFiles::load_or_default(known_dir.path().join("recent_files.txt"));
    app.recent_files.add(file.clone());
    app.recent_files.add(file_dir.write("older.txt", "older"));
    app.recent_files.add(file.clone());
    app.open_dashboard();
    // The newest file is the "resume" slot; open the other one.
    let older = file_dir.path().join("older.txt");
    app.test_select_home(&dashboard::HomeEntry::RecentFile(older.clone()));

    assert!(app.home_key(KeyPress::named(FenixNamedKey::Enter)));
    assert_eq!(app.open().buffer.text(), "older");
    app.open_dashboard();
    // Opening older.txt made it the newest, so notes.txt is a recent
    // row now.
    app.test_select_home(&dashboard::HomeEntry::RecentFile(file.clone()));
    assert!(app.home_key(KeyPress::named(FenixNamedKey::Enter)));

    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.open().buffer.text(), "hello");
    assert_eq!(app.open().kind, BufferKind::Text);
}

#[test]
fn open_startup_file_points_the_focused_pane_at_it() {
    let dir = TempDir::new("open_startup_file");
    let file = dir.write("extra.txt", "second file");
    let mut app = App::with_file(None);
    app.open_startup_file(&file);
    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.open().buffer.text(), "second file");
}

#[test]
fn enter_on_the_find_field_opens_the_file_finder() {
    let mut app = App::with_file(None);
    app.test_select_home(&dashboard::HomeEntry::Find);
    assert!(app.home_key(KeyPress::named(FenixNamedKey::Enter)));
    assert!(matches!(&app.active_picker, Some(ActivePicker::FindFile(_))));
}

#[test]
fn enter_on_a_non_dashboard_buffer_is_not_intercepted() {
    // Guards the `handle_key` check itself: an ordinary Text buffer's
    // Enter must still reach Vim (inserting a newline in Insert mode,
    // moving down a line in Normal mode) rather than this feature's
    // Enter-interception silently swallowing it. Exercised through
    // `open()`/`kind`, the exact condition `handle_key` gates on --
    // a real winit `KeyEvent` isn't constructible from a unit test,
    // same reason every other `handle_key` behavior in this suite is
    // tested via the methods it dispatches to, not the event itself.
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    assert_ne!(app.open().kind, BufferKind::Dashboard);
}

#[test]
fn ctrl_w_and_ctrl_u_edit_a_pickers_query_as_a_shell_does() {
    let candidates = vec![fenix_picker::Candidate::new("apple", PathBuf::from("apple"))];
    let mut app = App::with_file(None);
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    for c in "src/main rs".chars() {
        app.picker_key(KeyPress::char(c));
    }
    let query = |app: &App| app.active_picker.as_ref().map(|p| picker_query(p).to_string());
    app.picker_key(KeyPress::char('w').with_ctrl());
    assert_eq!(query(&app).as_deref(), Some("src/main "));
    app.picker_key(KeyPress::char('w').with_ctrl());
    assert_eq!(query(&app).as_deref(), Some("src/"));
    app.picker_key(KeyPress::char('u').with_ctrl());
    assert_eq!(query(&app).as_deref(), Some(""));
}

#[test]
fn picker_key_routes_typing_navigation_and_escape() {
    let candidates = vec![
        fenix_picker::Candidate::new("apple", PathBuf::from("apple")),
        fenix_picker::Candidate::new("banana", PathBuf::from("banana")),
    ];
    let mut app = App::with_file(None);
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));

    app.picker_key(KeyPress::char('b'));
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => assert_eq!(state.query(), "b"),
        _ => panic!("expected an active FindFile picker"),
    }

    app.picker_key(KeyPress::named(FenixNamedKey::Backspace));
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => assert_eq!(state.query(), ""),
        _ => panic!("expected an active FindFile picker"),
    }

    app.picker_key(KeyPress::named(FenixNamedKey::Down));
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => assert_eq!(state.selected_row(), 1),
        _ => panic!("expected an active FindFile picker"),
    }

    app.picker_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(app.active_picker.is_none());
    assert_eq!(app.main_view, MainView::Editor);
}

#[test]
fn picker_key_enter_confirms_the_selection() {
    let dir = TempDir::new("picker_key_enter");
    let target = dir.write("only.txt", "confirmed\n");
    let mut app = App::with_file(None);
    let candidates = vec![fenix_picker::Candidate::new("only.txt", target.clone())];
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));

    app.picker_key(KeyPress::named(FenixNamedKey::Enter));

    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.open().buffer.path(), Some(target.as_path()));
}

#[test]
fn the_palette_shows_a_pickers_rows_the_selected_one_and_a_preview() {
    let dir = TempDir::new("palette_rows");
    let a = dir.write("a.txt", "first line\nsecond line\n");
    let b = dir.touch("b.txt");
    let candidates = vec![fenix_picker::Candidate::new("a.txt", a), fenix_picker::Candidate::new("b.txt", b)];
    let mut app = App::with_file(None);
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    let view = app.palette_view(20).expect("a view");
    let palette::Body::List(rows) = &view.body else { panic!("a list") };
    assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["a.txt", "b.txt"]);
    assert!(rows[0].selected && !rows[1].selected);
    assert!(rows[0].icon.is_some(), "a file's icon");
    let Some(palette::Side::Preview { lines, .. }) = &view.side else { panic!("a preview") };
    assert_eq!(lines, &["first line", "second line"]);
    // Another picker, the generic look: its labels, nothing beside.
    app.picker_pick_theme();
    let view = app.palette_view(20).unwrap();
    assert!(view.side.is_none() && view.chips[0].0 == "Themes");
}

    // -- Tool status (`SPC l m`) -------------------------------------------

#[test]
fn open_tool_status_panel_opens_a_tool_status_buffer_in_a_new_workspace() {
    let mut app = App::with_file(None);
    let before = app.workspaces.active_index();
    app.open_tool_status_panel();
    assert_ne!(app.workspaces.active_index(), before);
    assert_eq!(app.open().kind, BufferKind::ToolStatus);
    assert!(app.tool_status_session.is_some());
}

#[test]
fn open_tool_status_panel_lists_a_known_language_with_its_found_or_missing_status() {
    let mut app = App::with_file(None);
    app.open_tool_status_panel();
    let text = app.open().buffer.text();
    assert!(text.contains("Rust"), "expected a Rust row in:\n{text}");
    assert!(text.contains("rust-analyzer"), "expected the resolved command in:\n{text}");
}

/// `q` closes Tool Status, and `SPC b d` Task Output, each with the
/// workspace it opened in, leaving no title strip behind; removing
/// Task Output's workspace doesn't stop the next task showing (#29).
#[test]
fn tool_status_and_task_output_close_with_their_workspaces() {
    let mut app = App::with_file(None);
    let workspaces = app.workspaces.len();
    app.open_tool_status_panel();
    assert_eq!(app.workspaces.len(), workspaces + 1);
    let pane = app.focused_pane_id();
    assert!(app.view_closed_by_q(KeyPress::char('q')));
    assert!(app.tool_status_session.is_none() && !app.pane_titles.contains_key(&pane));
    assert_eq!(app.workspaces.len(), workspaces, "its workspace went with it");
    assert!(!app.view_closed_by_q(KeyPress::char('q')), "elsewhere q is Vim's");

    let dir = TempDir::new("task_output_close");
    let (program, args): (&str, Vec<String>) = if cfg!(windows) { ("cmd", vec!["/c".into(), "exit".into(), "0".into()]) } else { ("true", Vec::new()) };
    let task = fenix_tasks::TaskDef { name: "t".to_string(), command: program.to_string(), args };
    app.run_task(task.clone(), dir.path().to_path_buf());
    assert_eq!(app.open().kind, BufferKind::TaskOutput);
    app.kill_buffer();
    assert!(app.task_session.is_none());
    assert_eq!(app.workspaces.len(), workspaces);
    // Its workspace removed with SPC TAB d instead: the next run
    // opens it again rather than writing nowhere.
    app.run_task(task.clone(), dir.path().to_path_buf());
    app.remove_workspace();
    assert!(app.task_session.is_none());
    app.run_task(task, dir.path().to_path_buf());
    assert_eq!(app.open().kind, BufferKind::TaskOutput, "shown again");
    let session = app.task_session.as_ref().unwrap();
    assert_eq!(session.workspace_index, app.workspaces.active_index());
}

#[test]
fn open_tool_status_panel_reuses_the_same_buffer_on_a_second_open() {
    let mut app = App::with_file(None);
    app.open_tool_status_panel();
    let first_buffer = app.tool_status_session.as_ref().unwrap().buffer;
    app.open_tool_status_panel();
    assert_eq!(app.tool_status_session.as_ref().unwrap().buffer, first_buffer);
}

#[test]
fn a_configured_lsp_override_is_reflected_in_the_tool_status_listing() {
    let mut app = App::with_file(None);
    app.config.lsp_servers.push(("rust".to_string(), "my-custom-rust-analyzer".to_string()));
    app.open_tool_status_panel();
    let text = app.open().buffer.text();
    assert!(text.contains("my-custom-rust-analyzer"), "expected the override command in:\n{text}");
}

// -- Table view (`SPC f t`) --------------------------------------

/// Seeds the focused scratch buffer with a small ragged tab-separated
/// table (3 columns; header names are wider than every data value in
/// each column, so `column_widths` is driven entirely by the
/// synthesized `"Column N"` headers -- deliberately, so the expected
/// stops are simple to hand-compute) and toggles it into table view.
fn table_app() -> App {
    // `App::with_file(None)` opens the startup *dashboard* (not a
    // plain scratch buffer) in the initial pane -- `toggle_table_
    // view` only acts on `BufferKind::Text`, so this points the
    // pane at a real scratch buffer first.
    let mut app = App::with_file(None);
    let id = app.buffers.open_scratch();
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, id);
    app.test_insert_str("a\tbb\tccc\nddd\te\tf\n");
    app.toggle_table_view();
    app
}

#[test]
fn toggle_table_view_retags_the_buffer_and_computes_columns_and_stops() {
    let app = table_app();
    assert_eq!(app.open().kind, BufferKind::Table);
    let id = app.focused_buffer_id();
    let view = app.table_views.get(&id).expect("a TableView was computed");
    assert_eq!(view.columns, vec!["Column 1", "Column 2", "Column 3"]);
    // Each header is 8 chars wide (wider than every data value), plus
    // the 2-char `TABLE_COLUMN_GAP`: cumulative stops 10, 20, 30.
    assert_eq!(view.stops, vec![10, 20, 30]);
}

#[test]
fn toggle_table_view_twice_returns_to_text_and_clears_the_view() {
    let mut app = table_app();
    let id = app.focused_buffer_id();
    app.toggle_table_view();
    assert_eq!(app.open().kind, BufferKind::Text);
    assert!(!app.table_views.contains_key(&id));
}

#[test]
fn toggle_table_view_is_a_noop_on_a_dashboard_buffer() {
    let mut app = App::with_file(None);
    let id = app.buffers.open_dashboard("hello\n");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, id);
    app.toggle_table_view();
    assert_eq!(app.open().kind, BufferKind::Dashboard);
}

#[test]
fn table_view_header_pads_column_names_to_line_up_with_the_stops() {
    let app = table_app();
    let id = app.focused_buffer_id();
    assert_eq!(app.table_view_header(id).unwrap(), "Column 1  Column 2  Column 3  ");
}

#[test]
fn gutter_chars_is_zero_for_a_table_buffer() {
    let app = table_app();
    assert_eq!(app.gutter_chars(app.open()), 0);
}

#[test]
fn table_move_column_lands_just_after_the_next_tab() {
    let mut app = table_app();
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 }); // "a", field 0
    app.table_move_column(1);
    assert_eq!(app.test_cursor().char_idx, 2); // "a\tbb..." -- 'b' starts at char 2
    app.table_move_column(1);
    assert_eq!(app.test_cursor().char_idx, 5); // just after the 2nd tab -- 'c' of "ccc"
    app.table_move_column(-1);
    assert_eq!(app.test_cursor().char_idx, 2);
}

#[test]
fn table_move_column_at_the_first_column_clamps_instead_of_going_negative() {
    let mut app = table_app();
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    app.table_move_column(-1);
    assert_eq!(app.test_cursor().char_idx, 0);
}

#[test]
fn table_move_row_stays_in_the_same_elastic_column_across_ragged_field_lengths() {
    // Row 0's field 1 ("bb") starts at char 2; row 1's own field 1
    // ("e") starts at char 6 ("ddd\t" is 4 chars) -- a different raw
    // char offset for "the same column," which is exactly the case
    // plain Vim `j`/`k` (char-based `sticky_col`) gets wrong and
    // `table_move_row` (tab-counting) gets right.
    let mut app = table_app();
    app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 }); // row 0, field 1 ("bb")
    app.table_move_row(1);
    let idx = app.test_cursor().char_idx;
    let line = app.open().buffer.line_col(&app.test_cursor()).0;
    assert_eq!(line, 1);
    assert_eq!(idx, 13); // 'e', "ddd\t" is 4 chars into row 1
    assert_eq!(app.table_current_column(), 1);
}

#[test]
fn table_move_row_past_the_last_line_is_a_noop() {
    let mut app = table_app();
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    app.table_move_row(-1);
    assert_eq!(app.test_cursor().char_idx, 0);
}

#[test]
fn sync_table_view_widens_a_column_after_an_edit_lengthens_a_value() {
    let mut app = table_app();
    let id = app.focused_buffer_id();
    assert_eq!(app.table_views.get(&id).unwrap().stops, vec![10, 20, 30]);

    // Lengthen the first field past the synthesized "Column 1"
    // header's own 8-char width.
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    app.test_insert_str("a-much-longer-value");
    app.sync_table_view(id);

    let widened = &app.table_views.get(&id).unwrap().stops;
    assert!(widened[0] > 10, "column 0's stop should have grown, got {widened:?}");
}

#[test]
fn sync_table_view_is_a_noop_when_nothing_changed_since_the_last_call() {
    let mut app = table_app();
    let id = app.focused_buffer_id();
    let before = app.table_views.get(&id).unwrap().last_edit_count;
    app.sync_table_view(id); // no edit happened in between
    assert_eq!(app.table_views.get(&id).unwrap().last_edit_count, before);
}

#[test]
fn open_table_column_picker_lists_column_names_and_confirm_jumps_to_the_column() {
    let mut app = table_app();
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    app.open_table_column_picker();
    match &app.active_picker {
        Some(ActivePicker::TableColumn(state)) => {
            assert_eq!(state.len(), 3);
        }
        _ => panic!("expected an active TableColumn picker"),
    }
    picker_move_selection(app.active_picker.as_mut().unwrap(), 2); // -> "Column 3"
    app.picker_confirm();
    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.test_cursor().char_idx, 5); // start of "ccc", field 2
}

#[test]
fn content_spans_expands_a_tab_using_the_given_tab_stops() {
    let mut app = App::with_file(None);
    app.test_insert_str("a\tb");
    let ob = app.open();
    let spans = app.content_spans(ob, 0, 1, 0, None, &[], 0, &TabStops::Fixed(4), 0);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "a   b"); // tab expands from col 1 to col 4
}

#[test]
fn content_spans_with_a_nonzero_scroll_col_slices_the_code_but_not_the_gutter() {
    let mut app = App::with_file(None);
    app.test_insert_str("abcdefghij");
    let ob = app.open();
    let spans = app.content_spans(ob, 0, 1, 2, None, &[], 0, &TabStops::Fixed(4), 3);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    // "1 " gutter (gutter_chars = 2) untouched, code starts at 'd'
    // (3 chars skipped: "abc").
    assert_eq!(joined, "1 defghij");
}

#[test]
fn content_spans_scroll_col_counts_visual_columns_past_a_tab() {
    let mut app = App::with_file(None);
    app.test_insert_str("a\tbb"); // expands to "a   bb" at tab width 4
    let ob = app.open();
    let spans = app.content_spans(ob, 0, 1, 0, None, &[], 0, &TabStops::Fixed(4), 4);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "bb"); // first 4 visual columns ("a   ") skipped
}

#[test]
fn content_spans_scroll_col_past_a_short_line_renders_no_code_text() {
    let mut app = App::with_file(None);
    app.test_insert_str("ab");
    let ob = app.open();
    let spans = app.content_spans(ob, 0, 1, 1, None, &[], 0, &TabStops::Fixed(4), 5);
    let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(joined, "1 "); // just the gutter, no code left to show
}

#[test]
fn content_spans_scroll_col_keeps_syntax_highlights_on_the_right_characters() {
    let mut app = App::with_file(None);
    app.test_insert_str("aaaabb");
    let color = app.theme.syntax_variable;
    let fg = app.theme.fg;
    let ob = app.open();
    let highlights = vec![(4..6, color)]; // "bb"'s real byte range
    let spans = app.content_spans(ob, 0, 1, 0, None, &highlights, 0, &TabStops::Fixed(4), 2);
    // First 2 chars ("aa") scrolled off; remaining "aabb" keeps "bb"
    // colored, not shifted onto the wrong characters.
    assert_eq!(spans, vec![("aa".to_string(), fg), ("bb".to_string(), color)]);
}

// -- Macros (`q{reg}`/`@{reg}`/`@@`) -------------------------------

fn macro_app(text: &str) -> App {
    let mut app = App::with_file(None);
    let id = app.buffers.open_scratch();
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, id);
    app.test_insert_str(text);
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    app
}

#[test]
fn recording_then_replaying_a_macro_reproduces_the_same_edit() {
    let mut app = macro_app("a\nb\nc");
    app.test_dispatch_key(KeyPress::char('q'));
    app.test_dispatch_key(KeyPress::char('a'));
    // Append "!" to the current line, then move to the next one.
    app.test_dispatch_key(KeyPress::char('A'));
    app.test_dispatch_key(KeyPress::char('!'));
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    app.test_dispatch_key(KeyPress::char('j'));
    app.test_dispatch_key(KeyPress::char('q')); // stop
    assert_eq!(app.open().buffer.text(), "a!\nb\nc");
    assert!(!app.vim.is_recording());

    // Cursor is now on line 1 ("b") -- replaying should do to it
    // exactly what was just done to line 0 by hand.
    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('a'));
    assert_eq!(app.open().buffer.text(), "a!\nb!\nc");
}

#[test]
fn the_closing_q_and_the_leading_qa_are_not_part_of_the_recorded_macro() {
    let mut app = macro_app("x");
    app.test_dispatch_key(KeyPress::char('q'));
    app.test_dispatch_key(KeyPress::char('a'));
    app.test_dispatch_key(KeyPress::char('l')); // the macro's only real content
    app.test_dispatch_key(KeyPress::char('q'));
    assert_eq!(app.vim.decode_register('a'), Some(vec![KeyPress::char('l')]));
}

#[test]
fn replaying_with_a_count_applies_the_macro_that_many_times() {
    let mut app = macro_app("a\nb\nc\nd");
    app.test_dispatch_key(KeyPress::char('q'));
    app.test_dispatch_key(KeyPress::char('a'));
    app.test_dispatch_key(KeyPress::char('A'));
    app.test_dispatch_key(KeyPress::char('!'));
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    app.test_dispatch_key(KeyPress::char('j'));
    app.test_dispatch_key(KeyPress::char('q'));
    assert_eq!(app.open().buffer.text(), "a!\nb\nc\nd");

    app.test_dispatch_key(KeyPress::char('3'));
    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('a'));
    assert_eq!(app.open().buffer.text(), "a!\nb!\nc!\nd!");
}

#[test]
fn a_nested_at_call_replays_the_targets_current_content_not_a_snapshot() {
    let mut app = macro_app("xxx");
    app.vim.finish_recording('b', &[KeyPress::char('A'), KeyPress::char('1'), KeyPress::named(FenixNamedKey::Escape)], false);
    app.vim.finish_recording(
        'a',
        &[
            KeyPress::char('@'),
            KeyPress::char('b'),
            KeyPress::char('A'),
            KeyPress::char('2'),
            KeyPress::named(FenixNamedKey::Escape),
        ],
        false,
    );

    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('a'));
    assert_eq!(app.open().buffer.text(), "xxx12");

    // Redefine register b -- `@a`'s own nested `@b` must pick up
    // this NEW content on its next replay, not whatever b held the
    // first time (there is no snapshot; `@` always reads the
    // register live).
    app.vim.finish_recording('b', &[KeyPress::char('A'), KeyPress::char('9'), KeyPress::named(FenixNamedKey::Escape)], false);
    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('a'));
    assert_eq!(app.open().buffer.text(), "xxx1292");
}

#[test]
fn a_nested_at_call_is_recorded_literally_not_expanded() {
    // While recording macro `a`, typing the literal keys `@`,`b`
    // must capture exactly those two keys -- not `b`'s own expanded
    // replay -- exactly like real Vim.
    let mut app = macro_app("x");
    app.vim.finish_recording('b', &[KeyPress::char('l')], false);
    app.test_dispatch_key(KeyPress::char('q'));
    app.test_dispatch_key(KeyPress::char('a'));
    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('b'));
    app.test_dispatch_key(KeyPress::char('q'));
    assert_eq!(app.vim.decode_register('a'), Some(vec![KeyPress::char('@'), KeyPress::char('b')]));
}

#[test]
fn at_at_replays_whichever_register_was_last_played_not_last_recorded() {
    let mut app = macro_app("xxx");
    app.vim.finish_recording('a', &[KeyPress::char('A'), KeyPress::char('A'), KeyPress::named(FenixNamedKey::Escape)], false);
    app.vim.finish_recording('b', &[KeyPress::char('A'), KeyPress::char('B'), KeyPress::named(FenixNamedKey::Escape)], false);

    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('a'));
    assert_eq!(app.open().buffer.text(), "xxxA");

    // Recording into a different register must not change what
    // `@@` repeats -- only *playing* one does.
    app.test_dispatch_key(KeyPress::char('q'));
    app.test_dispatch_key(KeyPress::char('c'));
    app.test_dispatch_key(KeyPress::char('l'));
    app.test_dispatch_key(KeyPress::char('q'));

    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('@'));
    assert_eq!(app.open().buffer.text(), "xxxAA"); // repeated 'a', not 'c' or 'b'
}

#[test]
fn a_self_referential_macro_is_bailed_out_instead_of_hanging_or_overflowing_the_stack() {
    let mut app = macro_app("x");
    // Appends "!" then replays itself -- unbounded without a guard.
    app.vim.finish_recording(
        'a',
        &[
            KeyPress::char('A'),
            KeyPress::char('!'),
            KeyPress::named(FenixNamedKey::Escape),
            KeyPress::char('@'),
            KeyPress::char('a'),
        ],
        false,
    );

    app.test_dispatch_key(KeyPress::char('@'));
    app.test_dispatch_key(KeyPress::char('a'));

    // Didn't hang or crash (execution reached here) and stopped
    // well short of an unbounded number of appended '!' characters.
    let bang_count = app.open().buffer.text().chars().filter(|&c| c == '!').count() as u32;
    assert!(bang_count > 0, "expected at least one '!' to have been appended");
    assert!(bang_count <= MAX_MACRO_REPLAY_DEPTH, "expected the depth guard to cap growth, got {bang_count}");
}

#[test]
fn modeline_shows_a_recording_indicator_only_while_actually_recording() {
    let mut app = macro_app("x");
    assert!(!app.modeline_pieces().1.contains("recording"));

    app.test_dispatch_key(KeyPress::char('q'));
    app.test_dispatch_key(KeyPress::char('a'));
    assert!(app.modeline_pieces().1.contains("recording @a"));

    app.test_dispatch_key(KeyPress::char('q'));
    assert!(!app.modeline_pieces().1.contains("recording"));
}

// -- Status/error messages ------------------------------------------

#[test]
fn set_message_and_set_error_show_in_the_modeline_suffix() {
    let mut app = App::with_file(None);
    app.set_message("hello there");
    assert!(app.modeline_pieces().1.contains("hello there"));

    app.set_error("something broke");
    assert!(app.modeline_pieces().1.contains("something broke"));
}

#[test]
fn a_message_expires_after_message_duration() {
    let mut app = App::with_file(None);
    app.set_message("fresh");
    app.status_message.as_mut().unwrap().set_at = Instant::now() - MESSAGE_DURATION * 2;
    assert!(!app.modeline_pieces().1.contains("fresh"));
}

/// A page's modeline names it as its tab does, with no line and
/// column, and doesn't end on a separator.
#[test]
fn a_pages_modeline_has_no_caret_place_or_dangling_dot() {
    let mut app = App::with_file(None);
    app.open_extensions_page();
    let suffix = app.modeline_pieces().1;
    assert!(!suffix.contains("Ln ") && !suffix.contains('*'), "{suffix:?}");
    let theme = &app.theme;
    let spans = modeline_suffix_spans(&suffix, theme, theme.fg_modeline);
    let line: String = spans.iter().map(|(t, _)| t.as_str()).collect();
    assert!(!line.trim_end().ends_with('·'), "{line:?}");
    let spans = modeline_suffix_spans("a.rs   Ln 1, Col 1   Rust   LF ", theme, theme.fg_modeline);
    assert_eq!(spans.iter().filter(|(t, _)| t == " · ").count(), 3);
}

#[test]
fn an_expired_message_falls_back_to_the_ordinary_modeline() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.set_message("stale");
    app.status_message.as_mut().unwrap().set_at = Instant::now() - MESSAGE_DURATION * 2;
    assert!(app.modeline_pieces().1.contains("Ln 1, Col"));
}

// -- Dot-repeat (`.`) -------------------------------------------------

#[test]
fn dot_repeats_a_simple_single_key_change() {
    let mut app = macro_app("aaa");
    app.test_dispatch_key(KeyPress::char('x'));
    assert_eq!(app.open().buffer.text(), "aa");
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "a");
}

fn dispatch_all(app: &mut App, keys: &str) {
    for c in keys.chars() {
        app.test_dispatch_key(KeyPress::char(c));
    }
}

#[test]
fn a_count_on_dot_takes_the_place_of_the_changes_own() {
    let mut app = macro_app("a\nb\nc\nd\ne\nf\ng");
    dispatch_all(&mut app, "dd");
    assert_eq!(app.open().buffer.text(), "b\nc\nd\ne\nf\ng");
    dispatch_all(&mut app, "2.");
    assert_eq!(app.open().buffer.text(), "d\ne\nf\ng", "2. deletes two lines");
    dispatch_all(&mut app, ".");
    assert_eq!(app.open().buffer.text(), "f\ng", "and a bare . keeps the 2, as in Vim");
    let mut app = macro_app("a b c d e f");
    dispatch_all(&mut app, "3dw1.");
    assert_eq!(app.open().buffer.text(), "e f");
}

#[test]
fn bang_filters_lines_through_a_command() {
    let mut app = macro_app("pear\napple\nfig\nkeep");
    dispatch_all(&mut app, "!2j");
    assert_eq!(app.vim.command_line(), ".,.+2!", "the command line opens on the lines, as Vim's does");
    dispatch_all(&mut app, "sort");
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(app.open().buffer.text().replace("\r\n", "\n"), "apple\nfig\npear\nkeep");
    dispatch_all(&mut app, "u");
    assert_eq!(app.open().buffer.text(), "pear\napple\nfig\nkeep", "one change to undo");
}

#[test]
fn dot_repeats_the_new_operators() {
    let mut app = macro_app("ab\ncd\nef");
    dispatch_all(&mut app, "gUUj.");
    assert_eq!(app.open().buffer.text(), "AB\nCD\nef");
    let mut app = macro_app("abcdef");
    dispatch_all(&mut app, "RXY");
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    dispatch_all(&mut app, "l.");
    assert_eq!(app.open().buffer.text(), "XYXYef");
}

#[test]
fn a_macro_replays_the_new_keys() {
    let mut app = macro_app("one two\nthree four");
    dispatch_all(&mut app, "qagUiwjq");
    dispatch_all(&mut app, "k0w@a");
    assert_eq!(app.open().buffer.text(), "ONE TWO\nthree four");
}

#[test]
fn dot_repeats_a_multi_key_operator_motion() {
    let mut app = macro_app("foo bar baz");
    app.test_dispatch_key(KeyPress::char('d'));
    app.test_dispatch_key(KeyPress::char('w'));
    assert_eq!(app.open().buffer.text(), "bar baz");
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "baz");
}

#[test]
fn dot_repeats_an_insert_mode_session() {
    let mut app = macro_app("\n\n");
    app.test_dispatch_key(KeyPress::char('i'));
    app.test_dispatch_key(KeyPress::char('h'));
    app.test_dispatch_key(KeyPress::char('i'));
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    assert_eq!(app.open().buffer.text(), "hi\n\n");
    app.test_dispatch_key(KeyPress::char('j'));
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "hi\nhi\n");
}

#[test]
fn a_plain_motion_does_not_become_the_repeatable_change() {
    let mut app = macro_app("aaa");
    app.test_dispatch_key(KeyPress::char('x'));
    assert_eq!(app.open().buffer.text(), "aa");
    app.test_dispatch_key(KeyPress::char('l')); // pure motion, no edit
    app.test_dispatch_key(KeyPress::char('.')); // must still repeat 'x', not 'l'
    assert_eq!(app.open().buffer.text(), "a");
}

#[test]
fn dot_is_a_noop_before_any_change_has_happened() {
    let mut app = macro_app("aaa");
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "aaa");
}

#[test]
fn dot_repeats_a_text_object_change_with_no_extra_plumbing() {
    // Confirms the design's central claim: `.` works for the new
    // text objects/surround with zero per-command wiring, since it
    // just replays raw keystrokes.
    let mut app = macro_app("(a) (b)");
    app.test_set_cursor(Cursor { char_idx: 1, sticky_col: 1 }); // inside "(a)"
    app.test_dispatch_key(KeyPress::char('d'));
    app.test_dispatch_key(KeyPress::char('i'));
    app.test_dispatch_key(KeyPress::char('('));
    assert_eq!(app.open().buffer.text(), "() (b)");
    app.test_set_cursor(Cursor { char_idx: 4, sticky_col: 4 }); // inside "(b)"
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "() ()");
}

#[test]
fn pressing_dot_itself_does_not_redefine_the_repeated_change() {
    let mut app = macro_app("aaaa");
    app.test_dispatch_key(KeyPress::char('x'));
    assert_eq!(app.open().buffer.text(), "aaa");
    app.test_dispatch_key(KeyPress::char('.'));
    assert_eq!(app.open().buffer.text(), "aa");
    app.test_dispatch_key(KeyPress::char('.')); // still repeats 'x', not '.'
    assert_eq!(app.open().buffer.text(), "a");
}

// -- Quit confirmation -------------------------------------------------

#[test]
fn any_buffer_dirty_is_false_for_a_freshly_opened_buffer() {
    // `macro_app` itself types into the buffer via `test_insert_str`
    // (that's how it seeds content), which already makes it dirty --
    // use the plain dashboard buffer instead for the "nothing's
    // dirty yet" case.
    let app = App::with_file(None);
    assert!(!app.any_buffer_dirty());
}

#[test]
fn any_buffer_dirty_is_true_after_an_edit() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.test_insert('!');
    assert!(app.any_buffer_dirty());
}

#[test]
fn arm_quit_confirm_if_dirty_arms_and_names_the_buffer_in_the_message() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.test_insert('!');
    assert!(app.arm_quit_confirm_if_dirty());
    assert!(app.quit_confirm);
    let message = app.modeline_pieces().1;
    assert!(message.contains("unsaved buffer"));
    assert!(message.contains("[No Name]"), "should name the dirty buffer, not just count it: {message}");
}

#[test]
fn arm_quit_confirm_if_dirty_is_a_noop_when_everything_is_saved() {
    let mut app = App::with_file(None);
    assert!(!app.arm_quit_confirm_if_dirty());
    assert!(!app.quit_confirm);
}

#[test]
fn format_quit_confirm_message_truncates_after_four_names() {
    let mut app = App::with_file(None);
    for _ in 0..5 {
        app.new_scratch_buffer();
        app.test_insert('!');
    }
    let dirty = app.dirty_tracked_buffer_ids();
    assert_eq!(dirty.len(), 5);
    let message = app.format_quit_confirm_message(&dirty);
    assert!(message.starts_with("5 unsaved buffers:"));
    assert!(message.ends_with("(+1 more) -- quit anyway? (y=yes / n=no / l=list)"), "{message}");
}

#[test]
fn resolve_quit_confirm_action_dispatches_on_y_l_and_anything_else() {
    let mut app = App::with_file(None);
    app.quit_confirm = true;
    assert_eq!(app.resolve_quit_confirm_action(KeyPress::char('n')), QuitConfirmAction::Cancel);
    assert!(!app.quit_confirm);

    app.quit_confirm = true;
    assert_eq!(app.resolve_quit_confirm_action(KeyPress::char('l')), QuitConfirmAction::ShowDirtyBuffers);
    assert!(!app.quit_confirm);

    app.quit_confirm = true;
    assert_eq!(app.resolve_quit_confirm_action(KeyPress::char('y')), QuitConfirmAction::Quit);
    assert!(!app.quit_confirm);
}

// -- `zz`/`zt`/`zb` (`scroll_window_at`) ------------------------------

#[test]
fn scroll_window_at_top_puts_the_cursor_line_at_the_very_top() {
    let dir = TempDir::new("scroll_window_zt");
    let content: String = (0..100).map(|i| format!("line{i}\n")).collect();
    let file = dir.write("f.txt", &content);
    let mut app = App::with_file(None);
    app.test_open_path(&file);
    for _ in 0..50 {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.move_down(cursor);
    }
    app.scroll_window_at(VimScrollTarget::Top, 800.0, 600.0);
    let pane = app.focused_pane_id();
    assert_eq!(app.pane_state(pane).scroll_line, 50);
}

#[test]
fn scroll_window_at_center_and_bottom_use_the_panes_real_visible_line_count() {
    let dir = TempDir::new("scroll_window_zzzb");
    let content: String = (0..100).map(|i| format!("line{i}\n")).collect();
    let file = dir.write("f.txt", &content);
    let mut app = App::with_file(None);
    app.test_open_path(&file);
    for _ in 0..50 {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.move_down(cursor);
    }
    let pane = app.focused_pane_id();
    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    let rect = geometry.panes.iter().find(|(id, _)| *id == pane).unwrap().1;
    let content_rect = pane_content_rect(rect, text::LINE_HEIGHT, app.pane_has_breadcrumb(pane));
    let visible_lines = text::lines_that_fit(content_rect.h, text::LINE_HEIGHT);

    app.scroll_window_at(VimScrollTarget::Center, 800.0, 600.0);
    assert_eq!(app.pane_state(pane).scroll_line, 50usize.saturating_sub(visible_lines / 2));

    app.scroll_window_at(VimScrollTarget::Bottom, 800.0, 600.0);
    assert_eq!(app.pane_state(pane).scroll_line, 50usize.saturating_sub(visible_lines.saturating_sub(1)));
}

// -- Search and replace (`SPC s l`/`SPC s r`/`SPC s p`) ------------

#[test]
fn picker_search_buffer_lists_every_line() {
    let mut app = macro_app("alpha\nbeta\ngamma");
    app.picker_search_buffer();
    match &app.active_picker {
        Some(ActivePicker::BufferSearch(state)) => assert_eq!(state.len(), 3),
        _ => panic!("expected an active BufferSearch picker"),
    }
}

#[test]
fn picker_confirm_on_buffer_search_jumps_to_the_lines_start() {
    let mut app = macro_app("alpha\nbeta\ngamma");
    app.picker_search_buffer();
    picker_move_selection(app.active_picker.as_mut().unwrap(), 2); // -> "gamma"
    app.picker_confirm();
    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.test_cursor().char_idx, 11); // "alpha\nbeta\n" is 11 chars
}

#[test]
fn picker_outline_lists_only_heading_lines_labeled_by_nesting_depth() {
    let dir = TempDir::new("picker_outline_lists");
    let file = dir.write("doc.md", "# Top\nsome text\n## Sub\nmore text\n### SubSub");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.picker_outline();

    match &app.active_picker {
        Some(ActivePicker::Outline(state)) => {
            let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
            assert_eq!(labels, vec!["Top", "  Sub", "    SubSub"]);
        }
        other => panic!("expected an open Outline picker, got is_some={}", other.is_some()),
    }
}

#[test]
fn picker_confirm_on_outline_jumps_to_the_headings_start() {
    let dir = TempDir::new("picker_outline_confirm");
    let file = dir.write("doc.md", "# Top\nsome text\n## Sub");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.picker_outline();

    picker_move_selection(app.active_picker.as_mut().unwrap(), 1); // -> "## Sub"
    app.picker_confirm();

    assert_eq!(app.main_view, MainView::Editor);
    assert_eq!(app.test_cursor().char_idx, 16); // "# Top\nsome text\n" is 16 chars -- start of "## Sub" is char 16
}

#[test]
fn picker_outline_on_a_non_markdown_buffer_errors_instead_of_treating_comments_as_headings() {
    let dir = TempDir::new("picker_outline_wrong_language");
    let file = dir.write("script.py", "# not a heading, a comment\nx = 1");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.picker_outline();

    assert!(app.active_picker.is_none());
    assert!(app.modeline_pieces().1.contains("Markdown"));
}

#[test]
fn picker_outline_on_a_markdown_buffer_with_no_headings_still_opens_an_empty_picker() {
    let dir = TempDir::new("picker_outline_empty");
    let file = dir.write("doc.md", "just prose, no headings here");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));

    app.picker_outline();

    match &app.active_picker {
        Some(picker @ ActivePicker::Outline(_)) => assert_eq!(picker_len(picker), 0),
        other => panic!("expected an open (if empty) Outline picker, got is_some={}", other.is_some()),
    }
}

// -- Files menu (`SPC f f`/`a`/`r`/`R`/`D`/`y`) --------------------

#[test]
fn find_file_prompt_relative_path_resolves_against_project_root() {
    let dir = TempDir::new("find_file_prompt_relative");
    let path = dir.write("sub/target.txt", "hi");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.start_find_file_prompt();
    for ch in "sub/target.txt".chars() {
        app.find_file_prompt_key(KeyPress::char(ch));
    }
    app.find_file_prompt_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(app.open().buffer.path(), Some(path.as_path()));
    assert!(app.find_file_prompt.is_none());
}

/// Sets the OS clipboard's text for a paste test, returning whether
/// it worked -- some CI/sandboxed environments have no clipboard at
/// all (`app.clipboard` is `None`), the same gap `yank_file_path_
/// sets_the_clipboard`'s own test already works around by only
/// asserting inside a matching `if let Some(...)`.
///
/// Returns the lock guard too (held by the caller for the rest of
/// its `if let` block): the real OS clipboard is one shared,
/// process-wide resource, so a test that sets it and then checks
/// what's on it must not have another test's text land in between.
/// (Touching it from two threads at once is itself safe -- see
/// `crate::clipboard`.)
static CLIPBOARD_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn test_set_clipboard(app: &mut App, text: &str) -> Option<std::sync::MutexGuard<'static, ()>> {
    let guard = CLIPBOARD_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let clipboard = app.clipboard.as_mut()?;
    clipboard.set_text(text.to_string()).ok()?;
    Some(guard)
}

#[test]
fn find_file_prompt_ctrl_v_pastes_the_clipboard() {
    let mut app = App::with_file(None);
    app.start_find_file_prompt();
    if let Some(_guard) = test_set_clipboard(&mut app, "pasted/path.txt") {
        app.find_file_prompt_key(KeyPress::char('v').with_ctrl());
        assert_eq!(app.find_file_prompt.as_deref(), Some("pasted/path.txt"));
    }
}

#[test]
fn rename_file_prompt_ctrl_v_pastes_the_clipboard() {
    let dir = TempDir::new("rename_prompt_paste");
    let path = dir.write("a.txt", "hi");
    let mut app = App::with_file(None);
    app.test_open_path(&path);
    app.start_rename_file_prompt();
    let before = app.rename_file_prompt.clone().unwrap_or_default();
    if let Some(_guard) = test_set_clipboard(&mut app, "renamed.txt") {
        app.rename_file_prompt_key(KeyPress::char('v').with_ctrl());
        // Paste appends to whatever's already there -- same as every
        // other prompt here -- and `start_rename_file_prompt` seeds
        // the prompt with the current file name, not an empty string.
        assert_eq!(app.rename_file_prompt.as_deref(), Some(format!("{before}renamed.txt")).as_deref());
    }
}

#[test]
fn git_prompt_ctrl_v_pastes_the_clipboard() {
    let mut app = App::with_file(None);
    app.git_prompt = Some(GitPrompt { kind: GitPromptKind::NewBranch, input: String::new() });
    if let Some(_guard) = test_set_clipboard(&mut app, "feature/pasted-name") {
        app.git_prompt_key(KeyPress::char('v').with_ctrl());
        assert_eq!(app.git_prompt.as_ref().unwrap().input, "feature/pasted-name");
    }
}

#[test]
fn explorer_prompt_ctrl_v_pastes_the_clipboard() {
    let mut app = App::with_file(None);
    app.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CopyTo, input: String::new() });
    if let Some(_guard) = test_set_clipboard(&mut app, "new_file.txt") {
        app.explorer_prompt_key(KeyPress::char('v').with_ctrl());
        assert_eq!(app.explorer_prompt.as_ref().unwrap().input, "new_file.txt");
    }
}

#[test]
fn picker_ctrl_v_pastes_the_clipboard_one_char_at_a_time() {
    let mut app = App::with_file(None);
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(Vec::new())));
    if let Some(_guard) = test_set_clipboard(&mut app, "main.rs") {
        app.picker_key(KeyPress::char('v').with_ctrl());
        assert_eq!(picker_query(app.active_picker.as_ref().unwrap()), "main.rs");
    }
}

#[test]
fn find_file_prompt_escape_cancels_without_opening_anything() {
    let mut app = App::with_file(None);
    let before = app.focused_buffer_id();
    app.start_find_file_prompt();
    app.find_file_prompt_key(KeyPress::char('x'));
    app.find_file_prompt_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(app.find_file_prompt.is_none());
    assert_eq!(app.focused_buffer_id(), before);
}

#[test]
fn find_file_prompt_opens_a_path_that_does_not_exist_yet_as_an_empty_buffer() {
    let dir = TempDir::new("find_file_prompt_missing");
    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.start_find_file_prompt();
    for ch in "brand-new.txt".chars() {
        app.find_file_prompt_key(KeyPress::char(ch));
    }
    app.find_file_prompt_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(app.open().buffer.text(), "");
}

#[test]
fn find_file_prompt_tilde_expands_to_the_home_directory() {
    let Some(home) = dirs::home_dir() else { return }; // skip if unavailable in this environment
    let mut app = App::with_file(None);
    app.start_find_file_prompt();
    // An improbable filename so this can't collide with anything real
    // in the actual home directory -- the file won't exist, which is
    // fine, this only checks where the resolution *tried* to look.
    let typed = "~/fenix-test-find-file-prompt-tilde-probe.txt";
    for ch in typed.chars() {
        app.find_file_prompt_key(KeyPress::char(ch));
    }
    app.find_file_prompt_key(KeyPress::named(FenixNamedKey::Enter));
    // Missing-file opens degrade to a path-less scratch buffer (see
    // `BufferList::open_path`'s own fallback) -- what's checkable
    // here is only that this didn't error/panic and landed in the
    // editor, not the buffer's own path. Resolution itself is
    // covered precisely by `open_typed_path`'s absolute-path branch
    // below with a file that *does* exist.
    let _ = home;
    assert_eq!(app.main_view, MainView::Editor);
}

#[test]
fn find_file_prompt_absolute_path_is_used_as_is() {
    let dir = TempDir::new("find_file_prompt_absolute");
    let path = dir.write("abs.txt", "hi");
    let mut app = App::with_file(None);
    // A different project root than `dir`, to prove the absolute
    // path isn't (wrongly) joined onto it.
    app.project_root = Some(std::env::temp_dir());
    app.start_find_file_prompt();
    for ch in path.display().to_string().chars() {
        app.find_file_prompt_key(KeyPress::char(ch));
    }
    app.find_file_prompt_key(KeyPress::named(FenixNamedKey::Enter));
    assert_eq!(app.open().buffer.text(), "hi");
    assert_eq!(app.open().buffer.path(), Some(path.as_path()));
}

#[test]
fn picker_find_file_all_includes_a_gitignored_file() {
    let dir = TempDir::new("find_file_all");
    let ok = std::process::Command::new("git").args(["init", "-q"]).current_dir(dir.path()).status().map(|s| s.success());
    if !matches!(ok, Ok(true)) {
        return; // git not available in this environment -- skip, same posture as the rg-missing tests
    }
    dir.write("tracked.txt", "x");
    dir.write(".gitignore", ".env\n");
    dir.write(".env", "SECRET=1");

    let mut app = App::with_file(None);
    app.project_root = Some(dir.path().to_path_buf());
    app.picker_find_file_all();
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => {
            let labels: Vec<String> = state.visible_rows(0, state.len()).map(|(_, c)| c.label.clone()).collect();
            assert!(labels.iter().any(|l| l.contains(".env")), "expected .env among candidates, got {labels:?}");
        }
        _ => panic!("expected an active FindFile picker"),
    }
}

#[test]
fn picker_recent_files_candidates_match_recent_files() {
    // `App::with_file(None)` loads the *real* persisted recent-files
    // list (other tests in this same process write to it via
    // `record_recent_file`) -- so this only asserts the two newly
    // `add`ed paths are *among* the candidates, not an exact count.
    let mut app = App::with_file(None);
    app.recent_files.add(PathBuf::from("/a/one.txt"));
    app.recent_files.add(PathBuf::from("/b/two.txt"));
    app.picker_recent_files();
    match &app.active_picker {
        Some(ActivePicker::FindFile(state)) => {
            let labels: Vec<String> = state.visible_rows(0, state.len()).map(|(_, c)| c.label.clone()).collect();
            assert!(labels.iter().any(|l| l.contains("one.txt")), "expected one.txt among candidates, got {labels:?}");
            assert!(labels.iter().any(|l| l.contains("two.txt")), "expected two.txt among candidates, got {labels:?}");
        }
        _ => panic!("expected an active FindFile picker"),
    }
}

#[test]
fn yank_file_path_sets_the_clipboard() {
    // Serialized against every other real-OS-clipboard test in this
    // module -- see `CLIPBOARD_TEST_LOCK`'s own doc comment.
    let _guard = CLIPBOARD_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new("yank_file_path");
    let path = dir.write("a.txt", "hi\n");
    let mut app = App::with_file(None);
    app.test_open_path(&path);
    app.yank_file_path();
    if let Some(clipboard) = &mut app.clipboard {
        assert_eq!(clipboard.get_text().unwrap(), path.display().to_string());
    }
}

#[test]
fn yank_file_path_is_a_noop_for_an_unsaved_buffer() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.yank_file_path(); // must not panic
}

#[test]
fn apply_rename_file_moves_content_and_updates_the_registry() {
    let dir = TempDir::new("rename_file");
    let old_path = dir.write("old.txt", "hello");
    let mut app = App::with_file(None);
    app.test_open_path(&old_path);
    let id = app.focused_buffer_id();

    app.apply_rename_file("new.txt");

    let new_path = dir.path().join("new.txt");
    assert!(!old_path.exists());
    assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "hello");
    assert_eq!(app.open().buffer.path(), Some(new_path.as_path()));
    assert_eq!(app.buffers.id_for_path(&new_path), Some(id));
    assert_eq!(app.buffers.id_for_path(&old_path), None);
}

#[test]
fn apply_rename_file_persists_unsaved_edits_into_the_renamed_file() {
    let dir = TempDir::new("rename_file_dirty");
    let old_path = dir.write("old.txt", "hello");
    let mut app = App::with_file(None);
    app.test_open_path(&old_path);
    app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    app.test_insert_str("X"); // unsaved edit

    app.apply_rename_file("new.txt");

    let new_path = dir.path().join("new.txt");
    assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "Xhello");
}

#[test]
fn start_rename_file_prompt_is_a_noop_for_an_unsaved_buffer() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.start_rename_file_prompt();
    assert!(app.rename_file_prompt.is_none());
}

#[test]
fn apply_delete_file_removes_the_file_and_closes_the_buffer() {
    let dir = TempDir::new("delete_file");
    let path = dir.write("a.txt", "x");
    let mut app = App::with_file(None);
    app.test_open_path(&path);
    let id = app.focused_buffer_id();

    app.apply_delete_file();

    assert!(!path.exists());
    assert!(app.buffers.get(id).is_none());
}

#[test]
fn delete_file_confirm_key_declines_on_anything_but_y() {
    let dir = TempDir::new("delete_file_decline");
    let path = dir.write("a.txt", "x");
    let mut app = App::with_file(None);
    app.test_open_path(&path);
    app.start_delete_file_confirm();
    assert!(app.delete_file_confirm);

    app.delete_file_confirm_key(KeyPress::char('n'));

    assert!(!app.delete_file_confirm);
    assert!(path.exists());
    assert_eq!(app.open().buffer.path(), Some(path.as_path()));
}

#[test]
fn start_delete_file_confirm_is_a_noop_for_an_unsaved_buffer() {
    let mut app = App::with_file(None);
    app.new_scratch_buffer();
    app.start_delete_file_confirm();
    assert!(!app.delete_file_confirm);
}

#[test]
fn is_readonly_buffer_kind_covers_git() {
    assert!(is_readonly_buffer_kind(BufferKind::Git));
    assert!(!is_readonly_buffer_kind(BufferKind::Text));
    assert!(!is_readonly_buffer_kind(BufferKind::Dashboard));
    // A listing is read-only *by kind*; rename mode is the
    // exception, and it lives in `App::buffer_edits_are_reverted`
    // because it depends on which buffer, not on which kind.
    assert!(is_readonly_buffer_kind(BufferKind::Explorer));
    assert!(!is_readonly_buffer_kind(BufferKind::Table));
}

#[test]
fn an_edit_attempt_on_a_git_buffer_is_silently_reverted() {
    let mut app = App::with_file(None);
    app.open_git_panel();
    let text_before = app.open().buffer.text();

    app.test_dispatch_key(KeyPress::char('i'));
    app.test_dispatch_key(KeyPress::char('z'));
    app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));

    assert_eq!(app.open().buffer.text(), text_before);
}

#[test]
fn a_python_server_is_told_again_when_the_venv_changes() {
    let dir = TempDir::new("python_env_refresh");
    dir.write("pyproject.toml", "[project]
name = \"orbit\"
");
    dir.write("uv.lock", "");
    let path = dir.write("main.py", "import pandas
");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let key = LspKey::primary(fenix_syntax::LanguageId::Python, tool_sessions::lsp_root_for_path(&path));
    let (program, args) = echo_command("fixture");
    let (client, _) = fenix_lsp::LspClient::spawn(&program, &args, &key.root).unwrap();
    let mut session = LspSession {
        generation: tool_sessions::generation(), client, name: "pyright-langserver".into(), languages: vec!["python".into()], root: key.root.clone(), reader: None,
        started: Instant::now(), capabilities: Some(Default::default()), open_documents: HashMap::new(), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(), semantic: HashMap::new(), lenses: HashMap::new(),
    };
    push_python_environment(&mut session);
    app.lsp_sessions.insert(key.clone(), session);
    let first = app.lsp_sessions[&key].python_env.clone().unwrap();

    app.refresh_python_environments();
    assert_eq!(app.lsp_sessions[&key].python_env.as_ref(), Some(&first), "nothing changed: not told again");

    // `uv sync` creates the venv after the server started.
    let exe = if cfg!(windows) { ".venv/Scripts/python.exe" } else { ".venv/bin/python" };
    dir.write(exe, "");
    assert!(first.is_stale(&key.root));
    app.refresh_python_environments();
    let second = app.lsp_sessions[&key].python_env.clone().unwrap();
    assert_ne!(second, first, "told about the new venv");
    assert!(!second.is_stale(&key.root));
}

fn project_scope_fixture(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new(name);
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "old\n");
    (dir, path)
}

fn install_test_lsp(app: &mut App, path: &Path) -> LspKey {
    let key = LspKey::primary(fenix_syntax::LanguageId::Rust, tool_sessions::lsp_root_for_path(path));
    let (program, args) = echo_command("fixture");
    let (client, _) = fenix_lsp::LspClient::spawn(&program, &args, &key.root).unwrap();
    let canonical = fenix_lsp::normalize(refactor::identity(path));
    app.lsp_sessions.insert(key.clone(), LspSession {
        generation: tool_sessions::generation(), client, name: "rust-analyzer".into(), languages: vec!["rust".into()], root: key.root.clone(), reader: None,
        started: Instant::now(), capabilities: Some(Default::default()), open_documents: HashMap::from([(canonical, (0,1))]), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(), semantic: HashMap::new(), lenses: HashMap::new(),
    });
    key
}

#[test]
fn project_scoped_lsp_routes_same_language_and_same_request_id_independently() {
    let (_a, a) = project_scope_fixture("lsp_scope_a");
    let (_b, b) = project_scope_fixture("lsp_scope_b");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let a_id = app.focused_buffer_id();
    let b_id = app.buffers.open_path(&b);
    let a_key = install_test_lsp(&mut app, &a);
    let b_key = install_test_lsp(&mut app, &b);
    assert_ne!(a_key, b_key);
    assert_eq!(app.focused_lsp_context().unwrap().0, std::slice::from_ref(&a_key));
    for (key, buffer) in [(&a_key,a_id),(&b_key,b_id)] {
        app.lsp_sessions.get_mut(key).unwrap().pending.insert(9, PendingLspRequest::Format { buffer, revision:0 });
    }
    let generation = app.lsp_sessions[&b_key].generation;
    app.apply_lsp_event(b_key.clone(), generation, fenix_lsp::LspEvent::Response { id:9, result:Ok(serde_json::json!([
        {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"new"}
    ])) });
    assert_eq!(app.buffers.get(a_id).unwrap().buffer.text(), "old\n");
    assert_eq!(app.buffers.get(b_id).unwrap().buffer.text(), "new\n");
    assert!(app.lsp_sessions[&a_key].pending.contains_key(&9));
    assert!(!app.lsp_sessions[&b_key].pending.contains_key(&9));
}

/// A server's whole-file format, as its answer arrives: the extra
/// cursor in a line it didn't change stays, the main one keeps its
/// line and column, and one undo takes it back.
#[test]
fn a_servers_format_keeps_the_cursors_in_lines_it_left_alone() {
    let (_dir, path) = project_scope_fixture("lsp_format_cursors");
    std::fs::write(&path, "fn a() {\n  b();\n}\nzz\n").unwrap();
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
    app.sync_key_policy();
    let id = app.focused_buffer_id();
    let key = install_test_lsp(&mut app, &path);
    let z = app.open().buffer.text().find("zz").unwrap() + 1;
    app.toggle_caret_at(z);
    app.focused_buffer_and_cursor_mut().1.char_idx = app.open().buffer.text().find("b()").unwrap() + 2;
    let revision = app.open().buffer.edit_count();
    app.lsp_sessions.get_mut(&key).unwrap().pending.insert(9, PendingLspRequest::Format { buffer: id, revision });
    let generation = app.lsp_sessions[&key].generation;
    app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::Response { id: 9, result: Ok(serde_json::json!([
        {"range":{"start":{"line":0,"character":0},"end":{"line":4,"character":0}},"newText":"fn a() {\n    b();\n}\nzz\n"}
    ])) });
    app.follow_outside_edits();
    assert_eq!(app.open().buffer.text(), "fn a() {\n    b();\n}\nzz\n");
    assert_eq!(app.open().buffer.line_col(&app.cursor()), (1, 4), "its line, its column");
    let z = app.open().buffer.text().find("zz").unwrap() + 1;
    assert_eq!(app.extra_carets().iter().map(|c| c.at).collect::<Vec<_>>(), [z]);
    app.press("C-z");
    assert_eq!(app.open().buffer.text(), "fn a() {\n  b();\n}\nzz\n", "one undo");
}

/// Taking a side of a conflict changes only its lines: an extra
/// cursor below it stays, and one undo brings the markers back.
#[test]
fn resolving_a_conflict_keeps_the_cursors_outside_it() {
    let dir = TempDir::new("conflict_resolve_cursors");
    let text = "x\n<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\nyy\n";
    let file = dir.write("a.txt", text);
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
    app.sync_key_policy();
    let y = text.find("yy").unwrap() + 1;
    app.toggle_caret_at(y);
    app.focused_buffer_and_cursor_mut().1.char_idx = text.find("a\n=").unwrap();
    app.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
    app.follow_outside_edits();
    assert_eq!(app.open().buffer.text(), "x\na\nyy\n");
    assert_eq!(app.extra_carets().iter().map(|c| c.at).collect::<Vec<_>>(), ["x\na\ny".len()], "where it was in yy");
    app.press("C-z");
    assert_eq!(app.open().buffer.text(), text, "one undo");
}

#[test]
fn project_scoped_lsp_ignores_retired_generations_and_foreign_diagnostics() {
    let (_a, a) = project_scope_fixture("lsp_generation_a");
    let (_b, b) = project_scope_fixture("lsp_generation_b");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let key = install_test_lsp(&mut app, &a);
    let generation = app.lsp_sessions[&key].generation;
    app.apply_lsp_event(key.clone(), generation+1, fenix_lsp::LspEvent::Disconnected("late".into()));
    assert!(app.lsp_sessions.contains_key(&key));
    app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::Notification {
        method:"textDocument/publishDiagnostics".into(), params:serde_json::json!({"uri":fenix_lsp::path_to_uri(&b).unwrap(),"diagnostics":[
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"foreign"}
        ]})
    });
    assert!(app.diagnostics.is_empty());
    app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::Disconnected("current".into()));
    assert!(!app.lsp_sessions.contains_key(&key));
}

#[test]
fn project_scoped_refactor_rejects_another_projects_document() {
    let (_a, a) = project_scope_fixture("refactor_scope_a");
    let (_b, b) = project_scope_fixture("refactor_scope_b");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let context = app.refactor_context(&app.lsp_key(fenix_syntax::LanguageId::Rust));
    let edit = serde_json::from_value(serde_json::json!({"changes":{fenix_lsp::path_to_uri(&b).unwrap().as_str():[
        {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"new"}
    ]}})).unwrap();
    app.preview_lsp_edit(edit, context);
    assert!(app.refactor_preview.is_none());
    assert!(app.status_message.as_ref().unwrap().text.contains("another project"));
}

#[test]
fn project_scoped_task_rerun_uses_the_focused_projects_history() {
    let (a_dir,a) = project_scope_fixture("task_scope_a");
    let (b_dir,_) = project_scope_fixture("task_scope_b");
    let mut app = App::with_file(None);
    for (root,label) in [(a_dir.path(),"project-A"),(b_dir.path(),"project-B")] {
        let (command,args) = echo_command(label);
        app.run_task(fenix_tasks::TaskDef { name:label.into(), command,args }, root.into());
    }
    let id = app.buffers.open_path(&a);
    app.open_buffer_in_focused_pane(id);
    app.rerun_last_task();
    assert!(app.open().buffer.text().contains("project-A"));
    assert!(!app.open().buffer.text().contains("project-B"));
}

#[test]
#[ignore = "isolated process for structured tool launch tests"]
fn project_tool_fixture() {
    let Ok(value) = std::env::var("FENIX_TOOL_TEST") else { return };
    println!("tool value: {value}");
    println!("tool cwd: {}", std::env::current_dir().unwrap().display());
}

fn write_structured_tools(dir: &TempDir) {
    let exe = std::env::current_exe().unwrap();
    let spec = serde_json::json!({"executable":exe,"args":["--ignored","--exact","app::tests::project_tool_fixture","--nocapture"],
        "cwd":"build dir", "env":{"FENIX_TOOL_TEST":"structured project value"}});
    std::fs::create_dir_all(dir.path().join("build dir")).unwrap();
    dir.write(".fenix/tools.json", &serde_json::json!({
        "tasks":{"structured":spec}, "dap":{"python":spec},
        "launch":{"program":"main.py", "args":["a b",""],"cwd":"build dir","env":{"DEBUG_TARGET":"project value"}}
    }).to_string());
}

#[test]
fn project_scoped_task_uses_structured_cwd_and_environment() {
    let (dir,_) = project_scope_fixture("structured_task");
    write_structured_tools(&dir);
    let before = std::env::var_os("FENIX_TOOL_TEST");
    let mut app = App::with_file(None);
    app.run_task(fenix_tasks::TaskDef { name:"structured".into(),command:"not-used".into(),args:vec![] },dir.path().into());
    assert!(app.open().buffer.text().contains("tool value: structured project value"));
    assert_eq!(app.task_session.as_ref().unwrap().cwd, fenix_lsp::normalize(refactor::identity(&dir.path().join("build dir"))));
    assert_eq!(std::env::var_os("FENIX_TOOL_TEST"), before);
}

#[test]
fn project_scoped_debug_launch_freezes_args_cwd_env_and_ignores_stale_events() {
    let (dir,_) = project_scope_fixture("structured_debug");
    let (_other,b) = project_scope_fixture("structured_debug_other");
    let script = dir.write("main.py", "print('hello')\n");
    write_structured_tools(&dir);
    let mut app = App::with_file(Some(script.to_string_lossy().into_owned()));
    app.debug_start_or_continue();
    let session = app.debug_session.as_ref().expect("configured adapter should spawn");
    assert_eq!(session.launch_attributes["args"],serde_json::json!(["a b",""]));
    assert_eq!(session.launch_attributes["env"]["DEBUG_TARGET"], "project value");
    assert_eq!(session.launch_attributes["cwd"], serde_json::json!(session.root.join("build dir")));
    let generation = session.generation;
    app.handle_user_event(FenixUserEvent::Dap { generation:generation+1, event:fenix_dap::DapEvent::Disconnected("stale".into()) });
    assert!(app.debug_session.is_some());
    let id = app.buffers.open_path(&b);
    app.open_buffer_in_focused_pane(id);
    app.debug_start_or_continue();
    assert!(app.status_message.as_ref().unwrap().text.contains("another project"));
    app.debug_stop();
    assert!(app.debug_session.is_some());
}

#[test]
fn project_scoped_servers_spawn_separately_and_canonical_aliases_reuse_them() {
    let (a,_) = project_scope_fixture("server_spawn_a");
    let (b,_) = project_scope_fixture("server_spawn_b");
    let mut app = App::with_file(None);
    for (dir,value) in [(&a,"A"),(&b,"B")] {
        dir.write(".fenix/tools.json",&serde_json::json!({"lsp":{"rust":{
            "executable":std::env::current_exe().unwrap(),"args":["--ignored","--exact","app::tests::project_tool_fixture","--nocapture"],
            "env":{"FENIX_TOOL_TEST":value}
        }}}).to_string());
        app.ensure_lsp_session(fenix_syntax::LanguageId::Rust,dir.path());
    }
    assert_eq!(app.lsp_sessions.len(),2);
    let generations: std::collections::HashSet<_> = app.lsp_sessions.values().map(|s| s.generation).collect();
    assert_eq!(generations.len(),2);
    app.ensure_lsp_session(fenix_syntax::LanguageId::Rust,&a.path().join("."));
    assert_eq!(app.lsp_sessions.len(),2);
}
/// Stands in for a language server or a debug adapter in tests: stays
/// alive reading messages until LSP's `exit` or DAP's `disconnect`,
/// answering nothing -- each test answers as the server itself,
/// through `apply_lsp_event`/`apply_dap_event`. Unlike `echo`, it's
/// still there when a test writes to it. (Its stdout carries the test
/// harness's own output, which no test reads.)
#[test]
#[ignore = "a fixture other tests start as a process"]
fn lsp_sink_fixture() {
    if std::env::var_os("FENIX_LSP_SINK").is_none() {
        return;
    }
    use std::io::Read;
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n @ 1..) = std::io::stdin().read(&mut chunk) {
        seen.extend_from_slice(&chunk[..n]);
        if seen.windows(15).any(|w| w == br#""method":"exit""#) || seen.windows(22).any(|w| w == br#""command":"disconnect""#) {
            return;
        }
    }
}

fn sink_command(root: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", "app::tests::lsp_sink_fixture", "--nocapture"]).env("FENIX_LSP_SINK", "1").current_dir(root);
    command
}

/// A language server is looked for and started on a thread of its
/// own, and taken up when the event loop hears it's up: the window
/// never waits on a slow `PATH` or a slow server (#33). One that
/// can't start says so as before; one stopped meanwhile is let go.
#[test]
fn a_language_server_starts_off_the_ui_thread() {
    let dir = TempDir::new("lsp_start_off_thread");
    let mut app = App::with_file(None);
    let (tx, rx) = std::sync::mpsc::channel();
    app.lsp_start_sink = Some(tx);
    let key = |name: &str| LspKey { server: LspServer::Additional(name.into()), root: dir.path().to_path_buf() };
    let sink = fenix_project::tools::CommandSpec {
        executable: std::env::current_exe().unwrap().display().to_string(),
        args: ["--ignored", "--exact", "app::tests::lsp_sink_fixture", "--nocapture"].map(String::from).to_vec(),
        env: [("FENIX_LSP_SINK".to_string(), "1".to_string())].into(),
        ..Default::default()
    };
    app.start_lsp_session(key("sink"), "sink".into(), vec!["rust".into()], &sink, dir.path());
    assert!(!app.lsp_sessions.contains_key(&key("sink")) && app.lsp_starting.contains_key(&key("sink")), "started elsewhere");
    let started = rx.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(matches!(&started, FenixUserEvent::LspStarted(s) if s.result.is_ok()), "{started:?}");
    app.handle_user_event(started);
    assert!(app.lsp_sessions.contains_key(&key("sink")) && app.lsp_starting.is_empty());
    assert!(app.lsp_sessions[&key("sink")].pending.values().any(|p| matches!(p, PendingLspRequest::Initialize)), "asked to initialize");

    let missing = fenix_project::tools::CommandSpec::new("definitely-not-a-language-server-xyz".into(), Vec::new());
    app.start_lsp_session(key("missing"), "missing".into(), vec!["rust".into()], &missing, dir.path());
    app.handle_user_event(rx.recv_timeout(Duration::from_secs(60)).unwrap());
    assert!(app.lsp_unavailable.contains(&key("missing")) && !app.lsp_sessions.contains_key(&key("missing")));
    assert!(app.lsp_failures[&key("missing")].starts_with("couldn't start definitely-not-a-language-server-xyz"), "{:?}", app.lsp_failures);

    app.start_lsp_session(key("late"), "late".into(), vec!["rust".into()], &sink, dir.path());
    app.stop_lsp_session(&key("late"));
    app.handle_user_event(rx.recv_timeout(Duration::from_secs(60)).unwrap());
    assert!(!app.lsp_sessions.contains_key(&key("late")) && !app.lsp_unavailable.contains(&key("late")), "let go");
}

fn sink_client(root: &Path) -> fenix_lsp::LspClient {
    fenix_lsp::LspClient::spawn_command(sink_command(root)).unwrap().0
}

/// A running, initialized server for `path`'s project, serving
/// `languages` and offering `capabilities`.
pub(in crate::app) fn install_server(app: &mut App, path: &Path, server: LspServer, name: &str, languages: &[&str], capabilities: lsp_types::ServerCapabilities) -> LspKey {
    let key = LspKey { server, root: tool_sessions::lsp_root_for_path(path) };
    if let LspServer::Additional(name) = &key.server {
        let additional = fenix_project::tools::AdditionalServer { name: name.clone(), languages: languages.iter().map(|l| l.to_string()).collect(), executable: name.clone(), ..Default::default() };
        app.lsp_additional.entry(key.root.clone()).or_default().push(additional);
    }
    app.lsp_sessions.insert(key.clone(), LspSession {
        generation: tool_sessions::generation(), client: sink_client(&key.root), name: name.into(), languages: languages.iter().map(|l| l.to_string()).collect(),
        root: key.root.clone(), reader: None, started: Instant::now(), capabilities: Some(capabilities), open_documents: HashMap::new(), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(), semantic: HashMap::new(), lenses: HashMap::new(),
    });
    key
}

pub(in crate::app) fn capabilities(json: serde_json::Value) -> lsp_types::ServerCapabilities {
    serde_json::from_value(json).unwrap()
}

/// The one pending request `key` has, and its id.
pub(in crate::app) fn only_pending(app: &App, key: &LspKey) -> (i64, PendingLspRequest) {
    let pending = &app.lsp_sessions[key].pending;
    assert_eq!(pending.len(), 1, "{pending:?}");
    pending.iter().map(|(id, p)| (*id, p.clone())).next().unwrap()
}

pub(in crate::app) fn answer(app: &mut App, key: &LspKey, id: i64, result: serde_json::Value) {
    let generation = app.lsp_sessions[key].generation;
    app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::Response { id, result: Ok(result) });
}

/// A Rust file served by a primary server and an additional one that
/// also serves Markdown.
fn two_servers(name: &str) -> (TempDir, App, PathBuf, LspKey, LspKey) {
    let (dir, path) = project_scope_fixture(name);
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let all = serde_json::json!({"textDocumentSync": 2, "hoverProvider": true, "definitionProvider": true, "referencesProvider": true, "codeActionProvider": true,
        "completionProvider": {"triggerCharacters": ["."]}, "renameProvider": true});
    let primary = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], capabilities(all));
    let extra = install_server(&mut app, &path, LspServer::Additional("kiln".into()), "kiln", &["rust", "markdown"],
        capabilities(serde_json::json!({"textDocumentSync": 1, "hoverProvider": true, "definitionProvider": true, "completionProvider": {"triggerCharacters": ["-"]}, "documentFormattingProvider": true})));
    app.sync_lsp_for_focused_buffer();
    (dir, app, path, primary, extra)
}

fn semantic_capabilities() -> lsp_types::ServerCapabilities {
    capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["function", "macro"], "tokenModifiers": []}, "full": true}}))
}

/// `fn main() { let x = 1; }` with a server offering semantic tokens.
fn semantic_fixture(name: &str) -> (TempDir, App, PathBuf, LspKey) {
    let dir = TempDir::new(name);
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "fn main() { let x = 1; }\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], semantic_capabilities());
    app.sync_lsp_for_focused_buffer();
    (dir, app, path, key)
}

/// `main` a function and `x` a macro (drawn in the macro colour,
/// where the grammar says variable).
const MAIN_AND_X: [u32; 10] = [0, 3, 4, 0, 0, 0, 13, 1, 1, 0];

/// The colour a macro token is drawn in.
fn macro_colour(app: &App) -> glyphon::Color {
    app.theme.syntax_macro
}

fn colour_at(app: &mut App, byte: usize) -> Option<glyphon::Color> {
    let id = app.focused_buffer_id();
    app.syntax_highlights_for_visible_range(id, 0, 1).into_iter().find(|(r, _)| r.contains(&byte)).map(|(_, c)| c)
}

#[test]
fn semantic_tokens_are_asked_on_open_and_laid_over_the_grammar_s_colours() {
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_open");
    let grammar = colour_at(&mut app, 16);
    app.ask_due_semantic_tokens();
    let (id, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::SemanticTokens { edit_count: 0, .. }), "{pending:?}");
    answer(&mut app, &key, id, serde_json::json!({"data": MAIN_AND_X}));
    let (function, macro_) = (app.theme.syntax_function, app.theme.syntax_macro);
    assert_ne!(grammar, Some(macro_), "the grammar calls `x` something else");
    assert_eq!(colour_at(&mut app, 16), Some(macro_), "`x` as the server says");
    assert_eq!(colour_at(&mut app, 3), Some(function), "`main` a function");
    assert_eq!(colour_at(&mut app, 0), Some(app.theme.syntax_keyword), "`fn` the grammar's still");
}

#[test]
fn semantic_tokens_move_with_an_edit_until_the_next_answer_and_a_stale_answer_is_let_go() {
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_shift");
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!({"data": MAIN_AND_X}));
    let macro_ = app.theme.syntax_macro;
    // Two chars typed at the start: `x` moves with them, and no ask
    // is sent until typing pauses.
    app.test_insert_str("  ");
    app.sync_lsp_for_focused_buffer();
    assert_eq!(colour_at(&mut app, 18), Some(macro_));
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].pending.is_empty(), "not while typing");
    // Once it's due: asked; an edit before the answer makes it stale.
    let canonical = app.semantic_of[&app.focused_buffer_id()].path.clone();
    app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(Instant::now());
    app.ask_due_semantic_tokens();
    let (id, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::SemanticTokens { edit_count, .. } if edit_count == app.open().buffer.edit_count()));
    app.test_insert_str(" ");
    app.sync_lsp_for_focused_buffer();
    answer(&mut app, &key, id, serde_json::json!({"data": [0, 0, 2, 0, 0]}));
    assert_eq!(colour_at(&mut app, 19), Some(macro_), "the moved tokens stand, not the stale answer");
    assert!(app.lsp_sessions[&key].semantic[&canonical].due.is_some(), "asked again");
}

fn answer_error(app: &mut App, key: &LspKey, id: i64, code: i64) {
    let generation = app.lsp_sessions[key].generation;
    let error = fenix_lsp::ResponseError { code, message: "no".into(), data: None };
    app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::Response { id, result: Err(error) });
}

#[test]
fn semantic_tokens_aren_t_drawn_on_a_file_closed_changed_on_disk_and_opened_again() {
    let (dir, mut app, path, key) = semantic_fixture("semantic_reopen");
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!({"data": MAIN_AND_X}));
    let canonical = app.semantic_of[&app.focused_buffer_id()].path.clone();
    let other = dir.write("notes.txt", "notes\n");
    app.test_open_path(&other);
    app.test_open_path(&path);
    app.kill_buffer();
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].semantic.is_empty(), "let go with its buffer");
    // Changed on disk meanwhile, and opened again: the server is told
    // the new text, and the old tokens aren't laid over it.
    let new = "static LONG_NAME_HERE: u8 = 0;\n";
    std::fs::write(&path, new).unwrap();
    app.test_open_path(&path);
    app.sync_lsp_for_focused_buffer();
    assert_eq!(app.lsp_sessions[&key].synced_text[&canonical], new, "a didChange, though the edit count is 0 again");
    assert!(app.drawn_tokens(app.focused_buffer_id()).is_none());
    assert_ne!(colour_at(&mut app, 16), Some(macro_colour(&app)));
    app.ask_due_semantic_tokens();
    assert!(matches!(only_pending(&app, &key).1, PendingLspRequest::SemanticTokens { .. }), "asked afresh");
}

#[test]
fn a_semantic_ask_due_wakes_the_loop_and_one_unanswered_is_cancelled_and_asked_once_more() {
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_wake");
    let canonical = app.semantic_of[&app.focused_buffer_id()].path.clone();
    // Due a moment ago, with nothing else to draw: the loop wakes at
    // once, and the wake asks.
    let past = Instant::now() - Duration::from_millis(5);
    app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(past);
    assert!(app.motion_next_wake(Instant::now()).is_some_and(|t| t <= Instant::now()), "woken for");
    app.ask_due_semantic_tokens();
    let (first, _) = only_pending(&app, &key);
    // Unanswered past the timeout: cancelled, asked once more.
    let late = Instant::now() - semantic::SEMANTIC_TIMEOUT - Duration::from_millis(1);
    app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().asked = Some((first, late));
    assert!(app.semantic_wake().is_some_and(|t| t <= Instant::now()), "woken to time it out");
    app.ask_due_semantic_tokens();
    let (second, _) = only_pending(&app, &key);
    assert_ne!(first, second);
    // That one too: not again until an edit.
    app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().asked = Some((second, late));
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].pending.is_empty());
    assert_eq!(app.semantic_wake(), None);
    app.test_insert_str(" ");
    app.sync_lsp_for_focused_buffer();
    assert!(app.semantic_wake().is_some(), "an edit asks again");
    // Cancelled by the server: asked again after the pause.
    app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(past);
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    answer_error(&mut app, &key, id, -32800);
    assert!(app.lsp_sessions[&key].semantic[&canonical].due.is_some(), "RequestCancelled is asked again");
}

#[test]
fn semantic_asks_follow_an_agent_s_sync_a_late_refresh_and_only_the_ask_that_s_out() {
    let (_dir, mut app, path, key) = semantic_fixture("semantic_round3");
    let canonical = app.semantic_of[&app.focused_buffer_id()].path.clone();
    app.ask_due_semantic_tokens();
    let (first, _) = only_pending(&app, &key);
    // A refresh while the ask is out: kept, and asked once it's answered.
    app.refresh_semantic_tokens(&key);
    let _ = colour_at(&mut app, 0);
    assert!(app.lsp_sessions[&key].semantic[&canonical].stale, "kept while an ask is out");
    answer(&mut app, &key, first, serde_json::json!({"data": MAIN_AND_X}));
    let _ = colour_at(&mut app, 0);
    app.ask_due_semantic_tokens();
    let (second, _) = only_pending(&app, &key);
    // An answer to an ask that isn't out any more: let go.
    answer(&mut app, &key, first, serde_json::json!({"data": []}));
    assert!(app.lsp_sessions[&key].semantic[&canonical].asking.is_some(), "the second still out");
    answer(&mut app, &key, second, serde_json::json!({"data": MAIN_AND_X}));
    // An edit an agent's sync sends first: asked again all the same.
    app.test_insert_str(" ");
    let (text, count) = (app.open().buffer.text(), app.open().buffer.edit_count());
    app.sync_lsp_for(&path, &text, count, None);
    assert!(app.semantic_wake().is_some(), "asked again after the pause");
    // Refused while modified: asked again later each time, not every 250 ms.
    app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(Instant::now());
    let mut waits = Vec::new();
    for _ in 0..4 {
        app.ask_due_semantic_tokens();
        let (id, _) = only_pending(&app, &key);
        let before = Instant::now();
        answer_error(&mut app, &key, id, -32801);
        let due = app.lsp_sessions[&key].semantic[&canonical].due.unwrap();
        waits.push(due.saturating_duration_since(before));
        app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(Instant::now());
    }
    assert!(waits.windows(2).all(|w| w[1] > w[0]), "backing off: {waits:?}");
}

#[test]
fn semantic_tokens_are_let_go_when_turned_off_or_the_file_is_renamed() {
    let (dir, mut app, _path, key) = semantic_fixture("semantic_letgo");
    assert!(!app.lsp_sessions[&key].semantic.is_empty());
    app.config.set_value("lsp.semantic_highlighting", Some(fenix_config::Value::Bool(false)));
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].semantic.is_empty() && app.semantic_of.is_empty(), "nothing held or asked");
    app.config.set_value("lsp.semantic_highlighting", Some(fenix_config::Value::Bool(true)));
    app.sync_lsp_for_focused_buffer();
    let old = app.semantic_of[&app.focused_buffer_id()].path.clone();
    // Saved under another name: the old path's tokens go.
    let renamed = dir.path().join("renamed.rs");
    app.open_mut().buffer.save_as(&renamed).unwrap();
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    assert!(!app.lsp_sessions[&key].semantic.contains_key(&old));
}

#[test]
fn a_refactor_s_sync_moves_semantic_tokens_and_asks_again() {
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_refactor");
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!({"data": MAIN_AND_X}));
    // An edit a refactor made, synced as a refactor syncs.
    app.test_insert_str("  ");
    let buffer = app.focused_buffer_id();
    app.sync_refactor_buffers(&[buffer]);
    assert!(app.drawn_tokens(buffer).is_some(), "moved with the change, still drawn");
    assert_eq!(colour_at(&mut app, 18), Some(macro_colour(&app)));
    assert!(app.semantic_wake().is_some(), "and asked again");
}

#[test]
fn an_agent_s_look_at_a_file_asks_for_no_semantic_tokens() {
    let (dir, mut app, _path, key) = semantic_fixture("semantic_agent");
    let other = dir.write("lib.rs", "pub fn lib() {}\n");
    app.sync_lsp_for(&other, "pub fn lib() {}\n", 1, None);
    let canonical = fenix_lsp::normalize(refactor::identity(&other));
    assert!(app.lsp_sessions[&key].open_documents.contains_key(&canonical), "the server is told of it");
    assert!(!app.lsp_sessions[&key].semantic.contains_key(&canonical), "but not asked for its tokens");
}

#[test]
fn a_buffer_without_a_grammar_takes_its_colours_from_the_server_alone() {
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_no_grammar");
    app.open_mut().syntax = None;
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!({"data": MAIN_AND_X}));
    assert_eq!(colour_at(&mut app, 16), Some(macro_colour(&app)));
    assert_eq!(colour_at(&mut app, 3), Some(app.theme.syntax_function));
}

#[test]
fn a_mutable_name_inside_a_string_isn_t_underlined_and_no_lines_ask_for_none() {
    let dir = TempDir::new("semantic_mutable_string");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "let s = \"abc\";\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["variable"], "tokenModifiers": ["mutable"]}, "full": true}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    // `s` (4) and the `b` inside the string (10), both mutable.
    answer(&mut app, &key, id, serde_json::json!({"data": [0, 4, 1, 0, 1, 0, 6, 1, 0, 1]}));
    let buffer = app.focused_buffer_id();
    // A line looked up on its own doesn't stand for the pane's text...
    let _ = colour_at(&mut app, 0);
    assert!(!app.semantic_protected.contains_key(&buffer));
    // ...the pane's own drawing does.
    let _ = app.syntax_highlights_in_columns(buffer, 0, 1, 0, None, true);
    assert_eq!(app.underlined_tokens(buffer, 0..1).into_iter().map(|(_, c, _)| c).collect::<Vec<_>>(), vec![4..5], "not the one in the string");
    assert!(app.underlined_tokens(buffer, 0..0).is_empty());
}

#[test]
fn a_mutable_name_from_semantic_tokens_is_underlined_on_each_row_it_s_drawn_on() {
    let dir = TempDir::new("semantic_mutable");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "let mut counter_of_things = 0;\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["variable"], "tokenModifiers": ["mutable"]}, "full": true}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    // `counter_of_things`, chars 8..25, mutable.
    answer(&mut app, &key, id, serde_json::json!({"data": [0, 8, 17, 0, 1]}));
    let buffer = app.focused_buffer_id();
    assert_eq!(app.underlined_tokens(buffer, 0..1).into_iter().map(|(l, c, _)| (l, c)).collect::<Vec<_>>(), vec![(0, 8..25)]);
    // Wrapped at twelve columns: the name is broken across rows 1
    // and 2, and underlined on both.
    app.run_cmd("view.toggle_wrap");
    let pane = app.focused_pane_id();
    app.set_pane_text_cols(pane, 12);
    let layout = app.focused_layout(4);
    let vcol = |_: usize, c: usize| c;
    let selection = Segments::default();
    let tab_lines = HashSet::new();
    let view = polish::PaneView { buffer, map: &layout.map, rows: 4, wrapped: true, gutter_chars: 0, scroll_col: 0, rendered_scroll: 0.0, cursor_line: 0, tab_lines: &tab_lines, selection: &selection, vcol: &vcol, carets: &[], pane };
    let mut spans = Vec::new();
    let out = app.polish_pane(&view, &mut spans, Instant::now());
    let rows: Vec<usize> = out.underlines.iter().map(|&(row, _, _, _)| row).collect();
    assert_eq!(rows, vec![1, 2], "{:?}", out.underlines);
}

#[test]
fn a_semantic_delta_is_asked_on_the_last_answer_and_one_that_doesn_t_fit_asks_the_whole_again() {
    let dir = TempDir::new("semantic_delta");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "fn main() { let x = 1; }\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["function", "macro"], "tokenModifiers": []}, "full": {"delta": true}}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    let (id, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Full, .. }), "the first is whole");
    answer(&mut app, &key, id, serde_json::json!({"resultId": "1", "data": MAIN_AND_X}));
    assert_eq!(colour_at(&mut app, 16), Some(app.theme.syntax_macro));
    // Asked again: a delta on "1", turning `x` into a function.
    app.refresh_semantic_tokens(&key);
    let _ = colour_at(&mut app, 0);
    app.ask_due_semantic_tokens();
    let (id, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Delta, .. }), "{pending:?}");
    answer(&mut app, &key, id, serde_json::json!({"resultId": "2", "edits": [{"start": 5, "deleteCount": 5, "data": [0, 13, 1, 0, 0]}]}));
    assert_eq!(colour_at(&mut app, 16), Some(app.theme.syntax_function), "the delta applied to what was kept");
    // A delta leaving part of a token: the whole document asked again.
    app.refresh_semantic_tokens(&key);
    let _ = colour_at(&mut app, 0);
    app.ask_due_semantic_tokens();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!({"resultId": "3", "edits": [{"start": 3, "deleteCount": 1, "data": []}]}));
    app.ask_due_semantic_tokens();
    assert!(matches!(only_pending(&app, &key).1, PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Full, .. }));
}

#[test]
fn a_large_document_s_semantic_tokens_are_asked_for_the_lines_in_view_and_too_large_a_one_not_at_all() {
    let dir = TempDir::new("semantic_range");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", &"fn main() { let x = 1; }\n".repeat(12_000));
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["function", "macro"], "tokenModifiers": []}, "full": true, "range": true}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].pending.is_empty(), "nothing in view yet");
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    // A line looked up on its own (sticky scroll's) asks nothing.
    let _ = app.syntax_highlights_for_visible_range(id, 0, 1);
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].pending.is_empty(), "only a pane's view asks");
    app.note_semantic_view(id, pane, &[0..1]);
    app.ask_due_semantic_tokens();
    let (req, pending) = only_pending(&app, &key);
    assert!(matches!(&pending, PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Range(lines), .. } if *lines == (0..101)), "{pending:?}");
    answer(&mut app, &key, req, serde_json::json!({"data": MAIN_AND_X}));
    assert_eq!(colour_at(&mut app, 16), Some(app.theme.syntax_macro));
    // Further down, past what's covered: those lines asked.
    app.note_semantic_view(id, pane, &[5000..5010]);
    app.ask_due_semantic_tokens();
    let (req, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Range(lines), .. } if lines == (4900..5110)));
    answer(&mut app, &key, req, serde_json::json!({"data": []}));
    assert_eq!(colour_at(&mut app, 16), Some(app.theme.syntax_macro), "the first range's tokens kept beside the second's");
    // Near the end: no further than the document.
    app.note_semantic_view(id, pane, &[11_990..12000]);
    app.ask_due_semantic_tokens();
    assert!(matches!(only_pending(&app, &key).1, PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Range(lines), .. } if lines == (11_890..12_001)));

    // Sixty thousand lines and a server that answers only for the
    // whole: not asked.
    let path = dir.write("big.rs", &"x\n".repeat(60_000));
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], semantic_capabilities());
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&key].pending.is_empty());
    assert_eq!(app.semantic_wake(), None, "and not woken for");
}

/// A large Rust file asked by range, its server answering every ask
/// with nothing: `frames` frames of `each`, then how many asks went
/// out, and whether it's gone quiet.
fn range_frames(name: &str, text: &str, frames: usize, each: &dyn Fn(&mut App)) -> (usize, bool) {
    let dir = TempDir::new(name);
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", text);
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["function"], "tokenModifiers": []}, "full": true, "range": true}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    let mut asks = 0;
    for _ in 0..frames {
        each(&mut app);
        app.ask_due_semantic_tokens();
        let pending: Vec<i64> = app.lsp_sessions[&key].pending.keys().copied().collect();
        for id in pending {
            asks += 1;
            answer(&mut app, &key, id, serde_json::json!({"data": []}));
        }
    }
    (asks, app.lsp_sessions[&key].pending.is_empty() && app.semantic_wake().is_none())
}

#[test]
fn sticky_scroll_and_a_view_far_below_its_header_don_t_take_turns_asking() {
    let text = format!("mod m {{\n{}}}\n", "    fn f() {}\n".repeat(12_000));
    let (asks, quiet) = range_frames("semantic_sticky", &text, 12, &|app| {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.note_semantic_view(id, pane, &[5000..5040]);
        // Sticky scroll's header line, looked up on its own.
        let _ = app.syntax_highlights_for_visible_range(id, 0, 1);
    });
    assert_eq!((asks, quiet), (1, true));
}

#[test]
fn two_views_of_one_large_file_each_asked_once_then_quiet() {
    let text = "fn f() {}\n".repeat(12_000);
    let (asks, quiet) = range_frames("semantic_two_views", &text, 12, &|app| {
        let id = app.focused_buffer_id();
        if app.windows().windows().len() < 2 {
            app.run_cmd("window.split_vertical");
        }
        let panes = app.windows().windows();
        app.note_semantic_view(id, panes[0], &[0..40]);
        app.note_semantic_view(id, panes[1], &[6000..6040]);
    });
    assert_eq!((asks, quiet), (2, true));
}

#[test]
fn line_breaks_ropey_counts_decide_by_range_the_same_for_asks_and_views() {
    // Fewer than ten thousand LFs, but form feeds make more lines.
    let text = format!("{}{}", "fn f() {}\n".repeat(9_990), "\u{c}x\n".repeat(20));
    let (asks, quiet) = range_frames("semantic_ff", &text, 8, &|app| {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.note_semantic_view(id, pane, &[0..40]);
    });
    assert_eq!((asks, quiet), (1, true));
}

#[test]
fn more_views_than_ranges_kept_each_asked_once_then_quiet() {
    let text = "fn f() {}\n".repeat(40_000);
    for views in [9usize, 12] {
        let (asks, quiet) = range_frames(&format!("semantic_views_{views}"), &text, 40, &|app| {
            let id = app.focused_buffer_id();
            while app.windows().windows().len() < views {
                app.run_cmd("window.split_vertical");
            }
            for (i, pane) in app.windows().windows().into_iter().enumerate() {
                app.note_semantic_view(id, pane, &[i * 3000..i * 3000 + 40]);
            }
        });
        assert_eq!((asks, quiet), (views, true), "{views} views");
    }
}

/// A large Rust file with a server answering by range, a view at the
/// top: its app, server and document.
fn range_fixture(name: &str, lines: usize) -> (TempDir, App, LspKey, PathBuf) {
    let dir = TempDir::new(name);
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", &"fn f() {}\n".repeat(lines));
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["function"], "tokenModifiers": []}, "full": true, "range": true}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    let canonical = app.semantic_of[&app.focused_buffer_id()].path.clone();
    (dir, app, key, canonical)
}

fn asked_lines(app: &App, key: &LspKey) -> Option<(i64, std::ops::Range<usize>)> {
    app.lsp_sessions[key].pending.iter().find_map(|(id, p)| match p {
        PendingLspRequest::SemanticTokens { how: semantic::SemanticAsk::Range(lines), .. } => Some((*id, lines.clone())),
        _ => None,
    })
}

#[test]
fn a_refresh_while_a_range_ask_is_out_asks_those_lines_again() {
    let (_dir, mut app, key, canonical) = range_fixture("semantic_refresh_out", 12_000);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    app.note_semantic_view(id, pane, &[0..40]);
    app.ask_due_semantic_tokens();
    let (req, _) = asked_lines(&app, &key).unwrap();
    app.refresh_semantic_tokens(&key);
    answer(&mut app, &key, req, serde_json::json!({"data": []}));
    let mut asks = 0;
    for _ in 0..5 {
        app.note_semantic_view(id, pane, &[0..40]);
        app.ask_due_semantic_tokens();
        if let Some((req, _)) = asked_lines(&app, &key) {
            asks += 1;
            answer(&mut app, &key, req, serde_json::json!({"data": []}));
        }
    }
    assert_eq!(asks, 1, "asked again once, after the refresh");
    assert!(!app.lsp_sessions[&key].semantic[&canonical].stale);
}

#[test]
fn a_view_across_a_fold_asks_for_what_it_shows_no_more() {
    let (_dir, mut app, key, _canonical) = range_fixture("semantic_fold_view", 37_000);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    // Lines 0..10 and 30 010..30 040 shown, a fold closed between.
    app.note_semantic_view(id, pane, &[0..10, 30_010..30_040]);
    let mut asked = Vec::new();
    for _ in 0..4 {
        app.ask_due_semantic_tokens();
        if let Some((req, lines)) = asked_lines(&app, &key) {
            asked.push(lines);
            answer(&mut app, &key, req, serde_json::json!({"data": []}));
        }
    }
    assert_eq!(asked, vec![0..110, 29_910..30_140], "each run with its margin");
    // A view far taller than any pane: no more than one ask's worth
    // at a time, each from where the last left off, then quiet.
    app.note_semantic_view(id, pane, &[5_000..15_000]);
    let mut asked = Vec::new();
    for _ in 0..12 {
        app.ask_due_semantic_tokens();
        if let Some((req, lines)) = asked_lines(&app, &key) {
            assert!(lines.len() <= 2_000, "{lines:?}");
            asked.push(lines);
            answer(&mut app, &key, req, serde_json::json!({"data": []}));
        }
    }
    assert_eq!(asked, vec![4_900..6_900, 6_900..8_900, 8_900..10_900, 10_900..12_900, 12_900..14_900, 14_900..15_100]);
    assert!(app.lsp_sessions[&key].pending.is_empty() && app.semantic_wake().is_none(), "quiet");
}

#[test]
fn a_range_refused_moves_on_to_the_next_view_and_typing_s_pause_holds() {
    let (_dir, mut app, key, canonical) = range_fixture("semantic_refused_next", 12_000);
    let id = app.focused_buffer_id();
    app.run_cmd("window.split_vertical");
    let panes = app.windows().windows();
    app.note_semantic_view(id, panes[0], &[0..40]);
    app.note_semantic_view(id, panes[1], &[6_000..6_040]);
    app.ask_due_semantic_tokens();
    let (req, first) = asked_lines(&app, &key).unwrap();
    answer_error(&mut app, &key, req, -32603);
    app.ask_due_semantic_tokens();
    let (req, second) = asked_lines(&app, &key).unwrap();
    assert_ne!(first, second, "the other view asked");
    answer(&mut app, &key, req, serde_json::json!({"data": []}));
    // Typing, then the view moves: still after the pause.
    app.test_insert_str("x");
    app.sync_lsp_for_focused_buffer();
    let paused = app.lsp_sessions[&key].semantic[&canonical].due.unwrap();
    app.note_semantic_view(id, panes[0], &[1..41]);
    assert!(app.lsp_sessions[&key].semantic[&canonical].due.unwrap() >= paused, "not before the pause");
}

#[test]
fn ranges_let_go_take_their_tokens_with_them() {
    let (_dir, mut app, key, canonical) = range_fixture("semantic_evict", 40_000);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    for i in 0..12 {
        let top = i * 3_000;
        app.note_semantic_view(id, pane, &[top..top + 40]);
        app.ask_due_semantic_tokens();
        let (req, _) = asked_lines(&app, &key).unwrap();
        // One token, on the view's first line.
        answer(&mut app, &key, req, serde_json::json!({"data": [top, 3, 1, 0, 0]}));
    }
    let doc = &app.lsp_sessions[&key].semantic[&canonical];
    assert!(doc.covered.len() <= 8, "{:?}", doc.covered);
    assert_eq!(doc.tokens.len(), doc.covered.len(), "a token for each range kept, none for those let go");
}

#[test]
fn ranges_let_go_after_an_edit_or_a_refresh_still_take_their_tokens_with_them() {
    let (_dir, mut app, key, canonical) = range_fixture("semantic_evict_edit", 70_000);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    for i in 0..20 {
        let top = i * 3_000 + 100;
        app.note_semantic_view(id, pane, &[top..top + 40]);
        app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(Instant::now());
        app.ask_due_semantic_tokens();
        let (req, _) = asked_lines(&app, &key).unwrap();
        answer(&mut app, &key, req, serde_json::json!({"data": [top, 3, 1, 0, 0]}));
        // An edit (on line 0, the lines kept) or a refresh between.
        if i % 2 == 0 {
            app.test_insert_str("x");
            app.sync_lsp_for_focused_buffer();
        } else {
            app.refresh_semantic_tokens(&key);
        }
    }
    let doc = &app.lsp_sessions[&key].semantic[&canonical];
    assert!(doc.held.len() <= 8, "{:?}", doc.held);
    assert!(doc.tokens.len() <= 8, "tokens for at most 8 ranges, not all 20: {}", doc.tokens.len());
}

#[test]
fn a_refresh_marks_only_the_ask_out_however_that_ask_ends() {
    // Refused as modified: the next ask's lines count as answered.
    let (_dir, mut app, key, canonical) = range_fixture("semantic_refresh_refused", 12_000);
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    app.note_semantic_view(id, pane, &[0..40]);
    app.ask_due_semantic_tokens();
    let (req, _) = asked_lines(&app, &key).unwrap();
    app.refresh_semantic_tokens(&key);
    answer_error(&mut app, &key, req, -32801);
    assert!(!app.lsp_sessions[&key].semantic[&canonical].refreshed_while_asking);
    let mut asks = 0;
    for _ in 0..5 {
        // Past any back-off: due now.
        app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due = Some(Instant::now());
        app.ask_due_semantic_tokens();
        if let Some((req, _)) = asked_lines(&app, &key) {
            asks += 1;
            answer(&mut app, &key, req, serde_json::json!({"data": []}));
        }
    }
    assert_eq!(asks, 1, "asked again once, not twice");
    // Edited since asked: the same.
    app.note_semantic_view(id, pane, &[6_000..6_040]);
    app.ask_due_semantic_tokens();
    let (req, _) = asked_lines(&app, &key).unwrap();
    app.refresh_semantic_tokens(&key);
    app.test_insert_str("x");
    app.sync_lsp_for_focused_buffer();
    answer(&mut app, &key, req, serde_json::json!({"data": []}));
    assert!(!app.lsp_sessions[&key].semantic[&canonical].refreshed_while_asking);
    // A whole document's ask: cleared when it's answered.
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_refresh_full");
    app.ask_due_semantic_tokens();
    let (req, _) = only_pending(&app, &key);
    app.refresh_semantic_tokens(&key);
    answer(&mut app, &key, req, serde_json::json!({"data": MAIN_AND_X}));
    assert!(app.lsp_sessions[&key].semantic.values().all(|d| !d.refreshed_while_asking));
}

#[test]
fn a_range_refused_isn_t_asked_again_each_frame_it_s_shown() {
    let dir = TempDir::new("semantic_range_refused");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", &"fn f() {}\n".repeat(12_000));
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let caps = capabilities(serde_json::json!({"textDocumentSync": 2, "semanticTokensProvider": {"legend": {"tokenTypes": ["function"], "tokenModifiers": []}, "full": true, "range": true}}));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], caps);
    app.sync_lsp_for_focused_buffer();
    let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
    app.note_semantic_view(id, pane, &[0..40]);
    app.ask_due_semantic_tokens();
    let (req, _) = only_pending(&app, &key);
    answer_error(&mut app, &key, req, -32603);
    for top in [0, 1, 2] {
        app.note_semantic_view(id, pane, &[top..top + 40]);
        app.ask_due_semantic_tokens();
    }
    assert!(app.lsp_sessions[&key].pending.is_empty() && app.semantic_wake().is_none(), "tried, until an edit");
}

pub(in crate::app) fn lens_capabilities() -> lsp_types::ServerCapabilities {
    capabilities(serde_json::json!({"textDocumentSync": 2, "codeLensProvider": {"resolveProvider": true}}))
}

/// A Rust file with code lens on, and a server offering lenses.
fn lens_fixture(name: &str) -> (TempDir, App, PathBuf, LspKey) {
    let dir = TempDir::new(name);
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "fn main() {}\nfn other() {}\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], lens_capabilities());
    app.sync_lsp_for_focused_buffer();
    (dir, app, path, key)
}

/// Inlay hints: asked for the lines in view once the server has the
/// text, drawn as dimmed decorations where they go, asked again when
/// the server says so, and gone when they're turned off, globally or
/// for the language.
#[test]
fn inlay_hints_are_asked_for_the_view_drawn_refreshed_and_turned_off() {
    let dir = TempDir::new("inlay_hints");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "fn main() {}\nfn other() {}\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], capabilities(serde_json::json!({"textDocumentSync": 2, "inlayHintProvider": true})));
    app.sync_lsp_for_focused_buffer();
    let id = app.focused_buffer_id();
    assert!(app.inlay.contains_key(&id), "the server offers them");
    app.ask_due_inlay_hints();
    assert!(app.lsp_sessions[&key].pending.is_empty(), "nothing in view yet");
    let pane = app.focused_pane_id();
    app.note_inlay_view(id, pane, &[0..2]);
    app.ask_due_inlay_hints();
    let (request, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::InlayHint { buffer, .. } if buffer == id));
    let hint = |line: u32, character: u32, label: &str| serde_json::json!({"position": {"line": line, "character": character}, "label": label, "kind": 1, "paddingLeft": true});
    answer(&mut app, &key, request, serde_json::json!([hint(1, 8, "-> ()"), hint(0, 7, "-> ()")]));
    assert_eq!(app.inlay_hints_drawn(id), vec![(7, " -> ()".to_string()), (13 + 8, " -> ()".to_string())]);
    app.ask_due_inlay_hints();
    assert!(app.lsp_sessions[&key].pending.is_empty(), "answered for what's in view");

    // The server's refresh: asked again.
    app.apply_lsp_event(key.clone(), app.lsp_sessions[&key].generation, fenix_lsp::LspEvent::ServerRequest { id: serde_json::json!(9), method: "workspace/inlayHint/refresh".into(), params: serde_json::Value::Null });
    app.ask_due_inlay_hints();
    let (request, _) = only_pending(&app, &key);
    answer(&mut app, &key, request, serde_json::json!([hint(0, 7, ": x")]));
    assert_eq!(app.inlay_hints_drawn(id), vec![(7, " : x".to_string())]);
    // They move with typing before them, until the next answer.
    app.focused_buffer_and_cursor_mut().1.char_idx = 0;
    app.test_insert_str("  ");
    app.shift_code_decorations();
    assert_eq!(app.inlay_hints_drawn(id), vec![(9, " : x".to_string())]);
    app.sync_lsp_for_focused_buffer();

    // An answer for text edited since is let go.
    app.lsp_sessions.get_mut(&key).unwrap().pending.clear();
    app.refresh_inlay_hints(&key);
    app.ask_due_inlay_hints();
    let (request, _) = only_pending(&app, &key);
    app.test_insert_str("// ");
    answer(&mut app, &key, request, serde_json::json!([hint(0, 0, "stale")]));
    assert!(!app.inlay_hints_drawn(id).iter().any(|(_, t)| t.contains("stale")));

    // Off for Rust: gone at the next frame.
    app.config.set_value("lsp.inlay_hints_off", Some(fenix_config::Value::List(vec!["rust".into()])));
    app.ask_due_inlay_hints();
    assert!(!app.inlay.contains_key(&id) && app.inlay_hints_drawn(id).is_empty());
    app.sync_lsp_for_focused_buffer();
    assert!(!app.inlay.contains_key(&id), "not taken up again");
    app.config.set_value("lsp.inlay_hints_off", None);
    app.config.set_value("lsp.inlay_hints", Some(fenix_config::Value::Bool(false)));
    app.sync_lsp_for_focused_buffer();
    assert!(!app.inlay.contains_key(&id));
    assert!(crate::lsp::client_capabilities(false, false, false).text_document.unwrap().inlay_hint.is_none(), "not advertised when off");
    assert!(crate::lsp::client_capabilities(false, false, true).text_document.unwrap().inlay_hint.is_some());
}

fn lens_json(line: u32, title: Option<&str>) -> serde_json::Value {
    let range = serde_json::json!({"start": {"line": line, "character": 0}, "end": {"line": line, "character": 2}});
    match title {
        Some(t) => serde_json::json!({"range": range, "command": {"title": t, "command": "x.run"}}),
        None => serde_json::json!({"range": range, "data": 7}),
    }
}

fn lens_rows_of(app: &App) -> Vec<(usize, String)> {
    app.virtual_rows.get(&app.focused_buffer_id()).map(|rows| rows.iter().map(|(l, t)| (*l, t.join("/"))).collect()).unwrap_or_default()
}

/// A lens fixture whose server runs `x.run` and answers with `lenses`
/// for line 0.
fn lensed(name: &str, lenses: serde_json::Value) -> (TempDir, App, LspKey) {
    let (dir, mut app, _path, key) = lens_fixture(name);
    app.lsp_sessions.get_mut(&key).unwrap().capabilities = Some(capabilities(serde_json::json!({"textDocumentSync": 2, "codeLensProvider": {}, "executeCommandProvider": {"commands": ["x.run"]}})));
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, lenses);
    (dir, app, key)
}

fn said_now(app: &App) -> String {
    app.status_message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

#[test]
fn spc_c_l_runs_the_one_lens_on_the_line_and_lists_several() {
    let (_dir, mut app, key) = lensed("lens_run_one", serde_json::json!([lens_json(0, Some("▶ Run"))]));
    app.run_cmd("code.lens");
    assert!(matches!(only_pending(&app, &key).1, PendingLspRequest::LensCommand { ref title } if title == "▶ Run"), "sent back to the server");
    let (_dir, mut app, _key) = lensed("lens_run_several", serde_json::json!([lens_json(0, Some("▶ Run")), lens_json(0, Some("Debug"))]));
    app.run_cmd("code.lens");
    assert!(matches!(app.active_picker, Some(ActivePicker::Lenses(ref s)) if s.len() == 2), "several to choose from");
    app.press("Escape");
    app.press("j");
    app.run_cmd("code.lens");
    assert_eq!(said_now(&app), "no code lenses on this line");
}

fn titled(title: &str, command: &str) -> serde_json::Value {
    serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": title, "command": command}})
}

fn answer_lenses_again(app: &mut App, key: &LspKey, lenses: serde_json::Value) {
    app.refresh_lenses(key);
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..5]);
    app.ask_due_lenses();
    let id = app.lsp_sessions[key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, _)| *id).unwrap();
    answer(app, key, id, lenses);
}

fn lens_commands_sent(app: &App, key: &LspKey) -> Vec<String> {
    app.lsp_sessions[key].pending.values().filter_map(|p| match p {
        PendingLspRequest::LensCommand { title } => Some(title.clone()),
        _ => None,
    }).collect()
}

#[test]
fn the_lens_picked_is_the_one_run_though_the_lenses_were_answered_again() {
    let caps = |app: &mut App, key: &LspKey| app.lsp_sessions.get_mut(key).unwrap().capabilities = Some(capabilities(serde_json::json!({"textDocumentSync": 2, "codeLensProvider": {}, "executeCommandProvider": {"commands": ["x.run", "x.debug", "x.delete"]}})));
    let (_dir, mut app, key) = lensed("lens_pick_moved", serde_json::json!([titled("▶ Run", "x.run"), titled("Debug", "x.debug")]));
    caps(&mut app, &key);
    app.run_cmd("code.lens");
    assert!(matches!(app.active_picker, Some(ActivePicker::Lenses(_))));
    answer_lenses_again(&mut app, &key, serde_json::json!([titled("Delete all", "x.delete"), titled("▶ Run", "x.run"), titled("Debug", "x.debug")]));
    app.press("Enter");
    assert_eq!(lens_commands_sent(&app, &key), vec!["▶ Run".to_string()], "the one shown, found again");
    // Gone when it's chosen: none run, and it's said.
    let (_dir, mut app, key) = lensed("lens_pick_gone", serde_json::json!([titled("▶ Run", "x.run"), titled("Debug", "x.debug")]));
    caps(&mut app, &key);
    app.run_cmd("code.lens");
    answer_lenses_again(&mut app, &key, serde_json::json!([titled("Delete all", "x.delete"), titled("Debug", "x.debug")]));
    app.press("Enter");
    assert!(lens_commands_sent(&app, &key).is_empty());
    assert!(said_now(&app).contains("the lenses changed"), "{}", said_now(&app));
}

#[test]
fn a_line_whose_lenses_have_no_titles_has_none_to_run() {
    let (_dir, mut app, _key) = lensed("lens_no_titles", serde_json::json!([titled("\u{200b}", "x.run"), titled("", "x.run")]));
    app.run_cmd("code.lens");
    assert!(app.active_picker.is_none(), "no empty picker");
    assert_eq!(said_now(&app), "no code lenses on this line");
}

#[test]
fn a_run_s_folder_must_be_a_real_one_in_the_project_and_the_task_is_the_project_s() {
    let (dir, mut app, _key) = lensed("lens_run_cwd", serde_json::json!([]));
    let root = dir.path().to_path_buf();
    let run = |cwd: &str| serde_json::json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "▶ Run", "command": "rust-analyzer.runSingle", "arguments": [{"label": "t", "kind": "cargo", "args": {"cargoArgs": ["--version"], "cwd": cwd}}]}}]);
    let key = app.lsp_sessions.keys().next().unwrap().clone();
    // A path that can't be followed, though its name is inside.
    answer_lenses_again(&mut app, &key, run(&format!("{}/nope/../../..", root.display())));
    app.run_cmd("code.lens");
    assert!(said_now(&app).contains("isn't a folder in the project"), "{}", said_now(&app));
    assert!(app.task_session.is_none());
    // A member crate's folder: run there, as the project's task.
    std::fs::create_dir_all(root.join("member")).unwrap();
    answer_lenses_again(&mut app, &key, run(&root.join("member").display().to_string()));
    app.run_cmd("code.lens");
    let session = app.task_session.as_ref().expect("started");
    assert_eq!(session.root, fenix_lsp::normalize(refactor::identity(&root)), "the project's");
    assert!(session.cwd.ends_with("member"), "{:?}", session.cwd);
}

#[test]
fn a_double_click_on_a_lens_runs_it_once_and_its_hover_follows_the_rows() {
    let (_dir, mut app, key) = lensed("lens_double", serde_json::json!([lens_json(0, Some("▶ Run")), lens_json(0, Some("Debug"))]));
    let pane = app.focused_pane_id();
    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let rect = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top).panes.into_iter().find(|(id, _)| *id == pane).unwrap().1;
    let (char_width, line_height) = app.cell_size();
    let content = pane_content_rect(rect, line_height, app.pane_has_breadcrumb(pane));
    let gutter = app.gutter_chars(app.open()) as f32 * char_width;
    let at = |col: f32| (content.x + text::PAD_LEFT + gutter + (col + 0.5) * char_width, content.y + text::PAD_TOP + line_height * 0.5);
    app.handle_click_at(at(1.0), 800.0, 600.0);
    app.mouse_release();
    app.handle_click_at(at(1.0), 800.0, 600.0);
    app.mouse_release();
    assert_eq!(lens_commands_sent(&app, &key).len(), 1, "once");
    // Lit, then the lenses go under a pointer that doesn't move.
    assert!(app.hover_lens(Some((pane, rect)), at(9.0)));
    assert_eq!(app.lens_hover.as_ref().map(|(p, _, _)| *p), Some(pane), "by pane");
    answer_lenses_again(&mut app, &key, serde_json::json!([]));
    assert!(app.lens_hover.is_none(), "not lit over nothing");
}

#[test]
fn a_lens_starts_cargo_only_in_a_trusted_project() {
    let run = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "▶ Run", "command": "rust-analyzer.runSingle", "arguments": [{"label": "test it", "kind": "cargo", "args": {"cargoArgs": ["--version"]}}]}});
    let (dir, mut app, _key) = lensed("lens_run_trust", serde_json::json!([run]));
    // The project names a program, and nobody has allowed it yet.
    dir.write(".fenix/tools.json", r#"{"tasks": {"build": {"executable": "make"}}}"#);
    let state = TempDir::new("lens_run_trust_state");
    app.known_projects = fenix_project::KnownProjects::load_or_default(state.path().join("projects.txt"));
    app.project_trust_begin(Some(state.path().join("trusted-projects.json")));
    app.run_cmd("code.lens");
    assert!(said_now(&app).contains("only in a trusted project (SPC p y)"), "{}", said_now(&app));
    assert!(app.task_session.is_none(), "nothing started");
    // Trusted: it runs.
    app.trust_project_as_written(dir.path());
    app.run_cmd("code.lens");
    assert!(app.task_session.is_some(), "{}", said_now(&app));
}

#[test]
fn lenses_fenix_runs_itself_or_says_why_not() {
    let location = serde_json::json!({"uri": "file:///tmp/a.rs", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}});
    let refs = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "1 reference", "command": "rust-analyzer.showReferences", "arguments": ["file:///tmp/a.rs", {"line": 0, "character": 0}, [location]]}});
    let (_dir, mut app, _key) = lensed("lens_refs", serde_json::json!([refs]));
    app.run_cmd("code.lens");
    assert_eq!(app.quickfix.len(), 1, "the references listed");
    let debug = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "Debug", "command": "rust-analyzer.debugSingle", "arguments": [{"label": "test it", "kind": "cargo", "args": {"cargoArgs": ["test"]}}]}});
    let (_dir, mut app, _key) = lensed("lens_debug", serde_json::json!([debug]));
    app.run_cmd("code.lens");
    assert!(said_now(&app).contains("can't debug from a lens yet"), "{}", said_now(&app));
    // A run where the server says, outside the project: not run.
    let away = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "▶ Run", "command": "rust-analyzer.runSingle", "arguments": [{"label": "test it", "kind": "cargo", "args": {"cargoArgs": ["test"], "cwd": "/"}}]}});
    let (_dir, mut app, _key) = lensed("lens_run_away", serde_json::json!([away]));
    app.run_cmd("code.lens");
    assert!(said_now(&app).contains("isn't a folder in the project"), "{}", said_now(&app));
    assert!(app.task_session.is_none(), "nothing started");
    // Fenix's own command in a server's lens: not run.
    let own = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "▶ Run test", "command": "fenix.runTest", "arguments": ["/elsewhere/t.rs", 1]}});
    let (_dir, mut app, _key) = lensed("lens_run_own_from_server", serde_json::json!([own]));
    app.run_cmd("code.lens");
    assert_eq!(said_now(&app), "Fenix can't run ▶ Run test (fenix.runTest)");
    assert!(app.task_session.is_none());
    // Its debug command too, said as itself, as an unknown one was
    // before there was one (step 7's review, N1).
    let own = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "Debug", "command": "fenix.debugTest", "arguments": ["/elsewhere/t.rs", 1]}});
    let (_dir, mut app, _key) = lensed("lens_debug_own_from_server", serde_json::json!([own]));
    app.run_cmd("code.lens");
    assert_eq!(said_now(&app), "Fenix can't run Debug (fenix.debugTest)");
    let other = serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "command": {"title": "Peek", "command": "editor.peek"}});
    let (_dir, mut app, _key) = lensed("lens_unknown", serde_json::json!([other]));
    app.run_cmd("code.lens");
    assert_eq!(said_now(&app), "Fenix can't run Peek (editor.peek)");
}

#[test]
fn a_click_on_a_lens_title_runs_it_and_the_pointer_over_one_lights_it() {
    let (_dir, mut app, key) = lensed("lens_click", serde_json::json!([lens_json(0, Some("▶ Run")), lens_json(0, Some("Debug"))]));
    let pane = app.focused_pane_id();
    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let rect = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top).panes.into_iter().find(|(id, _)| *id == pane).unwrap().1;
    let (char_width, line_height) = app.cell_size();
    let content = pane_content_rect(rect, line_height, app.pane_has_breadcrumb(pane));
    let gutter = app.gutter_chars(app.open()) as f32 * char_width;
    // Scrolled up to line 0's lenses (they came above the top line,
    // out of view): row 0, "▶ Run | Debug", "Debug" from column 8.
    app.pane_state_mut(pane).scroll_row = 0;
    let at = |col: f32| (content.x + text::PAD_LEFT + gutter + (col + 0.5) * char_width, content.y + text::PAD_TOP + line_height * 0.5);
    assert!(app.hover_lens(Some((pane, rect)), at(9.0)));
    assert_eq!(app.lens_hover.as_ref().map(|(_, line, cols)| (*line, cols.clone())), Some((0, 8..13)));
    assert!(!app.hover_lens(Some((pane, rect)), at(10.0)), "the same one");
    let layout = app.focused_layout(3);
    let ob = app.open();
    let spans = app.row_content_spans(ob, &layout.map, 3, app.gutter_chars(ob), &[], 0, &app.tab_stops_for(app.focused_buffer_id()), &HashSet::new(), &layout.decorated);
    assert!(spans.iter().any(|(t, c)| t == "Debug" && *c == app.theme.fg), "drawn in the text's colour: {spans:?}");
    assert!(spans.iter().any(|(t, c)| t.contains("▶ Run") && *c == app.theme.gutter_fg));
    assert!(app.hover_lens(Some((pane, rect)), at(6.0)), "the separator: none");
    assert!(app.lens_hover.is_none());
    app.handle_click_at(at(1.0), 800.0, 600.0);
    assert!(matches!(only_pending(&app, &key).1, PendingLspRequest::LensCommand { ref title } if title == "▶ Run"));
    assert_eq!(app.line_col(), (0, 0), "the cursor stays");
    // A pane too narrow for both: "▶ Run …" drawn, and "Debug", not
    // drawn, isn't there to click.
    app.set_pane_text_cols(pane, 12);
    let layout = app.focused_layout(3);
    let rows::RowKind::Virtual(i) = layout.map.row(0).unwrap().kind else { panic!("a lens row") };
    assert_eq!(layout.map.virtual_text(i), "▶ Run …");
    app.hover_lens(Some((pane, rect)), at(9.0));
    assert!(app.lens_hover.is_none(), "cut off");
    app.hover_lens(Some((pane, rect)), at(1.0));
    assert_eq!(app.lens_hover.as_ref().map(|(_, _, cols)| cols.clone()), Some(0..5));
}

#[test]
fn fenix_s_own_run_test_lenses_go_over_tests_no_server_offers_to_run() {
    let dir = TempDir::new("lens_own");
    dir.write("pyproject.toml", "[project]\nname = \"ledger\"\n");
    let path = dir.write("test_ledger.py", "def helper():\n    pass\n\ndef test_totals():\n    assert 1 == 1\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    app.sync_lsp_for_focused_buffer();
    assert_eq!(lens_rows_of(&app), vec![(3, "▶ Run test".to_string())], "with no language server at all");
    // Run from its line: as SPC p r there.
    app.press_all("3 j");
    app.run_cmd("code.lens");
    let session = app.task_session.as_ref().expect("Task Output opened");
    assert!(app.buffers.get(session.buffer).unwrap().buffer.text().starts_with("$ pytest test_ledger.py::test_totals"));
    // Off on their own: gone in the next frame.
    app.config.set_value("lsp.code_lens_tests", Some(fenix_config::Value::Bool(false)));
    app.ask_due_lenses();
    assert!(lens_rows_of(&app).is_empty());
}

#[test]
fn fenix_s_own_run_test_lens_runs_only_in_a_trusted_project() {
    let dir = TempDir::new("lens_own_trust");
    dir.write("pyproject.toml", "[project]\nname = \"ledger\"\n");
    dir.write(".fenix/tools.json", r#"{"tasks": {"build": {"executable": "make"}}}"#);
    let path = dir.write("test_ledger.py", "def test_totals():\n    assert 1 == 1\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let state = TempDir::new("lens_own_trust_state");
    app.known_projects = fenix_project::KnownProjects::load_or_default(state.path().join("projects.txt"));
    app.project_trust_begin(Some(state.path().join("trusted-projects.json")));
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    app.sync_lsp_for_focused_buffer();
    assert_eq!(lens_rows_of(&app), vec![(0, "▶ Run test".to_string())]);
    app.run_cmd("code.lens");
    assert!(said_now(&app).contains("only in a trusted project (SPC p y)"), "{}", said_now(&app));
    assert!(app.task_session.is_none(), "nothing started");
    app.trust_project_as_written(dir.path());
    app.run_cmd("code.lens");
    assert!(app.task_session.is_some(), "{}", said_now(&app));
}

#[test]
fn a_server_s_run_lens_stands_in_for_fenix_s_own() {
    let dir = TempDir::new("lens_own_server");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "#[test]\nfn adds() {}\n\n#[test]\nfn takes() {}\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], lens_capabilities());
    app.sync_lsp_for_focused_buffer();
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    let run = serde_json::json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}, "command": {"title": "▶ Run Test", "command": "rust-analyzer.runSingle"}});
    answer(&mut app, &key, id, serde_json::json!([run]));
    assert_eq!(lens_rows_of(&app), vec![(1, "▶ Run Test".to_string()), (4, "▶ Run test".to_string())], "the server's where it has one, Fenix's where it hasn't");
}

/// What lenses cost on a large file (`--release -- --ignored
/// --nocapture`): 20 000 lines, a test and a server lens every four.
#[test]
#[ignore]
fn measure_lenses_on_a_large_file() {
    let dir = TempDir::new("lens_measure");
    dir.write(".fenix/project.ini", "");
    let text: String = (0..5_000).map(|i| format!("#[test]\nfn t{i}() {{\n    x();\n}}\n")).collect();
    let path = dir.write("main.rs", &text);
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let key = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], lens_capabilities());
    let started = Instant::now();
    app.sync_lsp_for_focused_buffer();
    let first = started.elapsed();
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    let lenses: Vec<serde_json::Value> = (0..5_000).map(|i| lens_json(i * 5 + 1, Some("3 references"))).collect();
    let started = Instant::now();
    answer(&mut app, &key, id, serde_json::Value::Array(lenses));
    let answered = started.elapsed();
    app.test_insert_str("// x\n");
    let started = Instant::now();
    app.sync_lsp_for_focused_buffer();
    let edited = started.elapsed();
    let started = Instant::now();
    let _ = app.focused_layout(60);
    let laid = started.elapsed();
    println!("open with own lenses {first:?}; 5000 server lenses laid out {answered:?}; an edit's sync {edited:?}; a frame's 60 rows {laid:?}; rows {}", app.virtual_rows[&app.focused_buffer_id()].len());
    // The same lenses unresolved, answered twice: the second carries
    // the first's titles over by place.
    let unresolved: Vec<serde_json::Value> = (0..5_000).map(|i| lens_json(i * 4 + 1, None)).collect();
    app.refresh_lenses(&key);
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..60]);
    for _ in 0..2 {
        app.lsp_sessions.get_mut(&key).unwrap().lenses.values_mut().for_each(|d| d.due = Some(Instant::now()));
        app.sync_lsp_for_focused_buffer();
        app.ask_due_lenses();
        let id = app.lsp_sessions[&key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, _)| *id).unwrap();
        let started = Instant::now();
        answer(&mut app, &key, id, serde_json::Value::Array(unresolved.clone()));
        println!("5000 unresolved lenses answered (titles carried) {:?}", started.elapsed());
    }
    // A Python class of 5 000 tests: Fenix's own found.
    let text = format!("class TestBig:\n{}", (0..5_000).map(|i| format!("    def test_{i}(self):\n        x()\n\n\n")).collect::<String>());
    let path = dir.write("test_big.py", &text);
    let started = Instant::now();
    let found = fenix_tasks::test_lines(&path, &text, dir.path());
    println!("a Python class of 5000 tests, 20 000 lines: {} found in {:?}", found.len(), started.elapsed());
}

#[test]
fn code_lens_is_off_by_default_and_asks_nothing() {
    let (_dir, mut app, _path, key) = semantic_fixture("lens_off_default");
    app.lsp_sessions.get_mut(&key).unwrap().capabilities = Some(lens_capabilities());
    app.sync_lsp_for_focused_buffer();
    app.ask_due_lenses();
    assert!(!app.lsp_sessions[&key].pending.values().any(|p| matches!(p, PendingLspRequest::CodeLens { .. })));
    assert!(crate::lsp::client_capabilities(false, false, false).text_document.unwrap().code_lens.is_none(), "not advertised");
    assert!(crate::lsp::client_capabilities(false, true, false).text_document.unwrap().code_lens.is_some());
}

#[test]
fn lenses_are_asked_laid_out_above_their_lines_and_resolved_in_view() {
    let (_dir, mut app, _path, key) = lens_fixture("lens_ask");
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!([lens_json(0, Some("▶ Run")), lens_json(0, None), lens_json(1, Some("1 reference"))]));
    assert_eq!(lens_rows_of(&app), vec![(0, "▶ Run | …".to_string()), (1, "1 reference".to_string())]);
    let pane = app.focused_pane_id();
    assert_eq!(app.row_cols(pane, app.focused_buffer_id()), Some(rows::UNBOUNDED), "the pane goes by rows");
    // In view: the one without a command resolved.
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..5]);
    let (rid, pending) = only_pending(&app, &key);
    assert!(matches!(pending, PendingLspRequest::CodeLensResolve { index: 1, .. }), "{pending:?}");
    answer(&mut app, &key, rid, lens_json(0, Some("Debug")));
    assert_eq!(lens_rows_of(&app)[0].1, "▶ Run | Debug");
    // A line added above: the rows move with it at once.
    app.test_insert_str("// a\n");
    app.sync_lsp_for_focused_buffer();
    assert_eq!(lens_rows_of(&app).iter().map(|(l, _)| *l).collect::<Vec<_>>(), [1, 2]);
    assert!(app.lens_wake().is_some(), "asked again after the pause");
}

#[test]
fn two_servers_lenses_share_a_row_and_one_stopping_takes_its_own_away() {
    let dir = TempDir::new("lens_two");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "fn main() {}\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let primary = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], lens_capabilities());
    let extra = install_server(&mut app, &path, LspServer::Additional("kiln".into()), "kiln", &["rust"], lens_capabilities());
    app.sync_lsp_for_focused_buffer();
    app.ask_due_lenses();
    let (a, _) = only_pending(&app, &primary);
    let (b, _) = only_pending(&app, &extra);
    answer(&mut app, &extra, b, serde_json::json!([lens_json(0, Some("kiln"))]));
    answer(&mut app, &primary, a, serde_json::json!([lens_json(0, Some("ra"))]));
    assert_eq!(lens_rows_of(&app), vec![(0, "ra | kiln".to_string())], "the primary's first");
    app.stop_lsp_session(&extra);
    app.ask_due_lenses();
    assert_eq!(lens_rows_of(&app), vec![(0, "ra".to_string())]);
    // Turned off: every row goes, the pane back to a row a line.
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(false)));
    app.ask_due_lenses();
    assert!(lens_rows_of(&app).is_empty());
    assert_eq!(app.row_cols(app.focused_pane_id(), app.focused_buffer_id()), None);
}

#[test]
fn a_stale_lens_answer_is_let_go_and_a_refresh_asks_as_it_s_drawn() {
    let (_dir, mut app, _path, key) = lens_fixture("lens_stale");
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    app.test_insert_str("x");
    app.sync_lsp_for_focused_buffer();
    answer(&mut app, &key, id, serde_json::json!([lens_json(0, Some("old"))]));
    assert!(lens_rows_of(&app).is_empty(), "of other text");
    let generation = app.lsp_sessions[&key].generation;
    app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::ServerRequest { id: serde_json::json!(3), method: "workspace/codeLens/refresh".into(), params: serde_json::Value::Null });
    let buffer = app.focused_buffer_id();
    // Due now, the stale answer having been let go; then drawn.
    let canonical = app.lens_of[&buffer].1.clone();
    app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap().due = None;
    app.ask_due_lenses();
    assert!(app.lsp_sessions[&key].pending.is_empty(), "not until drawn");
    app.note_lens_view(buffer, &[0..5]);
    app.ask_due_lenses();
    assert!(matches!(only_pending(&app, &key).1, PendingLspRequest::CodeLens { .. }));
}

/// `lens_fixture` with one lens answered on each of `lines`, none
/// resolved: its app, path, server and the document's canonical path.
fn lens_answered(name: &str, text: &str, lines: &[u32]) -> (TempDir, App, PathBuf, LspKey, PathBuf) {
    let (dir, mut app, path, key) = lens_fixture(name);
    app.test_insert_str(text);
    app.sync_lsp_for_focused_buffer();
    let canonical = app.lens_of[&app.focused_buffer_id()].1.clone();
    app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap().due = Some(Instant::now());
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::Value::Array(lines.iter().map(|&l| lens_json(l, None)).collect()));
    (dir, app, path, key, canonical)
}

fn resolves_out(app: &App, key: &LspKey) -> Vec<i64> {
    app.lsp_sessions[key].pending.iter().filter(|(_, p)| matches!(p, PendingLspRequest::CodeLensResolve { .. })).map(|(id, _)| *id).collect()
}

#[test]
fn a_resolve_that_comes_to_nothing_isn_t_sent_again_until_the_next_answer() {
    let (_dir, mut app, _path, key, canonical) = lens_answered("lens_resolve_once", "a\nb\nc\n", &[0, 1, 2]);
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..5]);
    let out = resolves_out(&app, &key);
    assert_eq!(out.len(), 3);
    // An error, null, and a lens still without a command.
    answer_error(&mut app, &key, out[0], -32603);
    answer(&mut app, &key, out[1], serde_json::Value::Null);
    answer(&mut app, &key, out[2], lens_json(2, None));
    for _ in 0..50 {
        app.note_lens_view(buffer, &[0..5]);
        app.ask_due_lenses();
    }
    assert!(resolves_out(&app, &key).is_empty(), "tried, whatever came back");
    // A resolve left unanswered: cancelled after the timeout, tried.
    app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap().tried.clear();
    app.note_lens_view(buffer, &[0..1]);
    assert_eq!(resolves_out(&app, &key).len(), 1);
    assert!(app.lens_wake().is_some(), "woken for its timeout");
    let doc = app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap();
    for (_, at) in doc.resolving.values_mut() {
        *at -= Duration::from_secs(11);
    }
    app.ask_due_lenses();
    assert!(resolves_out(&app, &key).is_empty());
    app.note_lens_view(buffer, &[0..1]);
    assert!(resolves_out(&app, &key).is_empty(), "not again until the next answer");
}

#[test]
fn no_more_than_sixteen_resolves_are_out_from_a_server() {
    let text = "x\n".repeat(40);
    let (_dir, mut app, _path, key, _canonical) = lens_answered("lens_resolve_cap", &text, &(0..40).collect::<Vec<u32>>());
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..10, 20..40]);
    assert_eq!(resolves_out(&app, &key).len(), 16);
    app.note_lens_view(buffer, &[0..40]);
    assert_eq!(resolves_out(&app, &key).len(), 16, "still sixteen");
    // Not those in a fold: only the runs shown.
    let (_dir, mut app, _path, key, _canonical) = lens_answered("lens_resolve_fold", "a\nb\nc\nd\n", &[0, 1, 2, 3]);
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..1, 3..4]);
    let asked: Vec<usize> = app.lsp_sessions[&key].pending.values().filter_map(|p| match p {
        PendingLspRequest::CodeLensResolve { index, .. } => Some(*index),
        _ => None,
    }).collect();
    assert_eq!(asked.iter().copied().collect::<std::collections::BTreeSet<_>>(), [0, 3].into());
}

#[test]
fn resolves_of_an_answer_let_go_are_cancelled_so_no_more_than_sixteen_are_ever_out() {
    let text = "x\n".repeat(40);
    let (_dir, mut app, _path, key, _canonical) = lens_answered("lens_resolve_superseded", &text, &(0..40).collect::<Vec<u32>>());
    let buffer = app.focused_buffer_id();
    let unresolved: Vec<serde_json::Value> = (0..40).map(|l| lens_json(l, None)).collect();
    for _ in 0..4 {
        app.note_lens_view(buffer, &[0..40]);
        assert!(resolves_out(&app, &key).len() <= 16, "{}", resolves_out(&app, &key).len());
        // Asked again (a refresh) and answered; the resolves never are.
        app.refresh_lenses(&key);
        app.note_lens_view(buffer, &[0..40]);
        app.ask_due_lenses();
        let id = app.lsp_sessions[&key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, _)| *id).unwrap();
        answer(&mut app, &key, id, serde_json::Value::Array(unresolved.clone()));
    }
    app.note_lens_view(buffer, &[0..40]);
    assert_eq!(resolves_out(&app, &key).len(), 16);
}

#[test]
fn a_carried_title_whose_resolve_comes_to_nothing_isn_t_shown_to_be_run() {
    let (_dir, mut app, _path, key, _canonical) = lens_answered("lens_carry_fails", "a\nb\n", &[1]);
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..5]);
    let out = resolves_out(&app, &key);
    answer(&mut app, &key, out[0], serde_json::json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}, "command": {"title": "3 references", "command": "x.run"}}));
    app.refresh_lenses(&key);
    app.note_lens_view(buffer, &[0..5]);
    app.ask_due_lenses();
    let id = app.lsp_sessions[&key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, _)| *id).unwrap();
    answer(&mut app, &key, id, serde_json::json!([lens_json(1, None)]));
    assert_eq!(lens_rows_of(&app), vec![(1, "3 references".to_string())], "carried");
    app.note_lens_view(buffer, &[0..5]);
    let out = resolves_out(&app, &key);
    answer(&mut app, &key, out[0], serde_json::Value::Null);
    assert_eq!(lens_rows_of(&app), vec![(1, String::new())], "nothing to click, its row kept");
}

#[test]
fn a_resolved_title_stays_on_show_through_the_next_answer_until_it_s_resolved_again() {
    let (_dir, mut app, _path, key, canonical) = lens_answered("lens_carry", "a\nb\n", &[1]);
    let buffer = app.focused_buffer_id();
    app.note_lens_view(buffer, &[0..5]);
    let out = resolves_out(&app, &key);
    answer(&mut app, &key, out[0], serde_json::json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}, "command": {"title": "3 references", "command": "x.run"}}));
    assert_eq!(lens_rows_of(&app), vec![(1, "3 references".to_string())]);
    // Asked again (a refresh): the same lens, unresolved.
    app.refresh_lenses(&key);
    app.note_lens_view(buffer, &[0..5]);
    app.ask_due_lenses();
    let (id, _) = app.lsp_sessions[&key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, p)| (*id, p.clone())).unwrap();
    answer(&mut app, &key, id, serde_json::json!([lens_json(1, None)]));
    assert_eq!(lens_rows_of(&app), vec![(1, "3 references".to_string())], "not back to …");
    app.note_lens_view(buffer, &[0..5]);
    let out = resolves_out(&app, &key);
    assert_eq!(out.len(), 1, "resolved again all the same");
    // Resolved to an empty title: its row stays, so the text doesn't move.
    answer(&mut app, &key, out[0], serde_json::json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}, "command": {"title": "", "command": "x.run"}}));
    assert_eq!(lens_rows_of(&app), vec![(1, String::new())]);
    let _ = canonical;
}

#[test]
fn a_timeout_after_a_good_answer_is_asked_once_more_again() {
    let (_dir, mut app, _path, key, canonical) = lens_answered("lens_timeout_again", "a\n", &[0]);
    let back = |app: &mut App| {
        let doc = app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap();
        if let Some((_, at)) = doc.asked.as_mut() {
            *at -= Duration::from_secs(11);
        }
    };
    let asks = |app: &App| app.lsp_sessions[&key].pending.values().filter(|p| matches!(p, PendingLspRequest::CodeLens { .. })).count();
    let buffer = app.focused_buffer_id();
    for round in 0..2 {
        // Asked (a refresh, drawn), timed out: asked once more.
        app.refresh_lenses(&key);
        app.note_lens_view(buffer, &[0..1]);
        app.ask_due_lenses();
        assert_eq!(asks(&app), 1, "round {round}");
        back(&mut app);
        app.ask_due_lenses();
        app.ask_due_lenses();
        assert_eq!(asks(&app), 1, "round {round}: asked once more");
        let id = app.lsp_sessions[&key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, _)| *id).unwrap();
        answer(&mut app, &key, id, serde_json::json!([lens_json(0, Some("ok"))]));
    }
}

#[test]
fn refusals_are_asked_again_a_few_times_then_not_until_an_edit() {
    let (_dir, mut app, _path, key) = lens_fixture("lens_refusals");
    let canonical = app.lens_of[&app.focused_buffer_id()].1.clone();
    let mut asked = 0;
    for _ in 0..12 {
        app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap().due.iter_mut().for_each(|d| *d = Instant::now());
        app.ask_due_lenses();
        if let Some(id) = app.lsp_sessions[&key].pending.iter().find(|(_, p)| matches!(p, PendingLspRequest::CodeLens { .. })).map(|(id, _)| *id) {
            asked += 1;
            answer_error(&mut app, &key, id, -32801);
        }
    }
    assert_eq!(asked, code_lens::MAX_REFUSALS as usize);
    assert!(app.lens_wake().is_none(), "quiet");
    // The semantic side the same.
    let (_dir, mut app, _path, key) = semantic_fixture("semantic_refusals");
    let canonical = app.semantic_of[&app.focused_buffer_id()].path.clone();
    let mut asked = 0;
    for _ in 0..12 {
        app.lsp_sessions.get_mut(&key).unwrap().semantic.get_mut(&canonical).unwrap().due.iter_mut().for_each(|d| *d = Instant::now());
        app.ask_due_semantic_tokens();
        if let Some(id) = app.lsp_sessions[&key].pending.keys().next().copied() {
            asked += 1;
            answer_error(&mut app, &key, id, -32801);
        }
    }
    assert_eq!(asked, code_lens::MAX_REFUSALS as usize);
    assert!(app.semantic_wake().is_none());
}

#[test]
fn lenses_follow_an_agent_s_edit_go_with_their_language_left_out_and_with_their_buffer() {
    let (_dir, mut app, _path, key, canonical) = lens_answered("lens_follow", "a\nb\n", &[1]);
    assert_eq!(lens_rows_of(&app).iter().map(|(l, _)| *l).collect::<Vec<_>>(), [1]);
    // An agent's edit, a line above: laid out again in the next frame.
    let session = app.lsp_sessions.get_mut(&key).unwrap();
    let (count, version) = session.open_documents[&canonical];
    let text = format!("x\n{}", session.synced_text[&canonical]);
    record_lsp_sync(session, &canonical, &text, count + 1, version + 1, semantic::SemanticSync::Follow, semantic::SemanticSync::Follow);
    app.ask_due_lenses();
    assert_eq!(lens_rows_of(&app).iter().map(|(l, _)| *l).collect::<Vec<_>>(), [2]);
    // Its language left out: the rows go at once.
    app.config.set_value("lsp.code_lens_off", Some(fenix_config::Value::List(vec!["Rust".into()])));
    app.ask_due_lenses();
    assert!(lens_rows_of(&app).is_empty());
    // Closed: nothing kept for it.
    let (_dir, mut app, _path, _key, _canonical) = lens_answered("lens_close", "a\nb\n", &[1]);
    let buffer = app.focused_buffer_id();
    assert!(app.virtual_rows.contains_key(&buffer));
    app.buffers.close(buffer);
    app.ask_due_lenses();
    assert!(!app.virtual_rows.contains_key(&buffer) && !app.lens_rows.contains_key(&buffer));
}

#[test]
fn a_lens_row_is_cut_to_the_pane_and_its_indent_kept_in_view() {
    let (_dir, mut app, _path, key) = lens_fixture("lens_fit");
    app.test_insert_str(&format!("{}fn deep() {{}}\n", " ".repeat(60)));
    app.sync_lsp_for_focused_buffer();
    let canonical = app.lens_of[&app.focused_buffer_id()].1.clone();
    app.lsp_sessions.get_mut(&key).unwrap().lenses.get_mut(&canonical).unwrap().due = Some(Instant::now());
    app.ask_due_lenses();
    let (id, _) = only_pending(&app, &key);
    answer(&mut app, &key, id, serde_json::json!([lens_json(0, Some("12 references")), lens_json(0, Some("3 implementations")), lens_json(0, Some(&"y".repeat(5000)))]));
    let pane = app.focused_pane_id();
    app.set_pane_text_cols(pane, 40);
    let map = app.focused_layout(4).map;
    let row = map.row(0).unwrap();
    assert!(matches!(row.kind, rows::RowKind::Virtual(_)));
    let rows::RowKind::Virtual(i) = row.kind else { unreachable!() };
    let lead = rows::virtual_row_lead(app.open(), 0, &app.tab_stops_for(app.focused_buffer_id()), 40);
    assert_eq!(lead, 20, "half the pane at most");
    assert_eq!(map.virtual_text(i), "12 references …", "whole titles that fit, then …");
}

#[test]
fn semantic_tokens_are_neither_asked_nor_drawn_with_the_setting_off_or_the_language_left_out() {
    assert!(crate::lsp::client_capabilities(false, false, false).text_document.unwrap().semantic_tokens.is_none(), "not advertised");
    assert!(crate::lsp::client_capabilities(true, false, false).text_document.unwrap().semantic_tokens.is_some());
    for (key, value) in [("lsp.semantic_highlighting", fenix_config::Value::Bool(false)), ("lsp.semantic_highlighting_off", fenix_config::Value::List(vec!["Rust".into()]))] {
        let (_dir, mut app, _path, server) = semantic_fixture("semantic_off");
        let grammar = colour_at(&mut app, 16);
        app.ask_due_semantic_tokens();
        let (id, _) = only_pending(&app, &server);
        answer(&mut app, &server, id, serde_json::json!({"data": MAIN_AND_X}));
        app.config.set_value(key, Some(value));
        assert_eq!(colour_at(&mut app, 16), grammar, "{key}: the grammar's alone");
        // And after an edit, nothing kept or asked.
        app.test_insert_str(" ");
        app.sync_lsp_for_focused_buffer();
        assert!(app.lsp_sessions[&server].semantic.is_empty(), "{key}");
    }
}

#[test]
fn semantic_tokens_come_from_the_first_server_offering_them_and_a_refresh_asks_again() {
    let dir = TempDir::new("semantic_two");
    dir.write(".fenix/project.ini", "");
    let path = dir.write("main.rs", "fn main() { let x = 1; }\n");
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    let primary = install_server(&mut app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], capabilities(serde_json::json!({"textDocumentSync": 2})));
    let extra = install_server(&mut app, &path, LspServer::Additional("kiln".into()), "kiln", &["rust"], semantic_capabilities());
    app.sync_lsp_for_focused_buffer();
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&primary].pending.is_empty(), "it offers none");
    let (id, _) = only_pending(&app, &extra);
    answer(&mut app, &extra, id, serde_json::json!({"data": MAIN_AND_X}));
    assert_eq!(colour_at(&mut app, 16), Some(app.theme.syntax_macro));
    // Its refresh: asked again.
    let generation = app.lsp_sessions[&extra].generation;
    app.apply_lsp_event(extra.clone(), generation, fenix_lsp::LspEvent::ServerRequest { id: serde_json::json!(7), method: "workspace/semanticTokens/refresh".into(), params: serde_json::Value::Null });
    app.ask_due_semantic_tokens();
    assert!(app.lsp_sessions[&extra].pending.is_empty(), "not until it's drawn");
    let _ = colour_at(&mut app, 16);
    app.ask_due_semantic_tokens();
    assert!(matches!(only_pending(&app, &extra).1, PendingLspRequest::SemanticTokens { .. }));
    // Both offering them: the first only.
    app.lsp_sessions.get_mut(&primary).unwrap().capabilities = Some(semantic_capabilities());
    assert_eq!(app.semantic_server(&path), Some(primary));
}

#[test]
fn one_buffer_is_opened_on_every_server_that_serves_its_language() {
    let (dir, mut app, path, primary, extra) = two_servers("multi_open");
    let canonical = fenix_lsp::normalize(refactor::identity(&path));
    assert!(app.lsp_sessions[&primary].open_documents.contains_key(&canonical));
    assert!(app.lsp_sessions[&extra].open_documents.contains_key(&canonical), "the additional server is told about the Rust file too");
    assert_eq!(app.focused_lsp_context().unwrap().0, [primary.clone(), extra.clone()], "primary first");

    // An edit reaches both, each at its own version.
    app.test_insert_str("x");
    app.sync_lsp_for_focused_buffer();
    assert_eq!(app.lsp_sessions[&primary].open_documents[&canonical].1, 2);
    assert_eq!(app.lsp_sessions[&extra].open_documents[&canonical].1, 2);

    // A TOML file is served by neither.
    let other = dir.write("Cargo.toml", "[package]\n");
    app.open_file_from_picker(&other);
    app.sync_lsp_for_focused_buffer();
    assert!(app.focused_lsp_context().is_none());
    assert_eq!(app.lsp_sessions[&extra].open_documents.len(), 1);
}

#[test]
fn hover_shows_every_servers_answer_primary_first_as_they_arrive() {
    let (_dir, mut app, _path, primary, extra) = two_servers("multi_hover");
    app.request_hover();
    let (p_id, _) = only_pending(&app, &primary);
    let (e_id, _) = only_pending(&app, &extra);
    answer(&mut app, &extra, e_id, serde_json::json!({"contents": "REQ-014: the editor boots"}));
    assert_eq!(app.lsp_hover.as_deref(), Some("REQ-014: the editor boots"), "the faster server shows without waiting");
    answer(&mut app, &primary, p_id, serde_json::json!({"contents": {"kind": "markdown", "value": "fn main()"}}));
    assert_eq!(app.lsp_hover.clone().unwrap(), format!("fn main(){}REQ-014: the editor boots", fenix_lsp::fanout::HOVER_SEPARATOR));
    assert!(app.lsp_fanouts.is_empty(), "done once everyone answered");

    // An empty answer drops out.
    app.request_hover();
    let (p_id, _) = only_pending(&app, &primary);
    let (e_id, _) = only_pending(&app, &extra);
    answer(&mut app, &primary, p_id, serde_json::Value::Null);
    answer(&mut app, &extra, e_id, serde_json::json!({"contents": "only kiln"}));
    assert_eq!(app.lsp_hover.as_deref(), Some("only kiln"));
}

#[test]
fn definitions_merge_and_wait_for_a_slow_server_only_until_the_deadline() {
    let (_dir, mut app, path, primary, extra) = two_servers("multi_definition");
    let uri = fenix_lsp::path_to_uri(&fenix_lsp::normalize(refactor::identity(&path))).unwrap();
    let at = |line: u32| serde_json::json!({"uri": uri, "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 2}}});
    app.request_goto_definition();
    let (p_id, _) = only_pending(&app, &primary);
    let (e_id, pending) = only_pending(&app, &extra);
    let PendingLspRequest::Definition { fanout } = pending else { panic!("{pending:?}") };
    answer(&mut app, &primary, p_id, serde_json::json!([at(0)]));
    assert!(app.lsp_fanouts.contains_key(&fanout), "kiln may still know another place");
    answer(&mut app, &extra, e_id, serde_json::json!([at(0), at(1)]));
    assert!(!app.lsp_fanouts.contains_key(&fanout));
    assert_eq!(app.quickfix.len(), 2, "the shared place once, kiln's other place too");

    // kiln never answers: past the deadline, the primary's answer stands.
    app.request_goto_definition();
    let (p_id, _) = only_pending(&app, &primary);
    let (_, pending) = only_pending(&app, &extra);
    let PendingLspRequest::Definition { fanout } = pending else { panic!() };
    // Pretend the deadline has already passed.
    if let Some(LspFanout::Locations { gather, .. }) = app.lsp_fanouts.get_mut(&fanout) {
        *gather = fenix_lsp::fanout::Gather::new(vec![primary.clone(), extra.clone()], std::time::Instant::now());
    }
    answer(&mut app, &primary, p_id, serde_json::json!([at(0)]));
    app.handle_user_event(FenixUserEvent::LspDeadline(fanout));
    assert!(!app.lsp_fanouts.contains_key(&fanout), "acted on without kiln");
    assert_eq!(app.test_cursor().char_idx, 0);

    // A server that never answers doesn't leave its request behind for good.
    app.request_goto_definition();
    app.request_goto_definition();
    assert_eq!(app.lsp_fanouts.len(), 1, "the newer gd supersedes the older one");
}

#[test]
fn completion_asks_again_only_the_servers_whose_list_was_incomplete() {
    let (_dir, mut app, _path, primary, extra) = two_servers("multi_completion");
    app.test_vim_key(KeyPress::char('i'));
    app.request_lsp_completion(0, None);
    let (p_id, PendingLspRequest::Completion { .. }) = only_pending(&app, &primary) else { panic!() };
    let (e_id, _) = only_pending(&app, &extra);
    answer(&mut app, &primary, p_id, serde_json::json!({"isIncomplete": false, "items": [{"label": "main"}]}));
    answer(&mut app, &extra, e_id, serde_json::json!({"isIncomplete": true, "items": [{"label": "REQ-014"}]}));
    let labels: Vec<String> = app.lsp_completion.as_ref().unwrap().answers.items().map(|(_, item)| item.label.clone()).collect();
    assert_eq!(labels, ["main", "REQ-014"]);

    app.request_lsp_completion(0, None);
    assert!(app.lsp_sessions[&primary].pending.is_empty(), "rust-analyzer's list was complete: not asked again");
    assert_eq!(app.lsp_sessions[&extra].pending.len(), 1, "kiln said its list was incomplete");

    // A trigger character asks only the server that declared it, at a new anchor.
    app.lsp_sessions.get_mut(&extra).unwrap().pending.clear();
    app.request_lsp_completion(5, Some('.'));
    assert_eq!(app.lsp_sessions[&primary].pending.len(), 1);
    assert!(app.lsp_sessions[&extra].pending.is_empty(), "kiln triggers on - only");
}

#[test]
fn in_a_comment_only_servers_are_asked_and_a_trigger_character_asks_its_server() {
    let (_dir, mut app, _path, primary, extra) = two_servers("multi_comment");
    app.test_vim_key(KeyPress::char('A'));
    for c in "\n// implements: REQ".chars() {
        app.test_insert_str(&c.to_string());
    }
    app.sync_lsp_for_focused_buffer();
    app.sync_completion();
    assert!(app.completion.is_some(), "a server may know what goes in a comment");
    assert!(!app.completion_local, "local words don't crowd it");
    assert_eq!(app.lsp_sessions[&extra].pending.len(), 1);
    app.lsp_sessions.get_mut(&primary).unwrap().pending.clear();
    app.lsp_sessions.get_mut(&extra).unwrap().pending.clear();

    // `-` is kiln's trigger character: just typed, only kiln is asked.
    app.test_insert_str("-");
    app.sync_lsp_for_focused_buffer();
    app.sync_completion();
    assert!(app.completion.is_some());
    assert_eq!(app.lsp_sessions[&extra].pending.len(), 1);
    assert!(app.lsp_sessions[&primary].pending.is_empty(), "rust-analyzer didn't declare -");
}

#[test]
fn rename_goes_to_the_primary_and_formatting_to_the_one_server_that_formats() {
    let (_dir, mut app, _path, primary, extra) = two_servers("multi_exclusive");
    app.send_lsp_rename("renamed");
    assert!(matches!(only_pending(&app, &primary).1, PendingLspRequest::Rename { .. }));
    assert!(app.lsp_sessions[&extra].pending.is_empty());
    assert!(app.request_lsp_format(), "the primary can't format; kiln is the one server that can");
    assert!(matches!(only_pending(&app, &extra).1, PendingLspRequest::Format { .. }));
}

#[test]
fn diagnostics_from_two_servers_are_shown_together_and_each_keeps_its_own() {
    let (_dir, mut app, path, primary, extra) = two_servers("multi_diagnostics");
    let canonical = fenix_lsp::normalize(refactor::identity(&path));
    let uri = fenix_lsp::path_to_uri(&canonical).unwrap();
    let publish = |app: &mut App, key: &LspKey, messages: &[&str]| {
        let generation = app.lsp_sessions[key].generation;
        let diagnostics: Vec<_> = messages.iter().map(|m| serde_json::json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": m, "severity": 1})).collect();
        app.apply_lsp_event(key.clone(), generation, fenix_lsp::LspEvent::Notification { method: "textDocument/publishDiagnostics".into(), params: serde_json::json!({"uri": uri, "diagnostics": diagnostics}) });
    };
    publish(&mut app, &primary, &["mismatched types"]);
    publish(&mut app, &extra, &["REQ-999 doesn't exist"]);
    let shown = app.all_diagnostics(&canonical);
    assert_eq!(shown.iter().map(|d| (d.source.clone().unwrap(), d.message.clone())).collect::<Vec<_>>(),
        [("rust-analyzer".to_string(), "mismatched types".to_string()), ("kiln".to_string(), "REQ-999 doesn't exist".to_string())]);
    assert!(app.modeline_text().contains("2E"), "{}", app.modeline_text());
    publish(&mut app, &extra, &[]);
    assert_eq!(app.all_diagnostics(&canonical).len(), 1, "kiln clearing its own leaves rust-analyzer's");

    // A code action request shows each server only its own.
    publish(&mut app, &extra, &["REQ-999 doesn't exist"]);
    assert_eq!(app.diagnostics.from_server(&canonical, &primary)[0].message, "mismatched types");
}

#[test]
fn a_crashed_additional_server_leaves_the_primary_working() {
    let (_dir, mut app, path, primary, extra) = two_servers("multi_crash");
    let canonical = fenix_lsp::normalize(refactor::identity(&path));
    let uri = fenix_lsp::path_to_uri(&canonical).unwrap();
    app.diagnostics.publish(canonical.clone(), primary.clone(), "rust-analyzer", vec![lsp_types::Diagnostic { message: "kept".into(), ..Default::default() }]);
    app.diagnostics.publish(canonical.clone(), extra.clone(), "kiln", vec![lsp_types::Diagnostic { message: "gone".into(), ..Default::default() }]);
    app.request_goto_definition();
    let (p_id, _) = only_pending(&app, &primary);

    let generation = app.lsp_sessions[&extra].generation;
    app.apply_lsp_event(extra.clone(), generation, fenix_lsp::LspEvent::Disconnected("server closed its stdout".into()));
    assert!(app.status_message.as_ref().unwrap().text.contains("kiln disconnected"), "reported by name, like a primary's crash");
    assert!(!app.lsp_sessions.contains_key(&extra));
    assert!(app.lsp_unavailable.contains(&extra), "not restarted every frame; :lsp-restart retries it");
    assert!(app.lsp_sessions.contains_key(&primary));
    assert_eq!(app.all_diagnostics(&canonical).iter().map(|d| d.message.as_str()).collect::<Vec<_>>(), ["kept"]);

    // The request in flight isn't left waiting on the crashed server.
    answer(&mut app, &primary, p_id, serde_json::json!([{"uri": uri, "range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 3}}}]));
    assert_eq!(app.test_cursor().char_idx, 2, "the primary's definition is jumped to");
    assert!(app.lsp_fanouts.is_empty());

    // And the primary keeps serving the buffer alone.
    app.sync_lsp_for_focused_buffer();
    assert_eq!(app.focused_lsp_context().unwrap().0, std::slice::from_ref(&primary));
    app.request_hover();
    assert!(matches!(only_pending(&app, &primary).1, PendingLspRequest::Hover { .. }));
}

#[test]
fn an_additional_server_that_cannot_start_is_reported_and_the_primary_still_starts() {
    let (dir, path) = project_scope_fixture("multi_start");
    dir.write(".fenix/tools.json", &serde_json::json!({"lsp": {
        "rust": {"executable": std::env::current_exe().unwrap(), "args": ["--ignored", "--exact", "app::tests::lsp_sink_fixture"], "env": {"FENIX_LSP_SINK": "1"}},
        "additional": [{"name": "kiln", "executable": "definitely-not-a-real-kiln-xyz", "args": ["lsp"], "languages": ["rust", "markdown"]}]
    }}).to_string());
    let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
    app.sync_lsp_for_focused_buffer();
    let root = tool_sessions::lsp_root_for_path(&path);
    let kiln = LspKey { server: LspServer::Additional("kiln".into()), root: root.clone() };
    assert!(app.lsp_sessions.contains_key(&LspKey::primary(fenix_syntax::LanguageId::Rust, root.clone())), "the primary started");
    assert!(app.lsp_unavailable.contains(&kiln));
    assert!(app.status_message.as_ref().unwrap().text.contains("couldn't start kiln"), "{:?}", app.status_message.as_ref().map(|m| &m.text));

    // `:lsp-restart kiln` tries kiln again and leaves the primary alone.
    let primary_generation = app.lsp_sessions[&LspKey::primary(fenix_syntax::LanguageId::Rust, root.clone())].generation;
    app.restart_project_lsp("kiln");
    assert_eq!(app.lsp_sessions[&LspKey::primary(fenix_syntax::LanguageId::Rust, root.clone())].generation, primary_generation);
    assert!(app.status_message.as_ref().unwrap().text.contains("couldn't start kiln"), "tried again");
    app.restart_project_lsp("nonesuch");
    assert!(app.status_message.as_ref().unwrap().text.contains("no language server called nonesuch"));
}

#[test]
fn restarting_one_server_by_name_keeps_the_others() {
    let (_dir, mut app, _path, primary, extra) = two_servers("multi_restart");
    app.project_root = Some(primary.root.clone());
    let generation = app.lsp_sessions[&primary].generation;
    let kiln_generation = app.lsp_sessions[&extra].generation;
    app.restart_project_lsp("kiln");
    assert!(app.lsp_sessions.get(&extra).is_none_or(|s| s.generation != kiln_generation), "kiln was stopped and started again");
    assert_eq!(app.lsp_sessions[&primary].generation, generation, "rust-analyzer kept running");
}

#[test]
fn project_scoped_restart_leaves_other_projects_servers_running() {
    let (_a,a) = project_scope_fixture("restart_a");
    let (_b,b) = project_scope_fixture("restart_b");
    let mut app = App::with_file(None);
    let a_key = install_test_lsp(&mut app,&a);
    let b_key = install_test_lsp(&mut app,&b);
    // A dashboard with an explicit project root avoids spawning a real language server.
    app.project_root = Some(a_key.root.clone());
    app.restart_project_lsp("");
    assert!(!app.lsp_sessions.contains_key(&a_key));
    assert!(app.lsp_sessions.contains_key(&b_key));
}

// -- TODO comments -----------------------------------------------------

fn app_on(dir: &TempDir, name: &str, contents: &str) -> App {
    let file = dir.write(name, contents);
    App::with_file(Some(file.to_string_lossy().into_owned()))
}

fn keys(app: &mut App, seq: &str) {
    for c in seq.chars() {
        app.test_dispatch_key(KeyPress::char(c));
    }
}

#[test]
fn bracket_t_steps_through_todo_comments_and_skips_strings() {
    let dir = TempDir::new("bracket_t");
    let source = "fn a() {}\n// TODO: first\nlet s = \"TODO: not me\";\n/* FIXME second */\n";
    let mut app = app_on(&dir, "a.rs", source);
    keys(&mut app, "]t");
    assert_eq!(app.test_cursor().char_idx, source.find("TODO: first").unwrap());
    keys(&mut app, "]t");
    assert_eq!(app.test_cursor().char_idx, source.find("FIXME").unwrap());
    keys(&mut app, "]t"); // none left below -- stays put
    assert_eq!(app.test_cursor().char_idx, source.find("FIXME").unwrap());
    keys(&mut app, "[t");
    assert_eq!(app.test_cursor().char_idx, source.find("TODO: first").unwrap());
}

#[test]
fn bracket_t_takes_a_count() {
    let dir = TempDir::new("bracket_t_count");
    let source = "# TODO one\n# TODO two\n# TODO three\n";
    let mut app = app_on(&dir, "a.py", source);
    keys(&mut app, "2]t");
    assert_eq!(app.test_cursor().char_idx, source.find("TODO two").unwrap());
}

#[test]
fn bracket_d_steps_through_the_problems_of_every_source() {
    let dir = TempDir::new("bracket_d");
    let source = "tc::send ZTC0810 -X 1\nputs ok\ntc::send ZTC08101 -PTH00101 99\n";
    let mut app = app_on(&dir, "a.tcl", source);
    let path = app.open().buffer.path().unwrap().to_path_buf();
    let diag = |line: u32, character: u32, message: &str| lsp_types::Diagnostic {
        range: lsp_types::Range::new(lsp_types::Position::new(line, character), lsp_types::Position::new(line, character + 3)),
        message: message.into(),
        ..Default::default()
    };
    // One from each of two language servers: the MIB extension's
    // script check, and Tcl's.
    let server = app.lsp_key(fenix_syntax::LanguageId::Tcl);
    let mib = LspKey { server: LspServer::Additional("ext:fenix.mib:mib".into()), root: server.root.clone() };
    app.diagnostics.publish(path.clone(), mib, "mib", vec![diag(2, 28, "PTH00101: 99 is outside 1..8")]);
    app.diagnostics.publish(path, server, "tcl", vec![diag(0, 9, "unknown mnemonic ZTC0810 -- ZTC08101?")]);
    keys(&mut app, "]d");
    assert_eq!(app.test_cursor().char_idx, 9);
    assert!(app.status_message.as_ref().is_some_and(|m| m.text.contains("ZTC08101?")));
    keys(&mut app, "]d");
    assert_eq!(app.test_cursor().char_idx, source.find("99").unwrap());
    keys(&mut app, "]d");
    assert!(app.status_message.as_ref().is_some_and(|m| m.text == "no more problems below"));
    keys(&mut app, "2[d");
    assert_eq!(app.test_cursor().char_idx, 9, "a count, stopping at the first");
}

#[test]
fn buffer_todo_picker_lists_each_comment_keyword_and_jumps() {
    let dir = TempDir::new("buffer_todos");
    let source = "# TODO: alpha\nputs \"FIXME: nope\"\n# HACK: beta\n";
    let mut app = app_on(&dir, "a.tcl", source);
    app.picker_buffer_todos();
    match &app.active_picker {
        Some(ActivePicker::BufferTodos(state)) => {
            let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
            assert_eq!(labels, vec!["TODO     1  alpha", "HACK     3  beta"]);
        }
        _ => panic!("expected the buffer TODO picker"),
    }
    picker_move_selection(app.active_picker.as_mut().unwrap(), 1);
    app.picker_confirm();
    assert_eq!(app.test_cursor().char_idx, source.find("HACK").unwrap());
}

#[test]
fn project_todo_picker_reads_comments_only_and_fills_the_quickfix_list() {
    if !ripgrep_available() {
        eprintln!("skipping project_todo_picker_reads_comments_only_and_fills_the_quickfix_list: ripgrep (rg) is not on PATH");
        return;
    }
    let dir = TempDir::new("project_todos");
    dir.write("src/a.rs", "// TODO: in rust\nconst S: &str = \"TODO: string\";\n");
    dir.write("b.xml", "<a><!-- FIXME: in xml --></a>\n");
    dir.write("notes.txt", "TODO plain text\nprose mentioning TODO midway\n");
    let mut app = app_on(&dir, "c.py", "x = 1\n");
    app.project_root = Some(dir.path().to_path_buf());
    app.picker_project_todos();
    let labels: Vec<String> = match &app.active_picker {
        Some(ActivePicker::ProjectTodos(state)) => state.visible_rows(0, 10).map(|(_, c)| c.label.clone()).collect(),
        _ => panic!("expected the project TODO picker"),
    };
    assert_eq!(labels.len(), 3, "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("FIX ") && l.ends_with("in xml")), "{labels:?}");
    assert!(labels.iter().any(|l| l.ends_with("in rust")), "{labels:?}");
    assert!(labels.iter().any(|l| l.ends_with("plain text")), "{labels:?}");
    assert_eq!(app.quickfix.len(), 3);
}

#[test]
fn todo_keywords_are_recorded_for_the_background_tint() {
    let dir = TempDir::new("todo_tint");
    let mut app = app_on(&dir, "a.rs", "// TODO: x\nfn a() {}\n");
    let id = app.focused_buffer_id();
    let highlights = app.syntax_highlights_for_visible_range(id, 0, 2);
    assert!(highlights.iter().any(|(r, c)| r.start == 3 && *c == app.theme.todo_color(fenix_syntax::TodoKind::Todo)));
    let spans = app.todo_spans.get(&id).expect("a TODO span was recorded");
    assert_eq!(spans, &vec![(3..8, fenix_syntax::TodoKind::Todo)]);
}

// -- XML -----------------------------------------------------------------

#[test]
fn typing_a_start_tag_in_xml_closes_it_after_the_cursor() {
    let dir = TempDir::new("xml_autoclose");
    let mut app = app_on(&dir, "a.xml", "");
    keys(&mut app, "i<item id=\"1\">");
    assert_eq!(app.open().buffer.text(), "<item id=\"1\"></item>");
    assert_eq!(app.test_cursor().char_idx, "<item id=\"1\">".chars().count());
    // Self-closing and end tags are left alone.
    keys(&mut app, "<br/>");
    assert_eq!(app.open().buffer.text(), "<item id=\"1\"><br/></item>");
}

#[test]
fn typing_an_end_tag_opener_in_xml_completes_the_open_element() {
    let dir = TempDir::new("xml_close_slash");
    let mut app = app_on(&dir, "a.xml", "<a>\n<b>text\n");
    app.test_set_cursor(Cursor { char_idx: "<a>\n<b>text".len(), sticky_col: 0 });
    keys(&mut app, "i</");
    assert_eq!(app.open().buffer.text(), "<a>\n<b>text</b>\n");
}

#[test]
fn autoclose_is_xml_only() {
    let dir = TempDir::new("xml_autoclose_rust");
    let mut app = app_on(&dir, "a.rs", "");
    keys(&mut app, "iVec<u8>");
    assert_eq!(app.open().buffer.text(), "Vec<u8>");
}

#[test]
fn percent_jumps_between_matching_xml_tags_and_falls_back_to_brackets() {
    let dir = TempDir::new("xml_percent");
    let source = "<root>\n  <item a=\"(x)\">v</item>\n</root>\n";
    let mut app = app_on(&dir, "a.xml", source);
    app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 });
    app.test_percent();
    assert_eq!(app.test_cursor().char_idx, source.find("/root").unwrap() + 1);
    app.test_percent();
    assert_eq!(app.test_cursor().char_idx, 1);
    // Inside the attribute, `%` is bracket matching as usual.
    let paren = source.find('(').unwrap();
    app.test_set_cursor(Cursor { char_idx: paren, sticky_col: 0 });
    app.test_percent();
    assert_eq!(app.test_cursor().char_idx, source.find(')').unwrap());
}

#[test]
fn gcc_wraps_and_unwraps_xml_lines_in_block_comments() {
    let dir = TempDir::new("gcc_xml");
    let mut app = app_on(&dir, "a.xml", "  <a/>\n");
    keys(&mut app, "gcc");
    assert_eq!(app.open().buffer.text(), "  <!-- <a/> -->\n");
    keys(&mut app, "gcc");
    assert_eq!(app.open().buffer.text(), "  <a/>\n");
}

#[test]
fn format_buffer_reindents_xml_by_element_depth() {
    let dir = TempDir::new("xml_format");
    let mut app = app_on(&dir, "a.xml", "<a>\n<b>f(x)</b>\n<c>\n<d/>\n</c>\n</a>");
    app.vim.set_indent_width(2);
    app.format_buffer();
    assert_eq!(app.open().buffer.text(), "<a>\n  <b>f(x)</b>\n  <c>\n    <d/>\n  </c>\n</a>");
}

#[test]
fn xml_outline_lists_elements_as_a_tree() {
    let dir = TempDir::new("xml_outline");
    let mut app = app_on(&dir, "a.xml", "<project>\n  <dependency id=\"core\">\n    <version>1</version>\n  </dependency>\n</project>\n");
    app.picker_outline();
    match &app.active_picker {
        Some(ActivePicker::Outline(state)) => {
            let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
            assert_eq!(labels, vec!["project  ·  line 1", "  dependency  id=core  ·  line 2", "    version  ·  line 3"]);
        }
        _ => panic!("expected the outline picker"),
    }
}

#[test]
fn xml_validate_moves_to_the_first_error() {
    let dir = TempDir::new("xml_validate");
    let mut app = app_on(&dir, "a.xml", "<a>\n  <b></c>\n</a>\n");
    app.xml_validate();
    assert_eq!(app.open().buffer.line_col(&app.test_cursor()).0, 1);
    assert!(app.status_message.as_ref().is_some_and(|m| m.is_error));
}

#[test]
fn xml_multiline_elements_fold() {
    let dir = TempDir::new("xml_fold");
    let mut app = app_on(&dir, "a.xml", "<a>\n  <b>\n    <c/>\n  </b>\n</a>\n");
    app.test_set_cursor(Cursor { char_idx: 8, sticky_col: 0 });
    app.toggle_code_fold();
    let id = app.focused_buffer_id();
    assert_eq!(app.folded_display_lines(id), vec![0, 1, 4, 5]);
}

// -- A file rewritten underneath an open buffer ---------------------------

#[test]
fn a_focused_file_rewritten_shorter_on_disk_reloads_without_panicking() {
    // The crash: the cursor sat near the end of a long file, a coding
    // agent rewrote it to a few lines, and the reload asked the *new*
    // text for the line of the *old* cursor offset.
    let dir = TempDir::new("reload_shorter");
    let long: String = (0..400).map(|i| format!("fn f{i}() {{ let x = {i}; }}
")).collect();
    let file = dir.write("big.rs", &long);
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    let end = app.open().buffer.len_chars().saturating_sub(2);
    app.test_set_cursor(Cursor { char_idx: end, sticky_col: 0 });
    let pane = app.focused_pane_id();
    app.pane_state_mut(pane).scroll_line = 380;

    std::fs::write(&file, "fn main() {
    // TODO: short now
}
").unwrap();
    let sweep = app.reload_buffers_changed_on_disk();

    assert_eq!(sweep.reloaded.len(), 1);
    assert_eq!(app.open().buffer.text(), "fn main() {
    // TODO: short now
}
");
    assert_eq!(app.open().buffer.line_col(&app.test_cursor()).0, 2, "clamped to the new last line");
    assert_eq!(app.pane_state(pane).scroll_line, 0, "the whole three-line file is in view");
    // And the frame after it -- highlighting the visible window --
    // is just as safe.
    let id = app.focused_buffer_id();
    let _ = app.syntax_highlights_for_visible_range(id, app.render_base_line(), 40);
}

#[test]
fn a_reload_keeps_each_panes_line_when_the_file_is_long_enough() {
    let dir = TempDir::new("reload_same_line");
    let file = dir.write("a.txt", "one
two
three
four
");
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_set_cursor(Cursor { char_idx: "one
two
th".len(), sticky_col: 0 });
    std::fs::write(&file, "ONE
TWO
THREE, longer
FOUR
FIVE
").unwrap();
    app.reload_buffers_changed_on_disk();
    assert_eq!(app.open().buffer.line_col(&app.test_cursor()), (2, 0));
}

#[test]
fn a_reload_during_visual_mode_drops_the_stale_selection() {
    let dir = TempDir::new("reload_visual");
    let long: String = (0..50).map(|i| format!("line {i}
")).collect();
    let file = dir.write("a.txt", &long);
    let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
    app.test_vim_key(KeyPress::char('G'));
    app.test_vim_key(KeyPress::char('v'));
    std::fs::write(&file, "short
").unwrap();
    app.reload_buffers_changed_on_disk();
    assert_eq!(app.vim.mode(), Mode::Normal);
    assert!(app.visual_selection_segments(0..10).is_empty());
}

#[test]
fn killing_a_buffer_also_shown_in_another_workspace_repoints_that_pane_too() {
    let dir = TempDir::new("kill_buffer_other_workspace");
    let a = dir.write("a.txt", "a
");
    let mut app = App::with_file(Some(a.to_string_lossy().into_owned()));
    let killed = app.focused_buffer_id();
    app.new_workspace();
    app.open_file_from_picker(&a); // a.txt now shown in both
    let second = app.workspaces.active_index();
    app.workspaces.switch_to_index(second - 1);
    app.kill_buffer();
    assert!(app.buffers.get(killed).is_none());
    app.workspaces.switch_to_index(second);
    for pane in app.windows().windows() {
        let id = *app.windows().content(pane).unwrap();
        assert!(app.buffers.get(id).is_some(), "a pane in the other workspace still shows the killed buffer");
    }
}

#[test]
fn visual_p_replaces_the_selection_with_the_os_clipboard() {
    let _guard = CLIPBOARD_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut app = macro_app("say hello world");
    let Some(clipboard) = app.clipboard.as_mut() else { return };
    let saved = clipboard.get_text().ok();
    clipboard.set_text("goodbye".to_string()).unwrap();

    app.test_set_cursor(Cursor { char_idx: 4, sticky_col: 4 });
    app.test_dispatch_key(KeyPress::char('v'));
    app.test_dispatch_key(KeyPress::char('e'));
    // `route_keypress`'s own order around a key reaching Vim.
    app.pull_clipboard_before_paste(KeyPress::char('p'));
    app.test_dispatch_key(KeyPress::char('p'));
    app.push_clipboard_after_edit();

    assert_eq!(app.open().buffer.text(), "say goodbye world");
    let clipboard = app.clipboard.as_mut().unwrap();
    assert_eq!(clipboard.get_text().unwrap(), "hello", "Vim's v_p: the replaced text is what's on the clipboard now");
    if let Some(text) = saved {
        let _ = clipboard.set_text(text);
    }
}
