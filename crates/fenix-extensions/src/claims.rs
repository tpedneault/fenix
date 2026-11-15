//! Whose a name is, among the installed extensions: a service, a command,
//! a setting, an agent tool or a prompt is one extension's alone, or no
//! one's -- never the first one's in some order. `docs/plans/service-callers.md`
//! §4.
//!
//! What a manifest may name at all is `Manifest::parse`'s (one-word
//! namespaces; the word `fenix` only for `fenix.*` ids). Among the
//! switched-on code extensions, then:
//!
//! 1. Fenix's own names are Fenix's.
//! 2. A name under an installed extension's full id plus a dot is that
//!    extension's -- switched on or not.
//! 3. A namespace that is another installed extension's publisher claims
//!    nothing.
//! 4. A full id is a stronger claim than a borrowed word (the id's last
//!    part, a namespace).
//! 5. Between equal claims the machine's beats yours; else no one has it.
//!
//! A refused name is taken out of the extension's manifest as Fenix uses
//! it (`withdraw`), and said on the Extensions page.

use std::collections::BTreeMap;

use crate::manifest::{borrows, toolset_of, Claim, Manifest};

/// What sort of name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameKind {
    Service,
    Command,
    Setting,
    /// `<toolset>_<name>`, as agents see it.
    AgentTool,
    Prompt,
}

impl NameKind {
    pub fn word(self) -> &'static str {
        match self {
            NameKind::Service => "service",
            NameKind::Command => "command",
            NameKind::Setting => "setting",
            NameKind::AgentTool => "agent tool",
            NameKind::Prompt => "agent prompt",
        }
    }
}

/// A switched-on code extension, as it asks for names.
#[derive(Debug, Clone, Copy)]
pub struct Claimant<'a> {
    pub id: &'a str,
    pub manifest: &'a Manifest,
    /// The machine's (an administrator's), not yours.
    pub managed: bool,
}

/// A name an extension lists that isn't its, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The extension that listed it.
    pub id: String,
    pub kind: NameKind,
    pub name: String,
    pub why: String,
}

impl Refused {
    /// As the Extensions page says it: `service jira.request: ...`.
    pub fn line(&self) -> String {
        format!("{} {}: {}", self.kind.word(), self.name, self.why)
    }
}

/// Every name `manifest` lists, by kind, with the claim it makes to it.
fn names(id: &str, manifest: &Manifest) -> Vec<(NameKind, String, Option<Claim>)> {
    let c = &manifest.contributes;
    let claim = |name: &str| borrows(id, &c.namespaces, name);
    let toolset = toolset_of(id);
    let mut out = Vec::new();
    out.extend(c.services.iter().map(|s| (NameKind::Service, s.clone(), claim(s))));
    out.extend(c.commands.iter().map(|x| (NameKind::Command, x.id.clone(), claim(&x.id))));
    out.extend(c.settings.iter().map(|x| (NameKind::Setting, x.key.clone(), claim(&x.key))));
    // Named by the id's last part, which anyone may have.
    out.extend(c.agent_tools.iter().map(|t| (NameKind::AgentTool, format!("{toolset}_{}", t.name), Some(Claim::Borrowed))));
    out.extend(c.agent_prompts.iter().map(|p| (NameKind::Prompt, p.name.clone(), Some(Claim::Borrowed))));
    out.sort();
    out.dedup();
    out
}

/// The installed extension other than `id` that `name` belongs to by
/// its full id, if any: the longest id it's under (`acme.a.b.x` is
/// `acme.a.b`'s, not `acme.a`'s).
fn under_another<'a>(id: &str, name: &str, installed: &[&'a str]) -> Option<&'a str> {
    let owner = installed.iter().copied().filter(|other| name.strip_prefix(other).is_some_and(|rest| rest.starts_with('.'))).max_by_key(|other| other.len())?;
    (owner != id).then_some(owner)
}

