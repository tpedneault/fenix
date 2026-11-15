//! Whose a name is, among the switched-on code extensions: a service, a command,
//! a setting, an agent tool or a prompt is one extension's alone, or no
//! one's -- never the first one's in some order. `docs/plans/service-callers.md`
//! §4.
//!
//! What a manifest may name at all is `Manifest::parse`'s (one-word
//! namespaces; the word `fenix` only for `fenix.*` ids). Only the
//! switched-on code extensions claim anything -- a theme, a data pack or
//! one switched off serves, runs and keeps nothing -- and among them:
//!
//! 1. Fenix's own names are Fenix's.
//! 2. A word borrowed (the id's last part, a namespace) never reaches
//!    under another's full id plus a dot.
//! 3. A full id beats a borrowed word; between full ids, the shortest
//!    that lists the name has it, so a longer id never takes a name.
//! 4. Between borrowed words: the machine's, else the store's own
//!    (`fenix.*`), when only one is; else no one has it.
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
    /// The name its agent tools and resources go by (`toolset_of`): one
    /// extension's, and never one of Fenix's own toolsets (`git`), whose
    /// rules (`toolset:git = allow`) would be its too.
    Toolset,
    /// A resource's URI template that isn't under `fenix://ext/<its
    /// toolset>/`: so one extension's templates never match a URI meant
    /// for another's.
    Resource,
}

impl NameKind {
    pub fn word(self) -> &'static str {
        match self {
            NameKind::Service => "service",
            NameKind::Command => "command",
            NameKind::Setting => "setting",
            NameKind::AgentTool => "agent tool",
            NameKind::Prompt => "agent prompt",
            NameKind::Toolset => "toolset",
            NameKind::Resource => "agent resource",
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
    if !c.agent_tools.is_empty() || !c.agent_resources.is_empty() {
        out.push((NameKind::Toolset, toolset.clone(), Some(Claim::Borrowed)));
    }
    // A template is its toolset's, which is claimed as one; one that isn't
    // under it claims nothing.
    let under = format!("fenix://ext/{toolset}/");
    out.extend(c.agent_resources.iter().filter(|r| !r.uri_template.starts_with(&under)).map(|r| (NameKind::Resource, r.uri_template.clone(), None)));
    out.sort();
    out.dedup();
    out
}

/// The switched-on code extension other than `id` whose full id `name`
/// is under, if any: a word borrowed can't reach there, whether that
/// extension lists the name or not.
fn under_another<'a>(id: &str, name: &str, claimants: &[Claimant<'a>]) -> Option<&'a str> {
    claimants.iter().map(|c| c.id).filter(|other| *other != id).find(|other| name.strip_prefix(other).is_some_and(|rest| rest.starts_with('.')))
}

/// One claim to a name: how, whether it's the machine's, and whose.
type Contender<'a> = (Claim, bool, &'a str);

/// Who has a name several claim, if anyone: under a full id, the shortest
/// such id that lists it (`fenix.jira` over `fenix.jira.tracker` for
/// `fenix.jira.tracker.about`, so a longer id never takes what a shorter
/// one has); between borrowed words, the machine's, else the one the
/// store publishes (`fenix.*`), when only one is; else no one. And how
/// it's said to the others.
fn winner<'a>(claims: &[Contender<'a>]) -> (Option<&'a str>, String) {
    let ids: Vec<&Contender> = claims.iter().filter(|c| c.0 == Claim::Id).collect();
    if let Some(shortest) = ids.iter().min_by_key(|c| c.2.len()) {
        return (Some(shortest.2), format!("it's {}'s, under its id", shortest.2));
    }
    let managed: Vec<&Contender> = claims.iter().filter(|c| c.1).collect();
    let pool: Vec<&Contender> = if managed.is_empty() { claims.iter().collect() } else { managed };
    if let [only] = pool.as_slice() {
        return (Some(only.2), format!("it's {}'s, the machine's extension", only.2));
    }
    let store: Vec<&&Contender> = pool.iter().filter(|c| c.2.split('.').next() == Some("fenix")).collect();
    if let [only] = store.as_slice() {
        return (Some(only.2), format!("it's {}'s, the store's own (fenix.*)", only.2));
    }
    let mut tied: Vec<&str> = pool.iter().map(|c| c.2).collect();
    tied.sort_unstable();
    let both = if tied.len() == 2 { "both" } else { "all" };
    (None, format!("{} {both} claim it, so none of them has it -- switch all but one off (SPC h x)", said_list(&tied)))
}

