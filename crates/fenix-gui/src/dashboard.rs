use std::path::PathBuf;

/// What activating (`Enter`) a dashboard line does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardEntry {
    Project(PathBuf),
    RecentFile(PathBuf),
}

/// How one generated line should be colored -- consulted by `App` to
/// build the same kind of `(Range<usize>, glyphon::Color)` list
/// `fenix-syntax` highlighting already produces, without a real parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardLineStyle {
    Banner,
    Tagline,
    Header,
    Project,
    RecentFile,
    Footer,
}

/// Per-line metadata for one line of `Dashboard::text`, at the matching
/// index in `Dashboard::lines`.
#[derive(Debug, Clone)]
pub struct DashboardLine {
    pub style: DashboardLineStyle,
    /// `Some` only for a `Project`/`RecentFile` line -- what `Enter` on
    /// this line does.
    pub entry: Option<DashboardEntry>,
    /// For a `Project`/`RecentFile` line, the char column (within the
    /// line) where the dim path/parent-dir portion begins -- the name
    /// portion before it stays the default `theme.fg`. `None` for every
    /// other style (the whole line is one color).
    pub dim_from: Option<usize>,
}

/// The generated dashboard: `text` is real content for a real
/// `fenix_core::Buffer` (via `BufferList::open_dashboard`); `lines[i]`
/// describes `text`'s line `i` (`None` for a blank/unstyled line) --
/// `App` looks up "what is the line the cursor is on" by index, without
/// re-parsing the generated text.
pub struct Dashboard {
    pub text: String,
    pub lines: Vec<Option<DashboardLine>>,
}

const MAX_PROJECTS: usize = 5;
const MAX_RECENT_FILES: usize = 8;

struct Builder {
    text: String,
    lines: Vec<Option<DashboardLine>>,
}

