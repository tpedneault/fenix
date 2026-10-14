//! Several language servers on one document: which of them a request
//! goes to, and how their answers become one.
//!
//! A buffer has at most one *primary* server (its language's own) and
//! any number of *additional* ones. Requests whose answers combine --
//! completion, hover, definition, references, code actions, document
//! symbols -- go to every server that advertises the feature, and the
//! answers are merged, primary first. Requests that change the document
//! or describe one call -- rename, formatting, signature help -- go to
//! one server: the primary when it can, else the one additional server
//! that can (two that can would be a guess, so none is asked).
//!
//! Everything here is pure: the host owns the processes, sends the
//! requests, and feeds the answers in. Generic over the host's own key
//! for a server (`K`), so nothing here knows how sessions are named.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use lsp_types::{
    CompletionItem, CompletionResponse, Diagnostic, GotoDefinitionResponse, Location, OneOf,
    ServerCapabilities,
};

/// A request kind, as far as routing it is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Completion,
    Hover,
    Definition,
    References,
    CodeAction,
    DocumentSymbol,
    Rename,
    Formatting,
    SignatureHelp,
}

impl Feature {
    /// Whether every capable server is asked and the answers merged,
    /// rather than one server answering alone.
    pub fn is_merged(self) -> bool {
        !matches!(
            self,
            Feature::Rename | Feature::Formatting | Feature::SignatureHelp
        )
    }
}

/// Whether `capabilities` advertises `feature`.
pub fn provides(capabilities: &ServerCapabilities, feature: Feature) -> bool {
    fn on<T>(value: &Option<OneOf<bool, T>>) -> bool {
        matches!(value, Some(OneOf::Left(true)) | Some(OneOf::Right(_)))
    }
    match feature {
        Feature::Completion => capabilities.completion_provider.is_some(),
        Feature::Hover => !matches!(
            capabilities.hover_provider,
            None | Some(lsp_types::HoverProviderCapability::Simple(false))
        ),
        Feature::Definition => on(&capabilities.definition_provider),
        Feature::References => on(&capabilities.references_provider),
        Feature::CodeAction => !matches!(
            capabilities.code_action_provider,
            None | Some(lsp_types::CodeActionProviderCapability::Simple(false))
        ),
        Feature::DocumentSymbol => on(&capabilities.document_symbol_provider),
        Feature::Rename => on(&capabilities.rename_provider),
        Feature::Formatting => on(&capabilities.document_formatting_provider),
        Feature::SignatureHelp => capabilities.signature_help_provider.is_some(),
    }
}

/// One server attached to the document, as routing sees it.
#[derive(Debug, Clone, Copy)]
pub struct Attached<'a> {
    pub primary: bool,
    pub capabilities: &'a ServerCapabilities,
}

/// Which of `servers` (by index) to ask for `feature`, primary first.
pub fn route(feature: Feature, servers: &[Attached]) -> Vec<usize> {
    let capable: Vec<usize> = order(servers)
        .into_iter()
        .filter(|&i| provides(servers[i].capabilities, feature))
        .collect();
    if feature.is_merged() {
        return capable;
    }
    if let Some(&primary) = capable.iter().find(|&&i| servers[i].primary) {
        return vec![primary];
    }
    if capable.len() == 1 {
        capable
    } else {
        Vec::new()
    }
}

/// Which servers to ask for completion: all that complete, or -- when
/// the request was triggered by typing `trigger` -- only those that
/// declared it a trigger character.
pub fn completion_targets(servers: &[Attached], trigger: Option<char>) -> Vec<usize> {
    let mut targets = route(Feature::Completion, servers);
    if let Some(trigger) = trigger {
        targets.retain(|&i| declares_trigger(servers[i].capabilities, trigger));
    }
    targets
}

/// Whether `capabilities` names `c` as a completion trigger character.
pub fn declares_trigger(capabilities: &ServerCapabilities, c: char) -> bool {
    capabilities
        .completion_provider
        .as_ref()
        .and_then(|options| options.trigger_characters.as_ref())
        .is_some_and(|triggers| triggers.iter().any(|t| t.chars().eq(std::iter::once(c))))
}

