# Plan: who calls a service, and whose name it is

- **Status:** planned 15 November 2026, on `svc/security` (not merged).
  Recorded in [ADR 0007](../adr/0007-service-callers.md). §10 says what
  was built differently.
- **Scope:** a service learns who called it, from Fenix, for every call;
  a service can say who may call it, and Fenix refuses anyone else before
  the extension runs; and a name an extension serves, runs or keeps as a
  setting is one only it can have.
- **Lands as:** one Fenix pull request, in extension API 1.6 (not yet
  tagged or released): one new import, one new type, a new manifest
  section that needs `api = "1.6"`, and name rules that every extension
  in the store passes unchanged (§5).
- **Why:** two reviews of the store's extensions found that any installed
  extension -- one with no network permission -- can call the trackers'
  write services and Jira's `jira.request` and act as you, and that an
  impostor can take every call meant for another extension.

Paths are in this repository unless they say `fenix-extensions`. Claims
about what Fenix does today cite `master` at `40c5388`.

## 1. What happens today

### 1.1 How a call travels

An extension calls `call-service(name, payload)` (`wit/world.wit:1520`):
the host gives it an id and sends `Request::Service` to Fenix
(`fenix-wasm/src/host.rs:3137-3141`). Fenix knows which worker sent it --
each worker's events come in tagged with its id
(`fenix-gui/src/app/wasm_host.rs:2688`) -- so the asker can't be forged.
`answer_code_request` (`wasm_host.rs:3405-3425`) answers `agents.ask` and
`agents.cancel` itself, and otherwise sends the call to `code_server(name,
Some(asker))`: the first switched-on code extension, **other than the
asker**, whose `contributes.services` lists the name
(`wasm_host.rs:1902-1904`). The list is the machine's extensions, then
yours, each by id (`installed.rs:119-129`, `extensions.toml` being a
`BTreeMap`). No one listing it: `Err("nothing offers <name>")`.

`code_serve_with` (`wasm_host.rs:1922-1928`) files the call under the
server and Fenix's own number for it, with who gets the answer
(`Answer::Ext { asker, id }` or `Answer::Fenix(closure)`, `wasm_host.rs:76-81`),
and sends `Call::Serve { call, name, payload }` to the server's worker,
starting it if need be. The worker calls the export `serve(call, name,
payload)` (`host.rs:4129`), one call at a time per extension: a worker is
one thread with one `Store`. `serve` answers now, or `none` and later
`service-answer(call, ...)`. Nothing passes the asker on: `serve` is told
only the call's number, the name and the payload.

A service that calls another (`A -> B -> C`) does so with its own
`call-service`, which returns at once; B's `serve` has returned (or is
waiting for an event) before C's runs. No host import calls back into
the guest, and the component model forbids re-entering an instance, so
there is never a `serve` inside a `serve`.

### 1.2 Every origin of a call

Every `Call::Serve` is made in `code_serve_with`; its callers
(`grep code_serve`) are:

| Origin | Where | Services |
|---|---|---|
| Another extension | `wasm_host.rs:3419-3424` | any |
| Fenix: a forge for the Git pages | `wasm_host.rs:1757`, `1865` | `forge.branch`, `forge.open`, `forge.reviews` |
| Fenix: a PDF opened at a page | `wasm_host.rs:1866` (`open_with_extension_at`) | `pdf.open` |
| Fenix: a binary file in the hex view | `app.rs:15275` | `ccsds.hex` |
| Fenix: a `vnc:` link | `app.rs:16382` | `vnc.connect` |
| Fenix: the reading view | `reading_host.rs:288`, `342`, `362` | `diagrams.render`, `notebook.link`, `notebook.follow` |
| An agent tool: `fenix --mcp` and an agent thread (`fenix/agent.call`, `agent_calls.rs:563`), or `fenix/tools.call` | `control_agent.rs:442` | a tool's `service` |
| An agent reading a resource | `agent_context.rs:232` | a resource's `service` |
| An agent's client getting a prompt | `agent_context.rs:291` | a prompt's `service` |
| The control channel | `control_host.rs:98`, `106` | `agenda.timer` (`fenix/state`), `agenda.tasks`, `agenda.clock_in`, `agenda.clock_out`, `notebook.capture` |

The control channel is reachable only by programs running as you
(`docs/CONTROL.md`, "Windows", "Linux"): `fenix --mcp` is one, and so is
any program an extension may run, or reach through `permissions.ipc`
(§6).

