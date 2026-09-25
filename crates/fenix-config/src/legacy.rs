//! The `config.ini` Fenix used before `settings.toml`, read once to move
//! it over (see `Config::migrate_ini`). Lists were numbered keys
//! (`root1 = name|path`), tokens were in the file, and window placement
//! and explorer bookmarks were kept in it too.

use std::io;
use std::path::PathBuf;

use crate::ini;
use crate::{names, Config, JiraBlocked, WindowLayout};

/// Everything `config.ini` at `path` held, as a `Config` whose own path
/// is `into` -- tokens included, so the caller can put them in the
/// credential store.
pub(crate) fn load(path: &std::path::Path, into: PathBuf, state_dir: PathBuf) -> io::Result<Config> {
    let sections = match std::fs::read_to_string(path) {
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
    let embedded = sections.get("embedded");

    let mut c = Config::empty(into, state_dir);
    c.theme = editor.and_then(|s| s.get("theme")).cloned();
    c.font_size = editor.and_then(|s| s.get("font_size")).and_then(|v| v.parse().ok());
    c.font_family = editor.and_then(|s| s.get("font_family")).cloned();
    c.indent_width = editor.and_then(|s| s.get("indent_width")).and_then(|v| v.parse().ok());
    c.iskeyword_extra = editor.and_then(|s| s.get("iskeyword_extra")).cloned();
    c.tab_width = editor.and_then(|s| s.get("tab_width")).and_then(|v| v.parse().ok());
    c.animations = editor.and_then(|s| s.get("animations")).and_then(|v| v.parse().ok());
    c.completion_symbols_file = completion.and_then(|s| s.get("symbols_file")).map(PathBuf::from);
    c.lsp_servers = lsp.map(|s| parse_pair_list(s, "server")).unwrap_or_default();
    c.mib_roots = mib.map(parse_mib_roots).unwrap_or_default();
    c.explorer_bookmarks = sections
        .get("explorer")
        .map(|s| parse_pair_list(s, "bookmark").into_iter().map(|(name, path)| (name, PathBuf::from(path))).collect())
        .unwrap_or_default();
    c.agenda_categories = sections.get("agenda").map(|s| parse_single_list(s, "category")).unwrap_or_default();
    c.agenda_worklog_round = sections.get("agenda").and_then(|s| s.get("worklog_round")).and_then(|v| parse_minutes(v));
    c.embedded_arduino_cli = embedded.and_then(|s| s.get("arduino_cli")).map(PathBuf::from);
    c.embedded_clangd = embedded.and_then(|s| s.get("clangd")).map(PathBuf::from);
    c.embedded_arduino_language_server = embedded.and_then(|s| s.get("arduino_language_server")).map(PathBuf::from);
    c.mib_telecommand_template = mib.and_then(|s| s.get("telecommand_template")).cloned();
    c.mib_telecommand_argument_template = mib.and_then(|s| s.get("telecommand_argument_template")).cloned();
    c.mib_telecommand_argument_separator = mib.and_then(|s| s.get("telecommand_argument_separator")).cloned();
    c.watch_files = editor.and_then(|s| s.get("watch_files")).and_then(|v| v.parse().ok());
    c.jira_base_url = jira.and_then(|s| s.get("base_url")).cloned();
    c.jira_token = jira.and_then(|s| s.get("token")).cloned();
    c.jira_projects = jira.map(|s| parse_pair_list(s, "project")).unwrap_or_default();
    c.jira_users = jira.map(|s| parse_pair_list(s, "user")).unwrap_or_default();
    c.jira_blocked = jira.map(parse_jira_blocked).unwrap_or_default();
    c.jira_priority_map = jira.map(|s| parse_pair_list(s, "priority")).unwrap_or_default();
    c.jira_sync_minutes = jira.and_then(|s| s.get("sync_minutes")).and_then(|v| v.trim().parse().ok());
    c.git_graph_limit = git.and_then(|s| s.get("graph_limit")).and_then(|v| v.parse().ok());
    c.git_base_branch = git.and_then(|s| s.get("base_branch")).cloned();
    c.gitlab_base_url = gitlab.and_then(|s| s.get("base_url")).cloned();
    c.gitlab_token = gitlab.and_then(|s| s.get("token")).cloned();
    c.github_token = sections.get("github").and_then(|s| s.get("token")).cloned();
    c.git_graph_style = git.and_then(|s| s.get("graph_style")).cloned();
    c.git_layout = git.and_then(|s| s.get("layout")).cloned();
    c.git_auto_fetch_minutes = git.and_then(|s| s.get("auto_fetch")).and_then(|v| v.trim().trim_end_matches('m').trim().parse().ok()).filter(|m| *m > 0);
    c.git_reviewers = git.and_then(|s| s.get("reviewers")).map(|v| names(v)).unwrap_or_default();
    c.vnc_hosts = vnc.map(parse_vnc_hosts).unwrap_or_default();
    c.documents = documents.map(parse_documents).unwrap_or_default();
    c.windows = windows.map(parse_windows).unwrap_or_default();
    c.restore_session = windows.and_then(|s| s.get("restore_session")).and_then(|v| v.parse().ok());
    c.workspace_per_project = windows.and_then(|s| s.get("workspace_per_project")).and_then(|v| v.parse().ok());
    c.restore_windows = windows.and_then(|s| s.get("restore_windows")).and_then(|v| v.parse().ok());
    c.workspaces = workspaces.map(|s| parse_pair_list(s, "ws")).unwrap_or_default();
    Ok(c)
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

/// `[jira]`'s `blockedN = PROJ|ID|NAME`, `PROJ|flag` or `PROJ|local`,
/// ordinal-ordered. Anything else is skipped.
fn parse_jira_blocked(section: &std::collections::BTreeMap<String, String>) -> Vec<(String, JiraBlocked)> {
    let mut entries: Vec<(usize, String, JiraBlocked)> = section
        .iter()
        .filter_map(|(key, value)| {
            let n = key.strip_prefix("blocked")?.parse::<usize>().ok()?;
            let mut parts = value.splitn(3, '|').map(str::trim);
            let project = parts.next()?.to_string();
            let blocked = match (parts.next()?, parts.next()) {
                ("flag", None) => JiraBlocked::Flag,
                ("local", None) => JiraBlocked::Local,
                (id, Some(name)) if !id.is_empty() => JiraBlocked::Status { id: id.to_string(), name: name.to_string() },
                _ => return None,
            };
            Some((n, project, blocked))
        })
        .collect();
    entries.sort_by_key(|(n, ..)| *n);
    entries.into_iter().map(|(_, project, blocked)| (project, blocked)).collect()
}

/// `15`, `15m`, `1h` -> minutes.
fn parse_minutes(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hours) = value.strip_suffix('h') {
        return hours.trim().parse::<u32>().ok().map(|h| h * 60);
    }
    value.strip_suffix('m').unwrap_or(value).trim().parse().ok()
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

/// Parses a numbered-key `{prefix}1 = a`, `{prefix}2 = a`, ... list into an
/// ordered `Vec<String>`, sorted by the numeric ordinal (not the key
/// string) -- the single-value sibling to `parse_pair_list`, for the
/// `[agenda]` section's `categoryN` list, which has no second `|`-separated
/// field to carry. A key that doesn't match `{prefix}N` is silently
/// skipped, same "a bad entry loses only itself" posture every other field
/// in this file already has.
fn parse_single_list(section: &std::collections::BTreeMap<String, String>, prefix: &str) -> Vec<String> {
    let mut entries: Vec<(usize, String)> = section
        .iter()
        .filter_map(|(key, value)| {
            let n = key.strip_prefix(prefix)?.parse::<usize>().ok()?;
            Some((n, value.trim().to_string()))
        })
        .collect();
    entries.sort_by_key(|(n, _)| *n);
    entries.into_iter().map(|(_, v)| v).collect()
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("fenix-config-ini-{name}-{}-{n}.ini", std::process::id()))
    }

    fn load_ini(path: &std::path::Path) -> Config {
        load(path, path.with_extension("toml"), path.with_extension("state")).unwrap()
    }

    #[test]
    fn documents_are_parsed_in_ordinal_order_not_key_string_order() {
        let path = temp_path("documents");
        std::fs::write(
            &path,
            "[documents]\ndoc2 = Time Codes|/refs/301x0b4.pdf\ndoc10 = Tenth|/refs/tenth.pdf\ndoc1 = Space Packet Protocol|C:/refs/133x0b2e2.pdf\n",
        )
        .unwrap();

        let config = load_ini(&path);

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

        let config = load_ini(&path);

        let names: Vec<&str> = config.documents.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["Good", "Also Good"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lsp_servers_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("lsp_servers_ordinal_order");
        std::fs::write(&path, "[lsp]\nserver2 = second|cmd-two\nserver10 = tenth|cmd-ten\nserver1 = first|cmd-one\n").unwrap();

        let config = load_ini(&path);

        assert_eq!(
            config.lsp_servers,
            vec![("first".to_string(), "cmd-one".to_string()), ("second".to_string(), "cmd-two".to_string()), ("tenth".to_string(), "cmd-ten".to_string())]
        );
        std::fs::remove_file(&path).ok();
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

        let config = load_ini(&path);

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

        let config = load_ini(&path);

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
    fn workspaces_are_ordered_by_their_key_ordinal_not_alphabetically() {
        let path = temp_path("workspaces_ordinal");
        std::fs::write(&path, "[workspaces]\nws10 = Tenth|docker\nws2 = Second|git\nws1 = First|jira\n").unwrap();

        let config = load_ini(&path);

        let names: Vec<&str> = config.workspaces.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Tenth"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mib_roots_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("mib_root_order");
        // Deliberately written out of string order (root10 sorts before
        // root2 as a plain string) to prove numeric ordering is used.
        std::fs::write(&path, "[mib]\nroot2 = SECOND|/b\nroot10 = TENTH|/j\nroot1 = FIRST|/a\n").unwrap();

        let config = load_ini(&path);

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
    fn agenda_categories_are_parsed_in_ordinal_order_not_key_string_order() {
        let path = temp_path("agenda_categories_ordinal");
        std::fs::write(&path, "[agenda]\ncategory2 = Personal\ncategory10 = Tenth\ncategory1 = Fenix\n").unwrap();

        let config = load_ini(&path);

        assert_eq!(config.agenda_categories, vec!["Fenix".to_string(), "Personal".to_string(), "Tenth".to_string()]);
    }

    #[test]
    fn vnc_hosts_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("vnc_ordinal_order");
        std::fs::write(&path, "[vnc]\nhost2 = second|10.0.0.2|5900\nhost10 = tenth|10.0.0.10|5900\nhost1 = first|10.0.0.1|5900\n").unwrap();

        let config = load_ini(&path);

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

        let config = load_ini(&path);

        assert_eq!(config.vnc_hosts, vec![("good".to_string(), "10.0.0.1".to_string(), 5900), ("also-good".to_string(), "10.0.0.3".to_string(), 5901)]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_vnc_host_entry_with_an_unparsable_port_is_skipped_not_an_error() {
        let path = temp_path("vnc_bad_port");
        std::fs::write(&path, "[vnc]\nhost1 = good|10.0.0.1|5900\nhost2 = bad-port|10.0.0.2|not-a-port\n").unwrap();

        let config = load_ini(&path);

        assert_eq!(config.vnc_hosts, vec![("good".to_string(), "10.0.0.1".to_string(), 5900)]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_mib_root_entry_with_no_separator_is_skipped_not_an_error() {
        let path = temp_path("mib_root_bad_entry");
        std::fs::write(&path, "[mib]\nroot1 = MIB-A|/data/a\nroot2 = no-separator-here\nroot3 = MIB-C|/data/c\n").unwrap();

        let config = load_ini(&path);

        assert_eq!(
            config.mib_roots,
            vec![("MIB-A".to_string(), PathBuf::from("/data/a")), ("MIB-C".to_string(), PathBuf::from("/data/c"))]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_blocked_entry_is_skipped() {
        let path = temp_path("jira_blocked_bad");
        std::fs::write(&path, "[jira]
blocked1 = PROJ
blocked2 = OPS|flag
blocked3 = X|weird
").unwrap();
        let config = load_ini(&path);
        assert_eq!(config.jira_blocked, vec![("OPS".to_string(), JiraBlocked::Flag)]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn worklog_round_accepts_minutes_and_hours() {
        assert_eq!(parse_minutes("15"), Some(15));
        assert_eq!(parse_minutes("30m"), Some(30));
        assert_eq!(parse_minutes("1h"), Some(60));
        assert_eq!(parse_minutes("soon"), None);
    }
}