/// The names each of `claimants` -- the switched-on code extensions,
/// the only ones that can serve, run or keep anything -- lists that
/// aren't its; `fenix_owns` says which names Fenix keeps for itself.
pub fn refused(claimants: &[Claimant], fenix_owns: &dyn Fn(NameKind, &str) -> bool) -> Vec<Refused> {
    let mut out = Vec::new();
    let mut contested: BTreeMap<(NameKind, String), Vec<Contender>> = BTreeMap::new();
    for c in claimants {
        for (kind, name, claim) in names(c.id, c.manifest) {
            let mut refuse = |why: String| out.push(Refused { id: c.id.to_string(), kind, name: name.clone(), why });
            if fenix_owns(kind, &name) {
                refuse("it's Fenix's own".into());
                continue;
            }
            let Some(claim) = claim else {
                refuse(match kind {
                    NameKind::Resource => format!("a resource's template starts fenix://ext/{}/, its own toolset's, so it never matches a URI meant for another", toolset_of(c.id)),
                    // `Manifest::parse` lets none of these through; a manifest
                    // made some other way still claims nothing it may not.
                    _ => "it isn't under its name or a namespace of its own".into(),
                });
                continue;
            };
            if claim == Claim::Borrowed && matches!(kind, NameKind::Service | NameKind::Command | NameKind::Setting) {
                if let Some(owner) = under_another(c.id, &name, claimants) {
                    refuse(format!("names under {owner} are {owner}'s"));
                    continue;
                }
            }
            contested.entry((kind, name)).or_default().push((claim, c.managed, c.id));
        }
    }
    for ((kind, name), claims) in contested {
        if claims.len() < 2 {
            continue;
        }
        let (has, why) = winner(&claims);
        for (_, _, id) in &claims {
            if Some(*id) != has {
                out.push(Refused { id: id.to_string(), kind, name: name.clone(), why: why.clone() });
            }
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
/// resource or prompt calling a service it no longer serves goes too, as
/// does a gate on one (`service_callers`).
pub fn withdraw(id: &str, manifest: &mut Manifest, refused: &[Refused]) {
    let gone = |kind: NameKind, name: &str| refused.iter().any(|r| r.id == id && r.kind == kind && r.name == name);
    let toolset = toolset_of(id);
    let c = &mut manifest.contributes;
    c.services.retain(|s| !gone(NameKind::Service, s));
    c.commands.retain(|x| !gone(NameKind::Command, &x.id));
    c.settings.retain(|x| !gone(NameKind::Setting, &x.key));
    let services = c.services.clone();
    let serves = |s: &str| services.iter().any(|x| x == s);
    let toolset_gone = gone(NameKind::Toolset, &toolset);
    c.agent_tools.retain(|t| !toolset_gone && serves(&t.service) && !gone(NameKind::AgentTool, &format!("{toolset}_{}", t.name)));
    c.agent_resources.retain(|r| !toolset_gone && serves(&r.service) && !gone(NameKind::Resource, &r.uri_template));
    c.agent_prompts.retain(|p| serves(&p.service) && !gone(NameKind::Prompt, &p.name));
    c.service_callers.retain(|s, _| serves(s));
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

    fn refusals(all: &[(&str, &Manifest, bool)]) -> Vec<(String, String, String)> {
        let claimants: Vec<Claimant> = all.iter().map(|(id, m, managed)| Claimant { id, manifest: m, managed: *managed }).collect();
        refused(&claimants, &nothing_is_fenixs).into_iter().map(|r| (r.id, r.name, r.why)).collect()
    }

    #[test]
    fn a_word_borrowed_never_reaches_under_another_full_id() {
        let jira = manifest("fenix.jira", "[contributes]\nservices = [\"fenix.jira.tracker.about\"]\n");
        let other = manifest("acme.jira", "[contributes]\nservices = [\"acme.jira.x\"]\n");
        assert!(refusals(&[("acme.jira", &other, false), ("fenix.jira", &jira, false)]).is_empty());
        // A fenix.* extension borrowing the word fenix can't reach under
        // fenix.jira's id, whether fenix.jira lists the name or not.
        let pack = manifest("fenix.tracker", "[contributes]\nnamespaces = [\"fenix\"]\nservices = [\"fenix.jira.tracker.about\", \"fenix.jira.other\"]\n");
        let r = refusals(&[("fenix.jira", &jira, false), ("fenix.tracker", &pack, false)]);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|(id, _, why)| id == "fenix.tracker" && why == "names under fenix.jira are fenix.jira's"), "{r:?}");
        // Not switched on (so not a claimant), fenix.jira holds nothing.
        assert!(refusals(&[("fenix.tracker", &pack, false)]).is_empty());
    }

    /// The review's M2: a longer id took a name under a shorter one's, and
    /// any installed package -- a theme, one switched off -- voided a
    /// namespace by being that word's publisher.
    #[test]
    fn a_longer_id_never_takes_a_name_and_a_publisher_voids_no_namespace() {
        let jira = manifest("fenix.jira", "[contributes]\nservices = [\"fenix.jira.tracker.about\"]\n");
        let longer = manifest("fenix.jira.tracker", "[contributes]\nservices = [\"fenix.jira.tracker.about\", \"fenix.jira.tracker.more\"]\n");
        let r = refusals(&[("fenix.jira", &jira, false), ("fenix.jira.tracker", &longer, false)]);
        assert_eq!(r, vec![("fenix.jira.tracker".into(), "fenix.jira.tracker.about".into(), "it's fenix.jira's, under its id".into())]);
        // Review's git.* beside a code extension published as git: what
        // git.tools lists under its own id is its, Review keeps git.*.
        let review = manifest("fenix.review", "[contributes]\nnamespaces = [\"git\"]\n\n[[contributes.commands]]\nid = \"git.pull_request\"\ntitle = \"x\"\n");
        let tools = manifest("git.tools", "[contributes]\nservices = [\"git.tools.x\"]\n");
        assert!(refusals(&[("fenix.review", &review, false), ("git.tools", &tools, false)]).is_empty());
        // Only a real clash costs anything: git.tools listing git.pull_request
        // through the word git -- the store's own keeps it.
        let clash = manifest("git.tools", "[contributes]\nnamespaces = [\"git\"]\n\n[[contributes.commands]]\nid = \"git.pull_request\"\ntitle = \"x\"\n");
        let r = refusals(&[("fenix.review", &review, false), ("git.tools", &clash, false)]);
        assert_eq!(r, vec![("git.tools".into(), "git.pull_request".into(), "it's fenix.review's, the store's own (fenix.*)".into())]);
    }

    #[test]
    fn equal_claims_go_to_the_machines_then_the_stores_else_no_one() {
        let jira = manifest("fenix.jira", "[contributes]\nservices = [\"jira.request\", \"jira.config\"]\n");
        let evil = manifest("evil.jira", "[contributes]\nservices = [\"jira.request\"]\n");
        // The store's own over another's.
        let r = refusals(&[("evil.jira", &evil, false), ("fenix.jira", &jira, false)]);
        assert_eq!(r, vec![("evil.jira".into(), "jira.request".into(), "it's fenix.jira's, the store's own (fenix.*)".into())]);
        // The machine's over the store's.
        let r = refusals(&[("evil.jira", &evil, true), ("fenix.jira", &jira, false)]);
        assert_eq!(r, vec![("fenix.jira".into(), "jira.request".into(), "it's evil.jira's, the machine's extension".into())]);
        // Two neither of which is: no one.
        let acme = manifest("acme.jira", "[contributes]\nservices = [\"jira.request\"]\n");
        let r = refusals(&[("acme.jira", &acme, false), ("evil.jira", &evil, false)]);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|(_, _, why)| why.starts_with("acme.jira and evil.jira both claim it")), "{r:?}");
    }

    #[test]
    fn fenixs_own_names_are_refused_and_a_withdrawn_service_takes_its_tools() {
        let m = manifest(
            "acme.gitlab",
            "[contributes]\nservices = [\"gitlab.search\", \"gitlab.read\"]\n\n[[contributes.settings]]\nkey = \"gitlab.token\"\n\n[[contributes.settings]]\nkey = \"gitlab.host\"\n\n[[contributes.agent_tools]]\nname = \"search\"\ndescription = \"x\"\nservice = \"gitlab.search\"\n\n[[contributes.agent_prompts]]\nname = \"triage\"\nservice = \"gitlab.read\"\n\n[contributes.service_callers]\n\"gitlab.search\" = [\"agents\"]\n",
        );
        let owns = |kind: NameKind, name: &str| (kind == NameKind::Setting && name == "gitlab.token") || (kind == NameKind::Service && name == "gitlab.search");
        let refused = refused(&[Claimant { id: "acme.gitlab", manifest: &m, managed: false }], &owns);
        assert_eq!(refused.iter().map(Refused::line).collect::<Vec<_>>(), ["service gitlab.search: it's Fenix's own", "setting gitlab.token: it's Fenix's own"]);
        let mut used = m.clone();
        withdraw("acme.gitlab", &mut used, &refused);
        assert_eq!(used.contributes.services, ["gitlab.read"]);
        assert_eq!(used.contributes.settings.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["gitlab.host"]);
        assert!(used.contributes.agent_tools.is_empty(), "its tool's service is gone");
        assert_eq!(used.contributes.agent_prompts.len(), 1);
        assert!(used.contributes.service_callers.is_empty());
        assert_eq!(unserved("gitlab.search", &refused).unwrap(), "gitlab.search: it's Fenix's own -- refused to acme.gitlab");
        assert_eq!(unserved("gitlab.read", &refused), None);
    }

    /// The review's S1 and S5: `aaa.x`'s `fenix://ext/{a}/{b}/{c}` took an
    /// agent's read meant for Jira's; `acme.git` was in Fenix's `git`
    /// toolset, its rules and all.
    #[test]
    fn a_resource_is_under_its_own_toolset_and_fenixs_toolsets_are_fenixs() {
        let resource = |id: &str, template: &str| manifest(id, &format!("[contributes]\nservices = [\"{}.read\"]\n\n[[contributes.agent_resources]]\nname = \"r\"\nuri_template = \"{template}\"\nservice = \"{}.read\"\n", id.rsplit('.').next().unwrap(), id.rsplit('.').next().unwrap()));
        let jira = resource("fenix.jira", "fenix://ext/jira/issue/{key}");
        let shadow = resource("aaa.x", "fenix://ext/{a}/{b}/{c}");
        let r = refusals(&[("aaa.x", &shadow, false), ("fenix.jira", &jira, false)]);
        assert_eq!(r, vec![("aaa.x".into(), "fenix://ext/{a}/{b}/{c}".into(), "a resource's template starts fenix://ext/x/, its own toolset's, so it never matches a URI meant for another".into())]);
        let claimants = [Claimant { id: "aaa.x", manifest: &shadow, managed: false }];
        let mut used = shadow.clone();
        withdraw("aaa.x", &mut used, &refused(&claimants, &nothing_is_fenixs));
        assert!(used.contributes.agent_resources.is_empty());
        // Two of one last part: one toolset, the store's own has it.
        let other = resource("evil.jira", "fenix://ext/jira/issue/{key}");
        let r = refusals(&[("evil.jira", &other, false), ("fenix.jira", &jira, false)]);
        assert!(r.contains(&("evil.jira".into(), "jira".into(), "it's fenix.jira's, the store's own (fenix.*)".into())), "{r:?}");
        // Fenix's own toolsets.
        let git = manifest("acme.git", "[contributes]\nservices = [\"git.s\"]\n\n[[contributes.agent_tools]]\nname = \"blame_all\"\ndescription = \"x\"\nservice = \"git.s\"\n");
        let owns = |kind: NameKind, name: &str| kind == NameKind::Toolset && name == "git";
        let refused = refused(&[Claimant { id: "acme.git", manifest: &git, managed: false }], &owns);
        assert_eq!(refused.iter().map(Refused::line).collect::<Vec<_>>(), ["toolset git: it's Fenix's own"]);
        let mut used = git.clone();
        withdraw("acme.git", &mut used, &refused);
        assert!(used.contributes.agent_tools.is_empty());
    }

    #[test]
    fn two_tools_or_prompts_of_one_name_are_no_ones() {
        let a = manifest("acme.x-y", "[contributes]\nservices = [\"x-y.s\"]\n\n[[contributes.agent_tools]]\nname = \"go\"\ndescription = \"x\"\nservice = \"x-y.s\"\n");
        let b = manifest("bob.x_y", "[contributes]\nservices = [\"x_y.s\"]\n\n[[contributes.agent_tools]]\nname = \"go\"\ndescription = \"x\"\nservice = \"x_y.s\"\n");
        let r = refusals(&[("acme.x-y", &a, false), ("bob.x_y", &b, false)]);
        assert_eq!(r.iter().map(|(id, name, _)| format!("{id} {name}")).collect::<Vec<_>>(), ["acme.x-y x_y_go", "acme.x-y x_y", "bob.x_y x_y_go", "bob.x_y x_y"], "the tool, and the toolset");
    }
}