### 1.3 How names are checked

At `Manifest::parse` (`fenix-extensions/src/manifest.rs:1354-1488`), a
command's id, a setting's key and a service's name must start with
`ours`: the last part of the id plus a dot (`jira.`), the full id plus a
dot (`fenix.jira.`), or **any** namespace it lists plus a dot
(`manifest.rs:1368-1369`). Agents' ids are the last part or `ours`;
test providers' the same without namespaces (`manifest.rs:1490-1496`).
Namespaces themselves are not checked at all: `namespaces = ["fenix"]`,
`["fenix.jira"]` and `["*"]` all parse. Agent tools are named
`<toolset>_<name>`, the toolset being the id's last part with `-` as `_`
(`toolset_of`, `manifest.rs:1641-1643`); prompts by their bare name;
resources by a URI template. Pages are scoped by their extension
(`local_leader.rs:27`) and share no names.

### 1.4 Duplicates

Nothing compares one extension's names with another's:

- **Services** go to the first in the list (§1.1).
- **Commands**: Fenix's own first; then each extension's, a second one
  with the same id skipped silently (`commands.rs:478-496`).
- **Settings**: the first registered wins on the settings page
  (`fenix-config/src/schema.rs:547-554`), but every extension that
  declares a key is given its value (`wasm_host.rs:966-972`) and may
  write it (`wasm_host.rs:3988`).
- **Agent tools** with the same name (two ids with the same last part):
  the first; **prompts**: the first, ahead of Fenix's own of that name
  (`agent_context.rs:282`); **resources**: the first template that
  matches.

### 1.5 What the store relies on

Over the 74 manifests in `fenix-extensions/extensions/*/extension.toml`
(a script, §5.1): every id is `fenix.*`, and there is no extension built
elsewhere yet (`external.toml` has none). Services, commands and settings
are under the id's last part (`jira.request`, `agenda.tasks`), except the
trackers' seam, under the full id (`fenix.jira.tracker.about`, by
design: `fenix-extensions/plans/tracker-seam.md` §2), and six
namespaces: `diagram` (Diagrams), `insert` (Notebook), `reader` (PDF),
`git` (Review), `code` and `completion` (Tcl). No two extensions list the
same service, command, setting or prompt. Cross-extension calls are made
by the agenda, CCSDS, Diagrams, the four trackers, LaTeX, MIB, Notebook,
Pets, Pomodoro and Typst.

The trackers' need (`tracker-seam.md` §2, "What Fenix must add"): the
caller's id in `serve`, so a tracker takes writes from `fenix.agenda` and
no one else; reserved names; no silent second claimant; and, later,
discovery and deadlines. Until then each tracker asks you once a session
before the first write through the seam (`fenix_tracker::gate`,
`tracker-seam.md` §3.6), and Jira's `jira.request` -- any REST request,
as you -- stays open to everyone until Jira 2.0 (§6 there).

### 1.6 Corrections to the brief

- **"The first installed extension, in id order."** The machine's
  (managed) extensions first, then yours, each by id; and never the
  caller itself, so an extension asking for a name it lists itself is
  sent to another that lists it.