/// Indices of `servers`, the primary first, otherwise as given.
fn order(servers: &[Attached]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..servers.len()).collect();
    indices.sort_by_key(|&i| !servers[i].primary);
    indices
}

/// What goes between two servers' hover texts.
pub const HOVER_SEPARATOR: &str = "\n\n───\n\n";

/// The hover texts, in server order, as one: empty answers dropped,
/// `None` when nothing is left.
pub fn join_hovers<I: IntoIterator<Item = Option<String>>>(answers: I) -> Option<String> {
    let parts: Vec<String> = answers
        .into_iter()
        .flatten()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(HOVER_SEPARATOR))
}

/// A definition answer as plain locations (a link's target selection).
pub fn locations_of(response: GotoDefinitionResponse) -> Vec<Location> {
    match response {
        GotoDefinitionResponse::Scalar(location) => vec![location],
        GotoDefinitionResponse::Array(locations) => locations,
        GotoDefinitionResponse::Link(links) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    }
}

/// Several servers' locations as one list, in server order, without
/// repeats: two answers pointing at the same place (same document, same
/// start) are one place, whatever range each drew around it.
pub fn merge_locations<I: IntoIterator<Item = Vec<Location>>>(answers: I) -> Vec<Location> {
    let mut seen = HashSet::new();
    answers
        .into_iter()
        .flatten()
        .filter(|location| {
            seen.insert((
                location.uri.as_str().to_string(),
                location.range.start.line,
                location.range.start.character,
            ))
        })
        .collect()
}

/// The answers to one request sent to several servers, collected as they
/// arrive. Complete when every server has answered (an error or a crash
/// counts as an answer: that server is skipped), or *ready* earlier once
/// the deadline has passed and something worth showing has arrived -- so
/// a slow server doesn't hold up the others, and a lone slow server is
/// still waited for.
#[derive(Debug, Clone)]
pub struct Gather<K, T> {
    servers: Vec<K>,
    answers: Vec<Option<Option<T>>>,
    deadline: Instant,
}

impl<K: PartialEq, T> Gather<K, T> {
    /// `servers` in the order their answers should be shown.
    pub fn new(servers: Vec<K>, deadline: Instant) -> Self {
        let answers = servers.iter().map(|_| None).collect();
        Gather {
            servers,
            answers,
            deadline,
        }
    }

    /// Records `server`'s answer -- `None` when it failed. Returns whether
    /// `server` was one this request was waiting on.
    pub fn answer(&mut self, server: &K, answer: Option<T>) -> bool {
        match self.servers.iter().position(|s| s == server) {
            Some(i) if self.answers[i].is_none() => {
                self.answers[i] = Some(answer);
                true
            }
            _ => false,
        }
    }

    /// Whether `server` still owes an answer.
    pub fn waiting_on(&self, server: &K) -> bool {
        self.servers
            .iter()
            .zip(&self.answers)
            .any(|(s, a)| s == server && a.is_none())
    }

    pub fn is_complete(&self) -> bool {
        self.answers.iter().all(Option::is_some)
    }

    /// Whether to act on what has arrived: everyone answered, or the
    /// deadline is past and at least one answer `has_content`.
    pub fn is_ready(&self, now: Instant, has_content: impl Fn(&T) -> bool) -> bool {
        self.is_complete()
            || (now >= self.deadline && self.arrived().any(|(_, answer)| has_content(answer)))
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The answers so far, in server order, failures left out.
    pub fn arrived(&self) -> impl Iterator<Item = (&K, &T)> {
        self.servers
            .iter()
            .zip(&self.answers)
            .filter_map(|(server, answer)| {
                answer
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|answer| (server, answer))
            })
    }

    /// Every answer, in server order, failures left out.
    pub fn into_arrived(self) -> Vec<(K, T)> {
        self.servers
            .into_iter()
            .zip(self.answers)
            .filter_map(|(server, answer)| answer.flatten().map(|answer| (server, answer)))
            .collect()
    }
}

