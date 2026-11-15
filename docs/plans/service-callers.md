# Plan: who calls a service, and whose name it is

- **Status:** built 15 November 2026, on `svc/security` (not merged);
  reviewed, and the review's findings built (§11), for the maintainer to
  review. Recorded in [ADR 0007](../adr/0007-service-callers.md). §10
  says what was built differently; **§11.1 states the name rules as they
  now stand**, replacing §4.2's 4-7.
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
    /// An origin this version doesn't name (added in review, §11):
    /// untrusted.
    other(string),
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

**Revised in review (§11):** rules 4, 5 and 7 below let any installed
package -- a theme, one switched off -- void another's names, a longer
id take a shorter one's, and a setting in dispute fall back to its
default. §11.1 has the rules as built.

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

As built after the review (§11). Before it there were two more:
denying a **setting** made it fall back to its default --
`jira.seam_writes = "never"` read as `ask` once any `*.jira` declared the
key (M1); and any installed package, data-only or switched off, voided a
namespace by being that word's publisher, or a longer id took names
under a shorter one (M2).

- **Anything its permissions allow**, as before: a user-installed
  extension with network, secrets, processes or the project's files acts
  as you within them. Nothing here narrows a permission.
- **Call any service that doesn't restrict its callers** -- every
  service today, until its extension adds the section or checks
  `called-by`.
- **Pose as a program on this machine** by running one
  (`permissions.process`: an interpreter, `fenix --mcp`), whose calls
  arrive as `control` or `agent-tool`. Through `permissions.ipc` it no
  longer can: Fenix's control pipe and socket, and links to them, are
  refused. So `control` and `agents` are "a program of yours", never
  proof that no extension is behind it.
- **Set off `fenix`**: an extension opening a binary file has the hex
  view asked; a link in a note it wrote, `notebook.follow`; a Git command,
  a forge. `fenix` is Fenix's feature, not proof that you acted.
- **Use a deputy**: a gate listing `fenix.agenda` admits whatever can make
  the agenda act -- its open services (`agenda.sync_quietly` sends the
  queued writes; `agenda.add_issue`, `agenda.clock_in`,
  `agenda.tracker_changed`), any of its commands through `run-command`,
  and the control channel's `fenix/agenda.clockIn`, `clockOut` and
  `fenix/runCommand`. The writes it then makes are ones you queued, so
  little today; the agenda should gate those services once it's on 1.6
  (§9).
- **Install as `fenix.<x>` from a file.** Nothing authenticates a
  sideloaded package's id (§4.1): it replaces the one of that id and
  passes every gate that lists it, borrows the word `fenix`, and wins
  ties as the store's own. §12 says what it would take.
- **Deny a name.** Claiming a name another lists equally, where neither
  is the machine's nor the store's, makes it no one's. That is said in a
  notification and on the Extensions page, naming both, but it is a
  denial. For a setting, it keeps both extensions from running.
- **Take a borrowed name no running extension claims**: `jira.request`
  while nothing else switched on lists it -- except Fenix's own asks
  (`FENIX_ASKS`), held for the store extension they're for, and anything
  under the word `fenix`.
- **Answer a resource URI under its own toolset** that no other has:
  `fenix://ext/<its toolset>/...`.
- **Rely on "the machine's"**: rule 6's tie-break trusts whoever can
  write the managed folder (`FENIX_MANAGED_EXTENSIONS`,
  `$FENIX_HOME/managed`, `%ProgramData%\Fenix\extensions`,
  `/etc/fenix/extensions`). On Windows, a `%ProgramData%` folder that a
  user created is that user's. The installer's ACL on it isn't checked
  here.
- **Open files by pattern** (`contributes.opens`, `*.pdf`): the first
  extension that opens a pattern still gets the file. Only `pdf.open`, at
  a page, is held for `fenix.pdf`.

## 7. Left for later

- **Listing who provides a service** (`services::providers(pattern)`): a
  new import, additive; the trackers' discovery works without it by
  asking known ids (`tracker-seam.md` §2, "Discovery").
- **An event when an extension is installed, updated or removed**: a new
  export, called only when it's there; separate from this.
- **A deadline on calls between extensions**: `Answer::Ext` calls have
  none; one belongs with a manifest limit and its own tests.
- **`contributes.opens` patterns as names**: two extensions opening
  `*.pdf` still go by order.
- **Where a package came from**: §12.

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

