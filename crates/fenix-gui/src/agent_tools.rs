//! What agents may call through `fenix --mcp`, and what decides it: every
//! tool Fenix offers, the toolset it's in and its class, the user's rules
//! (`agents.mcp.allow`) and which toolsets an agent is shown
//! (`agents.mcp.toolsets`). Host-agnostic: the editor asks these
//! functions, then does what they say (`app/agent_calls.rs`).
//!
//! - A **class** says what a tool may do: read, move what's on screen,
//!   edit (always through the change ledger), run something, or act
//!   outside this machine.
//! - A **rule** (`allow`, `ask`, `deny`) is given per class, per toolset
//!   (`toolset:git`) or per tool; the most specific wins. A project's
//!   settings can only make a rule stricter.
//! - A **toolset** is a group of tools shown together. The agent sees
//!   the ones that fit the project, and `fenix_toolsets` turns on others.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

/// What a tool may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Reads what Fenix knows.
    Read,
    /// Moves what's on your screen.
    Show,
    /// Changes buffers or files, through the thread's change ledger.
    Edit,
    /// Starts processes: tasks, tests, commands, the debugger.
    Run,
    /// Acts outside this machine.
    External,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Read => "read",
            Class::Show => "show",
            Class::Edit => "edit",
            Class::Run => "run",
            Class::External => "external",
        }
    }

    pub fn parse(text: &str) -> Option<Class> {
        Some(match text.trim() {
            "read" => Class::Read,
            "show" => Class::Show,
            "edit" => Class::Edit,
            "run" => Class::Run,
            "external" => Class::External,
            _ => return None,
        })
    }

    /// What a class is, when nothing says otherwise. A client Fenix
    /// didn't start (`external_client`) may read and show, nothing more.
    pub fn default_rule(self, external_client: bool) -> Rule {
        match (self, external_client) {
            (Class::Read | Class::Show, _) => Rule::Allow,
            (Class::Edit | Class::Run, false) => Rule::Ask,
            (Class::Edit | Class::Run, true) | (Class::External, _) => Rule::Deny,
        }
    }
}

/// What happens when a tool is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    Allow,
    Ask,
    Deny,
}

impl Rule {
    pub fn parse(text: &str) -> Option<Rule> {
        Some(match text.trim() {
            "allow" => Rule::Allow,
            "ask" => Rule::Ask,
            "deny" => Rule::Deny,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Allow => "allow",
            Rule::Ask => "ask",
            Rule::Deny => "deny",
        }
    }
}

/// When a toolset is offered without being asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offered {
    Always,
    /// A language server is running for the project.
    WithLsp,
    /// The project is a Git repository.
    WithGit,
    /// The project has tasks.
    WithTasks,
    /// A debug session runs, or the project has launches.
    WithDebug,
    /// Only when turned on.
    Never,
}

pub struct Toolset {
    pub name: &'static str,
    pub description: &'static str,
    pub offered: Offered,
}

/// Fenix's own toolsets; each extension is one more, named as its tools'
/// prefix.
pub const TOOLSETS: &[Toolset] = &[
    Toolset { name: "core", description: "Problems, open files and their unsaved text, the selection, and the language servers' definitions, references, hover and symbols.", offered: Offered::Always },
    Toolset { name: "lsp", description: "More from the language servers: implementations, call and type hierarchies, signature help, code actions.", offered: Offered::WithLsp },
    Toolset { name: "refactor", description: "Edits by meaning: rename a symbol, apply a code action, format. Every change goes to the user's review.", offered: Offered::WithLsp },
    Toolset { name: "git", description: "The repository: status, diffs, log, a commit, blame, and staging.", offered: Offered::WithGit },
    Toolset { name: "search", description: "Search the project, unsaved text included and its ignores respected, and replace across it.", offered: Offered::Always },
    Toolset { name: "syntax", description: "The syntax tree: the node at a place and its parents, and tree-sitter queries.", offered: Offered::Always },
    Toolset { name: "tasks", description: "The project's tasks and tests: run one, wait for it, and read its problems and output.", offered: Offered::WithTasks },
    Toolset { name: "debug", description: "The debugger: launches, breakpoints, starting, stepping, running to a line, evaluating.", offered: Offered::WithDebug },
    Toolset { name: "commands", description: "Run any of Fenix's commands by id.", offered: Offered::Never },
];