- **One more hole, of the same kind.** An extension may declare one of
  **Fenix's own** settings as its own -- `gitlab.token`, with id
  `acme.gitlab` or `namespaces = ["gitlab"]` -- and is then given its
  value, the token, through `setting` (`code_settings_now` reads a
  contributed key through Fenix's schema, `wasm_host.rs:968-970`),
  without `permissions.secrets`. Fenix's own settings are made Fenix's
  alone (§4.2); a test shows the token no longer reaches it.
- **A new field in a service entry** isn't possible at all: `services`
  is a list of strings, in 1.5 as now (`v1.5.2`'s `manifest.rs:523`).
  1.5's `Contributes` has no `deny_unknown_fields` (`v1.5.2`
  `manifest.rs:514-515`), so a new section is read past (§5.3).

## 2. Who called: `called-by`

### 2.1 The API

In WIT, a new type in `types` and a new import in `host` -- additive, as
1.6's test-provider types and `test-provider-set` were:

```wit
/// Who called one of its services: set by Fenix for each call. 1.6.
variant service-caller {
    /// Another extension, by its id.
    extension(string),
    /// Fenix itself, for a feature of its own: a forge for the Git pages
    /// (`forge.*`), a PDF opened at a page, the hex view, the reading
    /// view's diagrams and links, a `vnc:` link.
    fenix,
    /// A coding agent calling one of its agent tools: through
    /// `fenix --mcp`, an agent thread, or `fenix/tools.call`.
    agent-tool,
    /// A coding agent reading one of its resources.
    agent-resource,
    /// A coding agent's client getting one of its prompts.
    agent-prompt,
    /// A program on this machine, through the control channel
    /// (`fenix/agenda.tasks`, `fenix/state`).
    control,
}

/// Who called the service it's answering, while `serve` runs; none
/// outside `serve`. 1.6.
called-by: func() -> option<service-caller>;
```

In Rust, `fenix_extension_api::called_by() -> Option<ServiceCaller>`,
and `ServiceCaller` re-exported. A free function the extension calls,
not an argument the crate's `serve` glue reads: a guest built on the 1.6
crate that never calls it doesn't import it, and keeps loading on Fenix
1.5 (`tests/f15.rs`).

The function is `called-by`, not `service-caller`: a WIT interface's
types and functions share one namespace.

### 2.2 In the host

`Call::Serve` gains `caller: ServiceCaller`. The worker sets it in its
`State` just before calling `serve` and clears it just after, whatever
`serve` returned or if it trapped -- as `restoring` is set around
`page-restore` (`host.rs:4113-4116`). `called-by` reads it. So:

- **Unforgeable:** set by the host from what Fenix routed, never from
  anything the extension says.
- **Per call:** a worker runs one export at a time; two calls from two
  callers are two `serve`s, each with its own. A service answering later
  reads it in `serve` and keeps it with the call's number.
- **Nested:** in `A -> B -> C`, C is called by B's `call-service`, so C
  sees `extension("B")`; B saw A. There is no re-entry (§1.1).
- **Outside `serve`:** none -- in `activate`, `handle`, a command, a
  page, and in the work of an `async` job after its first `await`.

### 2.3 In Fenix

`code_serve_with` takes the caller as an argument, with no default, so
every call site in §1.2 says which it is; for `Answer::Ext { asker }`
it's `extension(asker)`. What each origin gives:

| Origin | `called-by` |
|---|---|
| Another extension | `extension("<its id>")` |
| Fenix's forges, PDF, hex view, `vnc:`, reading view | `fenix` |
| An agent tool (MCP, an agent thread, `fenix/tools.call`) | `agent-tool` |
| A resource | `agent-resource` |
| A prompt | `agent-prompt` |
| The control channel's methods | `control` |
| Outside `serve` | none |

Agents' client names (`fenix-mcp`, an MCP client's own) are what the
client says, not known; they're not passed.

## 3. Who may call: `[contributes.service_callers]`

```toml
[contributes.service_callers]
"fenix.jira.tracker.transition" = ["fenix.agenda"]
"jira.request" = ["fenix.agenda"]
"notebook.capture" = ["control", "fenix.agenda"]
```

- A key is one of its own `services`; its value is who may call it:
  extension ids, and `fenix`, `agents` (tools, resources and prompts)
  and `control`. None of those three can be an id, which has a dot. At
  most 32 each, none twice, none empty.
- **Fenix holds to it before the extension runs**: in
  `code_serve_with`, before the call is filed or the extension started.
  A caller not listed gets `Err("<name>: <callee> takes it only from
  <list>, not from <caller>")`; the extension isn't started, isn't
  called, and isn't told.
- **A service it doesn't name is open to anyone**, as every service is
  today; otherwise every released extension would stop working.
- **It needs `engine.api = "1.6"`.** Fenix 1.5 reads past the section
  (§5.3) -- so an extension relying on it under `api = "1.5"` would run
  there with every service open. Asking for 1.6 makes 1.5 refuse to
  install it instead (`api_fits`).
- **An agent tool, resource or prompt whose service doesn't admit
  `agents` is refused** at parse: it could never answer.

Both: the section is the common case, held even if the extension's code
forgets to check; `called-by` is for finer rules -- reads from anyone,
writes from the agenda; or "ask once for anyone but the agenda".

## 4. Whose name it is

### 4.1 What authenticates a publisher

- **Installed from the store:** the index is signed with a key built into
  Fenix, and carries each package's SHA-256 (`registry.rs:1-6`,
  `127-140`); the store's repository lets a release under a publisher
  only from that publisher's owners (`fenix-extensions/publishers.toml`),
  and `fenix` is the repository's own. So a package from the store whose
  id is `fenix.*` was built by the store.
