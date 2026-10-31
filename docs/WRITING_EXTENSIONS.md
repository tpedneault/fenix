# Writing an extension

An extension that runs code is a Rust crate built for WebAssembly
(`wasm32-wasip2`), written with `fenix-extension-api`, plus an
`extension.toml` that says what it is, when it loads, what it may do
and what it adds. Fenix runs it on a thread of its own, in a sandbox.
An extension of data only (themes, snippets, templates, keys, language
files) needs no code: see [Extensions you install](CONFIGURATION.md#extensions-you-install).

## Starting one

`SPC p c` (a new project) has a **Fenix extension · Rust** template: a
command, a page, and the tasks to build, pack and install it. It needs
Rust's WebAssembly target, once:

```text
rustup target add wasm32-wasip2
```

Its tasks (`SPC p t`), or the same from a terminal:

```text
cargo build --release --target wasm32-wasip2
pwsh -File package.ps1             # dist/<id>/ and dist/<id>-<version>.fenixext
pwsh -File package.ps1 -Install    # and installs it into Fenix
```

An installed extension is used from Fenix's next start. `SPC h l` shows
what each one logs, and why one stopped.

## The extension

```rust
use fenix_extension_api::{export, info, page, Extension, Key, Page};

#[derive(Default)]
struct Counter {
    count: u32,
}

impl Extension for Counter {
    fn run_command(&mut self, id: &str) -> Result<(), String> {
        match id {
            "counter.open" => {
                page::open("main", "Counter");
                Ok(())
            }
            _ => Err(format!("no command {id}")),
        }
    }

    fn page(&mut self, _page: u64, _width: u32, _height: u32) -> Page {
        Page::new().line([page::title("Counter")]).rule().line([page::text(self.count.to_string())]).focus(2).key("j", "count")
    }

    fn page_key(&mut self, page: u64, key: Key) {
        if key == Key::Char('j') {
            self.count += 1;
            info!("counted to {}", self.count);
            page::refresh(page);
        }
    }
}

export!(Counter);
```

It implements only what it needs of `Extension`; the rest have defaults.
One instance lives for as long as Fenix runs it, so its state is its
own fields.

| Method | When Fenix calls it |
|---|---|
| `activate` | Once, when it loads. An error stops it, and says why. |
| `run_command` | One of its commands runs (`[[contributes.commands]]`). |
| `handle` | Something it asked for has come: a timer, a picker's choice, a prompt's text, a process's output or end, an HTTP response, a service's reply; one of its settings changed; or one of its pages came into view or left every pane (`Event::PageShown`), so a hidden page can stop refreshing; a task or a terminal of its own ended (`Event::TaskFinished`, `Event::TerminalClosed`); you moved to another file (`Event::FocusChanged`, its path under `/project`). |
| `page` | Fenix needs one of its pages laid out for its pane: `width` cells wide, `height` lines tall, asked again when either changes. The last page stays on screen while it works on the next. Besides its lines, a canvas page may have backgrounds (`.background(cells, Shade)`: the panel, focus, selection, accent, good, warn, bad, or a colour), underlines, vertical rules (by cell, a wide character two), more focused rows (`.also_focus(cells)`), colours of its own for some cells' text (`.color(cells, (r, g, b))`, over its spans'), and take more keys and the mouse (`.more_keys(true)`, `.mouse(true)`). |
| `page_key` | A key on one of its pages. `q` and `Esc` close the page instead, and Space is the leader, as anywhere. Fenix asks for the page again afterwards. |
| `page_state`, `page_restore` | Saving one of its pages with the session, and opening it again. Fenix asks for the state after each key, keeps it, and restores the page on the next start; a page it can't restore is closed. |
| `serve` | Another extension asks for one of its services (`contributes.services`), or Fenix asks the forge it is (`forge.*`). |
| `surface_input` | A key, the pointer or the wheel on one of its surfaces. |
| `page_closed` | You closed one of its pages or surfaces -- `q`, its buffer -- so what it kept for it can go. Not for one it closed itself. |
| `buffer_changed` | A buffer's text changed, once it's been still for 300 ms: one of its scratch buffers' (`Changed::Scratch(id)`), always; a file's (`Changed::File(path)`, a full path), when it asks (`events = ["changed"]`, with `editor = "read"`). |
| `scratch_closed` | You closed one of its scratch buffers. |
| `focus_moved` | Something else has the focus, when it asks (`events = ["focus"]`), whatever its other permissions: `Focus::File` (its full path with `editor = "read"`, else its path under `/project` when it's there, else none), `Focus::Page` (its kind, and its id when it's one of its own -- `<id>/<kind>` for another extension's, a name for Fenix's: `git`, `settings`, `home`), `Focus::Text` (a buffer that's no file: one of its scratch buffers' id, or none), or `Focus::None` (the explorer). Once as it starts, then as it changes. `Event::FocusChanged` still comes as before. |

| `page_key_press` | On a page that takes more keys: one `page_key` doesn't take -- PageUp, PageDown, Shift with a key that has a name, Ctrl, Alt or the Windows key with others (Ctrl+Z, Ctrl+Shift+Z, Ctrl+A, Ctrl+S) -- with its modifiers, Shift among them. Fenix keeps its own chords (Ctrl+Q, the font's size, Ctrl+O and Ctrl+I, Ctrl+Tab and Ctrl+PageUp/PageDown, Ctrl+\\, Alt+X) and the leader. |
| `page_mouse` | On a page that takes the mouse: a press (its button, and `clicks` 2 for a double click), a drag to another cell, the release, the wheel -- at a cell: its line, its column in characters as `Cells` count them, and the cell (a wide character two). The wheel then doesn't scroll the page. |

These are functions of their own, not `Event`s: what a later 1.x adds
for an extension to hear of is another of them, so an extension built
for 1.0, whose `handle` takes the `Event` it was built with, loads as it
did.

In a canvas a wide character (CJK, an emoji) takes two cells, as
`ui-kit` lays it out: its glyph is spaced to fill them. `Cells` are in
characters still; Fenix counts the cells.

## What it can do

Nothing blocks for long. A call that takes time returns an id at once,
and its answer comes to `handle` as an `Event` with that id.