and the agenda, once it's on 1.6, gates the services through which
others can make it write (§6's deputies), so that `fenix.agenda` in a
tracker's gate means the agenda acting for you -- for example:

```toml
[contributes.service_callers]
"agenda.sync_quietly" = ["fenix"]
"agenda.add_issue" = ["fenix.notebook", "control"]
"agenda.clock_in" = ["control", "fenix.pomodoro"]
"agenda.clock_out" = ["control", "fenix.pomodoro"]
"agenda.tracker_changed" = ["fenix.jira", "fenix.github-issues", "fenix.gitlab-issues", "fenix.linear"]
```

(who calls each today is the store's to check; these lists are a
starting point, not checked against every caller).

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

Three commits after the plan's, as §8 has them: the names, who called,
the gate; then the docs.

- **The names** -- `fenix-extensions/src/manifest.rs` (`namespace_problem`,
  `word_is_theirs`, `borrows`, `Claim`) and `src/claims.rs` (`refused`,
  `withdraw`, `unserved`); `fenix-gui/src/installed.rs` (`claimed`,
  `fenix_owns`, used by `code_of`, so by everything that lists the code
  extensions), the Extensions page's "Names left out", and
  `WasmHost::code_unserved` for a caller. Commands aren't checked
  against Fenix's own: Fenix's are registered first and an extension's of
  the same id never is, as before.
- **Who called** -- as §2, with one more origin than §1.2 listed:
  `fenix/agent.call` (MCP and agent threads) and `fenix/tools.call` both
  end in `control_tools_call_with`, so both are `agent-tool`.
- **The gate** -- as §3, in `code_serve_with`
  (`WasmHost::code_gate_refuses`). A refused call is answered at once; its
  `ServeId` names a call never filed, so cancelling it is a no-op.

**Rule 6 never decides in practice.** A borrowed word has no dot and an
id has one, so a name claimed under one extension's full id and through
another's word is always under an installed id, and rule 4 refuses the
borrower first. The order stays, for manifests made some other way. Rule
4 takes the **longest** installed id a name is under (`acme.a.b.x` is
`acme.a.b`'s, not `acme.a`'s).

**The store**, checked: `FENIX_STORE_MANIFESTS=<fenix-extensions>/extensions
cargo test -p fenix-gui -- --ignored every_store_extension` -- "74
manifests read, 60 code extensions, 0 names refused", each manifest
unchanged by the claims. The test guests: `callers.wasm` is new, built on
the 1.6 crate with Rust 1.100 (not `build.ps1`'s 1.98, which isn't
installed here), its `Cargo.lock` copied from `plain16`'s so its
dependencies are the same; `plain16.wasm` was rebuilt the same way on the
crate with `called-by` in it -- it differs from the old one, since the
crate's WIT changed -- and `tests/f15.rs` still starts it on Fenix 1.5's
bindings, with no `called-by` import.

**Not verified by anything here:** the store's trackers adopting §9 (that
repository is read-only for this work); Windows; a real `fenix --mcp`
client (the tests call `fenix/agent.call` and `fenix/tools.call` inside
the app); and the forges, PDF and reading-view routes as `fenix` other
than through the hex view and a `vnc:` link, which share the code.

## 11. Review: what changed

An independent review found that the caller identity can't be forged and
that the gate sits on the only route to `serve`. It found two things
that had to be fixed before merge, and more of the same class. Each fix
below is its own commit, with a test that fails before it.