/// One completion session's answers, per server: what each last said
/// and whether it called its list incomplete. While the popup stays
/// anchored at the same place, a server whose list was complete isn't
/// asked again -- the popup filters its items as you type -- and only
/// the ones that said `isIncomplete` are.
#[derive(Debug, Clone)]
pub struct CompletionSet<K> {
    answers: Vec<(K, Option<CompletionAnswer>)>,
}

/// One server's items, and whether it said its list was incomplete.
type CompletionAnswer = (Vec<CompletionItem>, bool);

impl<K: Clone + PartialEq> CompletionSet<K> {
    /// `servers` in display order, the primary first.
    pub fn new(servers: Vec<K>) -> Self {
        CompletionSet {
            answers: servers.into_iter().map(|server| (server, None)).collect(),
        }
    }

    /// Which of `targets` to (re-)ask: those that haven't answered yet,
    /// and those whose last list was incomplete.
    pub fn to_ask(&self, targets: &[K]) -> Vec<K> {
        targets
            .iter()
            .filter(
                |target| match self.answers.iter().find(|(server, _)| server == *target) {
                    Some((_, Some((_, incomplete)))) => *incomplete,
                    _ => true,
                },
            )
            .cloned()
            .collect()
    }

    /// Stores `server`'s answer, replacing any earlier one. A server this
    /// set wasn't started with is appended after the others.
    pub fn store(&mut self, server: K, response: CompletionResponse) {
        let answer = match response {
            CompletionResponse::Array(items) => (items, false),
            CompletionResponse::List(list) => (list.items, list.is_incomplete),
        };
        match self.answers.iter_mut().find(|(s, _)| *s == server) {
            Some(slot) => slot.1 = Some(answer),
            None => self.answers.push((server, Some(answer))),
        }
    }

    /// Forgets `server`'s answer -- it failed or went away.
    pub fn forget(&mut self, server: &K) {
        self.answers.retain(|(s, _)| s != server);
    }

    /// Every item, tagged with the server that produced it (where its
    /// `completionItem/resolve` must go), in server order.
    pub fn items(&self) -> impl Iterator<Item = (&K, &CompletionItem)> {
        self.answers
            .iter()
            .filter_map(|(server, answer)| answer.as_ref().map(|(items, _)| (server, items)))
            .flat_map(|(server, items)| items.iter().map(move |item| (server, item)))
    }
}

/// Diagnostics per document and per server: each server's latest
/// `publishDiagnostics` replaces only its own, and a document's list is
/// all of them together, each naming the server it came from.
#[derive(Debug, Clone)]
pub struct Diagnostics<K> {
    by_path: HashMap<PathBuf, Vec<(K, String, Vec<Diagnostic>)>>,
}

impl<K> Default for Diagnostics<K> {
    fn default() -> Self {
        Diagnostics {
            by_path: HashMap::new(),
        }
    }
}

impl<K: PartialEq> Diagnostics<K> {
    /// `server` (shown as `label` where a diagnostic names no source of
    /// its own) now reports `diagnostics` for `path`; an empty list
    /// clears its earlier ones.
    pub fn publish(&mut self, path: PathBuf, server: K, label: &str, diagnostics: Vec<Diagnostic>) {
        let entries = self.by_path.entry(path.clone()).or_default();
        entries.retain(|(s, _, _)| *s != server);
        if !diagnostics.is_empty() {
            entries.push((server, label.to_string(), diagnostics));
        }
        if entries.is_empty() {
            self.by_path.remove(&path);
        }
    }

    /// Everything reported for `path`, every server's, each with a source.
    pub fn for_path(&self, path: &Path) -> Vec<Diagnostic> {
        let Some(entries) = self.by_path.get(path) else {
            return Vec::new();
        };
        entries
            .iter()
            .flat_map(|(_, label, diagnostics)| {
                diagnostics.iter().cloned().map(move |mut d| {
                    d.source.get_or_insert_with(|| label.clone());
                    d
                })
            })
            .collect()
    }