/// The namespace through which alone `id` names `name`, when it's the
/// publisher of another installed extension (`acme` beside `acme.dusk`).
fn void_namespace<'a>(id: &str, manifest: &'a Manifest, name: &str, installed: &[&str]) -> Option<&'a str> {
    let short = id.rsplit('.').next().unwrap_or_default();
    let publisher = id.split('.').next().unwrap_or_default();
    let under = |word: &str| name.strip_prefix(word).is_some_and(|rest| rest.starts_with('.'));
    if under(short) {
        return None;
    }
    let publishers: Vec<&str> = installed.iter().filter(|other| **other != id).filter_map(|other| other.split('.').next()).collect();
    manifest.contributes.namespaces.iter().map(String::as_str).find(|n| under(n) && *n != publisher && publishers.contains(n))
}

/// The names each of `claimants` lists that aren't its: `installed` is
/// every installed extension's id, switched on or not, and `fenix_owns`
/// says which names Fenix keeps for itself.
pub fn refused(claimants: &[Claimant], installed: &[&str], fenix_owns: &dyn Fn(NameKind, &str) -> bool) -> Vec<Refused> {
    let mut out = Vec::new();
    let mut contested: BTreeMap<(NameKind, String), Vec<(Claim, bool, &str)>> = BTreeMap::new();
    for c in claimants {
        for (kind, name, claim) in names(c.id, c.manifest) {
            let mut refuse = |why: String| out.push(Refused { id: c.id.to_string(), kind, name: name.clone(), why });
            if fenix_owns(kind, &name) {
                refuse("it's Fenix's own".into());
                continue;
            }
            if matches!(kind, NameKind::Service | NameKind::Command | NameKind::Setting) {
                if let Some(owner) = under_another(c.id, &name, installed) {
                    refuse(format!("names under {owner} are {owner}'s"));
                    continue;
                }
                if let Some(namespace) = void_namespace(c.id, c.manifest, &name, installed) {
                    refuse(format!("its namespace {namespace} is another extension's publisher, so it claims nothing"));
                    continue;
                }
            }
            let Some(claim) = claim else {
                // `Manifest::parse` lets none of these through; a manifest
                // made some other way still claims nothing it may not.
                refuse("it isn't under its name or a namespace of its own".into());
                continue;
            };
            contested.entry((kind, name)).or_default().push((claim, c.managed, c.id));
        }
    }
    for ((kind, name), claims) in contested {
        if claims.len() < 2 {
            continue;
        }
        // Under its id is caught above, so these are borrowed words; the
        // order holds whatever a manifest made some other way claims.
        let best = claims.iter().map(|(claim, _, _)| *claim).max().unwrap_or(Claim::Borrowed);
        let strongest: Vec<&(Claim, bool, &str)> = claims.iter().filter(|(claim, _, _)| *claim == best).collect();
        let managed: Vec<&&(Claim, bool, &str)> = strongest.iter().filter(|(_, managed, _)| *managed).collect();
        let winner = match (strongest.len(), managed.len()) {
            (1, _) => Some(strongest[0].2),
            (_, 1) => Some(managed[0].2),
            _ => None,
        };
        let mut tied: Vec<&str> = if managed.len() > 1 { managed.iter().map(|c| c.2).collect() } else { strongest.iter().map(|c| c.2).collect() };
        tied.sort_unstable();
        for (_, _, id) in &claims {
            if Some(*id) == winner {
                continue;
            }
            let why = match winner {
                Some(w) if strongest.len() > 1 => format!("it's {w}'s, the machine's extension"),
                Some(w) => format!("it's {w}'s, under its id"),
                None if tied.contains(id) => format!("{} all claim it, so none of them has it -- switch all but one off (SPC h x)", said_list(&tied)),
                None => format!("{} claim it ahead of it, and none of them has it", said_list(&tied)),
            };
            out.push(Refused { id: id.to_string(), kind, name: name.clone(), why });
        }
    }
    out.sort_by(|a, b| (&a.id, a.kind, &a.name).cmp(&(&b.id, b.kind, &b.name)));
    out
}