- **Installed from a file** (`fenix --install-extension`, `SPC h x` on a
  `.fenixext`): nothing. Its id is what its manifest says. Fenix doesn't
  record where an installed package came from.
- **What Fenix can know, then:** the ids installed now (one package per
  id: installing another `fenix.jira` replaces it), the machine's from
  yours, and the manifests. The rules below rest on that, and on the one
  word no one but the store may publish under, `fenix`.

### 4.2 The rules

When a manifest is read (`Manifest::parse`), so a hostile manifest is
refused whatever else is installed:

1. **A namespace is one word**: lowercase letters, digits, `-`, `_` --
   no dot, so never an id or a prefix longer than a publisher.
2. **`fenix` is the store's.** No extension whose id isn't `fenix.*` may
   list the namespace `fenix`, or name anything starting `fenix.` by its
   last part (`acme.fenix`'s `fenix.x`) -- only under its own full id
   (`acme.fenix.x`).

Among the switched-on code extensions, each time the list is made
(`fenix_extensions::claims`, used by `installed::code_extensions`), for
services, commands, settings, agent tools and prompts:

3. **Fenix's own are Fenix's**: its settings (`gitlab.token`,
   `jira.token`), its services (`agents.ask`, `agents.cancel`), its
   agent tools and prompts. A claim to one is refused.
4. **A name under an installed extension's full id plus a dot is that
   extension's alone** -- switched on or not. `fenix.jira.tracker.about`
   is `fenix.jira`'s whenever `fenix.jira` is installed.
5. **A namespace that is the publisher of another installed extension**
   (`namespaces = ["acme"]` beside `acme.dusk`) gives no claim.
6. **A full id is a stronger claim than a borrowed word.** A name claimed
   by one extension under its full id and by another through its last
   part or a namespace is the first's. Neither an id's last part nor a
   namespace is anyone's: `acme.jira` has as good a claim to `jira.` as
   `fenix.jira`.
7. **The same name, claimed equally by two**: the machine's over yours
   (an administrator put it there); else **neither has it**. Fenix keeps
   no record of which was installed first (`extensions.toml` has no
   order, and every extension installed before this release would have
   none), and any order it could use -- by id, by a file's time -- is one
   an impostor can choose. Refusing both can't be won silently.

A name refused is taken out of that extension's manifest as Fenix uses
it: a refused service isn't routed to it, a command isn't registered, a
setting isn't on the settings page or given to it, and an agent tool,
resource or prompt calling a refused service goes too. Each is said on
the Extensions page, in the extension's details, under **Names left
out**: "service jira.request: fenix.jira and evil.jira both claim it --
switch one off". A caller of a name refused to everyone is told so:
`Err("jira.request: fenix.jira and evil.jira both claim it, so neither
answers it -- switch one off (SPC h x)")`; of a name no one lists,
`Err("nothing offers <name>")` as today.

The probe cases: `acme.fenix` serving `fenix.jira.tracker.about` --
refused at parse (2); `namespaces = ["fenix"]` -- refused at parse (2);
`namespaces = ["fenix.jira"]` -- refused at parse (1); a namespace that
is another installed extension's publisher -- no claim through it (5);
a name under another installed id -- refused (4); two listing one name
-- the full id's (6), the machine's (7), or neither (7).

## 5. Compatibility

### 5.1 The store

A script over the 74 manifests (§1.5) finds no namespace with a dot, no
`fenix` namespace, every id `fenix.*` (so rule 2 never applies), no
namespace equal to a publisher, and no name listed twice -- every store
extension keeps every name, installed together in any combination. A
test reads each store manifest through `Manifest::parse` and the claims
when `FENIX_STORE_MANIFESTS` names the folder (it's not in this
repository, so CI skips it); §10 has its result.

### 5.2 Older extensions

An extension built for 1.0 to 1.5 imports nothing new and is called as
before; `called-by` is there for it but it never calls it. A guest's
`serve` export is unchanged. The test guests `hello.wasm` (1.2's WIT)
and `keys.wasm` (1.5's crate) keep loading and serving.

### 5.3 An older Fenix

1.5 reads past `[contributes.service_callers]`: `Contributes` has never
had `deny_unknown_fields`. But an extension that has the section, or
calls `called-by`, says `api = "1.6"`, and 1.5 refuses to install it --
the registry keeps 1.5 users on the release before. A 1.6-crate guest
that uses neither keeps `api = "1.5"`.

## 6. What an attacker extension can still do

- **Anything its permissions allow**, as before: a user-installed
  extension with network, secrets, processes or the project's files acts
  as you within them. Nothing here narrows a permission.
- **Call any service that doesn't restrict its callers** -- every
  service today, until its extension adds the section or checks
  `called-by`.
- **Pose as a program on this machine.** An extension allowed to run a
  program (`permissions.process`) -- or to reach Fenix's own control
  socket or pipe through `permissions.ipc` (`fenix/control*`,
  `fenix-*`, `*`) -- can use the control channel, and its calls arrive
  as `control` or `agent-tool`. So `control` and `agents` are "a program
  of yours", never proof that no extension is behind it; a service that
  only Fenix's agenda should call admits `fenix.agenda` and nothing else.
- **Install as `fenix.<x>` from a file.** Nothing authenticates a
  sideloaded package's id (§4.1); a file you install yourself can claim
  any id that isn't installed, and replace one that is.
- **Deny a name.** An extension claiming a name another lists equally
  (two last parts, or namespaces) makes it no one's (rule 7): visible,
  and naming both, but a denial all the same.
- **Take a borrowed name nobody installed claims**: `jira.request` while
  no other extension lists it, `acme.future.*` while `acme.future` isn't
  installed (only `fenix.*` is held back for everyone).
- **Shadow a resource template**: two templates that match one URI
  aren't compared (§7).

## 7. Left for later

- **Listing who provides a service** (`services::providers(pattern)`): a
  new import, additive; the trackers' discovery works without it by
  asking known ids (`tracker-seam.md` §2, "Discovery").
- **An event when an extension is installed, updated or removed**: a new
  export, called only when it's there; separate from this.
- **A deadline on calls between extensions**: `Answer::Ext` calls have
  none (`wasm_host.rs:1922`); one belongs with a manifest limit and its
  own tests.
- **Resource templates that overlap**: comparing two URI templates for a
  common match; until then the first wins as today.
- **Refusing an `ipc` permission that reaches Fenix's own control
  endpoint** (§6): a check in `may_ipc`, worth doing on its own.

## 8. The steps

1. **Names** -- `manifest.rs`: rules 1 and 2; `claims.rs`: rules 3-7 and
   the notes; `installed.rs`: the claims applied where the code
   extensions are listed, and the notes on the Extensions page;
   `wasm_host.rs`: the error for a refused name.
2. **Who called** -- WIT, the crate's `called_by`, `Call::Serve`'s
   caller, `code_serve_with`'s, every call site.
3. **The gate** -- `[contributes.service_callers]` read and checked; held
   in `code_serve_with`.
4. **Docs** -- `WRITING_EXTENSIONS.md`, `FEATURES.md`, the changelog,
   §10.

Tests: a new guest, `callers.wasm` on the 1.6 crate, that answers who
called it, calls on, and counts what ran; in `fenix-wasm`, the caller as
Fenix routes it for each origin, two calls queued from two callers, none
outside `serve`; in `fenix-gui`, the real routes -- extension to
extension, nested, agent tool, resource, prompt, control channel, Fenix
-- the gate refusing before the extension runs, and the name rules on the
app's list and the Extensions page.

## 9. What the store's trackers do then

Each tracker, at its next release (with `api = "1.6"`):

```toml
[contributes.service_callers]
"fenix.jira.tracker.transition" = ["fenix.agenda"]
"fenix.jira.tracker.update" = ["fenix.agenda"]
"fenix.jira.tracker.comment" = ["fenix.agenda"]
"fenix.jira.tracker.worklog" = ["fenix.agenda"]
"fenix.jira.tracker.create" = ["fenix.agenda"]
"fenix.jira.tracker.remember" = ["fenix.agenda"]
"jira.request" = ["fenix.agenda"]
"jira.set_blocked" = ["fenix.agenda"]
"jira.new_issue" = ["fenix.agenda"]
```

and drops the once-a-session question for those (keeping
`<name>.seam_writes = "never"` as your off switch). Reads stay open, or
are held to `["fenix.agenda", "agents"]`. Where one wants another
extension to write too, it checks in code:

```rust
use fenix_extension_api::{called_by, ServiceCaller};
let from_agenda = matches!(called_by(), Some(ServiceCaller::Extension(id)) if id == "fenix.agenda");
```

and asks you for anyone else, naming them. **`jira.request`** is held to
`fenix.agenda` (the agenda 1.2 is its only caller) until it goes at 2.0.

## 10. As built

(Filled in when built.)