impl Builder {
    fn new() -> Self {
        Self { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, meta: Option<DashboardLine>) {
        self.text.push_str(text);
        self.text.push('\n');
        self.lines.push(meta);
    }

    fn blank(&mut self) {
        self.push("", None);
    }

    fn finish(self) -> Dashboard {
        Dashboard { text: self.text, lines: self.lines }
    }
}

/// Builds the dashboard shown when Fenix starts with no file argument
/// (and whenever `SPC d d` re-opens it): a generated ASCII wordmark
/// (built via string repeat/pad, not hand-typed art, so its box-border
/// alignment is correct by construction), up to `MAX_PROJECTS` known
/// projects (the section is omitted entirely if `projects` is empty),
/// up to `MAX_RECENT_FILES` *existing* recent files (dead paths are
/// filtered out before truncating, so a few stale entries don't shrink
/// the visible list below the cap), and a footer hint line.
pub fn render(projects: &[PathBuf], recent_files: &[PathBuf]) -> Dashboard {
    let mut b = Builder::new();
    push_banner(&mut b);
    push_projects(&mut b, projects);
    push_recent_files(&mut b, recent_files);
    push_footer(&mut b);
    b.finish()
}

/// Rows in the block-letter font `render_word` draws with -- tall enough
/// to read as a real logo (Doom Emacs/LazyVim-style), not just a label.
const GLYPH_ROWS: usize = 6;
type Glyph = [&'static str; GLYPH_ROWS];

/// A tiny hand-built 5-column bitmap font, `#` = lit pixel -- only the
/// letters `render_word` is ever actually called with need entries.
/// Alignment isn't trusted by eye: `banner_rows_are_all_the_same_width`
/// asserts every generated row of the word comes out the same length.
fn glyph_for(c: char) -> Glyph {
    match c {
        'F' => ["#####", "#....", "#....", "####.", "#....", "#...."],
        'E' => ["#####", "#....", "####.", "#....", "#....", "#####"],
        'N' => ["#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#"],
        'I' => ["#####", "..#..", "..#..", "..#..", "..#..", "#####"],
        'X' => ["#...#", ".#.#.", "..#..", "..#..", ".#.#.", "#...#"],
        _ => ["     ", "     ", "     ", "     ", "     ", "     "],
    }
}

/// Renders `word` as `GLYPH_ROWS` lines of big block-letter ASCII art,
/// one glyph per character with a blank column between letters.
fn render_word(word: &str) -> [String; GLYPH_ROWS] {
    let glyphs: Vec<Glyph> = word.chars().map(glyph_for).collect();
    std::array::from_fn(|row| {
        glyphs
            .iter()
            .map(|g| g[row].chars().map(|c| if c == '#' { '#' } else { ' ' }).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn push_banner(b: &mut Builder) {
    let tagline = "a keyboard-first editor";
    let banner_rows = render_word("FENIX");
    let banner_width = banner_rows[0].chars().count();
    let banner_meta = || Some(DashboardLine { style: DashboardLineStyle::Banner, entry: None, dim_from: None });

    b.blank();
    for row in &banner_rows {
        b.push(row, banner_meta());
    }
    b.blank();
    b.push(
        &format!("{tagline:^banner_width$}"),
        Some(DashboardLine { style: DashboardLineStyle::Tagline, entry: None, dim_from: None }),
    );
    b.blank();
    b.blank();
}

fn push_projects(b: &mut Builder, projects: &[PathBuf]) {
    if projects.is_empty() {
        return;
    }
    b.push("  Projects", Some(DashboardLine { style: DashboardLineStyle::Header, entry: None, dim_from: None }));
    for root in projects.iter().take(MAX_PROJECTS) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        let prefix = format!("    {name}  ");
        let dim_from = prefix.chars().count();
        let line = format!("{prefix}{}", root.display());
        b.push(
            &line,
            Some(DashboardLine {
                style: DashboardLineStyle::Project,
                entry: Some(DashboardEntry::Project(root.clone())),
                dim_from: Some(dim_from),
            }),
        );
    }
    b.blank();
}

fn push_recent_files(b: &mut Builder, recent_files: &[PathBuf]) {
    let existing: Vec<&PathBuf> = recent_files.iter().filter(|p| p.exists()).take(MAX_RECENT_FILES).collect();
    if existing.is_empty() {
        return;
    }
    b.push("  Recent Files", Some(DashboardLine { style: DashboardLineStyle::Header, entry: None, dim_from: None }));
    for path in existing {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let parent = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let prefix = format!("    {name}  ");
        let dim_from = prefix.chars().count();
        let line = format!("{prefix}{parent}");
        b.push(
            &line,
            Some(DashboardLine {
                style: DashboardLineStyle::RecentFile,
                entry: Some(DashboardEntry::RecentFile(path.clone())),
                dim_from: Some(dim_from),
            }),
        );
    }
    b.blank();
}

fn push_footer(b: &mut Builder) {
    b.push(
        "  SPC p a  add project    SPC f j  browse files    SPC p f  find file",
        Some(DashboardLine { style: DashboardLineStyle::Footer, entry: None, dim_from: None }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_banner_even_with_nothing_else_to_show() {
        let dashboard = render(&[], &[]);
        let banner_rows =
            dashboard.lines.iter().flatten().filter(|l| l.style == DashboardLineStyle::Banner).count();
        assert_eq!(banner_rows, GLYPH_ROWS);
        assert!(dashboard.text.contains("a keyboard-first editor"));
        assert_eq!(dashboard.text.lines().count(), dashboard.lines.len());
    }

    #[test]
    fn text_and_lines_always_stay_the_same_length() {
        let projects = vec![PathBuf::from("/repo/one"), PathBuf::from("/repo/two")];
        let dashboard = render(&projects, &[]);
        assert_eq!(dashboard.text.lines().count(), dashboard.lines.len());
    }

    #[test]
    fn empty_projects_list_omits_the_projects_section_entirely() {
        let dashboard = render(&[], &[]);
        assert!(!dashboard.text.contains("Projects"));
    }

    #[test]
    fn projects_are_listed_with_the_right_entry_at_the_right_line() {
        let projects = vec![PathBuf::from("/repo/fenix"), PathBuf::from("/repo/other")];
        let dashboard = render(&projects, &[]);

        let header_line = dashboard.text.lines().position(|l| l.trim() == "Projects").unwrap();
        let first_entry = dashboard.lines[header_line + 1].as_ref().unwrap();
        assert_eq!(first_entry.style, DashboardLineStyle::Project);
        assert_eq!(first_entry.entry, Some(DashboardEntry::Project(PathBuf::from("/repo/fenix"))));
        let second_entry = dashboard.lines[header_line + 2].as_ref().unwrap();
        assert_eq!(second_entry.entry, Some(DashboardEntry::Project(PathBuf::from("/repo/other"))));
    }

    #[test]
    fn only_the_first_five_projects_are_shown() {
        let projects: Vec<PathBuf> = (0..8).map(|i| PathBuf::from(format!("/repo/p{i}"))).collect();
        let dashboard = render(&projects, &[]);
        let shown =
            dashboard.lines.iter().flatten().filter(|l| l.style == DashboardLineStyle::Project).count();
        assert_eq!(shown, MAX_PROJECTS);
    }

    #[test]
    fn recent_files_section_is_omitted_when_none_of_the_paths_exist() {
        // None of these paths are real files on disk.
        let recent = vec![PathBuf::from("/definitely/does/not/exist/a.rs")];
        let dashboard = render(&[], &recent);
        assert!(!dashboard.text.contains("Recent Files"));
    }

    #[test]
    fn recent_files_filters_dead_paths_before_truncating_to_the_cap() {
        let dir = std::env::temp_dir().join(format!("fenix-dashboard-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 10 dead paths, then MAX_RECENT_FILES real ones -- if filtering
        // happened *after* truncating to the cap, the dead entries at the
        // front would crowd out real files and the section would show
        // fewer than the cap despite enough real files existing.
        let mut recent: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("/dead/path{i}.rs"))).collect();
        let mut real_paths = Vec::new();
        for i in 0..MAX_RECENT_FILES {
            let path = dir.join(format!("real{i}.rs"));
            std::fs::write(&path, "").unwrap();
            real_paths.push(path.clone());
            recent.push(path);
        }

        let dashboard = render(&[], &recent);
        let shown: Vec<PathBuf> = dashboard
            .lines
            .iter()
            .flatten()
            .filter_map(|l| match &l.entry {
                Some(DashboardEntry::RecentFile(p)) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(shown.len(), MAX_RECENT_FILES);
        assert_eq!(shown, real_paths);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn footer_hint_line_is_always_present() {
        let dashboard = render(&[], &[]);
        let has_footer = dashboard.lines.iter().flatten().any(|l| l.style == DashboardLineStyle::Footer);
        assert!(has_footer);
    }

    #[test]
    fn banner_rows_are_all_the_same_width() {
        // The block-letter font is hand-built (see `glyph_for`), not
        // trusted by eye -- every row of the rendered word must line up
        // to actually read as a block letter grid instead of jagged text.
        let rows = render_word("FENIX");
        let width = rows[0].chars().count();
        for row in &rows {
            assert_eq!(row.chars().count(), width);
        }
        assert!(width > 0);
    }

    #[test]
    fn unrecognized_characters_render_as_a_blank_glyph_not_a_panic() {
        let rows = render_word("F!X");
        let width = rows[0].chars().count();
        for row in &rows {
            assert_eq!(row.chars().count(), width);
        }
    }
}