/// `a and b`, `a, b and c`.
fn said_list(ids: &[&str]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `manifest`, extension `id`'s, without the names `refused` says aren't
/// its: a refused service isn't routed to it, a command isn't
/// registered, a setting isn't shown or given to it; and an agent tool,
/// resource or prompt calling a service it no longer serves goes too.
pub fn withdraw(id: &str, manifest: &mut Manifest, refused: &[Refused]) {
    let gone = |kind: NameKind, name: &str| refused.iter().any(|r| r.id == id && r.kind == kind && r.name == name);
    let toolset = toolset_of(id);
    let c = &mut manifest.contributes;
    c.services.retain(|s| !gone(NameKind::Service, s));
    c.commands.retain(|x| !gone(NameKind::Command, &x.id));
    c.settings.retain(|x| !gone(NameKind::Setting, &x.key));
    let services = c.services.clone();
    let serves = |s: &str| services.iter().any(|x| x == s);
    c.agent_tools.retain(|t| serves(&t.service) && !gone(NameKind::AgentTool, &format!("{toolset}_{}", t.name)));
    c.agent_resources.retain(|r| serves(&r.service));
    c.agent_prompts.retain(|p| serves(&p.service) && !gone(NameKind::Prompt, &p.name));
}

/// Why no one answers service `name`, when it's refused to every
/// extension listing it: what the caller is told.
pub fn unserved(name: &str, refused: &[Refused]) -> Option<String> {
    let mut why: Vec<&Refused> = refused.iter().filter(|r| r.kind == NameKind::Service && r.name == name).collect();
    why.sort_by(|a, b| a.id.cmp(&b.id));
    let first = why.first()?;
    Some(format!("{name}: {} -- refused to {}", first.why, said_list(&why.iter().map(|r| r.id.as_str()).collect::<Vec<_>>())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, contributes: &str) -> Manifest {
        Manifest::parse(&format!(
            "[extension]\nid = \"{id}\"\nname = \"X\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\nfenix = \">=1.0, <2\"\napi = \"1.6\"\n\n{contributes}"
        ))
        .unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn nothing_is_fenixs(_: NameKind, _: &str) -> bool {
        false
    }

    fn refusals(all: &[(&str, &Manifest, bool)], installed: &[&str]) -> Vec<(String, String, String)> {
        let claimants: Vec<Claimant> = all.iter().map(|(id, m, managed)| Claimant { id, manifest: m, managed: *managed }).collect();
        refused(&claimants, installed, &nothing_is_fenixs).into_iter().map(|r| (r.id, r.name, r.why)).collect()
    }

    #[test]
    fn a_name_under_an_installed_id_is_that_extensions() {
        let jira = manifest("fenix.jira", "[contributes]\nservices = [\"fenix.jira.tracker.about\"]\n");
        // Its own id ends in .jira: it may list jira.*, never fenix.jira.*.
        let other = manifest("acme.jira", "[contributes]\nservices = [\"acme.jira.x\"]\n");
        let r = refusals(&[("acme.jira", &other, false), ("fenix.jira", &jira, false)], &["acme.jira", "fenix.jira"]);
        assert!(r.is_empty(), "{r:?}");
        // A fenix.* extension with the namespace jira can't reach under the
        // installed fenix.jira's id: it's not jira.*, and fenix.jira.* is
        // fenix.jira's.
        let pack = manifest("fenix.tracker", "[contributes]\nnamespaces = [\"fenix\"]\nservices = [\"fenix.jira.tracker.about\"]\n");
        let r = refusals(&[("fenix.jira", &jira, false), ("fenix.tracker", &pack, false)], &["fenix.jira", "fenix.tracker"]);
        assert_eq!(r, vec![("fenix.tracker".into(), "fenix.jira.tracker.about".into(), "names under fenix.jira are fenix.jira's".into())]);
        // Switched off, it still holds them: installed is enough.
        let r = refusals(&[("fenix.tracker", &pack, false)], &["fenix.jira", "fenix.tracker"]);
        assert_eq!(r.len(), 1, "{r:?}");
    }

    #[test]
    fn a_full_id_beats_a_borrowed_word_and_equal_claims_leave_it_to_no_one() {
        let jira = manifest("fenix.jira", "[contributes]\nservices = [\"jira.request\", \"jira.config\"]\n");
        let evil = manifest("evil.jira", "[contributes]\nservices = [\"jira.request\"]\n");
        let r = refusals(&[("evil.jira", &evil, false), ("fenix.jira", &jira, false)], &["evil.jira", "fenix.jira"]);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|(_, name, why)| name == "jira.request" && why.contains("evil.jira and fenix.jira all claim it")), "{r:?}");
        // jira.config, which only fenix.jira lists, stays its.
        assert!(!r.iter().any(|(_, name, _)| name == "jira.config"));
        // The machine's over yours.
        let r = refusals(&[("evil.jira", &evil, false), ("fenix.jira", &jira, true)], &["evil.jira", "fenix.jira"]);
        assert_eq!(r, vec![("evil.jira".into(), "jira.request".into(), "it's fenix.jira's, the machine's extension".into())]);
        // A full id over a namespace: acme.tools serves acme.tools.x; a
        // namespace "acme" claims nothing beside acme.* installed (rule 3),
        // but its namespace tools does, and loses to the full id.
        let tools = manifest("acme.tools", "[contributes]\nservices = [\"acme.tools.x\"]\n");
        let borrower = manifest("bob.b", "[contributes]\nnamespaces = [\"acme\"]\nservices = [\"acme.tools.x\", \"acme.other\"]\n");
        let r = refusals(&[("acme.tools", &tools, false), ("bob.b", &borrower, false)], &["acme.tools", "bob.b"]);
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0], ("bob.b".into(), "acme.other".into(), "its namespace acme is another extension's publisher, so it claims nothing".into()));
        assert_eq!(r[1], ("bob.b".into(), "acme.tools.x".into(), "names under acme.tools are acme.tools's".into()));
    }

    #[test]
    fn fenixs_own_names_are_refused_and_a_withdrawn_service_takes_its_tools() {
        let m = manifest(
            "acme.gitlab",
            "[contributes]\nservices = [\"gitlab.search\", \"gitlab.read\"]\n\n[[contributes.settings]]\nkey = \"gitlab.token\"\n\n[[contributes.settings]]\nkey = \"gitlab.host\"\n\n[[contributes.agent_tools]]\nname = \"search\"\ndescription = \"x\"\nservice = \"gitlab.search\"\n\n[[contributes.agent_prompts]]\nname = \"triage\"\nservice = \"gitlab.read\"",
        );
        let owns = |kind: NameKind, name: &str| (kind == NameKind::Setting && name == "gitlab.token") || (kind == NameKind::Service && name == "gitlab.search");
        let refused = refused(&[Claimant { id: "acme.gitlab", manifest: &m, managed: false }], &["acme.gitlab"], &owns);
        assert_eq!(refused.iter().map(Refused::line).collect::<Vec<_>>(), ["service gitlab.search: it's Fenix's own", "setting gitlab.token: it's Fenix's own"]);
        let mut used = m.clone();
        withdraw("acme.gitlab", &mut used, &refused);
        assert_eq!(used.contributes.services, ["gitlab.read"]);
        assert_eq!(used.contributes.settings.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["gitlab.host"]);
        assert!(used.contributes.agent_tools.is_empty(), "its tool's service is gone");
        assert_eq!(used.contributes.agent_prompts.len(), 1);
        assert_eq!(unserved("gitlab.search", &refused).unwrap(), "gitlab.search: it's Fenix's own -- refused to acme.gitlab");
        assert_eq!(unserved("gitlab.read", &refused), None);
    }

    #[test]
    fn two_tools_or_prompts_of_one_name_are_no_ones() {
        let a = manifest("acme.x-y", "[contributes]\nservices = [\"x-y.s\"]\n\n[[contributes.agent_tools]]\nname = \"go\"\ndescription = \"x\"\nservice = \"x-y.s\"\n");
        let b = manifest("bob.x_y", "[contributes]\nservices = [\"x_y.s\"]\n\n[[contributes.agent_tools]]\nname = \"go\"\ndescription = \"x\"\nservice = \"x_y.s\"\n");
        let r = refusals(&[("acme.x-y", &a, false), ("bob.x_y", &b, false)], &["acme.x-y", "bob.x_y"]);
        assert_eq!(r.iter().map(|(id, name, _)| format!("{id} {name}")).collect::<Vec<_>>(), ["acme.x-y x_y_go", "bob.x_y x_y_go"]);
    }
}
