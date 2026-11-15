//! Whose a name is, as the app uses the extensions: a name two claim
//! equally is no one's and its callers are told why; a name under an
//! installed id is that extension's; Fenix's own settings are never an
//! extension's (`docs/plans/service-callers.md` §4).

use super::tests::hello;
use super::*;
use crate::installed::{claimed, CodeExtension};

/// The test extension as `id`, listing `services`.
fn serving(id: &str, services: &[&str]) -> CodeExtension {
    let list = services.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", ");
    let mut ext = hello(&format!("[contributes]\nservices = [{list}]\n"));
    ext.id = id.into();
    ext
}

/// The test extension as `id`, its manifest saying so, with `extra`.
fn as_id(id: &str, extra: &str) -> CodeExtension {
    let mut ext = hello("");
    let text = format!("[extension]\nid = \"{id}\"\nname = \"Hello\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"1.0\"\nfenix = \">=1.0, <2\"\n{extra}");
    ext.manifest = fenix_extensions::Manifest::parse(&text).unwrap();
    ext.id = id.into();
    ext
}

#[test]
fn a_service_two_claim_equally_is_no_ones_and_its_caller_is_told() {
    let mut app = App::with_file(None);
    let ours = serving("test.hello", &["hello.upper", "hello.later"]);
    let theirs = serving("evil.hello", &["hello.upper"]);
    app.wasm.installed = claimed(vec![(ours.clone(), false), (theirs.clone(), false)]);
    // Not routed by order to either.
    assert_eq!(app.code_server("hello.upper", None), None);
    let why = app.code_unserved("hello.upper");
    assert!(why.contains("evil.hello and test.hello both claim it") && why.contains("refused to evil.hello and test.hello"), "{why}");
    // What only one lists stays its.
    assert_eq!(app.code_server("hello.later", None).as_deref(), Some("test.hello"));
    assert_eq!(app.code_unserved("hello.nothing"), "nothing offers hello.nothing");
    // An extension asking for it is answered so: the reply is delivered
    // to it (here, not running, it's dropped) -- and nothing was started.
    app.answer_code_request("test.asker", Request::Service { id: 1, name: "hello.upper".into(), payload: String::new() });
    assert!(app.wasm.workers.is_empty(), "no extension started for it");
    // The machine's over yours.
    app.wasm.installed = claimed(vec![(ours, false), (theirs, true)]);
    assert_eq!(app.code_server("hello.upper", None).as_deref(), Some("evil.hello"));
    let ours = app.wasm.installed.iter().find(|e| e.id == "test.hello").unwrap();
    assert_eq!(ours.refused[0].line(), "service hello.upper: it's evil.hello's, the machine's extension");
    assert!(!ours.manifest.contributes.services.contains(&"hello.upper".to_string()));
}

#[test]
fn a_name_under_an_installed_id_is_that_extensions_whoever_borrows_the_word() {
    let mut app = App::with_file(None);
    // `fenix.hello` (a fenix.* extension, so it may borrow the word fenix)
    // lists a name under `fenix.jira`'s id; with fenix.jira switched on,
    // it's refused, though fenix.jira doesn't list it.
    let pack = as_id("fenix.hello", "[contributes]\nnamespaces = [\"fenix\"]\nservices = [\"fenix.jira.tracker.about\"]\n");
    app.wasm.installed = claimed(vec![(pack.clone(), false), (as_id("fenix.jira", ""), false)]);
    assert_eq!(app.code_server("fenix.jira.tracker.about", None), None);
    assert_eq!(app.code_unserved("fenix.jira.tracker.about"), "fenix.jira.tracker.about: names under fenix.jira are fenix.jira's -- refused to fenix.hello");
    // Not switched on (or a theme, a data pack): it holds nothing, and a
    // fenix.* extension may serve it.
    app.wasm.installed = claimed(vec![(pack, false)]);
    assert_eq!(app.code_server("fenix.jira.tracker.about", None).as_deref(), Some("fenix.hello"));
}

/// Before: an extension naming one of Fenix's settings as its own --
/// `gitlab.token`, through the namespace `gitlab` -- was given its value,
/// the token, with no `permissions.secrets`. Now it's refused that
/// setting, and so isn't run at all.
#[test]
fn fenixs_own_setting_is_never_an_extensions() {
    let mut app = App::with_file(None);
    app.config.gitlab_token = Some("glpat-secret".into());
    let ext = hello("[contributes]\nnamespaces = [\"gitlab\"]\n\n[[contributes.settings]]\nkey = \"gitlab.token\"\n\n[[contributes.settings]]\nkey = \"hello.own\"\n");
    let (unclaimed, _) = app.code_settings_now(&ext);
    assert_eq!(unclaimed.get("gitlab.token").map(String::as_str), Some("glpat-secret"), "the hole, as the manifest alone has it");
    assert!(claimed(vec![(ext.clone(), false)]).is_empty(), "not run");
    let held = crate::installed::claimed_all(vec![(ext, false)]).remove(0);
    assert!(held.held_back());
    assert_eq!(crate::installed::name_lines(&held)[1], "setting gitlab.token: it's Fenix's own");
}