| | |
|---|---|
| `info!`, `warn!`, `error!` | Log to its output page. |
| `notify(text)` | Say something in Fenix's message line. |
| `page::open`, `page::refresh`, `page::close` | Its pages, built with `Page::new().line([...]).rule().focus(n).key(k, what)`, and runs coloured `title`, `text`, `muted`, `accent`, `good`, `warn`, `bad`. `.popup(line, rows)` draws a menu or a question beside a line; keys still come to the page. |
| `open_text(title, text, language)` | Text read-only in a buffer of its own, highlighted (`json`, `yaml`, `log`). |
| `open_url(url)` | An http(s) URL in the system's browser. |
| `copy(text)` | The clipboard. |
| `status(Some(text))` | Its item in the modeline while it runs; `status(None)` takes it away. |
| `pick(title, items)` | Fenix's picker → `Event::Picked`. |
| `prompt(label, initial)` | A line to type in the modeline → `Event::Prompted`. |
| `host::run_command(id)`, `host::open_file(path, line)` | Run a Fenix command; open a file: under its folders, or a full path Fenix gave it. With `editor = "read"` any other full path opens too, sealed: as text, for you, and `buffer::text` and the rest don't reach it. |
| `setting(key)` | One of its own settings, as text. |
| `host::secret(key)` | A secret its permissions name. |
| `Command::new("git").args([...]).spawn()` | A program → `Event::ProcessOutput`, `Event::ProcessExit`. |
| `.run_in_terminal(title)`, `.run_below(title)` | It in a terminal: in the focused pane, or a new one under it. Its id is for `terminal::stop` (the pane and what it printed stay) and `terminal::restart`, with another command or title → `Event::TerminalClosed` when you close it. |
| `.run_as_task(name)` | It as a task, in Fenix's task output pane, its errors in the quickfix list → `Event::TaskFinished`. One task runs at a time. |
| `HttpRequest::get(url).send()` | HTTP through Fenix's own client → `Event::HttpResponse`. |
| `host::set_timer(ms, repeat)` | `Event::Timer`. |
| `find_program(name, hints)` | Where a program its manifest names is: beside Fenix itself (a program Fenix ships, `fenix-pdfium`), its own tools folder, Fenix's, `PATH`, or one of `hints` (`%LOCALAPPDATA%/Programs/...`, `/tools/x/bin`). A hint `./gradlew` is the project's own, looked at before `PATH`, and on Windows `name.cmd` and `name.bat` are found too (`gradlew.bat`, `mvnw.cmd`). The path it gives may run. |
| `new_project(args)` | The new-project wizard, as `:project-new args` opens it. |
| `project::language_server(languages, Some(&command))` | The language server for those languages (`c`, `cpp`) in its project, in place of Fenix's own; servers running there start again. `None` gives them back; `project::restart_language_server()` restarts them. |
| `project::doctor(&checks)` | Its checks on the project doctor's page: `Check::new(Section::Hardware, "board", Health::Warn, "none chosen").fix("pick a board", "<its command>")`. |
| `project::tasks(&tasks)` | Tasks it offers in the task picker (`SPC t t`), each run with its command's `env` and `cwd`. An env file (`.env`) is its own to read into `env`. |
| `call_service(name, payload)` | Another extension's or Fenix's service → `Event::ServiceReply`. |
| `serve(call, name, payload)` | Its own services: `Some(answer)` now, or `None` and `answer(call, result)` later, once what it asked for has come (an HTTP response). The control channel's methods (`fenix/agenda.tasks`) are answered this way by the extension that serves them. |
| `Page::canvas(grid.finish())` | A page laid out cell by cell, as Fenix lays out its own: `grid::Grid` puts coloured text at a line and column, hairlines, `heading`, a focused row, panels, a popup, pictures and a key strip, and `grid` has `frame`, `fit`, `wrap`, a `/` `Filter` and a field's caret (`edit_line`, `with_caret`). `.typing(true)` while a field has the keyboard (Space, `q`, Esc and pastes, through `page_paste`, are its); `.space(true)` when Space ticks the focused row. |
| `image::load(bytes)`, `image::file(path)` | A picture for its canvases (PNG, JPEG, GIF, WebP): its id and size in pixels, for `Grid::picture`. A file is under its folders, or a full path Fenix gave it. |
| `pick_many(title, items)` | A picker where Tab marks rows → `Event::PickedMany`. |
| `compose(title, verb, initial, allow_empty)` | A pane to write in → `Event::Composed`. |
| `here()` | With `editor = "read"`: the file you were in when you ran its command, the selection or the line, its project. |
| `home(Some(&section))` | Its section on Home; a row picked there → `Event::HomePicked`. |
| `set_setting(key, value)` | One of its own settings changed and saved. A `map` setting reads and writes as JSON, `[["key", "value"], ...]`. |
| `run::spawn(async { ... })` | Work that waits, written as `async`: `run::fetch(req).await`, `run::call(service, payload).await`, `run::sleep(ms).await`. Its `handle` gives each event to `run::deliver(event)` first, which keeps the answers its jobs wait for. |
| `lsp::Servers::handle` | Being a language server for Fenix's languages (`[[contributes.language_servers]]`): Fenix talks LSP to it through `lsp_message`, a project a server, and merges its hover, completion, definition and diagnostics with every other server's. `lsp::LanguageServer` has a method each; `lsp::publish_diagnostics` says a document's problems. |
| `buffer::text`, `buffer::edit`, `buffer::insert`, `buffer::save` | The buffers you edit (`editor = "read"` to read, `"write"` to change): one open on a file, saved or not; changes to it as one undo step; text at your cursor. A file under its folders, or one Fenix gave it, is opened to edit if it isn't; any other full path only while you have it open, and not one sealed (`open_file`). |
| `page::open_beside(kind, title, width)` | A page in a new pane beside the focused one: a preview, a sidebar. |
| `page::focus(page)` | One of its pages or surfaces brought to the pane you're in (and gone from any other pane showing it). |
| `copy_image(png)`, `pasted_image()` | Pictures through the clipboard. |
| `theme()` | The editor theme's colours, to draw in them; `Event::ThemeChanged` when they change. |
| `clock::now()` | The time where you are. Inside the sandbox there's no time zone, so chrono's `Local` is UTC there; `clock` asks Fenix for the offset. |
| `socket::connect(host, port)`, `socket::write`, `socket::close` | A TCP connection to where `permissions.sockets` lets it → `Event::SocketConnected`, `Event::SocketData` (in order), `Event::SocketClosed`. `socket::Inbox` gathers what came, a message at a time. |
| `socket::bind_udp(port)`, `socket::send_to` | A UDP socket: on a port of this machine's its manifest names (`:5000`), or 0 for any, to send from → `Event::Datagram`. |
| `surface::open(kind, title, &options)` | A surface: a pane of pixels it streams. See [Surfaces](#surfaces). |
| `Command::spawn_raw()` | A program whose stdout comes as bytes, `Event::ProcessBytes`: a companion that sends pictures back. |
| `read_file(path)` | A file's bytes: under its folders, or a full path Fenix gave it (`Event::Opened`, `here()`, `explorer_selected()`, a forge's repository). |
| `explorer_selected()` | What's selected in the explorer, with `editor = "read"`. |
| `cursor()` | With `here()` (whenever it's called: in `focus_moved`, `buffer_changed`, a timer): where the cursor was (line, column, from 0, in characters), the selection's range when there's one, and whether `here()`'s text is that selection or the cursor's line. Needs `editor = "read"`. |
| `scratch::open(title, text, language)`, `scratch::text`, `scratch::set`, `scratch::close` | A scratch buffer of its own, in the focused pane: text you edit, highlighted as `language`, never saved nor kept with the session. Its text read back, or replaced as one undo step (not told back); your edits come to `buffer_changed`, its closing to `scratch_closed`. No permission needed. |
| `folder_list(path)` | The files it may read in a folder, a full path: all of one Fenix gave it; in the folder of a file it was given (`Event::Opened`), those it opens itself too (`contributes.opens`) -- the next picture -- which `read_file` and `image::file` then take. Names and sizes, sorted. |
| `decorations::set(buffer, decorations)` | Its decorations of a buffer's text, in place of those it set before (none takes them away): `Decoration::before(at, text, role)` and `::after` -- text drawn before or after the character at `at`, taking room, staying with it as you type there; `::swatch(at, rgb)`, a block of a colour; `::hide(start, end, "••••")`, the text drawn as that except on the cursor's line; `::tint(start, end, shade)`; `::underline(start, end, role)`. Places are (line, column) from 0, in characters. They move with your edits until it sets them again. A file's (`decorations::file(path)`, the buffer open on it) needs `editor = "read"`; one of its scratch buffers' (`TextBuffer::Scratch(id)`), nothing. At most 10,000. |
| `buffer::undo(path)`, `buffer::redo(path)` | The buffer's last change undone, or done again -- yours or its own: a page for a file and the file's text are one buffer, one history. Needs `editor = "write"`. |
| `theme_palette()` | Eight colours of the theme's, distinct and readable on its background: for series in a chart, groups, matches. |
| `play_sound(path)` | A WAV of its own -- in its folder (`sounds/chime.wav`) or under `/data`, at most 8 MiB -- played once, by the system's player, while it carries on. Needs `sound = true`. |
| `pasted_text()` | The clipboard's text, with `clipboard = true`. |
| `project::debug_adapter(languages, Some(&command))` | The debug adapter for those languages in its project (`SPC u u`), in place of Fenix's own; `None` gives them back. The project's own (`.fenix/tools.json`'s `dap`) still wins. See [Language packs](#language-packs). |
| `project::formatter(languages, Some(&command))` | The formatter for those languages in its project: `SPC c F`, and saving with `editor.format_on_save`, run it with the buffer's text on its stdin and take its stdout as the text, when it ends well within 3 seconds; `${file}` in its args is the file's path. It goes before a language server's formatting. |
| `diagnostics::set(path, &problems)` | Its problems in a file, in place of those it gave before, shown with the language servers': `Problem::warning(start, end, message).source("ruff").code("F401")`, places (line, column) from 0 in characters. A file under `/project` or one of its folders, or with `editor = "read"` any full path. At most 10,000; they go when it stops. A linter needs no language server for this. |
| `process::write(id, bytes)`, `process::close_stdin(id)`, `process::kill(id)` | A program it started: bytes for its stdin; its stdin closed, so it reads to its end -- `jq`, a formatter. |
| `scratch::open_beside(title, text, language, width)`, `open_text_beside(...)` | A scratch buffer, or text, in a pane beside the focused one, `width` of it: the same pane each time while it's there. The focus stays. |
| `HttpRequest::get(url).download(path, sha256)` | Its body written to a file under `/tools` or `/data` -- up to 2 GiB -- rather than given back, checked against a SHA-256 when given → `Event::HttpResponse`, its body empty. |
| `tools::set_executable(path)` | A file under `/tools` made one that runs (Linux, macOS). |
| `HttpRequest::get(url).options(HttpOptions::new().timeout(5000).no_redirects().accept_invalid_certs().client_cert(cert, key).proxy("http://proxy:3128").streamed()).send()` | A request made as the options say. Its answer comes to `handle` as `Event::HttpResponse`, with its status, headers, body, final `url` and `timing` (microseconds: `dns`, `connect` and `tls` on https, `first_byte`, `total`); streamed, it comes to `Extension::http_event(id, part)` instead: `HttpPart::Response`, then `HttpPart::Chunk`s as they come and `HttpPart::End`. `http_cancel(id)` gives one up. See [Network](#network). |
| `platform()` | The machine: `os` (`windows`, `linux`, `macos`), `arch` (`x86_64`, `aarch64`), `env` (`gnu`, `musl`, `msvc`, or ""): which release to download. |
| `page::file(page, Some(path))` | The file its page shows now -- after `]` went to the next picture -- for `SPC f o` and the file's `buffer_changed`. |
| `decorations::rules(&[DecorationRule::new(&[".env", ".env.*"], "^\\w+=(.*)$", DecorationLook::Hide("••••".into())).group(1)])` | Decorations Fenix gives a file's buffer itself, from before it's first drawn and as it's edited: where the regular expression matches a line. With `editor = "read"`; at most 64 rules, files up to 4 MiB. |
| `Extension::file_saved(path)` | A file saved, with `contributes.events = ["saved"]`: its full path with `editor = "read"`, else its `/project/...` path. `Event::Saved` still comes, with the name. |
| `buffer::set_language(&buffer, Some("json"))` | A buffer highlighted as a language -- Fenix's, or one a grammar brings (`kotlin`) -- its folds and scopes with it; `None` goes back to what its name says. A file's needs `editor = "read"`; one of its scratch buffers', nothing. A language server still goes by the file's name. |
| `Decoration::swatch(at, rgb).tagged(5)` | A decoration a click on comes back to `Extension::decoration_clicked(buffer, id)`, set with the others by `decorations::set`: a swatch that opens a picker. |
| `Extension::view_changed(views)` | The lines each pane shows of a buffer, `TextView { buffer, pane, first, last }` (from 0, both shown), once they've been still for 100 ms, with `contributes.events = ["view"]`: its scratch buffers', and files' with `editor = "read"`. What to decorate in a long file, without `draw`. |
| `command.run_in_background(name)`, `task::cancel(id)`, `task::show(id)` | A task beside the task pane: several at once, at most 4 of its own, each one's output in a buffer of its own (`*name*`, which `task::show` brings up), its errors not in the quickfix list. `Event::TaskFinished` says how each went. `run_as_task` is still the one task in the task pane. |
| `project::add_language_server(name, languages, Some(&command))` | A language server called `name` in its project, beside the others for those languages -- Fenix's, the project's, other extensions' -- not in place of them: ruff beside basedpyright, ESLint and Tailwind beside typescript-language-server. Completions, problems and code actions are merged. Set again by name, it starts again; `None` takes it away. See [Language packs](#language-packs). |
| `process_id()` | Fenix's own process id -- `std::process::id()` has none to give in the sandbox: what Discord's `SET_ACTIVITY` takes. |
| `project::language_server_settings(name, Some(&json))` | The settings a language server is given when it asks (`workspace/configuration`): `name` is one it added (`project::add_language_server`) or a language, for that language's server; each section asked for (`""` the whole, `tailwindCSS.lint`) comes from the object, null where it has none. Over the project's own `.fenix/lsp/<name>.json`. A server running is told (`workspace/didChangeConfiguration`); `None` takes them away. |
| `Problem::warning(start, end, "unused").fix("Remove the import", edits)` | A problem with its fixes, given with `diagnostics::set`: `SPC c a` on its lines offers them beside the language servers' actions, and previews the one picked as theirs, while the file is as it was when they came. |
| `debug::launches(&[LaunchConfig::launch("app", &["cpp"]).program("/project/build/app").pre_launch_task("cmake: build app")])`, `debug::variable(name, &choices)` | Launches it works out as it runs, offered by `SPC u u` beside its launch files, and `${name}`s it fills in for both: one choice is its value, several are picked as the session starts. See [Language packs](#language-packs). |
| `Extension::task_started(name)` | A task started in the task pane, with `contributes.events = ["tasks"]`; `Event::TaskEnded` still says how it went. |
| `secret(key)`, `set_secret(key, Some(value))` | A secret its manifest asks for -- by key, or by pattern (`permissions.secrets = ["http.secret.*"]`) -- read, and kept: in the system's keychain where there's one (Windows' Credential Manager, the macOS Keychain, the Secret Service through `secret-tool` on Linux), else in `settings.toml` as plain text, which its output page says. A password asked for once. Fenix's own tokens (`gitlab.token`) can be read when it asks for them, never written. |
| `prompt_secret(label)` | `prompt`, what's typed shown as dots, kept in no history or log of Fenix's → `Event::Prompted`. |
| `socket::listen(port)` | A port on 127.0.0.1 listened on, one `permissions.listen` lets it take (`"127.0.0.1:0"` for any free one): its id and the port it got, for an OAuth redirect. See [Sockets](#sockets). |
| `Extension::socket_accepted(listener, socket, peer)` | A connection made to one of its ports: a socket of its own from then on, as one it connected. |
| `env_var(name)` | One of Fenix's environment variables `permissions.env` names (`HOME`, `JAVA_*`, `${setting:http.env}`, the names a setting holds): a `.http` file's `$processEnv`. `FENIX_*` (secrets' overrides) never. |
| `project::formatter_keeping_output(languages, Some(&command))` | `project::formatter`, and what it writes is kept when it fails but wrote something: ktlint. |
| `Extension::debug_changed(state)` | A debug session started (`DebugState::Started(DebugSession { id, name, language, parent })`), stopped where it was (`Paused((id, reason))`), ran on (`Resumed(id)`) or ended (`Ended(id)`), with `contributes.events = ["debug"]` -- an adapter's child sessions too, their `parent` the session they came from. |
| `agents::provide(id, Some(&command))` | A coding agent its manifest names (`contributes.agents`), started by `command`: an ACP server on stdio, often in its tools folder. See [Coding agents](#coding-agents). |
| `agents::ask(prompt, agent)` | A coding agent's answer to `prompt`, in a thread of its own, with `permissions.agents`: `agents::Answer { text, stop_reason, agent }`. |
| `Extension::agent_turn_ended(turn)` | A coding agent's turn ended (`AgentTurn { agent, thread, title, root, stop_reason, text }`), with `contributes.events = ["agents"]` and `permissions.agents`. |
| `forge::call`, `forge::branch_answer`, `forge::refresh` | Being a code-review forge for Fenix's Git pages. See [Forges](#forges). |
| `std::fs` | Its own folder, `/data`, kept between runs; its tools folder, `/tools`, when its manifest names a program there; and the project, `/project`, if its permissions say: the one you're in. Open a file in another project and Fenix starts the extension again there, its pages saved and opened again (`page_state`, `page_restore`), so keep in a page's state what it needs. |

## Drawing over the editor

With `draw = "over"` in its permissions, an extension draws in **layers**
over (or behind) the text: frames of its own sprite sheets, or its own 3D
models, placed by anchors, which Fenix animates. It decides what happens;
Fenix draws every frame.

```rust
use fenix_extension_api::{layer, Anchor, Ease, Item, LayerLevel, Place};

let sheet = layer::sheet("sprites/cat.png", (16, 16))?;   // a PNG in its folder
let pets = layer::open(LayerLevel::AboveText)?;
let floor = |x| Place::new(Anchor::Modeline, x, 0.0);     // the modeline's top edge
layer::set(pets, &[Item::new(1, sheet, 2, floor(4.0))
    .frames(&[2, 3], 8)                                   // walking, 8 a second
    .size(1.5)                                            // lines tall
    .glide(floor(30.0), 3000)                             // there in 3 s
    .ease(Ease::Smooth)                                   // starting and stopping gently
    .clickable()]);
```

A **model** is a binary glTF (`.glb`, its textures inside it, at most
4 MiB) with a skeleton and named clips. Fenix plays the clip an item
names, blends into the next when it changes, and turns it smoothly to
the yaw it's given; it's drawn toon-shaded and outlined, lit by the
theme's colours.

```rust
use fenix_extension_api::{layer, Item, Pose, Style};

let cat = layer::model("models/cat.glb")?;               // its clips, and how long each is
let pose = Pose::new(cat.id, "walk").yaw(90.0)           // facing right
    .style(Style::Pixel)                                  // chunky, to sit with pixel art
    .part("hat");                                         // wearing its node `opt:hat`
layer::set(pets, &[Item::model(1, pose, floor(4.0))
    .size(2.0)                                            // lines tall
    .redraws(12)                                          // posed 12 times a second
    .glide(floor(30.0), 3000)
    .ease(Ease::Smooth)]);
```

- **Levels**: `BelowText`, `AboveText` (under Fenix's popups), `AbovePopups`.
  The caret always draws over them.
- **Anchors**: the window, the modeline, a pane (by `Event::Layout`'s id),
  a cell of a pane's text (scrolls with it, hidden off screen), the caret.
  A place is in cells from the anchor, to the item's foot: a sheet's bottom
  left, the middle of where a model stands.
- **Items** are given whole on each `layer::set`. One given again keeps its
  clocks: its glide unless the glide (or `at`) changed, its frames unless
  they changed, and a model's clip unless it names another. Fenix scales a
  sheet by whole pixels, nearest-pixel, as near the item's `size` as that
  goes.
- **Glides** ease (`Ease::Linear`, `Smooth`, `Out`, `In`, `Spring`) and can
  bow up into an arc (`.arc(lines)`): a hop, a throw.
- **Models**: `Pose` names the clip (`""` stands it at rest), whether it
  plays `once` and holds its last frame, its `speed`, `yaw` (0 faces you,
  90 faces right), `style` (`Smooth`, or `Pixel`: drawn small and scaled up
  nearest-pixel), the optional parts it wears (nodes named `opt:<name>`),
  a `tint` and a `glow`. An item's `fps` is how often its pose is drawn
  again (0, every frame).
- **Events**: `Layout` (the window, the modeline's row, the panes and the
  caret, in cells) when it changes; `Arrived` when a glide ends;
  `LayerClicked` for a `clickable` item (other clicks go through). Nothing
  redraws for a layer unless something on it moves; while something glides
  it's drawn at the display's own rate. With animations off, glides jump to
  their end, frames hold, and models stand at their clip's start.

## Surfaces

A **surface** is a page whose content is pixels it streams: a remote
desktop, rendered PDF pages. It's one of its pages in all else: its tab
and title (`surface::retitle`), `page::close`, `page_state` and
`page_restore`, `Event::PageShown`.

```rust
use fenix_extension_api::{surface, SurfaceOptions, SurfaceScale};

let screen = surface::open("vm", "VNC: build-vm", &SurfaceOptions::new()
    .capture()                                  // every key, pressed and let go
    .scale(SurfaceScale::Stretch))?;            // fill the pane
// Event::SurfaceResized(size): the pane is size.width x size.height pixels.
surface::resize(screen, 1920, 1080)?;
surface::write(screen, x, y, w, h, &bgra)?;     // a rectangle, BGRA
surface::copy(screen, 0, 40, 1920, 1040, 0, 0)?; // scrolled up 40 pixels
surface::present(screen);                       // shown, as one frame
```

- **Pixels** are BGRA, row after row, at most 4096 x 4096. What it writes
  goes to a buffer Fenix shares with its worker. `present` shows what
  changed, all at once. Until then the pane shows the last frame whole.
  `surface::Frame` draws a whole frame in its own memory and shows it.
  `page::close` lets go of them: a surface opened again in its place (to
  show it in another pane) is a new one, drawn afresh. `page::focus`
  brings one to the pane you're in instead, as it is.
- **The session**: a surface is kept with it as a page is
  (`page_state`), asked again as it changes. In `page_restore`, a
  surface it opens takes the restored page's place: its tab and pane.
- **Scale**: `Fit` keeps its shape, at the top left. `Stretch` fills the
  pane. `None` draws a pixel for a pixel, and what doesn't fit is cut.
  Whichever it is, the pointer comes in the surface's own pixels.
- **Input** comes to `surface_input`:
  - `SurfaceInput::Key`: a character or a named key, down or up, with
    Ctrl, Alt and the Windows key.
  - `Pointer`: where it is and which buttons are held (1, 2, 4). It's
    told when a button changes, and on moves while one is held, or always
    while it has the keyboard.
  - `Wheel`: lines, up is more than 0.
  - `Capture(bool)`: it took the keyboard, or gave it back.
- **The keyboard**: a surface that captures it, focused, has every key
  until Ctrl+\ gives them back; a click takes them again. Fenix hides its
  own pointer then. A surface that doesn't capture it has a page's keys:
  Space is the leader, `:` the command line, `Ctrl+W` the windows, `q`
  closes it, and every other key is its own (presses only).
- **Over it**: `surface::cursor(Some(&image))` draws the pointer's
  picture where the pointer is (a remote desktop's own).
  `surface::marks(&[SurfaceMark::new(x, y, w, h, Role::Accent)])` tints
  boxes over it: a selection, search matches.

## Sockets

With `permissions.sockets`, an extension opens TCP connections, and UDP
sockets:

```toml
[permissions]
sockets = [
  "${setting:vnc.hosts}",   # the addresses a setting holds: a host:port,
                            # a list's, a table's rows (a host field, then
                            # a port field), tcp:// udp:// nats:// URLs
  "*:5900-5999",            # any host, these ports
  "tcp://*.example.com:443",
  "udp://:5000",            # this machine's UDP port 5000, to listen on
]
```

Nothing waits: `socket::connect` returns its id at once, and the rest
comes as events. A socket's reader stops when it's 8 MiB ahead of the
extension, until the extension catches up. Its sockets close when it
stops.

TLS: `socket::connect_tls(host, port, &TlsOptions::new())` is
connected once the handshake's done; `socket::start_tls(socket, &tls)`
starts it on a socket already talking -- PostgreSQL's `SSLRequest`,
MySQL's -- once the server said yes, and `Event::SocketConnected` comes
again when it's done. What's read and written is the plain text either
way. The server's certificate is checked against the system's;
`.pin(pem)` takes a server's own as it is, `.accept_invalid()` any, and
`.server_name(name)` checks it for another name. `.client_cert(cert,
key)` gives one, PEM.

Local sockets: `socket::ipc(name)` connects to a program on
this machine -- a named pipe on Windows (`\\.\pipe\<name>`), a Unix
socket on Linux and macOS (a path, or a name in `$XDG_RUNTIME_DIR`,
`$TMPDIR` or `/tmp`) -- by a name `permissions.ipc` lists, and is a
socket as a TCP one is from then on:

```toml
[permissions]
ipc = ["discord-ipc-*", "${setting:obs.socket}"]
```

Listening: `socket::listen(port)` takes a port of this machine's
loopback, 127.0.0.1 -- one `permissions.listen` names, or 0 for any free
one -- and gives back its id and the port it got. Each connection made
to it comes to `Extension::socket_accepted(listener, socket, peer)`, a
socket as a TCP one is from then on; `socket::close(listener)` stops it.
An OAuth sign-in's redirect, `http://127.0.0.1:<port>/callback`:

```toml
[permissions]
listen = ["127.0.0.1:0"]       # or a port, a range: 127.0.0.1:8400-8410
```

## Network

`permissions.network` names the hosts it may reach: `api.example.com`,
`*.example.com`, `${setting:key}` (the host of the URL a setting
holds), or `*` for any, which the store shows as "Reach any host on the
internet". A redirect is followed only to a host it may reach.

`HttpRequest::send` gives the answer whole to `handle`
(`Event::HttpResponse`), with where it ended up and how long each part
took. `.options(...)` sets a timeout, redirects or not, a certificate
that doesn't check out, a client certificate, an HTTP proxy (a host it
may reach); streamed, what comes goes to `Extension::http_event`
instead -- the answer, then its body as it comes (server-sent events, a
big body), each part read as the extension takes them.

## Language servers by file pattern, and files it opens

A language server it is can serve files by pattern as well as by
language: files Fenix has no language for (a MIB's `*.dat` tables), or
any file at all. A pattern with no `/` is matched against the file's
name; one with a `/` against its path below the project. A file that
isn't one of Fenix's languages is opened to it with the server's name as
its `languageId`.

```toml
[[contributes.language_servers]]
name = "mib"
languages = ["tcl"]            # Fenix's languages, as before
files = ["*.dat", "mib/**/*.txt"]
```

`contributes.opens` lists the files it opens itself, in place of a text
buffer (`opens = ["*.pdf"]`). Opening one (the explorer, the picker,
`:e`, the command line, a picture in Markdown's reading view) tells it
with `Event::Opened(path)`, and it may read that file. The page it opens
or brings forward then (`page::open`, `page::focus`, a surface) is that
file's: `SPC f o` (`file.open_as_text`) goes from it to the file's text,
and back. `open_file` on a file it opens itself opens its text: a file
it was given and can't show, declined.

`contributes.magic` says what a file it opens starts with, by
its pattern in `opens`: one that doesn't start so opens as text, never
reaching it -- a `.db` that isn't SQLite. Its text, or bytes in hex
after `hex:`:

```toml
[contributes]
opens = ["*.db", "*.sqlite"]

[contributes.magic]
"*.db" = "SQLite format 3"
"*.sqlite" = "hex:53 51 4c 69 74 65"
```

The page and the file's text are one: what it changes through
`buffer::edit` is in the buffer `SPC f o` shows, with yours, in one undo
history (`buffer::undo`, `buffer::redo` from the page: Ctrl+Z through
`page_key_press`); and its page hears `buffer_changed` for the file
without asking for `changed`.

The files it was given are kept with its pages in the session and given
again when they come back -- after a restart, or in another project -- so
a picture from outside the project shows again.

## Language packs

A language pack is a bundle that makes one language work out of the
box: data, and a small code extension. What each part is:

| Part | How |
|---|---|
| File types and the server | `languages/*.toml` (data): `language`, `extensions`, `filenames`, `server`. Or, from code, `project::language_server` once it has found or installed one; more beside it, `project::add_language_server`. |
| A grammar Fenix doesn't have | `languages/*.toml` names one, compiled to WebAssembly, in the extension's folder, below. |
| Snippets, project templates, keys | `snippets/`, `templates/`, `keymaps/` (data), as an extension of data has them. |
| Launch configurations | `launch/*.toml` (data), below; and those it works out as it runs, `debug::launches`. |
| Lessons | `tutorial/*.toml` (data): lessons for the tutorial (`SPC h T`), each a practice text and steps checked by what came of them -- the text, the cursor, the page or picker in front. Your pages are `page = "<your id>/<kind>"` (the Review extension's `fenix.review/new-request`); a lesson's `track` is its section, listed after Fenix's own. The format is Fenix's own lessons' (`crates/fenix-gui/tutorial/`, `src/tutorial.rs`). |
| The debug adapter | `project::debug_adapter(&["python"], Some(&command))`; one that talks over a socket, `project::debug_adapter_connect`, below. Both run it with the command's `env`. |
| A server's settings | `project::language_server_settings("eslint", Some(&json))`: what it's given when it asks. A project's own are `.fenix/lsp/<name>.json`. |
| A formatter | `project::formatter(&["go"], Some(&Command::new("gofmt")))`: Fenix runs it, so the text never passes through the extension. |
| A linter | Run it (on `file_saved`, which gives the file's path; or on `buffer_changed`) and `diagnostics::set` what it found. Or be a language server, or set one that lints (ruff's). |
| Tasks | `project::tasks`: build, test, run, lint. |
| Checks | `project::doctor`, a fix row running one of its commands (`.fix("install pyright", "python.install-server")`), which can install it -- `run_as_task` for a package manager, or a download, below. The servers, adapters and formatters it set are checked there too, found or not. |

A launch file is one configuration, offered by `SPC u u` in a project
that has no launch configuration of its own (`.fenix/tools.json`'s
`launch`), for the language you're in:

```toml
name = "Python: current file"
languages = ["python"]       # Fenix's names
request = "launch"           # or "attach"
program = "${file}"          # the file you're in when it's left out;
                             # false sends none
args = []
cwd = "${project}"
env = { PYTHONUNBUFFERED = "1" }
preLaunchTask = "build"      # run first, by its name in the task picker;
                             # the session starts once it succeeds

[attributes]                 # the rest of what the adapter takes
justMyCode = true
```

`${file}`, `${fileDir}` and `${project}` in its strings are the file
you're in, its folder and the project's; `${module}` is the file
as a dotted module of the project's (`pkg/cli/main.py` is
`pkg.cli.main`, a package's `__init__.py` the package). A launch that
leaves `program` out sends none when its attributes say what runs
another way -- debugpy's `module` or `code`:

```toml
name = "Python: module"
languages = ["python"]
[attributes]
module = "${module}"
```

An attach names no program
unless it says one; what to attach to goes in `attributes`
(`processId`, `port`).

`${pickProcess}` is a process of this machine's, picked as the
session starts (its id), and any other `${name}` a variable an
extension fills in, `debug::variable(name, &choices)`: with one choice,
its value; with several, picked. A value that's only a variable and
filled with a whole number is sent as a number:

```toml
name = "Go: attach"
languages = ["go"]
request = "attach"
[attributes]
mode = "local"
processId = "${pickProcess}"
```

What a launch file can't know -- a CMake project's targets, a Cargo
workspace's binaries -- it gives as it runs, the same fields as a launch
file's (`program` none sends none):

```rust
let targets: Vec<LaunchConfig> = self.targets.iter().map(|t| {
    LaunchConfig::launch(&format!("CMake: {t}"), &["c", "cpp"])
        .program(&format!("/project/build/{t}"))
        .attributes(&serde_json::json!({"type": "lldb"}))
        .pre_launch_task(&format!("cmake: build {t}"))
}).collect();
debug::launches(&targets)?;
```

A path under `/project`, `/data` or `/tools` in a launch's `program` or
`cwd` is the full path Fenix has for it.

An adapter that runs each program in a child session asks Fenix for one
(DAP's `startDebugging`, as js-debug does) and gets it: a
connection of its own -- another to the same port over a socket, the
program started again on stdio -- launched or attached with the
configuration it gave. It shows under the session it came from in the
Call Stack pane; its stops, stepping and watches are its own. The
project's own launch (`.fenix/tools.json`'s `launch`) takes `"program":
false` and `"preLaunchTask"` too. The adapter talks DAP over its stdin and stdout,
or, given with `project::debug_adapter_connect`, over a socket:
Fenix starts it and connects to 127.0.0.1 at the port -- 0 for one
Fenix picks, which `${port}` in its args is -- as soon as it listens
(within 10 seconds). With no command, it connects to one already
running there, a port `permissions.sockets` lets it reach.

```rust
// js-debug's server, and CodeLLDB's.
project::debug_adapter_connect(&["javascript", "typescript"], Some(&Command::new("node").args(["${tools}/js-debug/src/dapDebugServer.js", "${port}", "127.0.0.1"])), 0)?;
project::debug_adapter_connect(&["rust", "cpp"], Some(&Command::new("/tools/codelldb/adapter/codelldb").args(["--port", "${port}"])), 0)?;
```

A language file's `filenames` may be patterns: `filenames =
[".env", ".env.*"]`.

A language Fenix doesn't highlight comes with its grammar: a
tree-sitter grammar compiled to WebAssembly (`tree-sitter build --wasm`
in the grammar's folder), in the extension's own folder, and its queries
from `queries/<language>/` there:

```toml
# languages/kotlin.toml
language = "kotlin"
grammar = "grammars/tree-sitter-kotlin.wasm"
extensions = ["kt", "kts"]
server = "kotlin-language-server"
# symbol = "kotlin"          # what it exports is tree_sitter_<symbol>; by
#                            # default the file's name, `tree-sitter-` off
# queries = "queries/kotlin" # the default
```

`highlights.scm` colours it, by the same capture names Fenix's own
grammars use (`keyword`, `string`, `function`); `folds.scm`'s `@fold`s
are what folds, and are the scopes `SPC c s` and the breadcrumbs list;
`indents.scm`'s `@indent` (or `@indent.begin`) makes Enter indent a
level more inside one that starts on the cursor's line. `injections.scm`
isn't read: Fenix doesn't highlight one language inside another. The
language's name is new -- not one of Fenix's -- and other language files,
and `buffer::set_language`, may then use it. The grammar's code runs in
tree-sitter's own WebAssembly sandbox, with nothing but the text it's
given; it's compiled the first time a file of it is opened. A grammar of
language ABI 13 to 15 loads (tree-sitter 0.26's).

A language file's `comment` is what starts a line comment in
its language, for `gcc`: `comment = "//"`.

`find_program` takes a hint `${setting:key}`: the path one of its
settings holds -- the interpreter you chose -- looked at first. A hint
`./gradlew` is the project's own wrapper, looked at before
`PATH`, and on Windows `gradlew.bat` is found for it. `${data}` in a
command's args and env is its own folder's full path, as
`${tools}` is its tools folder's: a server's workspace folder
(`-data ${data}/workspace`).

Its tools folder, `/tools`, is where it installs a server or an adapter:
a folder of Fenix's tools folder (`<tools>/<id>`, on this machine only,
never synced), there when `permissions.process` names a program in it.
A program there runs by its path, as one under `/data` does:

```toml
[permissions]
process = ["/tools/*", "pyright-langserver", "node"]
network = ["github.com", "objects.githubusercontent.com"]
```

```rust
// Fetched, checked, unpacked (in the extension: zip, tar.gz), made to
// run, found and used.
HttpRequest::get(url).download("/tools/dl/server.tar.gz", Some(sha256))?;
// ... on Event::HttpResponse: unpack into /tools/server/ with std::fs
tools::set_executable("/tools/server/bin/server")?;
let found = find_program("server", &["/tools/server/bin"]);
project::language_server(&["python"], Some(&Command::new("/tools/server/bin/server")))?;
// A script a program there runs: `${tools}` in args is the folder.
Command::new("node").arg("${tools}/js-debug/src/dapDebugServer.js");
```

`find_program` looks in its tools folder too, a few folders down, so a
release that unpacks into a versioned folder is found.

## Forges

An extension can be a code-review forge (GitHub, GitLab) for Fenix's Git
pages. It names the hosts of the remotes it serves:

```toml
[[contributes.forges]]
name = "GitLab"
hosts = ["${setting:gitlab.base_url}", "*"]   # "*": any the others don't claim
```

For a repository whose `origin` is on one of them, Fenix calls its
`serve`. `forge::call(name, payload)` reads the call into a
`forge::Repository`: `root`, `remote`, `branch` and `number`. It may
read files under `root` and run its programs there.

| Service | When | Its answer |
|---|---|---|
| `forge.branch` | the Git status page is read | `forge::branch_answer(Some(&request))`: the branch's request for the page's header, or `None` |
| `forge.open` | `o` on the Git status page | it opens request `number`, or a new one for the branch |
| `forge.reviews` | the reviews are asked for | it opens them |

After it changes the repository (a request opened, a branch checked
out), `forge::refresh()` reads Fenix's Git pages again.

What you're doing comes as events too, for an extension that asks in
`contributes.events`: `saved` (`Event::Saved`), `diagnostics`
(`Event::Diagnostics`, the focused file's errors and warnings), `tasks`
(`Event::TaskEnded`), `typing` (`Event::Typing`, twice a second at most).

## Coding agents

Fenix talks to coding agents over the Agent Client Protocol (ACP): Claude
Code, Codex, Devin, Gemini CLI, and any other ACP server
([AGENTS.md](AGENTS.md)). An extension can bring one, ask one, and hear
what they do.

**Bringing an agent.** An agent is a program that speaks ACP on stdio.
The extension names it in its manifest, installs it in its tools folder
(or finds it), and says how it starts:

```toml
[[contributes.agents]]
id = "cline"                   # in acme.cline: its own name, or cline.*
name = "Cline"

[permissions]
process = ["/tools/*", "node"]

[activation]
on_startup = true              # so it's there when you open the agents
```

```rust
let found = find_program("cline", &["/tools/cline/bin"]);
agents::provide("cline", Some(&Command::new("node").args(["${tools}/cline/acp.js"])))?;
// agents::provide("cline", None) takes it away.
```

It's listed on the agents page (`SPC a p`) from when the extension is
installed, and offered once it has said how it starts; your own
`agents.servers` entry by the same id wins. Fenix runs it, as it runs its
own: permissions, edits, review and threads are the same for every agent.

**Asking an agent.** With `permissions.agents`, `agents::ask` sends a
prompt to an agent -- the project's default one (`agents.default`), or
one by id -- in a thread of its own, and gives back its answer when the
turn ends. It may read the project, not change it: whatever it asks
permission for is refused. The thread is on the threads page
(`SPC a t`), so you can see what was asked.

```rust
run::spawn(async {
    match agents::ask("Name this branch's change in five words.", None).await {
        Ok(answer) => notify(&answer.text),   // answer.stop_reason, answer.agent
        Err(e) => notify(&e),
    }
});
```

It's Fenix's service `agents.ask`: `{"prompt": "...", "agent": "codex"}`
in, `{"text", "stopReason", "agent", "thread"}` out. An ask unanswered
after 10 minutes, or by an agent that wants a login, is an error.

**Hearing them.** With `contributes.events = ["agents"]` (and
`permissions.agents`), `Extension::agent_turn_ended` is told of each turn
that ends, in any thread: the agent, the thread and its title, the
project, the stop reason (`end_turn`, `cancelled`, `max_tokens`,
`max_turn_requests`, `refusal`, `error`) and what the agent said.

## Its manifest

```toml
[extension]
id = "acme.counter"            # publisher.name
name = "Counter"
version = "0.1.0"
description = "Counts"
runtime = "wasm"

[engine]
fenix = ">=1.0, <2"
api = "1.0"                    # fenix-extension-api's API

[activation]                   # nothing loads until one of these
on_startup = false
commands = ["counter.*"]

[permissions]                  # nothing, unless it says
process = ["git", "${setting:counter.git}", "/data/tools/*", "/tools/*"]
                               # programs: by name; the path a setting
                               # holds; under its own folder; under its
                               # tools folder
network = ["api.example.com", "*.example.org", "${setting:counter.url}"]
                               # or ["*"], any host
secrets = ["counter.token", "counter.secret.*"]
                               # read, and kept (set_secret): by
                               # key, or by pattern
filesystem = "project-read"    # none, project-read or project-write
draw = "over"                  # draw over the editor (layers)
editor = "read"                # where you are, when its commands run;
                               # "write" to change what you edit too
sound = true                   # play WAVs of its own (play_sound)
fonts = true                   # the system's fonts, at /fonts/<n>
clipboard = true               # read the clipboard's text
sockets = ["*:5900-5999", "${setting:counter.hosts}", "udp://:5000"]
                               # see Sockets
ipc = ["discord-ipc-*"]        # local sockets, by name
listen = ["127.0.0.1:0"]       # ports to listen on, the loopback's
env = ["HOME", "JAVA_*", "${setting:counter.env}"]
                               # Fenix's environment variables it may
                               # read (env_var)
agents = true                  # ask your coding agents (agents.ask) and
                               # hear their turns end
folders = { notes = "${setting:notes.folder}|data:notes" }
                               # a folder of yours, at /folders/notes:
                               # a setting's path, else one in Fenix's
                               # data folder; made if it isn't there,
                               # and the setting reads as its full path.
                               # A map setting (name = folder) is a
                               # folder each, /folders/notes/<name>, the
                               # project's own value first, relative
                               # to the project; those aren't made
settings = ["ccsds"]           # read another's settings (ccsds.*), by
                               # the name before their dot

[[contributes.commands]]
id = "counter.open"            # its own name first
title = "Counter: open"

[[contributes.settings]]
key = "counter.url"
kind = "text"                  # text, bool, int, minutes, path, list, map,
                               # choice or secret (a map has columns =
                               # ["Key", "Name"])
description = "Where it counts"
default = "https://api.example.com"
project = true                 # a project may set it too (.fenix/settings.toml)
section = "Counter"            # its section of the settings page (SPC ,);
                               # the extension's name when left out

[contributes]
services = ["counter.count"]
namespaces = []                # other names its commands, settings and
                               # services may start with (diagram)

events = ["saved", "typing"]   # what you're doing it's told of:
                               # saved, diagnostics, tasks (and
                               # task_started), typing, active, changed (a
                               # file's buffer, with editor = "read"),
                               # focus, view, debug, agents
                               # (agent_turn_ended, with permissions.agents)

[limits]
call_ms = 30000                # how long a command or an event may take,
                               # up to a minute; 5000 when it doesn't say

[[contributes.contexts]]      # where its SPC m menu applies
name = "sketch"
markers = ["sketch.yaml", "{folder}.ino"]   # a folder holding one
languages = []                 # or a file in one of these (mermaid)
pages = []                     # or one of its own pages of these kinds
                               # (the PDF reader's surfaces, "pdf")

[[contributes.language_servers]]
name = "counter"
languages = ["markdown"]       # Fenix's names for them

[[contributes.agents]]          # coding agents it brings
id = "counter"                 # its own name, or one starting counter.
name = "Counter Agent"
login = "counter login"        # how to log in, for the agents page
```

A context's menu is its keymaps' `[[local.<name>]]` entries
(`keymaps/*.toml`), keys after `SPC m`:

```toml
[[local.sketch]]
keys = "b"
label = "build"
command = "counter.build"
```

What it says about its project -- a language server, a debug adapter, a
formatter, doctor checks, tasks -- Fenix keeps and uses there: nothing
waits on the extension.

Fenix holds it to `[permissions]`, and shows them before anyone
installs it. A command, setting or service starts with its own name, or
one of its `namespaces`; a setting under a namespace is read with
`setting` as its own are (the PDF reader's `reader.zoom`).

A file Fenix reads for it -- `read_file`, a picture, a sound, a sheet or
a model, a file it opens or edits -- is read once, as its path is checked:
Fenix asks the open file where it is, so a link it puts in the
file's place, or a folder's, after the check is refused rather than
followed out of its folders. A file it opens is opened from those bytes.

## Limits

A command has 5 seconds, a page or a key 1 second, and activating it
10 seconds. `limits.call_ms` gives its commands, events and
services longer, up to a minute: a database query. Its pages wait
meanwhile -- the last one stays on screen -- since an extension does one
thing at a time; work longer than that belongs in a program it runs
(`spawn`), whose output comes as events. Its memory is capped at 128 MiB. Past any of those, endless
recursion, or a panic, the extension stops with the reason on its
output page, and Fenix carries on. It starts again with Fenix.

The API follows semantic versioning. An extension built against 1.N runs
on a Fenix that offers 1.M, N no more than M: a minor version only adds,
and an extension that uses what one added says so (`api = "1.1"`). Nothing
is removed or changed within 1.x. This Fenix offers 1.0. Its definition
is `crates/fenix-extension-api/wit/world.wit`.
