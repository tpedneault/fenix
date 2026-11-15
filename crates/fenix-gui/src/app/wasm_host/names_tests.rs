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
    app.wasm.installed = claimed(vec![(ours.clone(), false), (theirs.clone(), false)], &["evil.hello", "test.hello"]);
    // Not routed by order to either.
    assert_eq!(app.code_server("hello.upper", None), None);
    let why = app.code_unserved("hello.upper");
    assert!(why.contains("evil.hello and test.hello all claim it") && why.contains("refused to evil.hello and test.hello"), "{why}");
    // What only one lists stays its.
    assert_eq!(app.code_server("hello.later", None).as_deref(), Some("test.hello"));
    assert_eq!(app.code_unserved("hello.nothing"), "nothing offers hello.nothing");
    // An extension asking for it is answered so: the reply is delivered
    // to it (here, not running, it's dropped) -- and nothing was started.
    app.answer_code_request("test.asker", Request::Service { id: 1, name: "hello.upper".into(), payload: String::new() });
    assert!(app.wasm.workers.is_empty(), "no extension started for it");
    // The machine's over yours.
    app.wasm.installed = claimed(vec![(ours, false), (theirs, true)], &["evil.hello", "test.hello"]);
    assert_eq!(app.code_server("hello.upper", None).as_deref(), Some("evil.hello"));
    let ours = app.wasm.installed.iter().find(|e| e.id == "test.hello").unwrap();
    assert_eq!(ours.refused[0].line(), "service hello.upper: it's evil.hello's, the machine's extension");
    assert!(!ours.manifest.contributes.services.contains(&"hello.upper".to_string()));
}

#[test]
fn a_name_under_an_installed_id_is_that_extensions_whoever_borrows_the_word() {
    let mut app = App::with_file(None);
    // `fenix.hello` (a fenix.* extension, so it may borrow the word fenix)
    // lists a name under `fenix.jira`'s id; with fenix.jira installed --
    // even switched off, so not among the code extensions -- it's refused.
    let pack = as_id("fenix.hello", "[contributes]\nnamespaces = [\"fenix\"]\nservices = [\"fenix.jira.tracker.about\"]\n");
    app.wasm.installed = claimed(vec![(pack.clone(), false)], &["fenix.hello", "fenix.jira"]);
    assert_eq!(app.code_server("fenix.jira.tracker.about", None), None);
    assert_eq!(app.code_unserved("fenix.jira.tracker.about"), "fenix.jira.tracker.about: names under fenix.jira are fenix.jira's -- refused to fenix.hello");
    // Not installed, nothing holds it: a fenix.* extension may serve it.
    app.wasm.installed = claimed(vec![(pack, false)], &["fenix.hello"]);
    assert_eq!(app.code_server("fenix.jira.tracker.about", None).as_deref(), Some("fenix.hello"));
}

/// Before: an extension naming one of Fenix's settings as its own --
/// `gitlab.token`, through the namespace `gitlab` -- was given its value,
/// the token, with no `permissions.secrets`. As Fenix lists the
/// extensions now, the setting is refused and never given to it.
#[test]
fn fenixs_own_setting_is_never_an_extensions() {
    let mut app = App::with_file(None);
    app.config.gitlab_token = Some("glpat-secret".into());
    let ext = hello("[contributes]\nnamespaces = [\"gitlab\"]\n\n[[contributes.settings]]\nkey = \"gitlab.token\"\n\n[[contributes.settings]]\nkey = \"hello.own\"\n");
    let (unclaimed, _) = app.code_settings_now(&ext);
    assert_eq!(unclaimed.get("gitlab.token").map(String::as_str), Some("glpat-secret"), "the hole, as the manifest alone has it");
    let used = claimed(vec![(ext, false)], &["test.hello"]).remove(0);
    let (settings, secrets) = app.code_settings_now(&used);
    assert!(!settings.contains_key("gitlab.token") && !secrets.contains_key("gitlab.token"), "{settings:?}");
    assert!(settings.contains_key("hello.own"));
    assert_eq!(used.refused.iter().map(|r| r.line()).collect::<Vec<_>>(), ["setting gitlab.token: it's Fenix's own"]);
    assert!(crate::installed::settings_of(&used).iter().all(|s| s.key != "gitlab.token"), "not on the settings page as its");
}

#[test]
fn a_command_two_claim_is_registered_for_neither() {
    let ours = hello("");
    let mut theirs = hello("");
    theirs.id = "evil.hello".into();
    let both = claimed(vec![(ours, false), (theirs, false)], &["evil.hello", "test.hello"]);
    let registry = CommandRegistry::with_builtins().with_code_extensions(&both);
    assert!(registry.get("hello.greet").is_none(), "not the first by order");
    assert!(both[0].refused.iter().any(|r| r.line().starts_with("command hello.greet: evil.hello and test.hello all claim it")), "{:?}", both[0].refused);
}