/// The review's M1: `fenix.jira` keeps `jira.seam_writes`, and you set it
/// to `never`. Another `*.jira` declaring the same key took it from both,
/// and the tracker read its default, `ask`: your off switch undone. Now the
/// store's own keeps it, with your value, and the other isn't run; where
/// neither is the store's, neither runs, and a notification that stays
/// says so -- no extension ever reads a default in the place of your
/// value.
#[test]
fn a_setting_in_dispute_never_falls_back_to_its_default() {
    let mut app = App::with_file(None);
    app.config.set_value("jira.seam_writes", Some(fenix_config::Value::Text("never".into())));
    let setting = "[[contributes.settings]]\nkey = \"jira.seam_writes\"\ndefault = \"ask\"\n";
    let jira = as_id("fenix.jira", setting);
    let evil = as_id("evil.jira", setting);
    let acme = as_id("acme.jira", setting);
    let on = claimed(vec![(evil.clone(), false), (jira, false)]);
    assert_eq!(on.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["fenix.jira"], "the other isn't run");
    let (settings, _) = app.code_settings_now(&on[0]);
    assert_eq!(settings.get("jira.seam_writes").map(String::as_str), Some("never"), "your value, as before the other came");
    // Neither the store's: neither runs, and it's said, critical.
    let all = crate::installed::claimed_all(vec![(acme, false), (evil, false)]);
    assert!(all.iter().all(|e| e.held_back()));
    app.wasm.installed = Vec::new();
    app.code_names_said(&all);
    let notes: Vec<&crate::notify::Note> = app.notifications.history().iter().filter(|n| n.summary.contains("isn't running")).collect();
    assert_eq!(notes.len(), 2, "one for each");
    assert!(notes.iter().all(|n| n.urgency == crate::notify::Urgency::Critical && n.body.contains("setting jira.seam_writes: acme.jira and evil.jira both claim it")), "{:?}", notes.iter().map(|n| &n.body).collect::<Vec<_>>());
    // Said once.
    app.code_names_said(&all);
    assert_eq!(app.notifications.history().iter().filter(|n| n.summary.contains("isn't running")).count(), 2);
}

#[test]
fn a_command_two_claim_is_registered_for_neither() {
    let ours = hello("");
    let mut theirs = hello("");
    theirs.id = "evil.hello".into();
    let both = claimed(vec![(ours, false), (theirs, false)]);
    let registry = CommandRegistry::with_builtins().with_code_extensions(&both);
    assert!(registry.get("hello.greet").is_none(), "not the first by order");
    assert!(both[0].refused.iter().any(|r| r.line().starts_with("command hello.greet: evil.hello and test.hello both claim it")), "{:?}", both[0].refused);
}

/// Another extension switched on or off changes what the others have,
/// at once (the review's M2): `code_extensions_now` puts the new claims in
/// use without a restart, and a setting given back is registered.
#[test]
fn a_change_in_the_claims_is_put_in_use_at_once() {
    let mut app = App::with_file(None);
    let ours = as_id("test.hello", "[contributes]\nservices = [\"hello.upper\"]\n\n[[contributes.settings]]\nkey = \"hello.given_back\"\n");
    let theirs = as_id("evil.hello", "[contributes]\nservices = [\"hello.upper\"]\n\n[[contributes.settings]]\nkey = \"hello.given_back\"\n");
    app.code_extensions_now(claimed(vec![(ours.clone(), false), (theirs, false)]));
    assert_eq!(app.code_server("hello.upper", None), None, "both claim it");
    assert!(fenix_config::schema::setting("hello.given_back").is_none(), "no one's");
    app.code_extensions_now(claimed(vec![(ours, false)]));
    assert_eq!(app.code_server("hello.upper", None).as_deref(), Some("test.hello"), "its again once the other is off");
    assert!(fenix_config::schema::setting("hello.given_back").is_some(), "its setting back on the settings page");
}

/// The review's S2: `fenix.review` and `zzz.x` both saying `github.com`,
/// the last by id was the forge, and was given the repository to read.
/// Now a forge's host goes as a name does.
#[test]
fn a_forges_host_two_claim_goes_as_a_name_does() {
    let forge = "[[contributes.forges]]\nname = \"F\"\nhosts = [\"github.com\"]\n";
    let review = as_id("fenix.review", forge);
    let other = as_id("zzz.x", forge);
    let claim = |e: &CodeExtension| e.manifest.contributes.forges[0].claims("github.com", &|_| None);
    assert!(claim(&review) > 0);
    assert_eq!(forge_for("github.com", &[(claim(&review), &review), (claim(&other), &other)]), Ok("fenix.review".to_string()));
    let another = as_id("acme.y", forge);
    let why = forge_for("github.com", &[(claim(&another), &another), (claim(&other), &other)]).unwrap_err().unwrap();
    assert!(why.contains("acme.y and zzz.x each say they're its forge, so neither is asked"), "{why}");
    let mut managed = another.clone();
    managed.managed = true;
    assert_eq!(forge_for("github.com", &[(claim(&managed), &managed), (claim(&review), &review)]), Ok("acme.y".to_string()));
    assert_eq!(forge_for("gitlab.com", &[(0, &review)]), Err(None));
}