/// A tool Fenix itself answers.
pub struct Tool {
    pub name: &'static str,
    pub toolset: &'static str,
    pub class: Class,
    pub description: &'static str,
    pub schema: fn() -> Value,
}

fn none() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn path_only() -> Value {
    json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] })
}

/// A place in a file: what the language tools take.
fn position() -> Value {
    json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer", "description": "From 1" }, "symbol": { "type": "string", "description": "The name on that line" }, "column": { "type": "integer", "description": "From 1" } }, "required": ["path", "line"] })
}

/// Every tool Fenix answers itself, in the order agents see them.
pub const TOOLS: &[Tool] = &[
    // core
    Tool { name: "diagnostics", toolset: "core", class: Class::Read, description: "The language servers' problems in one file (path), or every open file. Fresher than a build: they follow unsaved edits.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } } }) },
    Tool { name: "open_files", toolset: "core", class: Class::Read, description: "The files open in Fenix: which has the focus, and which have unsaved edits the disk doesn't have.", schema: none },
    Tool { name: "read_buffer", toolset: "core", class: Class::Read, description: "An open file as it stands in Fenix, unsaved edits included. Whole, or startLine to endLine (from 1).", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "startLine": { "type": "integer" }, "endLine": { "type": "integer" } }, "required": ["path"] }) },
    Tool { name: "selection", toolset: "core", class: Class::Read, description: "What the user has selected (or the cursor's line): its file, line and text.", schema: none },
    Tool { name: "terminal_output", toolset: "core", class: Class::Read, description: "What a terminal pane shows now: the user's shells, a server's output. Without terminal, the list of them; terminal is a number from it.", schema: || json!({ "type": "object", "properties": { "terminal": { "type": "integer", "minimum": 1 } } }) },
    Tool { name: "recent_locations", toolset: "core", class: Class::Read, description: "Where the user has been, newest first: the places they jumped from and to (files, lines, the text there), and the marks they set.", schema: || json!({ "type": "object", "properties": { "max": { "type": "integer" } } }) },
    Tool { name: "show_file", toolset: "core", class: Class::Show, description: "Shows the user a file at a line, to point at what you mean.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer", "minimum": 1 } }, "required": ["path"] }) },
    Tool { name: "definition", toolset: "core", class: Class::Read, description: "Where the symbol at a place is defined, from the language servers: exact, unlike searching.", schema: position },
    Tool { name: "references", toolset: "core", class: Class::Read, description: "Every use of the symbol at a place, across the project, with its line: what a change to it touches.", schema: position },
    Tool { name: "hover", toolset: "core", class: Class::Read, description: "The type and documentation of the symbol at a place.", schema: position },
    Tool { name: "document_symbols", toolset: "core", class: Class::Read, description: "A file's outline: its types, functions and so on, with their lines.", schema: path_only },
    Tool { name: "workspace_symbols", toolset: "core", class: Class::Read, description: "Finds types, functions and so on by name across the project.", schema: || json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }) },
    Tool { name: "fenix_toolsets", toolset: "core", class: Class::Read, description: "Lists Fenix's other toolsets (git, debug, tasks, refactor, search, extensions'...) and turns one on: {\"enable\": name}. Its tools are then listed, or callable through fenix_call.", schema: || json!({ "type": "object", "properties": { "enable": { "type": "string" }, "disable": { "type": "string" } } }) },
    Tool { name: "fenix_call", toolset: "core", class: Class::Read, description: "Calls any Fenix tool by name, for a toolset turned on with fenix_toolsets that isn't listed yet.", schema: || json!({ "type": "object", "properties": { "tool": { "type": "string" }, "arguments": { "type": "object" } }, "required": ["tool"] }) },
    // lsp
    Tool { name: "implementations", toolset: "lsp", class: Class::Read, description: "Where a trait, interface or abstract method at a place is implemented.", schema: position },
    Tool { name: "call_hierarchy", toolset: "lsp", class: Class::Read, description: "Who calls the function at a place (direction in, the default), or what it calls (out).", schema: || { let mut s = position(); s["properties"]["direction"] = json!({ "enum": ["in", "out"] }); s } },
    Tool { name: "type_hierarchy", toolset: "lsp", class: Class::Read, description: "The supertypes of the type at a place (direction super, the default), or its subtypes (sub).", schema: || { let mut s = position(); s["properties"]["direction"] = json!({ "enum": ["super", "sub"] }); s } },
    Tool { name: "signature_help", toolset: "lsp", class: Class::Read, description: "The signature, and the active parameter, of the call at a place (line, column).", schema: position },
    Tool { name: "code_actions", toolset: "lsp", class: Class::Read, description: "The language servers' code actions (fixes, refactorings) on line to endLine, each with an id for apply_code_action.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" }, "endLine": { "type": "integer" } }, "required": ["path", "line"] }) },
    // refactor
    Tool { name: "rename", toolset: "refactor", class: Class::Edit, description: "Renames the symbol at a place everywhere it's used, by the language server: exact, unlike a text replace.", schema: || { let mut s = position(); s["properties"]["new_name"] = json!({ "type": "string" }); s["required"] = json!(["path", "line", "new_name"]); s } },
    Tool { name: "apply_code_action", toolset: "refactor", class: Class::Edit, description: "Applies a code action by the id code_actions gave.", schema: || json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }) },
    Tool { name: "format", toolset: "refactor", class: Class::Edit, description: "Formats a file, or startLine to endLine, with its language server.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "startLine": { "type": "integer" }, "endLine": { "type": "integer" } }, "required": ["path"] }) },
    // git
    Tool { name: "git_status", toolset: "git", class: Class::Read, description: "The repository's branch, upstream, ahead/behind, conflicts, and each changed file, staged and unstaged.", schema: none },
    Tool { name: "git_diff", toolset: "git", class: Class::Read, description: "A diff: unstaged by default, staged with staged, or a branch's whole change against base (a ref). For one path, or the whole tree.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "staged": { "type": "boolean" }, "base": { "type": "string" }, "head": { "type": "string" } } }) },
    Tool { name: "git_log", toolset: "git", class: Class::Read, description: "Commits, newest first: the branch's, a path's, a range's (base..head), or a path's lines (startLine, endLine).", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "range": { "type": "string" }, "startLine": { "type": "integer" }, "endLine": { "type": "integer" }, "grep": { "type": "string" }, "max": { "type": "integer" } } }) },
    Tool { name: "git_show", toolset: "git", class: Class::Read, description: "A commit's message, files and diff, or one path of it.", schema: || json!({ "type": "object", "properties": { "commit": { "type": "string" }, "path": { "type": "string" } }, "required": ["commit"] }) },
    Tool { name: "git_blame", toolset: "git", class: Class::Read, description: "Who last changed each line, and in which commit; unsaved lines say so. startLine to endLine, 100 lines by default.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "startLine": { "type": "integer" }, "endLine": { "type": "integer" } }, "required": ["path"] }) },
    Tool { name: "git_stage", toolset: "git", class: Class::Edit, description: "Stages a file. Nothing is committed: the user commits.", schema: path_only },
    // search
    Tool { name: "search", toolset: "search", class: Class::Read, description: "Searches the project's files, unsaved text included and its ignores respected: literal unless regex, smart case unless case. glob: \"*.rs,!target\".", schema: || json!({ "type": "object", "properties": { "pattern": { "type": "string" }, "regex": { "type": "boolean" }, "case": { "type": "boolean" }, "word": { "type": "boolean" }, "glob": { "type": "string" }, "max": { "type": "integer" } }, "required": ["pattern"] }) },
    Tool { name: "replace", toolset: "search", class: Class::Edit, description: "Replaces a pattern across the project (as search finds it; exact case unless case is false). The user reviews each change.", schema: || json!({ "type": "object", "properties": { "pattern": { "type": "string" }, "replacement": { "type": "string" }, "regex": { "type": "boolean" }, "case": { "type": "boolean" }, "word": { "type": "boolean" }, "glob": { "type": "string" }, "preserve_case": { "type": "boolean" } }, "required": ["pattern", "replacement"] }) },
    // syntax
    Tool { name: "syntax_node", toolset: "syntax", class: Class::Read, description: "The syntax node at a place (line, column from 1) and its parents, innermost first: what function, class or block it's in.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" }, "column": { "type": "integer" } }, "required": ["path", "line"] }) },
    Tool { name: "syntax_query", toolset: "syntax", class: Class::Read, description: "Runs a tree-sitter query on a file and gives its @captures with their lines.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "query": { "type": "string" }, "max": { "type": "integer" } }, "required": ["path", "query"] }) },
    // tasks
    Tool { name: "tasks", toolset: "tasks", class: Class::Read, description: "The project's tasks (build, test, lint...), by name.", schema: none },
    Tool { name: "run_task", toolset: "tasks", class: Class::Run, description: "Runs a project task in Fenix's task pane, where the user sees it, and (unless wait is false) answers when it ends: success, problems found, tests, the end of its output.", schema: || json!({ "type": "object", "properties": { "name": { "type": "string" }, "wait": { "type": "boolean" }, "timeout": { "type": "integer", "description": "Seconds" } }, "required": ["name"] }) },
    Tool { name: "run_test", toolset: "tasks", class: Class::Run, description: "Runs the test at a line (Rust, pytest, Go) in Fenix's task pane and answers when it ends, as run_task does.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" }, "timeout": { "type": "integer" } }, "required": ["path", "line"] }) },
    Tool { name: "tests", toolset: "tasks", class: Class::Read, description: "The project's tests as Fenix's test explorer has them: provider, id, path, line, state, durationMs, stale. path, state (failed, errored, passed, skipped, not_run) and filter (words) narrow it. At most 500, saying how many more.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "state": { "type": "string" }, "filter": { "type": "string" } } }) },
    Tool { name: "test_failures", toolset: "tasks", class: Class::Read, description: "The failed and errored tests (in path, if given) as their last runs left them: provider, id, place, message, and the failure's text (4 KiB each).", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } } }) },
    Tool { name: "run_tests", toolset: "tasks", class: Class::Run, description: "Runs tests through Fenix's test explorer -- tests ([{provider, id}]), a path's, the failed (failed: true) or all (all: true) -- and answers when the run ends: its counts and failures.", schema: || json!({ "type": "object", "properties": { "tests": { "type": "array", "items": { "type": "object", "properties": { "provider": { "type": "string" }, "id": { "type": "string" } } } }, "path": { "type": "string" }, "failed": { "type": "boolean" }, "all": { "type": "boolean" }, "timeout": { "type": "integer" } } }) },
    Tool { name: "task_output", toolset: "tasks", class: Class::Read, description: "The last task's output, or startLine to endLine of it.", schema: || json!({ "type": "object", "properties": { "startLine": { "type": "integer" }, "endLine": { "type": "integer" } } }) },
    // debug
    Tool { name: "debug_state", toolset: "debug", class: Class::Read, description: "Where the debugger is stopped: the stack, the selected frame's variables, watches, the program's latest output.", schema: none },
    Tool { name: "debug_evaluate", toolset: "debug", class: Class::Run, description: "Evaluates an expression where the debugger is stopped. It may run code in the program.", schema: || json!({ "type": "object", "properties": { "expression": { "type": "string" } }, "required": ["expression"] }) },
    Tool { name: "debug_test", toolset: "debug", class: Class::Run, description: "Debugs the test at a line (path, line from 1) through Fenix's test explorer, built first if its provider says so, and answers at the first stop, as debug_start does.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" } }, "required": ["path", "line"] }) },
    Tool { name: "debug_launches", toolset: "debug", class: Class::Read, description: "The launches there are for a program's language (path: a file of it; else the file in front): \"project\" for the project's own.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } } }) },
    Tool { name: "debug_start", toolset: "debug", class: Class::Run, description: "Starts debugging (launch, from debug_launches; path, a file of the program) and answers at the first stop, or when it ends.", schema: || json!({ "type": "object", "properties": { "launch": { "type": "string" }, "path": { "type": "string" } } }) },
    Tool { name: "breakpoints", toolset: "debug", class: Class::Read, description: "Every breakpoint, with its condition, hit count, log message, and whether it's off.", schema: none },
    Tool { name: "set_breakpoint", toolset: "debug", class: Class::Run, description: "Sets a breakpoint at a line, with a condition or a log message (a logpoint) if given.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" }, "condition": { "type": "string" }, "log": { "type": "string" } }, "required": ["path", "line"] }) },
    Tool { name: "clear_breakpoint", toolset: "debug", class: Class::Run, description: "Takes away the breakpoint at a line.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" } }, "required": ["path", "line"] }) },
    Tool { name: "debug_continue", toolset: "debug", class: Class::Run, description: "Continues the stopped program; answers at its next stop, its end, or after 10 s.", schema: none },
    Tool { name: "debug_step", toolset: "debug", class: Class::Run, description: "Steps (kind: over, the default, into or out) and answers with the new stop.", schema: || json!({ "type": "object", "properties": { "kind": { "enum": ["over", "into", "out"] } } }) },
    Tool { name: "debug_run_to", toolset: "debug", class: Class::Run, description: "Runs on to a line and answers when it stops there, or anywhere else first.", schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" } }, "required": ["path", "line"] }) },
    Tool { name: "debug_pause", toolset: "debug", class: Class::Run, description: "Stops the running program where it is.", schema: none },
    Tool { name: "debug_stop", toolset: "debug", class: Class::Run, description: "Ends the debug session.", schema: none },
    // commands
    Tool { name: "run_command", toolset: "commands", class: Class::Run, description: "Runs a Fenix command by its id.", schema: || json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }) },
];