    /// Only `server`'s own diagnostics for `path`, as it sent them -- what
    /// goes back to it in a code action request.
    pub fn from_server(&self, path: &Path, server: &K) -> Vec<Diagnostic> {
        self.by_path
            .get(path)
            .and_then(|entries| entries.iter().find(|(s, _, _)| s == server))
            .map(|(_, _, d)| d.clone())
            .unwrap_or_default()
    }

    /// Drops everything `server` reported, for every document.
    pub fn forget_server(&mut self, server: &K) {
        self.by_path.retain(|_, entries| {
            entries.retain(|(s, _, _)| s != server);
            !entries.is_empty()
        });
    }

    /// Drops everything reported for `path`.
    pub fn remove_path(&mut self, path: &Path) {
        self.by_path.remove(path);
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{CompletionList, CompletionOptions, Position, Range, Uri};
    use std::str::FromStr;
    use std::time::Duration;

    fn caps(features: &[Feature]) -> ServerCapabilities {
        let mut c = ServerCapabilities::default();
        for feature in features {
            match feature {
                Feature::Completion => c.completion_provider = Some(CompletionOptions::default()),
                Feature::Hover => {
                    c.hover_provider = Some(lsp_types::HoverProviderCapability::Simple(true))
                }
                Feature::Definition => c.definition_provider = Some(OneOf::Left(true)),
                Feature::References => c.references_provider = Some(OneOf::Left(true)),
                Feature::CodeAction => {
                    c.code_action_provider =
                        Some(lsp_types::CodeActionProviderCapability::Simple(true))
                }
                Feature::DocumentSymbol => c.document_symbol_provider = Some(OneOf::Left(true)),
                Feature::Rename => c.rename_provider = Some(OneOf::Left(true)),
                Feature::Formatting => c.document_formatting_provider = Some(OneOf::Left(true)),
                Feature::SignatureHelp => c.signature_help_provider = Some(Default::default()),
            }
        }
        c
    }

    fn with_triggers(triggers: &[&str]) -> ServerCapabilities {
        ServerCapabilities {
            completion_provider: Some(CompletionOptions {
                trigger_characters: Some(triggers.iter().map(|t| t.to_string()).collect()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn location(file: &str, line: u32, character: u32, end: u32) -> Location {
        Location {
            uri: Uri::from_str(&format!("file:///{file}")).unwrap(),
            range: Range::new(Position::new(line, character), Position::new(line, end)),
        }
    }

    #[test]
    fn merged_features_go_to_every_capable_server_primary_first() {
        let (a, p, b) = (
            caps(&[Feature::Hover]),
            caps(&[Feature::Hover, Feature::Completion]),
            caps(&[Feature::Completion]),
        );
        let servers = [
            Attached {
                primary: false,
                capabilities: &a,
            },
            Attached {
                primary: true,
                capabilities: &p,
            },
            Attached {
                primary: false,
                capabilities: &b,
            },
        ];
        assert_eq!(route(Feature::Hover, &servers), [1, 0]);
        assert_eq!(route(Feature::Completion, &servers), [1, 2]);
        assert_eq!(route(Feature::References, &servers), Vec::<usize>::new());
    }

    #[test]
    fn exclusive_features_go_to_the_primary_or_the_one_additional_that_can() {
        let (p, a, b) = (
            caps(&[Feature::Rename]),
            caps(&[Feature::Rename, Feature::Formatting]),
            caps(&[Feature::SignatureHelp]),
        );
        let servers = [
            Attached {
                primary: true,
                capabilities: &p,
            },
            Attached {
                primary: false,
                capabilities: &a,
            },
            Attached {
                primary: false,
                capabilities: &b,
            },
        ];
        assert_eq!(
            route(Feature::Rename, &servers),
            [0],
            "the primary wins even when another could"
        );
        assert_eq!(
            route(Feature::Formatting, &servers),
            [1],
            "the primary can't; exactly one other can"
        );
        assert_eq!(route(Feature::SignatureHelp, &servers), [2]);
        let both = [
            Attached {
                primary: true,
                capabilities: &b,
            },
            Attached {
                primary: false,
                capabilities: &a,
            },
            Attached {
                primary: false,
                capabilities: &a,
            },
        ];
        assert_eq!(
            route(Feature::Formatting, &both),
            Vec::<usize>::new(),
            "two additional servers that can: neither is guessed"
        );
        let no_primary = [Attached {
            primary: false,
            capabilities: &a,
        }];
        assert_eq!(
            route(Feature::Rename, &no_primary),
            [0],
            "a language with only additional servers"
        );
    }

    #[test]
    fn a_trigger_character_asks_only_the_servers_that_declared_it() {
        let (dot, dash, none) = (
            with_triggers(&[".", "::"]),
            with_triggers(&["-"]),
            caps(&[Feature::Completion]),
        );
        let servers = [
            Attached {
                primary: true,
                capabilities: &dot,
            },
            Attached {
                primary: false,
                capabilities: &dash,
            },
            Attached {
                primary: false,
                capabilities: &none,
            },
        ];
        assert_eq!(completion_targets(&servers, None), [0, 1, 2]);
        assert_eq!(completion_targets(&servers, Some('.')), [0]);
        assert_eq!(completion_targets(&servers, Some('-')), [1]);
        assert_eq!(
            completion_targets(&servers, Some(':')),
            Vec::<usize>::new(),
            "a multi-character trigger isn't a single typed character"
        );
    }

    #[test]
    fn hovers_join_in_order_and_empty_ones_drop_out() {
        assert_eq!(
            join_hovers([
                Some("fn main()".into()),
                None,
                Some("  ".into()),
                Some("REQ-014: boot".into())
            ])
            .unwrap(),
            format!("fn main(){HOVER_SEPARATOR}REQ-014: boot")
        );
        assert_eq!(join_hovers([None, Some(String::new())]), None);
        assert_eq!(join_hovers([Some("only".into())]).unwrap(), "only");
    }

    #[test]
    fn locations_merge_in_order_without_repeating_a_place() {
        let merged = merge_locations([
            vec![location("a.rs", 1, 4, 9), location("b.rs", 2, 0, 1)],
            vec![location("a.rs", 1, 4, 20), location("c.md", 0, 0, 1)],
        ]);
        assert_eq!(
            merged,
            [
                location("a.rs", 1, 4, 9),
                location("b.rs", 2, 0, 1),
                location("c.md", 0, 0, 1)
            ]
        );
        let link = lsp_types::LocationLink {
            origin_selection_range: None,
            target_uri: location("d.rs", 0, 0, 0).uri,
            target_range: Range::new(Position::new(0, 0), Position::new(9, 0)),
            target_selection_range: Range::new(Position::new(3, 2), Position::new(3, 5)),
        };
        assert_eq!(
            locations_of(GotoDefinitionResponse::Link(vec![link])),
            [location("d.rs", 3, 2, 5)]
        );
    }

    #[test]
    fn a_gather_waits_for_everyone_until_the_deadline_then_shows_what_it_has() {
        let start = Instant::now();
        let deadline = start + Duration::from_millis(500);
        let mut gather: Gather<&str, Vec<u32>> = Gather::new(vec!["primary", "extra"], deadline);
        assert!(
            !gather.answer(&"stranger", Some(vec![9])),
            "an unknown server's answer is ignored"
        );
        assert!(gather.answer(&"extra", Some(vec![2])));
        assert!(
            !gather.answer(&"extra", Some(vec![3])),
            "only the first answer counts"
        );
        assert!(gather.waiting_on(&"primary"));
        assert!(
            !gather.is_ready(start, |v| !v.is_empty()),
            "before the deadline, the slow one is waited for"
        );
        assert!(
            gather.is_ready(deadline, |v| !v.is_empty()),
            "after it, what arrived is shown"
        );
        gather.answer(&"primary", Some(vec![1]));
        assert!(gather.is_complete());
        assert_eq!(
            gather.into_arrived(),
            [("primary", vec![1]), ("extra", vec![2])],
            "in server order, not arrival order"
        );
    }

    #[test]
    fn a_gather_with_nothing_to_show_keeps_waiting_and_skips_failures() {
        let start = Instant::now();
        let mut gather: Gather<u8, Vec<u32>> = Gather::new(vec![1, 2, 3], start);
        gather.answer(&1, Some(Vec::new()));
        assert!(
            !gather.is_ready(start, |v| !v.is_empty()),
            "an empty answer isn't worth stopping for"
        );
        gather.answer(&2, None);
        gather.answer(&3, Some(vec![7]));
        assert!(gather.is_ready(start, |v| !v.is_empty()));
        assert_eq!(
            gather.arrived().map(|(k, _)| *k).collect::<Vec<_>>(),
            [1, 3],
            "the failed server is skipped"
        );
    }

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn completion_requeries_only_incomplete_or_silent_servers_and_tags_each_item() {
        let mut set = CompletionSet::new(vec!["ra", "kiln", "spell"]);
        assert_eq!(
            set.to_ask(&["ra", "kiln", "spell"]),
            ["ra", "kiln", "spell"]
        );
        set.store(
            "kiln",
            CompletionResponse::List(CompletionList {
                is_incomplete: true,
                items: vec![item("REQ-014")],
            }),
        );
        set.store(
            "ra",
            CompletionResponse::Array(vec![item("main"), item("map")]),
        );
        assert_eq!(
            set.to_ask(&["ra", "kiln", "spell"]),
            ["kiln", "spell"],
            "ra's list was complete; spell hasn't answered"
        );
        let tagged: Vec<_> = set
            .items()
            .map(|(server, item)| (*server, item.label.as_str()))
            .collect();
        assert_eq!(
            tagged,
            [("ra", "main"), ("ra", "map"), ("kiln", "REQ-014")],
            "primary's first, each knows its server"
        );
        set.store(
            "kiln",
            CompletionResponse::List(CompletionList {
                is_incomplete: false,
                items: vec![item("REQ-0141")],
            }),
        );
        assert_eq!(set.to_ask(&["ra", "kiln"]), Vec::<&str>::new());
        set.forget(&"ra");
        assert_eq!(set.items().count(), 1);
        assert_eq!(
            set.to_ask(&["ra"]),
            ["ra"],
            "a forgotten server is asked again"
        );
    }

    #[test]
    fn diagnostics_are_kept_per_server_and_shown_together_with_their_source() {
        let path = PathBuf::from("/p/main.rs");
        let d = |message: &str, source: Option<&str>| Diagnostic {
            message: message.to_string(),
            source: source.map(str::to_string),
            ..Default::default()
        };
        let mut all: Diagnostics<&str> = Diagnostics::default();
        all.publish(
            path.clone(),
            "ra",
            "rust-analyzer",
            vec![d("unused", Some("rustc"))],
        );
        all.publish(
            path.clone(),
            "kiln",
            "kiln",
            vec![d("REQ-999 doesn't exist", None)],
        );
        let shown = all.for_path(&path);
        assert_eq!(
            shown
                .iter()
                .map(|d| d.source.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["rustc", "kiln"],
            "a server's own source is kept; a missing one is the server's name"
        );
        assert_eq!(
            all.from_server(&path, &"kiln")[0].source,
            None,
            "sent back to its server as it came"
        );

        all.publish(path.clone(), "kiln", "kiln", Vec::new());
        assert_eq!(
            all.for_path(&path).len(),
            1,
            "kiln clearing its list leaves rust-analyzer's"
        );
        all.publish(path.clone(), "kiln", "kiln", vec![d("again", None)]);
        all.forget_server(&"ra");
        assert_eq!(
            all.for_path(&path)[0].message,
            "again",
            "a crashed server's diagnostics go, the others stay"
        );
        all.remove_path(&path);
        assert!(all.is_empty());
    }
}
