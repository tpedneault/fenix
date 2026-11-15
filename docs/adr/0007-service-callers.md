# ADR 0007: A service knows its caller, may choose its callers, and owns its names

- **Status:** proposed 2026-11-15, for Fenix 1.6 (extension API 1.6)
- **Date:** 2026-11-15
- **Plan:** [`docs/plans/service-callers.md`](../plans/service-callers.md)

## Context

Extensions call each other's services (`call-service`, `serve`). Fenix
knows which extension asked -- each worker's requests come in tagged with
its id -- but doesn't pass it on, and calling a service needs no
permission. So any installed extension can call the store's trackers'
"move this issue", "comment", "edit" services, and Jira's `jira.request`
(any REST request), and act as you. The trackers ask you once a session
before the first write through them, since they can't tell the agenda
from anyone else.

Names aren't anyone's. A manifest may name a service, command or setting
under its id's last part, its full id, or any namespace it lists, and
namespaces aren't checked: `acme.fenix`, `namespaces = ["fenix"]` and
`namespaces = ["fenix.jira"]` may all serve `fenix.jira.tracker.about`.
A call goes to the first extension listing the name, the machine's then
yours, by id. An extension may also declare one of Fenix's own settings
(`gitlab.token`) as its own and be given its value.

What authenticates a publisher: the store's signed index and package
hashes, and its rule that only `fenix` publishes `fenix.*`. Nothing, for
a package installed from a file; and Fenix doesn't record where an
installed package came from.

Extension API 1.6 isn't released; a minor only adds: a new import, a new
type, an export Fenix calls only when it's there. A WIT record can't gain
a field, and `serve` can't change. 1.5's manifest reader ignores a
`[contributes]` section it doesn't know.

## Decision

1. **`called-by: func() -> option<service-caller>`**, a new import, and
   `service-caller` a new variant: `extension(id)`, `fenix`,
   `agent-tool`, `agent-resource`, `agent-prompt`, `control`. Fenix sets
   it per call, the host keeps it while `serve` runs and clears it after;
   none outside `serve`. In Rust, `fenix_extension_api::called_by()`, a
   function the extension calls -- never read by the crate's glue, so a
   guest that doesn't call it doesn't import it.
2. **`[contributes.service_callers]`**: a service of its own, mapped to
   who may call it (ids, `fenix`, `agents`, `control`). Fenix refuses
   anyone else before the extension is started or called, with an error
   naming who may. A service it doesn't name is open to anyone. The
   section needs `api = "1.6"`, so Fenix 1.5, which would ignore it,
   refuses the extension instead of running it open.
3. **Names.** At parse: a namespace is one word; only `fenix.*` ids may
   use the word `fenix`. Among the installed: Fenix's own names are
   Fenix's; a name under an installed extension's full id is that
   extension's; a namespace that is another installed extension's
   publisher claims nothing; a full id beats a borrowed word (last part
   or namespace); between equal claims the machine's beats yours, else
   **neither** has the name. A refused name is taken out of the
   extension as Fenix uses it and said on the Extensions page; its
   callers are told why.

## Rejected

- **The caller as an argument to `serve`, or a field of a record.** Not
  additive: every guest's `serve` would change.
- **Only the import, or only the manifest section.** The import alone is
  a check every service must remember; the section alone can't say
  "reads from anyone, writes from the agenda". Both, as the plan says.
- **A `callers` field on each service.** `services` is a list of strings;
  a table there would make Fenix 1.5 refuse the manifest.
- **The section without `api = "1.6"`.** Fenix 1.5 would run the
  extension with its gate gone and nothing to say so.
- **The later of two claimants loses.** Fenix keeps no install order;
  every extension installed before this release has none, and any order
  it could use is one an impostor can choose. Refusing both can't be won
  silently.
- **Routing by id order, as now.** An impostor whose id sorts first wins
  every call.
- **Reserving every publisher's word, not only `fenix`.** Installing
  someone's `git.tools` would take the Review extension's `git.*`
  commands; only the store's own word is held for everyone, and a
  namespace loses only to an installed publisher.

## Consequences

- The trackers can take writes from `fenix.agenda` alone, with no
  question, and Jira's `jira.request` can be closed to everyone else, by
  a few manifest lines -- at the price of `api = "1.6"`.
- Every extension in the store keeps every name.
- An impostor installed beside an extension can deny it a borrowed name
  (both lose it, said on the Extensions page), but no longer take it.
- `control` and `agents` mean "a program running as you", which an
  extension allowed to run programs, or to reach Fenix's control socket,
  can be. Only an extension id is an extension.
- A package installed from a file is trusted as its manifest says.