/// The test explorer's tools (plan `test-explorer.md` §10.3): offered only
/// with `tests.enabled` on; off, Fenix has no such tools, as in 1.5.
pub const TESTS_TOOLS: &[&str] = &["tests", "test_failures", "run_tests", "debug_test"];

/// `run_test`'s description with the test explorer on: the same tool,
/// through the explorer where a test provider serves the file.
pub const RUN_TEST_EXPLORER: &str = "Runs the test at a line and answers when it ends: through Fenix's test explorer, with its result, where a test provider serves the file; else (Rust, pytest, Go) in the task pane, as run_task does.";

#[cfg(test)]
fn toolset(name: &str) -> Option<&'static Toolset> {
    TOOLSETS.iter().find(|t| t.name == name)
}

/// A tool as `tools/list` has it.
pub fn listed(tool: &Tool) -> Value {
    json!({ "name": tool.name, "description": tool.description, "inputSchema": (tool.schema)() })
}

/// What the project has, for the toolsets offered by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub lsp: bool,
    pub git: bool,
    pub tasks: bool,
    pub debug: bool,
}

/// The toolsets an agent is shown: the defaults that fit `facts`, then
/// `setting` (`agents.mcp.toolsets`: `+name` adds, `-name` or `!name`
/// removes, a bare name adds), then the session's own `enabled` and
/// `disabled` (`fenix_toolsets`). `extensions` are the extensions'
/// toolsets with whether each is offered by default. `core` is always
/// there.
pub fn offered(facts: Facts, setting: &[String], enabled: &BTreeSet<String>, disabled: &BTreeSet<String>, extensions: &[(String, bool)]) -> BTreeSet<String> {
    let mut on: BTreeSet<String> = TOOLSETS
        .iter()
        .filter(|t| match t.offered {
            Offered::Always => true,
            Offered::WithLsp => facts.lsp,
            Offered::WithGit => facts.git,
            Offered::WithTasks => facts.tasks,
            Offered::WithDebug => facts.debug,
            Offered::Never => false,
        })
        .map(|t| t.name.to_string())
        .collect();
    on.extend(extensions.iter().filter(|(_, default)| *default).map(|(name, _)| name.clone()));
    for entry in setting {
        let entry = entry.trim();
        if let Some(name) = entry.strip_prefix('-').or_else(|| entry.strip_prefix('!')) {
            on.remove(name.trim());
        } else if let Some(name) = entry.strip_prefix('+').map(str::trim).or(Some(entry)).filter(|n| !n.is_empty()) {
            on.insert(name.to_string());
        }
    }
    on.extend(enabled.iter().cloned());
    for name in disabled {
        on.remove(name);
    }
    on.insert("core".into());
    on
}