| Finding | What was built | Test |
|---|---|---|
| **M1**: a setting in dispute fell back to its default (`jira.seam_writes = "never"` read as `ask`) | An extension that can't have every setting it declares isn't run ("held back", `installed::claimed_all`), and the names are worked out again without it. The machine's, else the store's own, keeps the setting with your value. A critical notification, which stays, names both and the setting. The API can't tell "withheld" from "unset" (a declared setting always reads as a value or its default), so not running is the safe answer | `names_tests::a_setting_in_dispute_never_falls_back_to_its_default`, `fenixs_own_setting_is_never_an_extensions` |
| **M2**: any installed package voided names; a longer id took a shorter one's | Only switched-on code extensions claim names. Rule 5 (a publisher voids a namespace) is gone. Between full ids, the shortest that lists the name has it. `code_extensions_now` puts a change in the claims in use at once and registers a setting given back | `claims::tests::a_longer_id_never_takes_a_name_and_a_publisher_voids_no_namespace`, `a_word_borrowed_never_reaches_under_another_full_id`, `names_tests::a_change_in_the_claims_is_put_in_use_at_once` |
| `service-caller` had no open case | `other(string)`, untrusted and never let through a gate. `ServiceCaller::is_extension` and `called_by_extension`. The docs' example denies by default | `host.rs::a_service_is_told_who_called_it_call_by_call`, `callers_tests::the_gate_refuses_an_unlisted_caller_before_the_extension_runs` |
| S1: resource templates took others' reads | A template is under `fenix://ext/<its toolset>/`, or it's left out; the toolset is a name | `claims::tests::a_resource_is_under_its_own_toolset_and_fenixs_toolsets_are_fenixs` |
| S2: a forge's host went to the last by id, along with the repository | `forge_for`: the strongest claim, then the machine's, then the store's own, else none, and that is said | `names_tests::a_forges_host_two_claim_goes_as_a_name_does` |
| S3: Fenix's own asks went to whoever listed the name | `installed::FENIX_ASKS`, one table, held in `code_server`. `AGENTS_ASK` and `AGENTS_CANCEL` are one list, which `answer_code_request` matches | `names_tests::fenixs_own_asks_go_only_to_the_extension_meant` |
| S5: `acme.git` was in Fenix's `git` toolset | Fenix's toolsets are Fenix's (`NameKind::Toolset`) | the S1 test |
| S6: another's settings were read by the word; Fenix's commands weren't said | Only Fenix's own settings are read by the last part. A Fenix command an extension lists is said under "Names left out" | `names_tests::another_extensions_settings_arent_read_by_the_word_and_fenixs_commands_are_said` |
| The control endpoint through `ipc` | `may_ipc` refuses Fenix's pipe and socket, and links to them | `host.rs::fenixs_control_socket_is_never_an_extensions` |
| S7: denials were seen only on the Extensions page | A notification the first time: when an extension is installed or switched on, or another one is | `names_tests::a_setting_in_dispute_never_falls_back_to_its_default` |
| S8: `build.ps1` named 1.98 | One `$toolchain`, 1.100.0. `provider.wasm` was rebuilt with it; `counter.wasm` is kept as built (its `Cargo.lock` is 1.0's) | the fenix-wasm suite |
| S4: deputies | Documented (§6, `WRITING_EXTENSIONS.md`), with the agenda's services to gate (§9) | -- |

One suggestion was not built: deciding "held before" by a stored value
recorded for the extension. Fenix keeps settings by key, not by who
declared them, so there is nothing to read it from. The machine's and the
store's own are what decide.

### 11.1 The name rules as they stand

1. A namespace is one word; only a `fenix.*` id may use the word `fenix`
   (at parse).
2. Only switched-on code extensions claim names.
3. Fenix's own settings, services, commands, agent tools, toolsets and
   prompts are Fenix's.
4. A borrowed word (the id's last part, a namespace) never reaches under
   another claimant's full id.
5. A full id beats a borrowed word; between full ids, the shortest that
   lists the name has it.
6. Between equal borrowed claims: the machine's, else the store's own
   (`fenix.*`) when only one is, else no one.
7. Toolsets and resource templates (`fenix://ext/<toolset>/`) are names.
8. Fenix's own asks (`FENIX_ASKS`) go only to the store extension they're
   for.
9. An extension denied a setting it declares isn't run, and the rest are
   worked out without it.
10. A forge's host goes as rule 6. What's refused is said in a
    notification and on the Extensions page.

The store, checked again (`FENIX_STORE_MANIFESTS=<fenix-extensions>/extensions
cargo test -p fenix-gui --bin fenix -- --ignored every_store_extension`):
"74 manifests read, 60 code extensions, 0 names refused".

## 12. Open question for the owner: where a package came from

A sideloaded package with the id `fenix.agenda` replaces the store's and
passes every gate that lists `fenix.agenda`. One called `fenix.<x>`
borrows the word `fenix` and wins ties as the store's own. Fenix can't
tell the difference: it doesn't record where an installed package came
from.

What it would take:

- `extensions.toml` keeps an origin per extension: `origin = "store"`
  when installed by `Registry::install`, its hash checked against the
  signed index, or `"file"`. An older Fenix reads past the field.
- A `fenix.*` id from a file then counts as no publisher's: it isn't the
  store's own for ties, and it isn't allowed the word `fenix`.
- A gate listing an id could say whether it takes that id from a file.
- Packages installed before this would be checked against the index once,
  by their SHA-256, or counted as from a file.

Meanwhile, the install prompt for a file (`fenix --install-extension`, or
`SPC h x` on a `.fenixext`) should say, for a `fenix.*` id: "This package
calls itself fenix.agenda, but isn't from the store: it will replace the
store's and be trusted as it is. Install only a file you built, or one
the store's maintainers gave you." For an id already installed from the
store, it should also say that the file replaces that one.
