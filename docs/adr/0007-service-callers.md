# ADR 0007: A service knows its caller, may choose its callers, and owns its names

- **Status:** proposed 2026-11-15, for Fenix 1.6 (extension API 1.6);
  revised in review the same day (the plan's §11)
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
   `agent-tool`, `agent-resource`, `agent-prompt`, `control`, and
   `other(word)` for an origin a later API names -- untrusted, never let
   through a gate -- since a guest's imported variant can't gain a case.
   `called_by_extension(id)` is the safe check. Fenix sets
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
   use the word `fenix`. Only switched-on code extensions claim names,
   and among them: Fenix's own (settings, services, commands, agent
   tools, toolsets, prompts) are Fenix's; a borrowed word (last part or
   namespace) never reaches under another's full id; a full id beats a
   borrowed word, the shortest of several; between equal claims the
   machine's, else the store's own (`fenix.*`) when only one is, else
   **no one**. Toolsets, resource templates (`fenix://ext/<toolset>/`)
   and forge hosts go the same way, and Fenix's own asks (`ccsds.hex`,
   `pdf.open`...) only to the store extension they're for. A refused
   name is taken out of the extension as Fenix uses it, said in a
   notification and on the Extensions page; its callers are told why.
4. **A setting in dispute is never withdrawn from an extension that
   runs**: one that can't have every setting it declares isn't run at
   all, so it never reads a default in the place of your value.

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
  commands. As first built, a namespace lost to any installed package
  of that publisher -- which did exactly that; the review removed it.
- **Any installed package as a claimant**, and **the longest id owns a
  name** (as first built): a theme or a switched-off package took
  names, and a sideloaded `fenix.jira.tracker` took the tracker seam
  from `fenix.jira`.
- **Withdrawing a disputed setting** (as first built): the extension read
  the setting's default, `ask` for `never`.
- **Wildcards in a gate** (`"acme.*"`): publishers aren't authenticated
  for sideloads, so a pattern would admit any file calling itself
  `acme.<x>`.

## Consequences

- The trackers can take writes from `fenix.agenda` alone, with no
  question, and Jira's `jira.request` can be closed to everyone else, by
  a few manifest lines -- at the price of `api = "1.6"`.
- Every extension in the store keeps every name.
- An impostor installed beside a store extension loses every tie to it;
  beside another's, it can deny it a borrowed name (both lose it, said),
  and with a setting keep both from running -- but no longer take it.
- `control` and `agents` mean "a program running as you", which an
  extension allowed to run programs, or to reach Fenix's control socket,
  can be. Only an extension id is an extension.
- A package installed from a file is trusted as its manifest says, a
  `fenix.*` one as the store's own: recording where a package came from
  is the plan's open question (§12).