/// The user's rules, and the project's, which can only tighten them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub user: BTreeMap<String, Rule>,
    pub project: BTreeMap<String, Rule>,
    /// The caller is a client Fenix didn't start: stricter defaults.
    pub external_client: bool,
}

/// Reads a rules map (`agents.mcp.allow`): keys as given, values that
/// aren't `allow`, `ask` or `deny` dropped.
pub fn parse_rules(map: &BTreeMap<String, String>) -> BTreeMap<String, Rule> {
    map.iter().filter_map(|(k, v)| Some((k.trim().to_string(), Rule::parse(v)?))).collect()
}

/// The rules 1.0's switches mean, for whoever still has them set:
/// `fenix_write = on` lets agents edit and run, `fenix_auto_read = off`
/// makes them ask to read. A rule the user wrote for a class wins.
pub fn migrated(mut user: BTreeMap<String, Rule>, fenix_write: Option<bool>, fenix_auto_read: Option<bool>) -> BTreeMap<String, Rule> {
    if fenix_write == Some(true) {
        user.entry("edit".into()).or_insert(Rule::Allow);
        user.entry("run".into()).or_insert(Rule::Allow);
    }
    if fenix_auto_read == Some(false) {
        user.entry("read".into()).or_insert(Rule::Ask);
    }
    user
}

