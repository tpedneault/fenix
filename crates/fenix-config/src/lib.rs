//! Fenix's unified settings file -- `dirs::config_dir()/fenix/config.ini`
//! (`%AppData%\fenix\config.ini` on Windows, `~/.config/fenix/config.ini`
//! on Unix). Replaces what used to be three separate flat files (theme
//! choice, font size, indent width), each with its own free-function
//! trio, now that there's more than one setting worth persisting -- one
//! shared file, one struct, the same `load`/`save` shape `fenix_project::
//! KnownProjects`/`RecentFiles` already established for multi-field
//! persisted state.
//!
//! Every field is `Option<T>`: "this key was present and parsed" vs.
//! not. `Config` itself has no notion of what a *sane default* is for
//! any of them -- those constants (`theme::ORBIT_DARK`, `text::
//! FONT_SIZE`, `fenix_vim::DEFAULT_INDENT_WIDTH`) live in crates that
//! would create a dependency cycle if `fenix-config` depended on them,
//! so each real consumer in `fenix-gui` supplies its own already-
//! existing default via `.unwrap_or(...)`. This also means a corrupted
//! or missing individual key only ever loses *that* setting, not every
//! setting in the file.

mod ini;

use std::io;
use std::path::PathBuf;

pub struct Config {
    path: PathBuf,
    pub theme: Option<String>,
    pub font_size: Option<f32>,
    pub font_family: Option<String>,
    pub indent_width: Option<usize>,
    /// How many visual columns a literal `\t` character expands to when
    /// rendered (real Vim's own `:set tabstop`) -- distinct from
    /// `indent_width`, which governs what typing Tab in Insert mode or
    /// `>>`/`<<` actually *inserts* (always spaces; Fenix never inserts
    /// a real tab character itself). Consulted by `fenix-gui`'s
    /// `tabstops` module.
    pub tab_width: Option<usize>,
    /// Whether caret-fade, scroll-ease, and yank/paste-pulse animations
    /// play at all -- `None`/unset means "on" (the default look); `false`
    /// snaps every one of them straight to its end state instead, for a
    /// user who wants to rule animation cost in/out of a responsiveness
    /// complaint, or who just prefers snappier motion.
    pub animations: Option<bool>,
    pub completion_symbols_file: Option<PathBuf>,
    /// Configured language server commands, `(language, command_line)`
    /// -- `[lsp]`'s `serverN = LANGUAGE|COMMAND_LINE`, same numbered-key
    /// list convention `mib_roots`/`jira_projects` already established.
    /// `LANGUAGE` matches `fenix_syntax::LanguageId`'s own name (e.g.
    /// `python`, `rust`); `COMMAND_LINE` is a plain space-separated
    /// program-plus-arguments string (e.g. `pyright-langserver
    /// --stdio`), split on whitespace at the point of use -- no shell
    /// quoting support, since every server this actually needs to
    /// launch takes simple flag-only arguments. A language with no
    /// entry here falls back to `fenix_lsp`'s own built-in default
    /// command for it, so the common case (the obvious server already
    /// on `PATH`) needs no configuration at all; this section exists
    /// for anyone who wants a different server, extra flags, or a
    /// language `fenix-lsp` has no built-in default for yet. Hand-
    /// authored by the user -- same re-read-fresh-before-writing
    /// protection as `vnc_hosts`/`documents`/`workspaces` (see `save`'s
    /// own doc comment), since nothing in this app writes to it itself.
    pub lsp_servers: Vec<(String, String)>,
    /// Configured SCOS-2000 MIB directories, `(label, path)`, in the
    /// order they appear in `config.ini`'s `[mib]` section -- an actual
    /// list, not an `Option<T>` like every other field here, since
    /// "nothing configured" is just an empty `Vec`, no need to
    /// distinguish that from "key present but empty" the way a scalar
    /// setting would. Hand-authored by the user (see `[mib]`'s own
    /// numbered-key format in `load`), never written by the app itself
    /// -- `save` still round-trips it losslessly since it regenerates
    /// every section from struct state on every call.
    pub mib_roots: Vec<(String, PathBuf)>,
    pub mib_telecommand_template: Option<String>,
    pub mib_telecommand_argument_template: Option<String>,
    pub mib_telecommand_argument_separator: Option<String>,
    /// Whether to notice files changing on disk while they're open and
    /// re-read them. Unset means on, which is what anyone expects; the
    /// setting exists for a working copy on a network share, where a
    /// `stat` every couple of seconds per open file is not free.
    pub watch_files: Option<bool>,
    /// The self-hosted Jira instance's own REST API root (e.g.
    /// `https://jira.mycompany.com`), and a personal access token for
    /// it -- plaintext, same as every other setting in this file (a
    /// deliberate choice, not an oversight: `fenix-jira`'s own design
    /// notes cover the tradeoff against an OS credential store).
    pub jira_base_url: Option<String>,
    pub jira_token: Option<String>,
    /// Tracked projects, `(key, display name)` -- same numbered-key
    /// `[jira]` list convention `mib_roots` already established, just a
    /// plain `(String, String)` pair instead of `(String, PathBuf)`.
    /// Hand-typed by the user (`SPC j p a`), not looked up against a
    /// live Jira API at add-time.
    pub jira_projects: Vec<(String, String)>,
    /// Tracked users, `(id, display name)` -- same shape/convention as
    /// `jira_projects`, e.g. `("jo1111111", "John Doe")`.
    pub jira_users: Vec<(String, String)>,
    /// The GitLab instance's own root URL (e.g.
    /// `https://gitlab.mycompany.com` -- the instance, *not* `/api/v4`,
    /// which `fenix-gitlab` appends itself) and a personal access token
    /// with `api` scope. Plaintext, same tradeoff `jira_token` already
    /// documents.
    ///
    /// There is deliberately no project setting: which project a repo
    /// belongs to is read from its own `origin` remote, so one pair of
    /// values covers every repo on the instance.
    pub gitlab_base_url: Option<String>,
    pub gitlab_token: Option<String>,
    /// Frequently-read documents, `(display name, path)`, in the order
    /// they appear in `config.ini`'s `[documents]` section -- what the
    /// reader's `SPC r f` index picks from. Same numbered-key `docN =
    /// NAME|PATH` convention (and same `Vec`-not-`Option` reasoning) as
    /// `mib_roots`; the path can be any file Fenix can open, not just a
    /// PDF, though a reference shelf is mostly PDFs in practice.
    /// Hand-authored by the user, never written by the app itself --
    /// `save` re-reads this section fresh from disk right before
    /// writing rather than trusting this struct's own (possibly
    /// session-old) copy, so a hand-edit made after this was loaded
    /// survives the next save instead of being silently erased by it.
    pub documents: Vec<(String, PathBuf)>,
    /// How many commits the History view's graph loads (`SPC g l`) --
    /// unset means 200, enough to cover recent work without making
    /// `git log --all` on a large repo feel slow.
    /// Directories worth a key rather than a walk -- `[explorer]`'s
    /// `bookmarkN = NAME|PATH`, the same numbered-pair convention
    /// `mib_roots`/`documents` already use.
    ///
    /// Driven by `self` on save (not re-read fresh like `vnc_hosts`),
    /// because unlike those this one genuinely has an in-app add flow:
    /// `SPC e m` bookmarks wherever you are. Hand-editing the file is
    /// still fine; it just has to happen between sessions, the same
    /// deal `mib_roots` has.
    pub explorer_bookmarks: Vec<(String, PathBuf)>,
    pub git_graph_limit: Option<usize>,
    /// The branch ref-comparison defaults its base to (`SPC g c`), e.g.
    /// `develop` -- unset means `main`. What "how does my branch differ
    /// from the mainline" means depends on the project's own convention,
    /// and there's no way to infer it reliably.
    pub git_base_branch: Option<String>,
    /// How the History view draws its commit graph: `ascii` (default)
    /// or `unicode`. Unicode looks better *if* the configured font has
    /// the box-drawing glyphs; when it doesn't, the fallback font's
    /// different advance width knocks every row out of alignment, which
    /// is why it isn't the default (see `graph_view::GraphStyle`).
    pub git_graph_style: Option<String>,
    /// Configured VNC hosts, `(name, host, port)` -- same numbered-key
    /// `[vnc]` list convention `mib_roots`/`jira_projects` already
    /// established, just a 3-field tuple instead of 2 (`parse_vnc_hosts`
    /// is its own sibling to `parse_pair_list` rather than reusing it,
    /// since `parse_pair_list` only splits one `|`). Hand-typed by the
    /// user (`SPC v v`), e.g. `("build-vm", "10.0.0.5", 5900)`. Same
    /// re-read-fresh-before-writing protection as `documents` -- see
    /// its own doc comment.
    pub vnc_hosts: Vec<(String, String, u16)>,
    /// Where each OS window sat when Fenix last exited, in the order
    /// they were open -- restored on the next launch when
    /// `restore_windows` is on. Unlike every other list here this one
    /// is written by the app, not hand-authored, so a two-monitor
    /// setup comes back the way it was left without arranging it
    /// again each morning.
    pub windows: Vec<WindowLayout>,
    /// Whether to reopen the windows recorded in `windows` at startup.
    /// `None` means on -- restoring what you had is the behaviour
    /// worth defaulting to; `restore_windows = false` opts out and
    /// always starts with a single window.
    pub restore_windows: Option<bool>,
    /// Named workspace launchers, `(display name, action)`, in the
    /// order they appear in `config.ini`'s `[workspaces]` section --
    /// what `SPC TAB f` picks from. `action` is one of `git`, `jira`,
    /// `docker` (opens the matching built-in panel; `docker` also
    /// covers Podman, since that panel autodetects the engine),
    /// `vnc:HOST` (a name from `[vnc]`), or anything else (including
    /// empty) for a plain workspace with no live session behind it --
    /// what "editor" would be. The actual parsing
    /// (`App::parse_workspace_action`) lives in `fenix-gui`, not here,
    /// since it dispatches to session state this crate has no business
    /// knowing about. Hand-authored by the user, same as `documents`
    /// -- including its own re-read-fresh-before-writing protection.
    pub workspaces: Vec<(String, String)>,
}