/// The rule in `map` for this tool, most specific first: the tool's
/// name, its toolset (`toolset:<name>`), its class; each with `prefix`
/// in front (`client:` for clients Fenix didn't start).
fn most_specific(map: &BTreeMap<String, Rule>, prefix: &str, name: &str, toolset: &str, class: Class) -> Option<Rule> {
    [name.to_string(), format!("toolset:{toolset}"), class.as_str().to_string()].iter().find_map(|key| map.get(&format!("{prefix}{key}")).copied())
}

/// What happens when `name` (in `toolset`, of `class`) is called.
///
/// A client Fenix didn't start (an MCP client of yours, over stdio or
/// HTTP) has rules of its own, written `client:<key>`
/// (`"client:edit" = "allow"`); the agents' rules don't apply to it.
pub fn decide(rules: &Rules, name: &str, toolset: &str, class: Class) -> Rule {
    let prefix = if rules.external_client { "client:" } else { "" };
    let user = most_specific(&rules.user, prefix, name, toolset, class).unwrap_or_else(|| class.default_rule(rules.external_client));
    // A project's rule, for either kind of caller, can only tighten.
    let project = most_specific(&rules.project, prefix, name, toolset, class).into_iter().chain(most_specific(&rules.project, "", name, toolset, class)).max();
    match project {
        Some(project) => user.max(project),
        None => user,
    }
}