/// One OS window's remembered geometry: where its *outer* frame sat on
/// the desktop, how big its *inner* (client) area was, and whether it
/// was maximized.
///
/// That pairing isn't arbitrary -- it's the pair a window can actually
/// be restored from. `set_outer_position` and `with_inner_size` are
/// what winit offers, so recording anything else would mean converting
/// by the border and title-bar thickness on the way back, and getting
/// that wrong makes a window creep across the screen a little further
/// on every save-and-restore cycle.
///
/// Deliberately a plain rectangle rather than a monitor name plus an
/// offset into it. Monitor identifiers are long, platform-specific and
/// not stable across driver or dock changes, whereas a rectangle
/// degrades gracefully all by itself: a window whose saved position no
/// longer lands on any connected monitor just gets placed by the
/// window manager instead of opening off-screen (see
/// `App::restore_windows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowLayout {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Config {
    /// `dirs::config_dir()/fenix/config.ini` -- same location convention
    /// every other persisted file in this project already established.
    /// `None` only on a platform with no notion of a config directory.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("fenix").join("config.ini"))
    }

    /// Loads settings from `path`. A missing file means "nothing
    /// configured yet," not an error -- every field comes back `None`,
    /// same as `KnownProjects::load` starting with an empty list.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let sections = match std::fs::read_to_string(&path) {
            Ok(contents) => ini::parse(&contents),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Default::default(),
            Err(e) => return Err(e),
        };

        let editor = sections.get("editor");
        let completion = sections.get("completion");
        let lsp = sections.get("lsp");
        let mib = sections.get("mib");
        let jira = sections.get("jira");
        let git = sections.get("git");
        let gitlab = sections.get("gitlab");
        let vnc = sections.get("vnc");
        let documents = sections.get("documents");
        let windows = sections.get("windows");
        let workspaces = sections.get("workspaces");

        Ok(Self {
            path,
            theme: editor.and_then(|s| s.get("theme")).cloned(),
            font_size: editor.and_then(|s| s.get("font_size")).and_then(|v| v.parse().ok()),
            font_family: editor.and_then(|s| s.get("font_family")).cloned(),
            indent_width: editor.and_then(|s| s.get("indent_width")).and_then(|v| v.parse().ok()),
            tab_width: editor.and_then(|s| s.get("tab_width")).and_then(|v| v.parse().ok()),
            animations: editor.and_then(|s| s.get("animations")).and_then(|v| v.parse().ok()),
            completion_symbols_file: completion.and_then(|s| s.get("symbols_file")).map(PathBuf::from),
            lsp_servers: lsp.map(|s| parse_pair_list(s, "server")).unwrap_or_default(),
            mib_roots: mib.map(parse_mib_roots).unwrap_or_default(),
            explorer_bookmarks: sections
                .get("explorer")
                .map(|s| parse_pair_list(s, "bookmark").into_iter().map(|(name, path)| (name, PathBuf::from(path))).collect())
                .unwrap_or_default(),
            mib_telecommand_template: mib.and_then(|s| s.get("telecommand_template")).cloned(),
            mib_telecommand_argument_template: mib.and_then(|s| s.get("telecommand_argument_template")).cloned(),
            mib_telecommand_argument_separator: mib.and_then(|s| s.get("telecommand_argument_separator")).cloned(),
            watch_files: editor.and_then(|s| s.get("watch_files")).and_then(|v| v.parse().ok()),
            jira_base_url: jira.and_then(|s| s.get("base_url")).cloned(),
            jira_token: jira.and_then(|s| s.get("token")).cloned(),
            jira_projects: jira.map(|s| parse_pair_list(s, "project")).unwrap_or_default(),
            jira_users: jira.map(|s| parse_pair_list(s, "user")).unwrap_or_default(),
            git_graph_limit: git.and_then(|s| s.get("graph_limit")).and_then(|v| v.parse().ok()),
            git_base_branch: git.and_then(|s| s.get("base_branch")).cloned(),
            gitlab_base_url: gitlab.and_then(|s| s.get("base_url")).cloned(),
            gitlab_token: gitlab.and_then(|s| s.get("token")).cloned(),
            git_graph_style: git.and_then(|s| s.get("graph_style")).cloned(),
            vnc_hosts: vnc.map(parse_vnc_hosts).unwrap_or_default(),
            documents: documents.map(parse_documents).unwrap_or_default(),
            windows: windows.map(parse_windows).unwrap_or_default(),
            restore_windows: windows.and_then(|s| s.get("restore_windows")).and_then(|v| v.parse().ok()),
            workspaces: workspaces.map(|s| parse_pair_list(s, "ws")).unwrap_or_default(),
        })
    }

    /// Same as `load`, but never fails -- any read error (not just a
    /// missing file) just starts with every field `None`. A convenience
    /// preference, not critical data, same posture as `KnownProjects::
    /// load_or_default`.
    pub fn load_or_default(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or(Self {
            path,
            theme: None,
            font_size: None,
            font_family: None,
            indent_width: None,
            tab_width: None,
            animations: None,
            completion_symbols_file: None,
            lsp_servers: Vec::new(),
            mib_roots: Vec::new(),
            explorer_bookmarks: Vec::new(),
            mib_telecommand_template: None,
            mib_telecommand_argument_template: None,
            mib_telecommand_argument_separator: None,
            watch_files: None,
            jira_base_url: None,
            jira_token: None,
            jira_projects: Vec::new(),
            jira_users: Vec::new(),
            git_graph_limit: None,
            git_base_branch: None,
            gitlab_base_url: None,
            gitlab_token: None,
            git_graph_style: None,
            vnc_hosts: Vec::new(),
            documents: Vec::new(),
            windows: Vec::new(),
            restore_windows: None,
            workspaces: Vec::new(),
        })
    }

    /// Writes the known two-section, five-key layout, creating parent
    /// directories as needed -- only `Some` fields are written, so a
    /// setting cleared back to `None` doesn't leave a stale empty line
    /// behind on the next save. Not a generic INI writer: there's only
    /// ever this one shape to write, so building one would be unused
    /// machinery.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `[vnc]`/`[documents]`/`[workspaces]`/`[lsp]` are hand-edit-only
        // -- nothing in this app ever assigns to `vnc_hosts`/`documents`/
        // `workspaces`/`lsp_servers` itself (unlike `mib_roots`/
        // `jira_projects`/`jira_users`, which really do have an in-app
        // add/delete flow and so stay driven by `self` below). `self`'s
        // own copy of these four is only ever as fresh as whenever it
        // was loaded, which could be an entire session ago -- writing it
        // back verbatim would silently erase a host/document/launcher/
        // server entry added by hand-editing the file *after* that, the
        // moment anything else triggers a save (previously only a
        // deliberate settings change; now also every quit, once window-
        // layout persistence started saving automatically). Re-reading
        // them fresh from whatever's on disk right now, and falling back
        // to `self`'s own copy only if the file can't be read at all
        // (the very first save, nothing on disk yet), means a hand-edit
        // always wins instead of racing a stale in-memory copy.
        let (vnc_hosts, documents, workspaces, lsp_servers) = match std::fs::read_to_string(&self.path) {
            Ok(contents) => {
                let sections = ini::parse(&contents);
                let vnc_hosts = sections.get("vnc").map(parse_vnc_hosts).unwrap_or_default();
                let documents = sections.get("documents").map(parse_documents).unwrap_or_default();
                let workspaces = sections.get("workspaces").map(|s| parse_pair_list(s, "ws")).unwrap_or_default();
                let lsp_servers = sections.get("lsp").map(|s| parse_pair_list(s, "server")).unwrap_or_default();
                (vnc_hosts, documents, workspaces, lsp_servers)
            }
            Err(_) => (self.vnc_hosts.clone(), self.documents.clone(), self.workspaces.clone(), self.lsp_servers.clone()),
        };
        let mut out = String::new();
        out.push_str("[editor]\n");
        if let Some(theme) = &self.theme {
            out.push_str(&format!("theme = {}\n", ini::quote_if_needed(theme)));
        }
        if let Some(font_size) = self.font_size {
            out.push_str(&format!("font_size = {font_size}\n"));
        }
        if let Some(font_family) = &self.font_family {
            out.push_str(&format!("font_family = {}\n", ini::quote_if_needed(font_family)));
        }
        if let Some(indent_width) = self.indent_width {
            out.push_str(&format!("indent_width = {indent_width}\n"));
        }
        if let Some(tab_width) = self.tab_width {
            out.push_str(&format!("tab_width = {tab_width}\n"));
        }
        if let Some(watch) = self.watch_files {
            out.push_str(&format!("watch_files = {watch}
"));
        }
        if let Some(animations) = self.animations {
            out.push_str(&format!("animations = {animations}\n"));
        }
        out.push('\n');
        out.push_str("[completion]\n");
        if let Some(symbols_file) = &self.completion_symbols_file {
            out.push_str(&format!("symbols_file = {}\n", symbols_file.display()));
        }
        out.push('\n');
        out.push_str("[lsp]\n");
        for (i, (language, command_line)) in lsp_servers.iter().enumerate() {
            out.push_str(&format!("server{} = {language}|{command_line}\n", i + 1));
        }
        out.push('\n');
        out.push_str("[explorer]\n");
        for (i, (name, path)) in self.explorer_bookmarks.iter().enumerate() {
            out.push_str(&format!("bookmark{} = {name}|{}\n", i + 1, path.display()));
        }
        out.push('\n');
        out.push_str("[mib]\n");
        for (i, (label, root_path)) in self.mib_roots.iter().enumerate() {
            out.push_str(&format!("root{} = {label}|{}\n", i + 1, root_path.display()));
        }
        if let Some(template) = &self.mib_telecommand_template {
            out.push_str(&format!("telecommand_template = {}\n", ini::quote_if_needed(template)));
        }
        if let Some(template) = &self.mib_telecommand_argument_template {
            out.push_str(&format!("telecommand_argument_template = {}\n", ini::quote_if_needed(template)));
        }
        if let Some(separator) = &self.mib_telecommand_argument_separator {
            out.push_str(&format!("telecommand_argument_separator = {}\n", ini::quote_if_needed(separator)));
        }
        out.push('\n');
        out.push_str("[jira]\n");
        if let Some(base_url) = &self.jira_base_url {
            out.push_str(&format!("base_url = {}\n", ini::quote_if_needed(base_url)));
        }
        if let Some(token) = &self.jira_token {
            out.push_str(&format!("token = {}\n", ini::quote_if_needed(token)));
        }
        for (i, (key, name)) in self.jira_projects.iter().enumerate() {
            out.push_str(&format!("project{} = {key}|{name}\n", i + 1));
        }
        for (i, (id, name)) in self.jira_users.iter().enumerate() {
            out.push_str(&format!("user{} = {id}|{name}\n", i + 1));
        }
        out.push('\n');
        out.push_str("[git]\n");
        if let Some(limit) = self.git_graph_limit {
            out.push_str(&format!("graph_limit = {limit}\n"));
        }
        if let Some(base) = &self.git_base_branch {
            out.push_str(&format!("base_branch = {}\n", ini::quote_if_needed(base)));
        }
        out.push('\n');
        out.push_str("[gitlab]\n");
        if let Some(url) = &self.gitlab_base_url {
            out.push_str(&format!("base_url = {}\n", ini::quote_if_needed(url)));
        }
        if let Some(token) = &self.gitlab_token {
            out.push_str(&format!("token = {}\n", ini::quote_if_needed(token)));
        }
        out.push('\n');
        out.push_str("[vnc]\n");
        for (i, (name, host, port)) in vnc_hosts.iter().enumerate() {
            out.push_str(&format!("host{} = {name}|{host}|{port}\n", i + 1));
        }
        out.push('\n');
        out.push_str("[documents]\n");
        for (i, (name, doc_path)) in documents.iter().enumerate() {
            out.push_str(&format!("doc{} = {name}|{}\n", i + 1, doc_path.display()));
        }
        out.push('\n');
        out.push_str("[workspaces]\n");
        for (i, (name, action)) in workspaces.iter().enumerate() {
            out.push_str(&format!("ws{} = {name}|{action}\n", i + 1));
        }
        out.push('\n');
        out.push_str("[windows]\n");
        if let Some(restore) = self.restore_windows {
            out.push_str(&format!("restore_windows = {restore}\n"));
        }
        for (i, window) in self.windows.iter().enumerate() {
            out.push_str(&format!(
                "window{} = {},{},{},{}|{}\n",
                i + 1,
                window.x,
                window.y,
                window.width,
                window.height,
                window.maximized
            ));
        }
        std::fs::write(&self.path, out)
    }

    #[cfg(test)]
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// Parses the `[mib]` section's `root1 = LABEL|PATH`, `root2 = ...`
/// numbered keys into an ordered `(label, path)` list. Numbered rather
/// than one `roots = ...` key: the INI parser (`ini::parse`) is single-
/// value-per-key, so a growable list needs its own key per entry, the
/// same convention plenty of other hand-rolled INI readers use for
/// lists. `|` splits label from path (not `:`, which Windows paths
/// contain); any key that isn't `rootN`, or a value with no `|`, is
/// silently skipped -- same "a bad entry loses only itself" posture
/// every other field in this file already has. Sorted by the numeric
/// ordinal, not by key string, so `root2` sorts before `root10`.
fn parse_mib_roots(section: &std::collections::BTreeMap<String, String>) -> Vec<(String, PathBuf)> {
    parse_pair_list(section, "root").into_iter().map(|(label, path)| (label, PathBuf::from(path))).collect()
}

/// Parses a numbered-key `{prefix}1 = a|b`, `{prefix}2 = a|b`, ... list
/// into an ordered `Vec<(String, String)>`, sorted by the numeric
/// ordinal (not the key string, so `{prefix}2` sorts before `{prefix}10`)
/// -- the shared engine behind `parse_mib_roots` (which further maps
/// the second field into a `PathBuf`) and the `[jira]` section's own
/// `project`/`user` lists, which need this exact `(String, String)`
/// shape directly. `|` splits the two halves (not `:`, which a Windows
/// path -- `parse_mib_roots`'s own second field -- can contain); any
/// key that doesn't match `{prefix}N`, or a value with no `|`, is
/// silently skipped, same "a bad entry loses only itself" posture every
/// other field in this file already has.
/// Parses the `[vnc]` section's `host1 = NAME|HOST|PORT`, `host2 = ...`
/// numbered keys into an ordered `(name, host, port)` list -- the same
/// numbered-key convention as `parse_mib_roots`/`parse_pair_list`, just a
/// 3-field split (those only handle two fields) since a VNC target needs
/// a display name, an address, and a port. Sorted by the numeric ordinal,
/// not the key string. Any key that isn't `hostN`, a value with fewer
/// than 3 `|`-separated fields, or an unparsable port is silently
/// skipped -- same "a bad entry loses only itself" posture every other
/// field in this file already has.
/// Parses the `[documents]` section's `doc1 = NAME|PATH`, `doc2 = ...`
/// numbered keys into an ordered `(name, path)` list -- the same shape
/// and reasoning as `parse_mib_roots`, just a different key prefix and a
/// user-facing display name rather than an internal label. `|` splits
/// name from path (not `:`, which Windows paths contain); a key that
/// isn't `docN`, or a value with no `|`, is silently skipped.
fn parse_documents(section: &std::collections::BTreeMap<String, String>) -> Vec<(String, PathBuf)> {
    parse_pair_list(section, "doc").into_iter().map(|(name, path)| (name, PathBuf::from(path))).collect()
}

fn parse_vnc_hosts(section: &std::collections::BTreeMap<String, String>) -> Vec<(String, String, u16)> {
    let mut hosts: Vec<(usize, String, String, u16)> = section
        .iter()
        .filter_map(|(key, value)| {
            let n = key.strip_prefix("host")?.parse::<usize>().ok()?;
            let mut parts = value.splitn(3, '|');
            let name = parts.next()?.trim().to_string();
            let host = parts.next()?.trim().to_string();
            let port: u16 = parts.next()?.trim().parse().ok()?;
            Some((n, name, host, port))
        })
        .collect();
    hosts.sort_by_key(|(n, ..)| *n);
    hosts.into_iter().map(|(_, name, host, port)| (name, host, port)).collect()
}

/// `windowN = X,Y,WIDTH,HEIGHT|MAXIMIZED`, ordinal-ordered the same
/// way every other numbered-key list here is. An entry that doesn't
/// parse is skipped rather than failing the whole load -- one
/// hand-mangled line shouldn't cost you the rest of your layout.
fn parse_windows(section: &std::collections::BTreeMap<String, String>) -> Vec<WindowLayout> {
    let mut windows: Vec<(usize, WindowLayout)> = section
        .iter()
        .filter_map(|(key, value)| {
            let n = key.strip_prefix("window")?.parse::<usize>().ok()?;
            let (rect, maximized) = value.split_once('|')?;
            let mut parts = rect.split(',');
            let x = parts.next()?.trim().parse().ok()?;
            let y = parts.next()?.trim().parse().ok()?;
            let width = parts.next()?.trim().parse().ok()?;
            let height = parts.next()?.trim().parse().ok()?;
            let maximized = maximized.trim().parse().ok()?;
            Some((n, WindowLayout { x, y, width, height, maximized }))
        })
        .collect();
    windows.sort_by_key(|(n, _)| *n);
    windows.into_iter().map(|(_, window)| window).collect()
}

fn parse_pair_list(section: &std::collections::BTreeMap<String, String>, prefix: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(usize, String, String)> = section
        .iter()
        .filter_map(|(key, value)| {
            let n = key.strip_prefix(prefix)?.parse::<usize>().ok()?;
            let (a, b) = value.split_once('|')?;
            Some((n, a.trim().to_string(), b.trim().to_string()))
        })
        .collect();
    pairs.sort_by_key(|(n, _, _)| *n);
    pairs.into_iter().map(|(_, a, b)| (a, b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("fenix-config-test-{name}-{}-{n}.ini", std::process::id()))
    }

    #[test]
    fn documents_are_parsed_in_ordinal_order_not_key_string_order() {
        let path = temp_path("documents");
        std::fs::write(
            &path,
            "[documents]\ndoc2 = Time Codes|/refs/301x0b4.pdf\ndoc10 = Tenth|/refs/tenth.pdf\ndoc1 = Space Packet Protocol|C:/refs/133x0b2e2.pdf\n",
        )
        .unwrap();

        let config = Config::load(path.clone()).unwrap();

        let names: Vec<&str> = config.documents.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["Space Packet Protocol", "Time Codes", "Tenth"]);
        // A Windows path keeps its drive-letter colon -- `|`, not `:`,
        // is what splits the name from the path.
        assert_eq!(config.documents[0].1, PathBuf::from("C:/refs/133x0b2e2.pdf"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_document_entry_with_no_pipe_is_skipped_without_losing_the_others() {
        let path = temp_path("documents_bad");
        std::fs::write(&path, "[documents]\ndoc1 = Good|/refs/a.pdf\ndoc2 = no-pipe-here\ndoc3 = Also Good|/refs/b.pdf\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        let names: Vec<&str> = config.documents.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["Good", "Also Good"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn documents_round_trip_through_save_and_load() {
        let path = temp_path("documents_round_trip");
        let mut config = Config::load(path.clone()).unwrap();
        config.documents = vec![
            ("Space Packet Protocol".to_string(), PathBuf::from("C:/refs/133x0b2e2.pdf")),
            ("Notes".to_string(), PathBuf::from("/refs/notes.md")),
        ];

        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();

        assert_eq!(reloaded.documents, config.documents);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn windows_round_trip_through_save_and_load() {
        let path = temp_path("windows_round_trip");
        let mut config = Config::load(path.clone()).unwrap();
        config.windows = vec![
            WindowLayout { x: 0, y: 0, width: 2560, height: 1440, maximized: true },
            // A monitor to the left of the primary one sits at a
            // negative x on Windows.
            WindowLayout { x: -1920, y: -120, width: 1920, height: 1080, maximized: false },
        ];
        config.restore_windows = Some(false);

        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();

        assert_eq!(reloaded.windows, config.windows);
        assert_eq!(reloaded.restore_windows, Some(false));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn saving_does_not_erase_a_vnc_host_hand_added_to_the_file_after_load() {
        // The actual reported bug: something else in the app (window-
        // layout persistence, a theme change, ...) triggers a save
        // with an in-memory `Config` that's older than the file on
        // disk -- a `[vnc]` entry added by hand-editing config.ini
        // *after* this `Config` was loaded must not be wiped out by
        // that save.
        let path = temp_path("vnc_survives_stale_save");
        let mut config = Config::load(path.clone()).unwrap();
        assert!(config.vnc_hosts.is_empty(), "nothing configured yet at load time");

        // Simulate a hand-edit landing on disk while this `Config` is
        // still the old, vnc-hosts-empty one in memory.
        std::fs::write(&path, "[vnc]\nhost1 = build-vm|10.0.0.5|5900\n").unwrap();

        // An unrelated save -- window-layout persistence is exactly
        // this shape: it only ever touches `windows`, never `vnc_hosts`.
        config.windows = vec![WindowLayout { x: 0, y: 0, width: 800, height: 600, maximized: false }];
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.vnc_hosts, vec![("build-vm".to_string(), "10.0.0.5".to_string(), 5900)]);
        assert_eq!(reloaded.windows, config.windows, "the actual save this call was for should still take effect");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn saving_does_not_erase_a_hand_edited_document_or_workspace_entry_either() {
        let path = temp_path("documents_and_workspaces_survive_stale_save");
        let mut config = Config::load(path.clone()).unwrap();

        std::fs::write(
            &path,
            "[documents]\ndoc1 = Notes|C:/refs/notes.md\n\n[workspaces]\nws1 = Editor|\n",
        )
        .unwrap();

        config.theme = Some("Nord".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.documents, vec![("Notes".to_string(), PathBuf::from("C:/refs/notes.md"))]);
        assert_eq!(reloaded.workspaces, vec![("Editor".to_string(), String::new())]);
        assert_eq!(reloaded.theme, Some("Nord".to_string()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn saving_does_not_erase_an_lsp_server_hand_added_to_the_file_after_load() {
        let path = temp_path("lsp_servers_survive_stale_save");
        let mut config = Config::load(path.clone()).unwrap();

        std::fs::write(&path, "[lsp]\nserver1 = python|pyright-langserver --stdio\n").unwrap();

        config.theme = Some("Nord".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.lsp_servers, vec![("python".to_string(), "pyright-langserver --stdio".to_string())]);
        assert_eq!(reloaded.theme, Some("Nord".to_string()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lsp_servers_round_trip_through_save_and_load() {
        let path = temp_path("lsp_servers_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.lsp_servers = vec![("python".to_string(), "pyright-langserver --stdio".to_string()), ("rust".to_string(), "rust-analyzer".to_string())];
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(
            reloaded.lsp_servers,
            vec![("python".to_string(), "pyright-langserver --stdio".to_string()), ("rust".to_string(), "rust-analyzer".to_string())]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lsp_servers_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("lsp_servers_ordinal_order");
        std::fs::write(&path, "[lsp]\nserver2 = second|cmd-two\nserver10 = tenth|cmd-ten\nserver1 = first|cmd-one\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.lsp_servers,
            vec![("first".to_string(), "cmd-one".to_string()), ("second".to_string(), "cmd-two".to_string()), ("tenth".to_string(), "cmd-ten".to_string())]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_still_persists_an_in_app_change_to_mib_roots_or_jira_lists() {
        // The re-read-from-disk protection is specifically scoped to
        // `vnc_hosts`/`documents`/`workspaces` -- `mib_roots`/
        // `jira_projects`/`jira_users` really do have an in-app add/
        // delete flow (`SPC m a`/`SPC j p a`/...) and must keep coming
        // from `self`, or that flow would stop working.
        let path = temp_path("mib_and_jira_still_save_from_self");
        let mut config = Config::load(path.clone()).unwrap();
        config.mib_roots = vec![("MIB-A".to_string(), PathBuf::from("C:/data/mib-a"))];
        config.jira_projects = vec![("PROJ".to_string(), "My Project".to_string())];

        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.mib_roots, config.mib_roots);
        assert_eq!(reloaded.jira_projects, config.jira_projects);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn windows_are_ordered_by_their_key_ordinal_not_alphabetically() {
        let path = temp_path("windows_ordinal");
        // `window10` sorts before `window2` as text -- the ordinal has
        // to be parsed, not compared as a string.
        std::fs::write(
            &path,
            "[windows]\nwindow10 = 100,0,800,600|false\nwindow2 = 200,0,800,600|false\nwindow1 = 300,0,800,600|false\n",
        )
        .unwrap();

        let config = Config::load(path.clone()).unwrap();

        let xs: Vec<i32> = config.windows.iter().map(|w| w.x).collect();
        assert_eq!(xs, vec![300, 200, 100]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_mangled_window_entry_is_skipped_without_losing_the_rest_of_the_layout() {
        let path = temp_path("windows_mangled");
        std::fs::write(
            &path,
            "[windows]\nwindow1 = 0,0,800,600|true\nwindow2 = not-a-rectangle\nwindow3 = 10,20,30|false\nwindow4 = 5,5,640,480|false\n",
        )
        .unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.windows,
            vec![
                WindowLayout { x: 0, y: 0, width: 800, height: 600, maximized: true },
                WindowLayout { x: 5, y: 5, width: 640, height: 480, maximized: false },
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn no_windows_section_means_nothing_recorded_and_no_opt_out() {
        let config = Config::load(temp_path("windows_absent")).unwrap();
        assert!(config.windows.is_empty());
        assert_eq!(config.restore_windows, None, "unset means restore, which is the default the app applies");
    }

    #[test]
    fn workspaces_round_trip_through_save_and_load() {
        let path = temp_path("workspaces_round_trip");
        let mut config = Config::load(path.clone()).unwrap();
        config.workspaces = vec![
            ("Git".to_string(), "git".to_string()),
            ("VNC Build".to_string(), "vnc:build-vm".to_string()),
            ("Editor".to_string(), String::new()),
        ];

        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();

        assert_eq!(reloaded.workspaces, config.workspaces);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn workspaces_are_ordered_by_their_key_ordinal_not_alphabetically() {
        let path = temp_path("workspaces_ordinal");
        std::fs::write(&path, "[workspaces]\nws10 = Tenth|docker\nws2 = Second|git\nws1 = First|jira\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        let names: Vec<&str> = config.workspaces.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Tenth"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn no_workspaces_section_means_an_empty_launcher_list() {
        let config = Config::load(temp_path("workspaces_absent")).unwrap();
        assert!(config.workspaces.is_empty());
    }

    #[test]
    fn loading_a_missing_file_yields_every_field_none_not_an_error() {
        let config = Config::load(temp_path("missing")).unwrap();
        assert!(config.theme.is_none());
        assert!(config.font_size.is_none());
        assert!(config.font_family.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.tab_width.is_none());
        assert!(config.animations.is_none());
        assert!(config.completion_symbols_file.is_none());
        assert!(config.mib_roots.is_empty());
        assert!(config.mib_telecommand_template.is_none());
        assert!(config.mib_telecommand_argument_template.is_none());
        assert!(config.mib_telecommand_argument_separator.is_none());
        assert!(config.vnc_hosts.is_empty());
    }

    #[test]
    fn load_or_default_never_fails_even_when_the_path_is_unreadable() {
        let path = temp_path("unreadable");
        std::fs::create_dir(&path).unwrap(); // a directory, not a file -- read_to_string fails non-NotFound
        assert!(Config::load(path.clone()).is_err());
        let config = Config::load_or_default(path.clone());
        assert!(config.theme.is_none());
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn all_five_fields_round_trip_through_save_and_load() {
        let path = temp_path("round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("TempleOS".to_string());
        config.font_size = Some(18.0);
        config.font_family = Some("Fira Code".to_string());
        config.indent_width = Some(2);
        config.tab_width = Some(4);
        config.completion_symbols_file = Some(PathBuf::from("/home/thomas/tcl-symbols.txt"));
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.theme, Some("TempleOS".to_string()));
        assert_eq!(reloaded.font_size, Some(18.0));
        assert_eq!(reloaded.font_family, Some("Fira Code".to_string()));
        assert_eq!(reloaded.indent_width, Some(2));
        assert_eq!(reloaded.tab_width, Some(4));
        assert_eq!(reloaded.completion_symbols_file, Some(PathBuf::from("/home/thomas/tcl-symbols.txt")));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mib_roots_and_templates_round_trip_through_save_and_load() {
        let path = temp_path("mib_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.mib_roots = vec![
            ("MIB-A".to_string(), PathBuf::from("/data/mib-a")),
            ("MIB-B".to_string(), PathBuf::from("/data/mib-b")),
        ];
        config.mib_telecommand_template = Some("TC {mnemo}".to_string());
        config.mib_telecommand_argument_template = Some("{name}:{value}".to_string());
        config.mib_telecommand_argument_separator = Some(";".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(
            reloaded.mib_roots,
            vec![("MIB-A".to_string(), PathBuf::from("/data/mib-a")), ("MIB-B".to_string(), PathBuf::from("/data/mib-b"))]
        );
        assert_eq!(reloaded.mib_telecommand_template, Some("TC {mnemo}".to_string()));
        assert_eq!(reloaded.mib_telecommand_argument_template, Some("{name}:{value}".to_string()));
        assert_eq!(reloaded.mib_telecommand_argument_separator, Some(";".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn animations_setting_round_trips_through_save_and_load() {
        let path = temp_path("animations_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.animations = Some(false);
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.animations, Some(false));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn jira_settings_round_trip_through_save_and_load() {
        let path = temp_path("jira_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.jira_base_url = Some("https://jira.example.com".to_string());
        config.jira_token = Some("secret-token-value".to_string());
        config.jira_projects = vec![("PROJ".to_string(), "My Project".to_string()), ("OTHER".to_string(), "Other Project".to_string())];
        config.jira_users = vec![("jo1111111".to_string(), "John Doe".to_string())];
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.jira_base_url, Some("https://jira.example.com".to_string()));
        assert_eq!(reloaded.jira_token, Some("secret-token-value".to_string()));
        assert_eq!(
            reloaded.jira_projects,
            vec![("PROJ".to_string(), "My Project".to_string()), ("OTHER".to_string(), "Other Project".to_string())]
        );
        assert_eq!(reloaded.jira_users, vec![("jo1111111".to_string(), "John Doe".to_string())]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn jira_projects_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("jira_ordinal_order");
        let mut config = Config::load_or_default(path.clone());
        // 10 project entries so lexical-vs-numeric key ordering actually
        // differs ("project10" sorts before "project2" as plain strings).
        config.jira_projects = (1..=10).map(|i| (format!("P{i}"), format!("Project {i}"))).collect();
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.jira_projects, config.jira_projects);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_whitespace_only_argument_separator_round_trips_through_save_and_load() {
        // The specific gap `ini::quote_if_needed`/`ini::parse`'s quote
        // handling exists to close: a separator that's pure whitespace
        // (a single space, here) previously trimmed away to nothing on
        // every save/load round trip -- there was no way to configure
        // one at all.
        let path = temp_path("whitespace_separator_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.mib_telecommand_argument_separator = Some(" ".to_string());
        config.save().unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("telecommand_argument_separator = \" \""),
            "expected the written value to be quoted, got:\n{contents}"
        );

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.mib_telecommand_argument_separator, Some(" ".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mib_roots_survive_a_save_triggered_by_an_unrelated_field_change() {
        // Regression guard: `save` regenerates every section from struct
        // state on every call, so a hand-authored `[mib]` section must
        // still be in the in-memory `Config` (loaded once at startup) or
        // an unrelated theme-cycle save would silently wipe it from disk.
        let path = temp_path("mib_survives_unrelated_save");
        std::fs::write(&path, "[mib]\nroot1 = MIB-A|/data/mib-a\n").unwrap();
        let mut config = Config::load(path.clone()).unwrap();
        config.theme = Some("Nord".to_string()); // unrelated change
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.mib_roots, vec![("MIB-A".to_string(), PathBuf::from("/data/mib-a"))]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mib_roots_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("mib_root_order");
        // Deliberately written out of string order (root10 sorts before
        // root2 as a plain string) to prove numeric ordering is used.
        std::fs::write(&path, "[mib]\nroot2 = SECOND|/b\nroot10 = TENTH|/j\nroot1 = FIRST|/a\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.mib_roots,
            vec![
                ("FIRST".to_string(), PathBuf::from("/a")),
                ("SECOND".to_string(), PathBuf::from("/b")),
                ("TENTH".to_string(), PathBuf::from("/j")),
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_gitlab_section_round_trips_through_a_save() {
        let path = temp_path("config_gitlab");
        let mut config = Config::load_or_default(path.clone());
        config.gitlab_base_url = Some("https://gitlab.example.com".to_string());
        config.gitlab_token = Some("glpat-secret".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.gitlab_base_url.as_deref(), Some("https://gitlab.example.com"));
        assert_eq!(reloaded.gitlab_token.as_deref(), Some("glpat-secret"));
        // No project key: which project a repo belongs to comes from
        // its own `origin` remote, so one pair covers every repo.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[gitlab]"), "got:
{text}");
        assert!(!text.contains("project ="), "got:
{text}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn git_settings_round_trip_through_save_and_load() {
        let path = temp_path("git_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.git_graph_limit = Some(500);
        config.git_base_branch = Some("develop".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.git_graph_limit, Some(500));
        assert_eq!(reloaded.git_base_branch, Some("develop".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn git_settings_are_none_when_the_section_is_absent() {
        let config = Config::load(temp_path("git_absent")).unwrap();
        assert_eq!(config.git_graph_limit, None);
        assert_eq!(config.git_base_branch, None);
    }

    #[test]
    fn explorer_bookmarks_round_trip_through_save_and_load() {
        // Unlike `vnc_hosts`, these are written by the app -- `SPC e m`
        // bookmarks wherever you are -- so a save has to carry them.
        let path = temp_path("explorer_bookmarks_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.explorer_bookmarks =
            vec![("nas".to_string(), PathBuf::from(r"\\nas\media")), ("work".to_string(), PathBuf::from(r"C:\work"))];

        config.save().unwrap();

        let reloaded = Config::load(path).unwrap();
        assert_eq!(reloaded.explorer_bookmarks, config.explorer_bookmarks);
    }

    #[test]
    fn vnc_hosts_round_trip_through_save_and_load() {
        let path = temp_path("vnc_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.vnc_hosts = vec![("build-vm".to_string(), "10.0.0.5".to_string(), 5900), ("test-vm".to_string(), "10.0.0.6".to_string(), 5901)];
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(
            reloaded.vnc_hosts,
            vec![("build-vm".to_string(), "10.0.0.5".to_string(), 5900), ("test-vm".to_string(), "10.0.0.6".to_string(), 5901)]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn vnc_hosts_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("vnc_ordinal_order");
        std::fs::write(&path, "[vnc]\nhost2 = second|10.0.0.2|5900\nhost10 = tenth|10.0.0.10|5900\nhost1 = first|10.0.0.1|5900\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.vnc_hosts,
            vec![
                ("first".to_string(), "10.0.0.1".to_string(), 5900),
                ("second".to_string(), "10.0.0.2".to_string(), 5900),
                ("tenth".to_string(), "10.0.0.10".to_string(), 5900),
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_vnc_host_entry_missing_a_field_is_skipped_not_an_error() {
        let path = temp_path("vnc_bad_entry");
        std::fs::write(&path, "[vnc]\nhost1 = good|10.0.0.1|5900\nhost2 = missing-port|10.0.0.2\nhost3 = also-good|10.0.0.3|5901\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(config.vnc_hosts, vec![("good".to_string(), "10.0.0.1".to_string(), 5900), ("also-good".to_string(), "10.0.0.3".to_string(), 5901)]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_vnc_host_entry_with_an_unparsable_port_is_skipped_not_an_error() {
        let path = temp_path("vnc_bad_port");
        std::fs::write(&path, "[vnc]\nhost1 = good|10.0.0.1|5900\nhost2 = bad-port|10.0.0.2|not-a-port\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(config.vnc_hosts, vec![("good".to_string(), "10.0.0.1".to_string(), 5900)]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_mib_root_entry_with_no_separator_is_skipped_not_an_error() {
        let path = temp_path("mib_root_bad_entry");
        std::fs::write(&path, "[mib]\nroot1 = MIB-A|/data/a\nroot2 = no-separator-here\nroot3 = MIB-C|/data/c\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.mib_roots,
            vec![("MIB-A".to_string(), PathBuf::from("/data/a")), ("MIB-C".to_string(), PathBuf::from("/data/c"))]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_corrupted_font_size_does_not_blank_out_the_other_settings() {
        let path = temp_path("partial_corruption");
        std::fs::write(&path, "[editor]\ntheme = TempleOS\nfont_size = not-a-number\nindent_width = 3\n").unwrap();

        let config = Config::load(path.clone()).unwrap();
        assert_eq!(config.theme, Some("TempleOS".to_string()));
        assert!(config.font_size.is_none()); // only this one is lost
        assert_eq!(config.indent_width, Some(3));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let path = temp_path("creates_parents").parent().unwrap().join("nested-fenix-config-test").join("config.ini");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("Orbit Dark".to_string());
        config.save().unwrap();
        assert!(path.exists());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn save_omits_keys_that_are_none_instead_of_writing_an_empty_value() {
        let path = temp_path("omits_none");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("Orbit Dark".to_string());
        config.save().unwrap();

        let contents = std::fs::read_to_string(config.path()).unwrap();
        assert!(!contents.contains("font_size"));
        assert!(!contents.contains("symbols_file"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn default_path_is_under_a_fenix_directory_named_config_dot_ini() {
        if let Some(path) = Config::default_path() {
            assert!(path.ends_with("fenix/config.ini") || path.ends_with("fenix\\config.ini"));
        }
    }
}