/// What `fenix_toolsets` answers: every toolset, whether it's on, and a
/// line about it; with the tools of one just turned on.
pub fn toolsets_answer(on: &BTreeSet<String>, extensions: &[(String, String)], turned_on: Option<(&str, Vec<Value>)>) -> Value {
    let mut sets: Vec<Value> = TOOLSETS.iter().map(|t| json!({ "name": t.name, "description": t.description, "on": on.contains(t.name) })).collect();
    sets.extend(extensions.iter().map(|(name, description)| json!({ "name": name, "description": description, "on": on.contains(name), "extension": true })));
    let mut answer = json!({ "toolsets": sets });
    if let Some((name, tools)) = turned_on {
        answer["enabled"] = json!(name);
        answer["tools"] = json!(tools);
        answer["note"] = json!("They're in your tool list if your client re-reads it; otherwise call them with fenix_call.");
    }
    answer
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The most a default tool list may be, in bytes: about 3,000 tokens
    /// of the agent's context, every turn.
    const BUDGET: usize = 12 * 1024;

    fn list_bytes(tools: &[Value]) -> usize {
        serde_json::to_string(tools).map(|s| s.len()).unwrap_or(0)
    }

    fn rules(user: &[(&str, &str)], project: &[(&str, &str)]) -> Rules {
        let map = |pairs: &[(&str, &str)]| parse_rules(&pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        Rules { user: map(user), project: map(project), external_client: false }
    }

    #[test]
    fn classes_have_their_defaults() {
        let r = rules(&[], &[]);
        assert_eq!(decide(&r, "diagnostics", "core", Class::Read), Rule::Allow);
        assert_eq!(decide(&r, "show_file", "core", Class::Show), Rule::Allow);
        assert_eq!(decide(&r, "rename", "refactor", Class::Edit), Rule::Ask);
        assert_eq!(decide(&r, "run_task", "tasks", Class::Run), Rule::Ask);
        assert_eq!(decide(&r, "jira_transition", "jira", Class::External), Rule::Deny);
    }

    #[test]
    fn the_most_specific_rule_wins() {
        let r = rules(&[("edit", "allow"), ("toolset:git", "deny"), ("git_stage", "ask"), ("run", "allow")], &[]);
        assert_eq!(decide(&r, "rename", "refactor", Class::Edit), Rule::Allow);
        assert_eq!(decide(&r, "git_diff", "git", Class::Read), Rule::Deny, "the toolset's rule beats the class's");
        assert_eq!(decide(&r, "git_stage", "git", Class::Edit), Rule::Ask, "the tool's beats the toolset's");
        assert_eq!(decide(&r, "run_task", "tasks", Class::Run), Rule::Allow);
    }

    #[test]
    fn a_project_can_only_tighten() {
        let r = rules(&[("run", "allow")], &[("run", "ask"), ("edit", "allow"), ("external", "allow"), ("diagnostics", "deny")]);
        assert_eq!(decide(&r, "run_task", "tasks", Class::Run), Rule::Ask, "tightened");
        assert_eq!(decide(&r, "rename", "refactor", Class::Edit), Rule::Ask, "a project can't loosen edit");
        assert_eq!(decide(&r, "x_post", "x", Class::External), Rule::Deny, "nor external");
        assert_eq!(decide(&r, "diagnostics", "core", Class::Read), Rule::Deny);
    }

    #[test]
    fn a_client_fenix_didnt_start_gets_strict_defaults() {
        let mut r = rules(&[("edit", "allow")], &[]);
        r.external_client = true;
        assert_eq!(decide(&r, "diagnostics", "core", Class::Read), Rule::Allow);
        assert_eq!(decide(&r, "run_task", "tasks", Class::Run), Rule::Deny);
        assert_eq!(decide(&r, "rename", "refactor", Class::Edit), Rule::Deny, "the agents' rules aren't its");
        let mut r = rules(&[("client:edit", "allow")], &[("edit", "ask")]);
        r.external_client = true;
        assert_eq!(decide(&r, "rename", "refactor", Class::Edit), Rule::Ask, "its own rule, tightened by the project's");
    }

    #[test]
    fn values_that_arent_rules_are_dropped() {
        let r = rules(&[("edit", "sometimes")], &[]);
        assert!(r.user.is_empty());
    }

    #[test]
    fn the_old_switches_become_rules() {
        let user = migrated(BTreeMap::new(), Some(true), Some(false));
        assert_eq!(user.get("edit"), Some(&Rule::Allow));
        assert_eq!(user.get("run"), Some(&Rule::Allow));
        assert_eq!(user.get("read"), Some(&Rule::Ask));
        let kept = migrated(BTreeMap::from([("run".to_string(), Rule::Deny)]), Some(true), None);
        assert_eq!(kept.get("run"), Some(&Rule::Deny), "a rule written beats the old switch");
        assert!(migrated(BTreeMap::new(), Some(false), Some(true)).is_empty());
    }

    #[test]
    fn toolsets_follow_the_project_the_setting_and_the_session() {
        let none = BTreeSet::new();
        let bare = offered(Facts::default(), &[], &none, &none, &[]);
        assert_eq!(bare, BTreeSet::from(["core".to_string(), "search".into(), "syntax".into()]));
        let full = offered(Facts { lsp: true, git: true, tasks: true, debug: true }, &[], &none, &none, &[("jira".into(), true), ("db".into(), false)]);
        for name in ["core", "lsp", "refactor", "git", "tasks", "debug", "jira"] {
            assert!(full.contains(name), "{name}");
        }
        assert!(!full.contains("db") && !full.contains("commands"));
        let set = offered(Facts::default(), &["-search".into(), "+commands".into(), "db".into()], &none, &none, &[]);
        assert!(!set.contains("search") && set.contains("commands") && set.contains("db"));
        let session = offered(Facts::default(), &["-core".into()], &BTreeSet::from(["debug".to_string()]), &BTreeSet::from(["syntax".to_string()]), &[]);
        assert!(session.contains("debug") && !session.contains("syntax"));
        assert!(session.contains("core"), "core can't be taken away");
    }

    #[test]
    fn every_tool_is_in_a_toolset_and_names_are_unique() {
        let mut seen = BTreeSet::new();
        for tool in TOOLS {
            assert!(toolset(tool.toolset).is_some(), "{}", tool.name);
            assert!(seen.insert(tool.name), "{} twice", tool.name);
            assert!(tool.description.len() <= 300, "{}", tool.name);
            assert_eq!((tool.schema)()["type"], "object", "{}", tool.name);
        }
    }

    #[test]
    fn the_default_tool_list_fits_the_budget() {
        // A typical project: language servers, Git, tasks; no debugger yet.
        let on = offered(Facts { lsp: true, git: true, tasks: true, debug: false }, &[], &BTreeSet::new(), &BTreeSet::new(), &[]);
        let tools: Vec<Value> = TOOLS.iter().filter(|t| on.contains(t.toolset)).map(listed).collect();
        let bytes = list_bytes(&tools);
        assert!(bytes <= BUDGET, "the default tool list is {bytes} bytes, over the {BUDGET}-byte budget: put a tool in a toolset that's off by default, or say less");
        // With the test explorer on: its tools, and run_test's longer line
        // (the larger list of the two).
        let explorer: Vec<Value> = TOOLS
            .iter()
            .filter(|t| on.contains(t.toolset))
            .map(|t| if t.name == "run_test" { json!({ "name": t.name, "description": RUN_TEST_EXPLORER, "inputSchema": (t.schema)() }) } else { listed(t) })
            .collect();
        let with = list_bytes(&explorer);
        let without: Vec<Value> = tools.into_iter().filter(|t| !TESTS_TOOLS.contains(&t["name"].as_str().unwrap_or_default())).collect();
        assert!(with <= BUDGET, "with the test explorer on, the default tool list is {with} bytes, over the {BUDGET}-byte budget");
        assert!(list_bytes(&without) < with, "the explorer's tools are in the default list (tasks)");
        // With the debugger too, debug_test among them.
        let debugging = offered(Facts { lsp: true, git: true, tasks: true, debug: true }, &[], &BTreeSet::new(), &BTreeSet::new(), &[]);
        assert!(debugging.contains("debug") && TOOLS.iter().any(|t| t.name == "debug_test" && t.toolset == "debug"));
    }

    #[test]
    fn the_test_explorers_tools_say_little_and_run_tools_are_asked_about() {
        assert!(RUN_TEST_EXPLORER.len() <= 300);
        for name in TESTS_TOOLS {
            let tool = TOOLS.iter().find(|t| t.name == *name).unwrap_or_else(|| panic!("{name}"));
            assert!(tool.description.len() <= 300, "{name}");
            let rule = decide(&rules(&[], &[]), tool.name, tool.toolset, tool.class);
            match *name {
                "run_tests" | "debug_test" => assert_eq!((tool.class, rule), (Class::Run, Rule::Ask), "{name}"),
                _ => assert_eq!((tool.class, rule), (Class::Read, Rule::Allow), "{name}"),
            }
        }
    }
}
