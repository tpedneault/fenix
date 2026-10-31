//! Writing a Fenix extension in Rust.
//!
//! An extension is a type that implements [`Extension`] -- only the parts
//! it needs; the rest have defaults -- and is exported with [`export!`].
//! It's built for `wasm32-wasip2`, and Fenix runs it in a sandbox: what it
//! may do beyond its own folder (`/data`) its manifest asks for in
//! `[permissions]`.
//!
//! ```no_run
//! use fenix_extension_api::{export, info, page, Extension, Key, Page};
//!
//! #[derive(Default)]
//! struct Counter { count: u32 }
//!
//! impl Extension for Counter {
//!     fn run_command(&mut self, id: &str) -> Result<(), String> {
//!         match id {
//!             "counter.open" => {
//!                 page::open("counter", "Counter");
//!                 Ok(())
//!             }
//!             _ => Err(format!("no command {id}")),
//!         }
//!     }
//!
//!     fn page(&mut self, _page: u64, _width: u32, _height: u32) -> Page {
//!         Page::new().line([page::title("Counter")]).rule().line([page::text(format!("{}", self.count))]).focus(2).key("j", "count")
//!     }
//!
//!     fn page_key(&mut self, page: u64, key: Key) {
//!         if key == Key::Char('j') {
//!             self.count += 1;
//!             info!("counted to {}", self.count);
//!             page::refresh(page);
//!         }
//!     }
//! }
//!
//! export!(Counter);
//! ```
//!
//! Nothing a host call does blocks for long. What takes time -- a
//! process, an HTTP request, a timer, a picker, a prompt, another
//! extension's service -- returns an id at once, and the answer comes to
//! [`Extension::handle`] as an [`Event`] carrying that id.

/// What `wit/world.wit` makes; wit-bindgen's code, so its lints are its own.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "extension",
        pub_export_macro: true,
        export_macro_name: "__export_world",
        default_bindings_module: "fenix_extension_api::bindings",
        additional_derives: [PartialEq],
    });
}

pub use bindings::fenix::extension::host;
pub use bindings::fenix::extension::types::{
    Anchor, Canvas, CanvasPopup, CellSpan, Cells, Check, Command, Ease, EditorTheme, Glide, Health, Here, HomeMark, HomeRow, HomeSection, HttpRequest, HttpResponse, ImageInfo, Item, KeyHint,
    LayerLevel, Layout, Level, Line, Mark, ModelInfo, PaneBox, PickerItem, Picture, Place, Popup, Pose, ProcessOutput, ProjectInfo, Role, Section, Span, Style, Task, TextEdit,
    CursorImage, Datagram, KeyEvent, Modifiers, NamedKey, PointerEvent, SurfaceKey, SurfaceMark, SurfaceOptions, SurfaceScale, SurfaceSize, WheelEvent,
    Changed, CursorInfo, Focus, FocusedPage, FolderEntry,
    CellStyle, Decoration, DecorationLook, KeyMods, KeyPress, MouseAction, MouseButton, MouseEvent, MousePress, Rgb, Shade, TextBuffer, VerticalRule, VirtualText,
    Problem, Severity,
    ClientCert, DecorationRule, HttpOptions, HttpPart, HttpTiming, PlatformInfo, TlsOptions,
    CellColor, TextView,
    DebugSession, DebugState, LaunchConfig, LaunchRequest, QuickFix,
    AgentTurn,
};
pub use bindings::{Event, Key, Page, SurfaceInput};

pub mod forge;
pub mod grid;
pub mod lsp;
pub mod run;
pub mod socket;
pub mod surface;

/// The chrono clock works with, for an extension's own use.
pub use chrono;
/// The JSON a server's settings and a launch's attributes are.
pub use serde_json;

use std::any::Any;
use std::cell::RefCell;

/// What an extension does. Every method has a default, so it implements
/// only what it needs; one instance (made with `Default`) lives for as
/// long as Fenix runs it, so its state is its own fields.
pub trait Extension: Default + 'static {
    /// Once, when Fenix loads it: set up, or say why it can't.
    fn activate(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// One of the commands its manifest contributes, by id.
    fn run_command(&mut self, id: &str) -> Result<(), String> {
        Err(format!("no command {id}"))
    }

    /// Something it asked for has come, or something changed.
    fn handle(&mut self, _event: Event) {}

    /// Its page `page` (from [`page::open`]), laid out `width` cells wide
    /// for a pane `height` lines tall: asked again when the pane's size
    /// changes, so a table can fit it, or a key strip sit at its foot.
    fn page(&mut self, _page: u64, _width: u32, _height: u32) -> Page {
        Page::new()
    }

    /// A key on its page `page`. Fenix asks for the page again after.
    fn page_key(&mut self, _page: u64, _key: Key) {}

    /// Text pasted on its page `page` while it's typing ([`Page::typing`]).
    fn page_paste(&mut self, _page: u64, _text: &str) {}

    /// A message for its language server `server` ([`lsp`]): give it to
    /// [`lsp::Servers::handle`].
    fn lsp_message(&mut self, _server: u64, _message: &str) {}

    /// What it needs to open page `page` again next time, if anything.
    fn page_state(&mut self, _page: u64) -> Option<String> {
        None
    }

    /// Open a page of `kind` again from `state`, as page `page`; false
    /// when it can't.
    fn page_restore(&mut self, _page: u64, _kind: &str, _state: &str) -> bool {
        false
    }

    /// Answer one of the services its manifest offers, as call `call`:
    /// now, or `None` and later with [`answer`] (when it has to ask
    /// something first -- an HTTP request, a process).
    fn serve(&mut self, _call: u64, name: &str, _payload: &str) -> Option<Result<String, String>> {
        Some(Err(format!("no service {name}")))
    }

    /// A key, the pointer or the wheel on its surface `surface` (from
    /// [`surface::open`]).
    fn surface_input(&mut self, _surface: u64, _input: SurfaceInput) {}

    /// You closed its page or surface `page` -- its tab, `q`, the buffer
    /// -- so what it kept for it can go. Not for one it closed itself
    /// ([`page::close`]).
    fn page_closed(&mut self, _page: u64) {}

    /// A buffer's text changed, once it's been still for 300 ms: one of
    /// its scratch buffers' ([`scratch`]), always; a file's, when its
    /// manifest asks (`contributes.events = ["changed"]`, with `editor =
    /// "read"`).
    fn buffer_changed(&mut self, _what: Changed) {}

    /// You closed one of its scratch buffers ([`scratch::open`]).
    fn scratch_closed(&mut self, _scratch: u64) {}

    /// Something else has the focus -- a file, a page, nothing -- when its
    /// manifest asks (`contributes.events = ["focus"]`): once as it starts,
    /// and as it changes.
    fn focus_moved(&mut self, _focus: Focus) {}

    /// A key [`Extension::page_key`] doesn't take -- PageUp, Shift+Left,
    /// Ctrl+Z -- on its page `page`, when the page takes more keys
    /// ([`Page::more_keys`]). Fenix asks for the page again after.
    fn page_key_press(&mut self, _page: u64, _key: KeyPress) {}

    /// A click, a drag or the wheel on its page `page`, when the page asks
    /// ([`Page::mouse`]). Fenix asks for the page again after.
    fn page_mouse(&mut self, _page: u64, _event: MouseEvent) {}

    /// What came of its streamed request `request` ([`HttpOptions::streamed`]):
    /// its answer, its body in parts, and then its end.
    fn http_event(&mut self, _request: u64, _part: HttpPart) {}

    /// A file was saved, when its manifest asks (`contributes.events =
    /// ["saved"]`): its full path with `editor = "read"`, else its path
    /// under `/project`. It comes after [`Event::Saved`], which has only
    /// the name.
    fn file_saved(&mut self, _path: &str) {}

    /// The lines each pane shows of a buffer it may know of, once they've
    /// been still a moment, when its manifest asks (`contributes.events =
    /// ["view"]`): its scratch buffers', and with `editor = "read"` files'
    /// -- what to decorate in a long file.
    fn view_changed(&mut self, _views: Vec<TextView>) {}

    /// A click on one of its decorations with an id
    /// ([`Decoration::tagged`]): a swatch that opens a picker.
    fn decoration_clicked(&mut self, _buffer: TextBuffer, _id: u32) {}

    /// A task started in the task pane, by name, when its manifest asks
    /// (`contributes.events = ["tasks"]`): [`Event::TaskEnded`] comes when
    /// it's done.
    fn task_started(&mut self, _name: &str) {}

    /// A debug session started, paused, ran again or ended -- an adapter's
    /// child sessions too -- when its manifest asks (`contributes.events =
    /// ["debug"]`).
    fn debug_changed(&mut self, _state: DebugState) {}

    /// A connection made to one of its ports ([`socket::listen`]): the
    /// listener, the new socket, and who connected (`127.0.0.1:53712`).
    /// What comes on it is [`Event::SocketData`], as on one it connected.
    ///
    fn socket_accepted(&mut self, _listener: u64, _socket: u64, _peer: &str) {}

    /// A coding agent's turn ended, in any of your threads, when
    /// its manifest asks (`contributes.events = ["agents"]`, with
    /// `permissions.agents`).
    fn agent_turn_ended(&mut self, _turn: AgentTurn) {}
}

thread_local! {
    static INSTANCE: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

/// Runs `f` on the extension's one instance, made the first time.
#[doc(hidden)]
pub fn with<T: Extension, R>(f: impl FnOnce(&mut T) -> R) -> R {
    INSTANCE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let any = slot.get_or_insert_with(|| Box::new(T::default()));
        f(any.downcast_mut::<T>().expect("one extension type per component"))
    })
}

/// Exports `$ty` (an [`Extension`]) as this component's extension.
#[macro_export]
macro_rules! export {
    ($ty:ty) => {
        struct __FenixExtension;

        impl $crate::bindings::Guest for __FenixExtension {
            fn activate() -> Result<(), String> {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::activate(e))
            }
            fn run_command(id: String) -> Result<(), String> {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::run_command(e, &id))
            }
            fn handle(event: $crate::Event) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::handle(e, event))
            }
            fn page_render(page: u64, width: u32, height: u32) -> $crate::Page {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page(e, page, width, height))
            }
            fn page_key(page: u64, key: $crate::Key) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_key(e, page, key))
            }
            fn page_state(page: u64) -> Option<String> {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_state(e, page))
            }
            fn page_restore(page: u64, kind: String, state: String) -> bool {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_restore(e, page, &kind, &state))
            }
            fn lsp_message(server: u64, message: String) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::lsp_message(e, server, &message))
            }
            fn page_paste(page: u64, text: String) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_paste(e, page, &text))
            }
            fn serve(call: u64, name: String, payload: String) -> Option<Result<String, String>> {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::serve(e, call, &name, &payload))
            }
            fn surface_input(surface: u64, input: $crate::SurfaceInput) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::surface_input(e, surface, input))
            }
            fn page_closed(page: u64) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_closed(e, page))
            }
            fn buffer_changed(what: $crate::Changed) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::buffer_changed(e, what))
            }
            fn scratch_closed(scratch: u64) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::scratch_closed(e, scratch))
            }
            fn focus_moved(focus: $crate::Focus) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::focus_moved(e, focus))
            }
            fn page_key_press(page: u64, key: $crate::KeyPress) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_key_press(e, page, key))
            }
            fn page_mouse(page: u64, event: $crate::MouseEvent) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::page_mouse(e, page, event))
            }
            fn http_event(request: u64, part: $crate::HttpPart) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::http_event(e, request, part))
            }
            fn file_saved(path: String) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::file_saved(e, &path))
            }
            fn view_changed(views: Vec<$crate::TextView>) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::view_changed(e, views))
            }
            fn decoration_clicked(buffer: $crate::TextBuffer, id: u32) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::decoration_clicked(e, buffer, id))
            }
            fn task_started(name: String) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::task_started(e, &name))
            }
            fn debug_changed(state: $crate::DebugState) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::debug_changed(e, state))
            }
            fn socket_accepted(listener: u64, socket: u64, peer: String) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::socket_accepted(e, listener, socket, &peer))
            }
            fn agent_turn_ended(turn: $crate::AgentTurn) {
                $crate::with::<$ty, _>(|e| <$ty as $crate::Extension>::agent_turn_ended(e, turn))
            }
        }

        $crate::bindings::__export_world!(__FenixExtension with_types_in $crate::bindings);
    };
}

/// Logs to its output page (`SPC h l`), `format!`-style.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::host::log($crate::Level::Info, &format!($($arg)*)) };
}

/// Logs a warning to its output page, `format!`-style.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::host::log($crate::Level::Warn, &format!($($arg)*)) };
}

/// Logs an error to its output page, `format!`-style.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::host::log($crate::Level::Error, &format!($($arg)*)) };
}

/// Laying out its pages, and opening them.
pub mod page {
    use super::{host, Line, Page, Role, Span};

    /// Opens a page of `kind`, called `title`; its id comes back.
    pub fn open(kind: &str, title: &str) -> u64 {
        host::page_open(kind, title)
    }

    /// The same, in a new pane beside the focused one, `width` of it (a
    /// fraction), the focus staying where it was: a preview, a sidebar.
    pub fn open_beside(kind: &str, title: &str, width: f32) -> u64 {
        host::page_open_beside(kind, title, width)
    }

    /// Its page `page` has changed: Fenix lays it out again.
    pub fn refresh(page: u64) {
        host::page_refresh(page)
    }

    pub fn close(page: u64) {
        host::page_close(page)
    }

    /// Its page or surface `page`, brought to the pane you're in: from
    /// another pane, or from behind another tab. It needs a Fenix that has
    /// `page-focus` (a later).
    pub fn focus(page: u64) {
        host::page_focus(page)
    }

    /// The file its page `page` shows now -- one it may read -- or none:
    /// `SPC f o` and the file's `buffer-changed` go by it.
    pub fn file(page: u64, path: Option<&str>) -> Result<(), String> {
        host::page_file(page, path)
    }

    fn span(text: impl Into<String>, role: Role) -> Span {
        Span { text: text.into(), role }
    }

    /// A run of text in the page's title colour.
    pub fn title(text: impl Into<String>) -> Span {
        span(text, Role::Title)
    }
    /// A run of plain text.
    pub fn text(text: impl Into<String>) -> Span {
        span(text, Role::Text)
    }
    /// A run of quieter text.
    pub fn muted(text: impl Into<String>) -> Span {
        span(text, Role::Muted)
    }
    /// A run in the theme's accent.
    pub fn accent(text: impl Into<String>) -> Span {
        span(text, Role::Accent)
    }
    /// Something good: running, passed, done.
    pub fn good(text: impl Into<String>) -> Span {
        span(text, Role::Good)
    }
    /// Something to watch.
    pub fn warn(text: impl Into<String>) -> Span {
        span(text, Role::Warn)
    }
    /// Something wrong.
    pub fn bad(text: impl Into<String>) -> Span {
        span(text, Role::Bad)
    }

    impl Page {
        /// An empty page.
        pub fn new() -> Page {
            Page {
                lines: Vec::new(),
                focus: None,
                keys: Vec::new(),
                popup: None,
                canvas: None,
                typing: false,
                space: false,
                styles: Vec::new(),
                vertical_rules: Vec::new(),
                focuses: Vec::new(),
                more_keys: false,
                mouse: false,
                colors: Vec::new(),
            }
        }

        /// A page laid out cell by cell ([`crate::grid::Grid`]): drawn as it
        /// is, with no title, frame or key line of Fenix's.
        pub fn canvas(canvas: super::Canvas) -> Page {
            Page { canvas: Some(canvas), ..Page::new() }
        }

        /// A field on it has the keyboard: Space, `q`, Esc and pastes are
        /// its.
        pub fn typing(mut self, typing: bool) -> Page {
            self.typing = typing;
            self
        }

        /// Space is its key on the focused row, not the leader.
        pub fn space(mut self, space: bool) -> Page {
            self.space = space;
            self
        }

        /// A box over the page beside line `line` -- a menu, a question --
        /// of `rows`, each a run of spans. Keys still come to the page.
        pub fn popup(mut self, line: u32, rows: impl IntoIterator<Item = Vec<Span>>) -> Page {
            self.popup = Some(super::Popup { line, rows: rows.into_iter().collect() });
            self
        }

        /// A line of runs, left to right.
        pub fn line(mut self, spans: impl IntoIterator<Item = Span>) -> Page {
            self.lines.push(Line { spans: spans.into_iter().collect(), rule: false });
            self
        }

        /// A blank line.
        pub fn blank(self) -> Page {
            self.line([])
        }

        /// A hairline across the page.
        pub fn rule(mut self) -> Page {
            self.lines.push(Line { spans: Vec::new(), rule: true });
            self
        }

        /// The line the cursor is on, by its index.
        pub fn focus(mut self, line: u32) -> Page {
            self.focus = Some(line);
            self
        }

        /// A key and what it does, at the page's foot.
        pub fn key(mut self, keys: &str, what: &str) -> Page {
            self.keys.push(super::KeyHint { keys: keys.into(), what: what.into() });
            self
        }

        /// How many lines it has: the index the next one gets.
        pub fn len(&self) -> u32 {
            self.lines.len() as u32
        }

        pub fn is_empty(&self) -> bool {
            self.lines.is_empty()
        }
    }

    impl Default for Page {
        fn default() -> Page {
            Page::new()
        }
    }

    impl Page {
        /// `cells`' background, as `shade`.
        pub fn background(mut self, cells: super::Cells, shade: super::Shade) -> Page {
            self.styles.push(super::CellStyle { cells, background: Some(shade), underline: None });
            self
        }

        /// `cells` underlined, in `role`'s colour.
        pub fn underline(mut self, cells: super::Cells, role: super::Role) -> Page {
            self.styles.push(super::CellStyle { cells, background: None, underline: Some(role) });
            self
        }

        /// A hairline down cell column `column` (a wide character two),
        /// from line `top` up to `bottom`.
        pub fn vertical_rule(mut self, column: u32, top: u32, bottom: u32) -> Page {
            self.vertical_rules.push(super::VerticalRule { column, top, bottom });
            self
        }

        /// Another focused row of its canvas: tinted, not kept in view.
        pub fn also_focus(mut self, cells: super::Cells) -> Page {
            self.focuses.push(cells);
            self
        }

        /// It takes more keys ([`crate::Extension::page_key_press`]).
        pub fn more_keys(mut self, more: bool) -> Page {
            self.more_keys = more;
            self
        }

        /// The mouse comes to it ([`crate::Extension::page_mouse`]), the
        /// wheel too.
        pub fn mouse(mut self, mouse: bool) -> Page {
            self.mouse = mouse;
            self
        }

        /// The text of `cells` in `color`, over its spans': a contrast
        /// preview's text in the colour chosen.
        pub fn color(mut self, cells: super::Cells, color: super::Rgb) -> Page {
            self.colors.push(super::CellColor { cells, color });
            self
        }
    }

}

impl Command {
    /// `program` (by name: its manifest's `permissions.process` lists it),
    /// with no arguments.
    pub fn new(program: &str) -> Command {
        Command { program: program.into(), args: Vec::new(), cwd: None, env: Vec::new() }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Command {
        self.args.push(arg.into());
        self
    }

    pub fn args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Command {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Where it runs: under `/project`, `/data` or `/tools`.
    pub fn cwd(mut self, dir: &str) -> Command {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Command {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Starts it; its lines and its end come as [`Event::ProcessOutput`]
    /// and [`Event::ProcessExit`].
    pub fn spawn(&self) -> Result<u64, String> {
        host::spawn(self)
    }

    /// Starts it with its stdout as bytes, [`Event::ProcessBytes`], not
    /// lines: a companion program that sends pictures back. Its stderr
    /// still comes a line at a time.
    pub fn spawn_raw(&self) -> Result<u64, String> {
        host::spawn_raw(self)
    }

    /// Runs it in a terminal called `title`, in the focused pane; its id
    /// is for [`terminal::stop`] and [`terminal::restart`].
    pub fn run_in_terminal(&self, title: &str) -> Result<u64, String> {
        host::run_in_terminal(self, title, false)
    }

    /// Runs it in a terminal in a new pane under the focused one.
    pub fn run_below(&self, title: &str) -> Result<u64, String> {
        host::run_in_terminal(self, title, true)
    }

    /// Runs it as a task called `name`, in Fenix's task output pane, its
    /// errors in the quickfix list; [`Event::TaskFinished`] says how it
    /// went.
    pub fn run_as_task(&self, name: &str) -> Result<u64, String> {
        host::run_task(name, self)
    }

    /// Runs it as a task beside the task pane, called `name`: several at
    /// once (at most 4 of its own), each one's output in a buffer of its
    /// own ([`task::show`]), its errors not in the quickfix list.
    /// [`Event::TaskFinished`] says how it went.
    pub fn run_in_background(&self, name: &str) -> Result<u64, String> {
        host::run_task_background(name, self)
    }
}

/// Its tasks from [`Command::run_in_background`].
pub mod task {
    use super::host;

    /// Its task `task` stopped: [`crate::Event::TaskFinished`] says it
    /// failed.
    pub fn cancel(task: u64) {
        host::task_cancel(task)
    }

    /// Its task `task`'s output, in the focused pane.
    pub fn show(task: u64) -> Result<(), String> {
        host::task_show(task)
    }
}

impl HttpRequest {
    /// A `GET` of `url`: a host its manifest's `permissions.network` names.
    pub fn get(url: &str) -> HttpRequest {
        HttpRequest { method: "GET".into(), url: url.into(), headers: Vec::new(), body: Vec::new(), options: HttpOptions::default() }
    }

    /// A `POST` of `body` to `url`.
    pub fn post(url: &str, body: impl Into<Vec<u8>>) -> HttpRequest {
        HttpRequest { method: "POST".into(), url: url.into(), headers: Vec::new(), body: body.into(), options: HttpOptions::default() }
    }

    pub fn header(mut self, name: &str, value: &str) -> HttpRequest {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sends it; the answer -- with its final URL and timing -- comes as
    /// [`Event::HttpResponse`], or, streamed, to [`Extension::http_event`].
    pub fn send(&self) -> Result<u64, String> {
        host::http(self)
    }

    /// Sends it, its body written to file `path` under `/tools` or `/data`
    /// rather than given back -- a release to unpack -- and checked against
    /// `sha256` (hex) when given. The answer comes as
    /// [`Event::HttpResponse`], its body empty.
    pub fn download(&self, path: &str, sha256: Option<&str>) -> Result<u64, String> {
        host::http_download(self, path, sha256)
    }

    /// Made as `options` say: a timeout, a proxy, streamed.
    pub fn options(mut self, options: HttpOptions) -> HttpRequest {
        self.options = options;
        self
    }

    /// Sends it as `options` say ([`HttpRequest::send`]).
    pub fn send_with(&self, options: &HttpOptions) -> Result<u64, String> {
        self.clone().options(options.clone()).send()
    }
}

/// The machine Fenix runs on -- `windows`, `x86_64`, `msvc` -- for picking
/// a release's file.
pub fn platform() -> PlatformInfo {
    host::platform()
}

/// A request from [`HttpRequest::send_with`] given up: nothing more comes
/// of it.
pub fn http_cancel(request: u64) {
    host::http_cancel(request)
}

impl Default for HttpOptions {
    /// 30 seconds, redirects followed, certificates checked, no proxy, the
    /// body at its end.
    fn default() -> HttpOptions {
        HttpOptions { timeout: None, follow_redirects: true, accept_invalid_certs: false, client_cert: None, proxy: None, stream: false }
    }
}

impl HttpOptions {
    pub fn new() -> HttpOptions {
        HttpOptions::default()
    }

    /// The most it may take, in milliseconds (a stream: each wait).
    pub fn timeout(mut self, millis: u32) -> HttpOptions {
        self.timeout = Some(millis);
        self
    }

    /// A 3xx is the answer, not followed.
    pub fn no_redirects(mut self) -> HttpOptions {
        self.follow_redirects = false;
        self
    }

    /// A certificate that doesn't check out is taken: a server of yours.
    pub fn accept_invalid_certs(mut self) -> HttpOptions {
        self.accept_invalid_certs = true;
        self
    }

    /// A client certificate and its key, PEM.
    pub fn client_cert(mut self, cert: &str, key: &str) -> HttpOptions {
        self.client_cert = Some(ClientCert { cert: cert.into(), key: key.into() });
        self
    }

    /// By way of an HTTP proxy, `http://host:port`.
    pub fn proxy(mut self, url: &str) -> HttpOptions {
        self.proxy = Some(url.into());
        self
    }

    /// Its body given as it comes ([`HttpPart::Chunk`]).
    pub fn streamed(mut self) -> HttpOptions {
        self.stream = true;
        self
    }
}

impl HttpResponse {
    /// An answer of `status` with `body`, no headers and no timing: what
    /// a test, or a fake of Fenix, gives an extension.
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> HttpResponse {
        HttpResponse { status, headers: Vec::new(), body: body.into(), url: String::new(), timing: HttpTiming { dns: None, connect: None, tls: None, first_byte: 0, total: 0 } }
    }

    /// Its body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl PickerItem {
    /// An item shown as `label`, giving back `value` when picked.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> PickerItem {
        PickerItem { label: label.into(), detail: String::new(), value: value.into() }
    }

    /// With a second, quieter column.
    pub fn detail(mut self, detail: impl Into<String>) -> PickerItem {
        self.detail = detail.into();
        self
    }
}

/// Shows a picker; the choice comes as [`Event::Picked`] with the id it
/// returns.
pub fn pick(title: &str, items: &[PickerItem]) -> u64 {
    host::show_picker(title, items)
}

/// A picker where Tab marks rows; what's marked (or the row Enter was on)
/// comes as [`Event::PickedMany`].
pub fn pick_many(title: &str, items: &[PickerItem]) -> u64 {
    host::show_picker_many(title, items)
}

/// A pane to write in, called `title`, seeded with `initial`; Esc then
/// Enter does `verb`, and the text comes as [`Event::Composed`].
pub fn compose(title: &str, verb: &str, initial: &str, allow_empty: bool) -> u64 {
    host::compose(title, verb, initial, allow_empty)
}

/// Where you were when the command it's running was picked: the file,
/// the selection or the line, its project. Needs `editor = "read"`.
pub fn here() -> Option<Here> {
    host::editor_here()
}

/// Its section on Home; `None` takes it away. A row picked there comes as
/// [`Event::HomePicked`] with its value.
pub fn home(section: Option<&HomeSection>) {
    host::home_set(section)
}

/// The answer to service call `call`, which [`Extension::serve`] said
/// would come later.
pub fn answer(call: u64, result: Result<String, String>) {
    host::service_answer(call, result.as_deref().map_err(String::as_str))
}

/// Asks another extension's, or Fenix's, service `name`; the answer comes
/// as [`Event::ServiceReply`] with the id it returns.
pub fn call_service(name: &str, payload: &str) -> u64 {
    host::call_service(name, payload)
}

/// Pictures for its canvases ([`grid::Grid::picture`]).
pub mod image {
    use super::{host, ImageInfo};

    /// A picture it made: PNG, JPEG, GIF or WebP bytes.
    pub fn load(bytes: &[u8]) -> Result<ImageInfo, String> {
        host::image_load(bytes)
    }

    /// A picture file under `/data` or `/project`.
    pub fn file(path: &str) -> Result<ImageInfo, String> {
        host::image_file(path)
    }

    pub fn free(image: u32) {
        host::image_free(image)
    }
}

/// Asks for a line of text; it comes as [`Event::Prompted`].
pub fn prompt(label: &str, initial: &str) -> u64 {
    host::prompt(label, initial)
}

/// [`prompt`], what's typed shown as dots and kept nowhere by Fenix: a
/// password.
pub fn prompt_secret(label: &str) -> u64 {
    host::prompt_secret(label)
}

/// A secret its manifest asks for (`permissions.secrets`, by key or by
/// pattern: `http.secret.*`).
pub fn secret(key: &str) -> Option<String> {
    host::secret(key)
}

/// Secret `key` kept -- in the system's keychain where there's one -- or
/// with `None`, taken away: a password asked for once. Held to
/// `permissions.secrets` as [`secret`] is.
pub fn set_secret(key: &str, value: Option<&str>) -> Result<(), String> {
    host::secret_set(key, value)
}

/// One of Fenix's environment variables its manifest names
/// (`permissions.env`: `JAVA_HOME`, `JAVA_*`, `${setting:key}`), when it's
/// set.
pub fn env_var(name: &str) -> Option<String> {
    host::env_var(name)
}

/// Says `message` in Fenix's message line.
pub fn notify(message: &str) {
    host::notify(Level::Info, message)
}

/// One of its own settings, as text.
pub fn setting(key: &str) -> Option<String> {
    host::setting(key)
}

/// The editor theme's colours: [`Event::ThemeChanged`] says when they
/// change.
pub fn theme() -> EditorTheme {
    host::theme_colors()
}

/// Eight colours from the editor theme, distinct from each other and
/// readable on its background: for series, groups, matches.
pub fn theme_palette() -> Vec<Rgb> {
    host::theme_palette()
}

/// One of its own settings changed, and saved in `settings.toml`: as
/// text, the way [`setting`] gives it; `None` clears it.
pub fn set_setting(key: &str, value: Option<&str>) -> Result<(), String> {
    host::setting_set(key, value)
}

/// Opens `text` read-only in a buffer called `title`, highlighted as
/// `language` (`json`, `yaml`, `log`).
pub fn open_text(title: &str, text: &str, language: Option<&str>) {
    host::open_text(title, text, language)
}

/// Opens an http(s) URL in the system's browser.
pub fn open_url(url: &str) -> Result<(), String> {
    host::open_url(url)
}

/// What it says about its project, `/project`: Fenix keeps the last of
/// each and uses it there.
pub mod project {
    use super::{host, Check, Command, Health, Section, Task};

    /// The language server for `languages` (`c`, `cpp`) there, in place of
    /// Fenix's own; `None` gives them back.
    pub fn language_server(languages: &[&str], command: Option<&Command>) -> Result<(), String> {
        let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
        host::language_server_set(&languages, command)
    }

    /// A language server there called `name`, for `languages`, beside the
    /// others rather than in place of them -- ruff beside basedpyright --
    /// their answers merged; set again, it starts again; `None` takes it
    /// away.
    pub fn add_language_server(name: &str, languages: &[&str], command: Option<&Command>) -> Result<(), String> {
        let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
        host::language_server_add(name, &languages, command)
    }

    /// The debug adapter for `languages` there (`SPC u u`), in place of
    /// Fenix's own; `None` gives them back. The project's own
    /// (`.fenix/tools.json`) still wins.
    pub fn debug_adapter(languages: &[&str], command: Option<&Command>) -> Result<(), String> {
        let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
        host::debug_adapter_set(&languages, command)
    }

    /// The formatter for `languages` there: Fenix runs it with the text on
    /// its stdin and takes its stdout (`SPC c F`, and saving with
    /// `editor.format_on_save`); `${file}` in its args is the file's path.
    /// `None` takes it away.
    pub fn formatter(languages: &[&str], command: Option<&Command>) -> Result<(), String> {
        let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
        host::formatter_set(&languages, command, false)
    }

    /// [`formatter`], what it wrote taken even when it exits with a
    /// failure, if it wrote anything: ktlint, which fails when it found
    /// what it can't fix.
    pub fn formatter_keeping_output(languages: &[&str], command: Option<&Command>) -> Result<(), String> {
        let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
        host::formatter_set(&languages, command, true)
    }

    /// The debug adapter for `languages` there, over a socket: `command`
    /// started, then 127.0.0.1:`port` connected to -- 0 lets Fenix pick a
    /// free one, which `${port}` in its args becomes (`--port ${port}`).
    /// With no command, one already listening there, which its manifest
    /// lets it reach (`permissions.sockets`). [`debug_adapter`] with
    /// `None` gives them back.
    pub fn debug_adapter_connect(languages: &[&str], command: Option<&Command>, port: u16) -> Result<(), String> {
        let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
        host::debug_adapter_connect(&languages, command, port)
    }

    /// The settings its language server `name` there is given when it
    /// asks (`workspace/configuration`) -- one it added, or a language's
    /// server (`typescript`): an object, each section the server asks for
    /// (`eslint`, `tailwindCSS`) taken from it, over the project's
    /// `.fenix/lsp/<name>.json`. `None` takes them away. Servers running
    /// are told.
    pub fn language_server_settings(name: &str, settings: Option<&serde_json::Value>) -> Result<(), String> {
        host::language_server_settings(name, settings.map(|s| s.to_string()).as_deref())
    }

    /// Its language servers there, started again.
    pub fn restart_language_server() {
        host::language_server_restart()
    }

    /// Its checks on the project doctor's page.
    pub fn doctor(checks: &[Check]) {
        host::doctor_set(checks)
    }

    /// The tasks it offers in the task picker.
    pub fn tasks(tasks: &[Task]) -> Result<(), String> {
        host::tasks_set(tasks)
    }

    impl Check {
        pub fn new(section: Section, label: impl Into<String>, health: Health, detail: impl Into<String>) -> Check {
            Check { section, label: label.into(), health, detail: detail.into(), fix: None }
        }

        /// With a fix row saying `label`, which runs its command `command`.
        pub fn fix(mut self, label: &str, command: &str) -> Check {
            self.fix = Some((label.into(), command.into()));
            self
        }
    }
}

/// Fenix's own process id, which WASI has none of: what Discord's
/// `SET_ACTIVITY` takes.
pub fn process_id() -> u32 {
    host::process_id()
}

/// Debugging: launches it works out as it runs, beside its `launch/*.toml`
/// files, and variables it fills in for them.
pub mod debug {
    use super::{host, LaunchConfig, LaunchRequest, PickerItem};

    /// The launches `SPC u u` offers in its project, in place of those it
    /// gave before: one per CMake target, per Cargo binary.
    pub fn launches(launches: &[LaunchConfig]) -> Result<(), String> {
        host::launches_set(launches)
    }

    /// `${name}` in its launches and launch files, filled in from
    /// `choices`: with one, its value; with several, the one you pick as
    /// the session starts (a target, a process). None takes it away.
    pub fn variable(name: &str, choices: &[PickerItem]) -> Result<(), String> {
        host::launch_variable_set(name, choices)
    }

    impl LaunchConfig {
        /// A launch called `name` for `languages`, of the program the
        /// adapter's attributes name, or of [`LaunchConfig::program`].
        pub fn launch(name: &str, languages: &[&str]) -> LaunchConfig {
            LaunchConfig {
                name: name.into(),
                languages: languages.iter().map(|l| l.to_string()).collect(),
                request: LaunchRequest::Launch,
                program: None,
                args: Vec::new(),
                cwd: None,
                env: Vec::new(),
                attributes: "{}".into(),
                pre_launch_task: None,
            }
        }

        /// One that attaches to a running program (`processId`, a port).
        pub fn attach(name: &str, languages: &[&str]) -> LaunchConfig {
            LaunchConfig { request: LaunchRequest::Attach, ..LaunchConfig::launch(name, languages) }
        }

        /// The program it runs: a path, `${file}`, `/project/build/app`.
        pub fn program(mut self, program: &str) -> LaunchConfig {
            self.program = Some(program.into());
            self
        }

        pub fn arg(mut self, arg: impl Into<String>) -> LaunchConfig {
            self.args.push(arg.into());
            self
        }

        pub fn cwd(mut self, dir: &str) -> LaunchConfig {
            self.cwd = Some(dir.into());
            self
        }

        pub fn env(mut self, key: &str, value: &str) -> LaunchConfig {
            self.env.push((key.into(), value.into()));
            self
        }

        /// The rest of what the adapter takes: an object (`{"type": "lldb"}`).
        pub fn attributes(mut self, attributes: &serde_json::Value) -> LaunchConfig {
            self.attributes = attributes.to_string();
            self
        }

        /// A task run first, by its name in the task picker: the session
        /// starts once it succeeds.
        pub fn pre_launch_task(mut self, task: &str) -> LaunchConfig {
            self.pre_launch_task = Some(task.into());
            self
        }
    }
}

/// Where program `name` (one its manifest names) is: its own tools folder
/// (`/tools`, a few folders down), Fenix's, `PATH`, or one of `hints`
/// (`%LOCALAPPDATA%/Programs/...`, `/tools/x/bin`). A hint
/// `${setting:key}` is the path one of its settings holds, looked at first
///: an interpreter you chose. A hint `./gradlew` is under its
/// project, looked at before `PATH`; on Windows a `.cmd` or `.bat` does
/// too (`gradlew.bat`). The path may then be run.
pub fn find_program(name: &str, hints: &[&str]) -> Option<String> {
    let hints: Vec<String> = hints.iter().map(|h| h.to_string()).collect();
    host::find_program(name, &hints)
}

/// Opens the new-project wizard as `:project-new args` does.
pub fn new_project(args: &str) {
    host::new_project(args)
}

/// Drawing over the editor, with `draw = "over"` in its permissions:
/// sprite sheets, and layers of items Fenix places and animates.
pub mod layer {
    use super::{host, Anchor, Ease, Glide, Item, LayerLevel, ModelInfo, Place, Pose, Style};

    /// A PNG in its own folder, cut into `frame` (width, height) frames.
    pub fn sheet(path: &str, frame: (u32, u32)) -> Result<u32, String> {
        host::sheet_load(path, frame.0, frame.1)
    }

    /// A `.glb` in its own folder: its id, its clips and their lengths.
    pub fn model(path: &str) -> Result<ModelInfo, String> {
        host::model_load(path)
    }

    pub fn open(level: LayerLevel) -> Result<u64, String> {
        host::layer_open(level)
    }

    /// Everything `layer` draws, in place of what it drew.
    pub fn set(layer: u64, items: &[Item]) {
        host::layer_set(layer, items)
    }

    pub fn close(layer: u64) {
        host::layer_close(layer)
    }

    impl Place {
        pub fn new(anchor: Anchor, x: f32, y: f32) -> Place {
            Place { anchor, x, y }
        }
    }

    impl Pose {
        /// Model `model` playing `clip` round, facing you.
        pub fn new(model: u32, clip: &str) -> Pose {
            Pose { model, clip: clip.into(), once: false, speed: 1.0, yaw: 0.0, style: Style::Smooth, parts: Vec::new(), tint: (1.0, 1.0, 1.0), glow: 0.0 }
        }

        /// Played once, then held on its last frame.
        pub fn once(mut self) -> Pose {
            self.once = true;
            self
        }

        pub fn speed(mut self, speed: f32) -> Pose {
            self.speed = speed;
            self
        }

        /// Turned `degrees` about its up axis: 0 faces you, 90 faces right.
        pub fn yaw(mut self, degrees: f32) -> Pose {
            self.yaw = degrees;
            self
        }

        pub fn style(mut self, style: Style) -> Pose {
            self.style = style;
            self
        }

        /// Wearing its optional part `name` (its node is `opt:<name>`).
        pub fn part(mut self, name: &str) -> Pose {
            self.parts.push(name.into());
            self
        }

        /// Its colours multiplied by `rgb`.
        pub fn tint(mut self, rgb: (f32, f32, f32)) -> Pose {
            self.tint = rgb;
            self
        }

        /// Lit from within, 0 to 1.
        pub fn glow(mut self, glow: f32) -> Pose {
            self.glow = glow;
            self
        }
    }

    impl Item {
        /// Item `id`: frame `frame` of `sheet`, a line tall, its foot at
        /// `at`.
        pub fn new(id: u32, sheet: u32, frame: u32, at: Place) -> Item {
            Item { id, sheet, frames: vec![frame], fps: 0, at, size: 1.0, glide: None, flip: false, opacity: 1.0, clickable: false, keep_clear: false, pose: None }
        }

        /// Item `id`: a model posed by `pose`, a line tall, standing at
        /// `at`, drawn again every frame.
        pub fn model(id: u32, pose: Pose, at: Place) -> Item {
            Item { id, sheet: 0, frames: Vec::new(), fps: 0, at, size: 1.0, glide: None, flip: false, opacity: 1.0, clickable: false, keep_clear: false, pose: Some(pose) }
        }

        /// A model's pose drawn again `fps` times a second (0, every
        /// frame): a few a second sits well with `Style::Pixel`.
        pub fn redraws(mut self, fps: u8) -> Item {
            self.fps = fps;
            self
        }

        /// Cycling `frames`, `fps` a second.
        pub fn frames(mut self, frames: &[u32], fps: u8) -> Item {
            self.frames = frames.to_vec();
            self.fps = fps;
            self
        }

        /// `lines` tall.
        pub fn size(mut self, lines: f32) -> Item {
            self.size = lines;
            self
        }

        /// Gliding to `to` over `millis`, at one speed.
        pub fn glide(mut self, to: Place, millis: u32) -> Item {
            self.glide = Some(Glide { to, millis, ease: Ease::Linear, arc: 0.0 });
            self
        }

        /// Its glide eased: `Ease::Smooth` starts and stops gently.
        pub fn ease(mut self, ease: Ease) -> Item {
            if let Some(glide) = &mut self.glide {
                glide.ease = ease;
            }
            self
        }

        /// Its glide bowed up `lines` high halfway: a hop, a throw.
        pub fn arc(mut self, lines: f32) -> Item {
            if let Some(glide) = &mut self.glide {
                glide.arc = lines;
            }
            self
        }

        pub fn flip(mut self, flip: bool) -> Item {
            self.flip = flip;
            self
        }

        pub fn opacity(mut self, opacity: f32) -> Item {
            self.opacity = opacity;
            self
        }

        /// A click on it comes to it (`Event::LayerClicked`).
        pub fn clickable(mut self) -> Item {
            self.clickable = true;
            self
        }

        /// Faded while it's over the caret's line.
        pub fn keep_clear(mut self) -> Item {
            self.keep_clear = true;
            self
        }
    }
}

/// Its terminals (from [`Command::run_in_terminal`]).
pub mod terminal {
    use super::{host, Command};

    /// Stops what runs in it, keeping its pane and what it printed.
    pub fn stop(terminal: u64) {
        host::terminal_stop(terminal)
    }

    /// Starts it again: with `command` in place of what ran before, and
    /// `title`, when given.
    pub fn restart(terminal: u64, command: Option<&Command>, title: Option<&str>) -> Result<(), String> {
        host::terminal_restart(terminal, command, title)
    }
}

/// The time where you are. Inside the sandbox there's no time zone (it's
/// UTC), so the machine's comes from Fenix.
pub mod clock {
    use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};

    /// This machine's offset from UTC at `utc`.
    pub fn offset_at(utc: DateTime<Utc>) -> FixedOffset {
        #[cfg(target_arch = "wasm32")]
        let east = super::host::utc_offset(utc.timestamp());
        #[cfg(not(target_arch = "wasm32"))]
        let east = chrono::Local.timestamp_opt(utc.timestamp(), 0).earliest().map(|t| chrono::Offset::fix(t.offset()).local_minus_utc()).unwrap_or(0);
        FixedOffset::east_opt(east).unwrap_or(FixedOffset::east_opt(0).unwrap())
    }

    /// Now, where you are.
    pub fn now() -> DateTime<FixedOffset> {
        local(Utc::now())
    }

    /// `utc`, where you are.
    pub fn local(utc: DateTime<Utc>) -> DateTime<FixedOffset> {
        utc.with_timezone(&offset_at(utc))
    }

    /// A wall-clock time where you are, as an instant.
    pub fn from_local(naive: NaiveDateTime) -> DateTime<FixedOffset> {
        // The offset at about that time: right but within an hour of a
        // change of clocks.
        let guess = offset_at(naive.and_utc());
        let offset = offset_at(naive.and_utc() - chrono::Duration::seconds(i64::from(guess.local_minus_utc())));
        offset.from_local_datetime(&naive).earliest().unwrap_or_else(|| guess.from_utc_datetime(&naive))
    }
}

/// Puts `text` on the clipboard.
pub fn copy(text: &str) {
    host::clipboard_set(text)
}

/// Puts a picture (a PNG) on the clipboard.
pub fn copy_image(png: &[u8]) -> Result<(), String> {
    host::clipboard_set_image(png)
}

/// The clipboard's picture, as a PNG, when it holds one.
pub fn pasted_image() -> Option<Vec<u8>> {
    host::clipboard_image()
}

/// The buffers you edit: their text, and changes to it -- with
/// `editor = "write"` (`read` to read them).
pub mod buffer {
    use super::{host, TextEdit};

    /// The text of the buffer open on `path`, saved or not: under
    /// `/project` or one of its folders, a full path Fenix gave it, or any
    /// full path you have open (not one sealed, [`crate::host::open_file`]).
    pub fn text(path: &str) -> Option<String> {
        host::buffer_text(path)
    }

    /// `edits` to the buffer on `path`, as one change you can undo.
    pub fn edit(path: &str, edits: &[TextEdit]) -> Result<(), String> {
        host::buffer_edit(path, edits)
    }

    /// `text` typed where your cursor is.
    pub fn insert(text: &str) -> Result<(), String> {
        host::buffer_insert(text)
    }

    pub fn save(path: &str) -> Result<(), String> {
        host::buffer_save(path)
    }

    /// `buffer` highlighted as `language` -- Fenix's (`json`) or a
    /// grammar's an extension brings (`kotlin`) -- or, with `None`, as its
    /// file's name says again. A file's needs `editor = "read"`.
    pub fn set_language(buffer: &super::TextBuffer, language: Option<&str>) -> Result<(), String> {
        host::buffer_language_set(buffer, language)
    }

    /// The buffer on `path`'s last change undone: yours or one it made --
    /// one history, whether the file's page or its text was edited. Needs
    /// `editor = "write"`.
    pub fn undo(path: &str) -> Result<(), String> {
        host::buffer_undo(path)
    }

    /// The buffer on `path`'s last undone change done again.
    pub fn redo(path: &str) -> Result<(), String> {
        host::buffer_redo(path)
    }

    impl TextEdit {
        /// `text` in place of what's from `start` to `end`, each a (line,
        /// column) from 0, in characters.
        pub fn new(start: (u32, u32), end: (u32, u32), text: impl Into<String>) -> TextEdit {
            TextEdit { start, end, text: text.into() }
        }

        /// `text` put in at `at`.
        pub fn insert(at: (u32, u32), text: impl Into<String>) -> TextEdit {
            TextEdit { start: at, end: at, text: text.into() }
        }
    }
}

/// Its item in the modeline while it runs; `None` takes it away.
pub fn status(text: Option<&str>) {
    host::status_set(text)
}

/// The clipboard's text, with `clipboard = true` in its permissions.
pub fn pasted_text() -> Option<String> {
    host::clipboard_text()
}

/// A file's bytes (at most 64 MiB): under `/data`, `/project` or one of its
/// folders, or a full path Fenix gave it -- [`Event::Opened`], [`here`],
/// [`explorer_selected`], a forge's repository.
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    host::file_read(path)
}

/// Its decorations of a buffer's text: inlays, swatches, hidden text,
/// tints, underlines, drawn with it and moved with it as it's edited.
pub mod decorations {
    use super::{host, Decoration, DecorationLook, DecorationRule, Rgb, Role, Shade, TextBuffer, VirtualText};

    /// Its decorations of `buffer`, in place of those it set before; none
    /// takes them away. A file's needs `editor = "read"`; one of its
    /// scratch buffers' ([`crate::scratch`]), nothing.
    pub fn set(buffer: &TextBuffer, decorations: &[Decoration]) -> Result<(), String> {
        host::decorations_set(buffer, decorations)
    }

    /// The buffer open on file `path`.
    pub fn file(path: impl Into<String>) -> TextBuffer {
        TextBuffer::File(path.into())
    }

    /// Its rules for decorating files, in place of those it gave before:
    /// Fenix applies them to a file's buffer from before it's first drawn,
    /// and again as it's edited -- `.env` values masked from the start.
    /// With `editor = "read"`; at most 64.
    pub fn rules(rules: &[DecorationRule]) -> Result<(), String> {
        host::decoration_rules_set(rules)
    }

    impl DecorationRule {
        /// Where `pattern` (a regular expression) matches a line of a file
        /// named as one of `files` (`.env`, `.env.*`), all of the match
        /// looks like `look`.
        pub fn new(files: &[&str], pattern: &str, look: DecorationLook) -> DecorationRule {
            DecorationRule { files: files.iter().map(|f| f.to_string()).collect(), pattern: pattern.into(), group: 0, look }
        }

        /// Only group `group` of the match.
        pub fn group(mut self, group: u32) -> DecorationRule {
            self.group = group;
            self
        }
    }

    impl Decoration {
        /// `text` drawn before the character at `at` (a (line, column)
        /// from 0, in characters), in `role`'s colour, taking room.
        pub fn before(at: (u32, u32), text: impl Into<String>, role: Role) -> Decoration {
            Decoration { start: at, end: at, look: DecorationLook::Text(VirtualText { text: text.into(), role, after: false }), id: None }
        }

        /// The same, after the character at `at`.
        pub fn after(at: (u32, u32), text: impl Into<String>, role: Role) -> Decoration {
            Decoration { start: at, end: at, look: DecorationLook::Text(VirtualText { text: text.into(), role, after: true }), id: None }
        }

        /// A block of `color` before the character at `at`.
        pub fn swatch(at: (u32, u32), color: Rgb) -> Decoration {
            Decoration { start: at, end: at, look: DecorationLook::Swatch(color), id: None }
        }

        /// It, a click on it coming back to
        /// [`crate::Extension::decoration_clicked`] with `id`.
        pub fn tagged(mut self, id: u32) -> Decoration {
            self.id = Some(id);
            self
        }

        /// The text from `start` to `end` drawn as `with`, but as it is
        /// while the cursor is on its line.
        pub fn hide(start: (u32, u32), end: (u32, u32), with: impl Into<String>) -> Decoration {
            Decoration { start, end, look: DecorationLook::Hide(with.into()), id: None }
        }

        /// The text from `start` to `end` on `shade`.
        pub fn tint(start: (u32, u32), end: (u32, u32), shade: Shade) -> Decoration {
            Decoration { start, end, look: DecorationLook::Tint(shade), id: None }
        }

        /// The text from `start` to `end` underlined in `role`'s colour.
        pub fn underline(start: (u32, u32), end: (u32, u32), role: Role) -> Decoration {
            Decoration { start, end, look: DecorationLook::Underline(role), id: None }
        }
    }
}

/// What's selected in the explorer, as a full path (`editor = "read"`):
/// it may then read it ([`read_file`]).
pub fn explorer_selected() -> Option<String> {
    host::explorer_selected()
}

/// Where the cursor and the selection were when the command it's running
/// was picked, with [`here`]: (line, column), from 0, in characters.
/// Needs `editor = "read"`.
pub fn cursor() -> Option<CursorInfo> {
    host::editor_cursor()
}

/// The files in folder `path` it may read: all of a folder Fenix gave
/// it; in the folder of a file it was given ([`Event::Opened`]), those it
/// opens itself too (`contributes.opens`) -- the next picture -- which it
/// may then read. Sorted by name.
pub fn folder_list(path: &str) -> Result<Vec<FolderEntry>, String> {
    host::folder_list(path)
}

/// Plays a WAV of its own -- in its folder (`sounds/chime.wav`) or under
/// `/data` -- once, while it carries on. Needs `sound = true` in its
/// permissions.
pub fn play_sound(path: &str) -> Result<(), String> {
    host::play_sound(path)
}

/// Scratch buffers of its own: text you edit, never saved. Your edits come
/// to [`Extension::buffer_changed`] as [`Changed::Scratch`], their closing
/// to [`Extension::scratch_closed`].
pub mod scratch {
    use super::host;

    /// Opens one in the focused pane, called `title`, holding `text`,
    /// highlighted as `language` (`md`, `json`); its id comes back.
    pub fn open(title: &str, text: &str, language: Option<&str>) -> u64 {
        host::scratch_open(title, text, language)
    }

    /// Opens one in a pane beside the focused one, `width` of it -- the
    /// same pane each time while it's there -- the focus staying.
    pub fn open_beside(title: &str, text: &str, language: Option<&str>, width: f32) -> u64 {
        host::scratch_open_beside(title, text, language, width)
    }

    /// Its text as it is now; `None` once it's closed.
    pub fn text(scratch: u64) -> Option<String> {
        host::scratch_text(scratch)
    }

    /// Its text replaced with `text`, as one change you can undo.
    pub fn set(scratch: u64, text: &str) -> Result<(), String> {
        host::scratch_set(scratch, text)
    }

    pub fn close(scratch: u64) {
        host::scratch_close(scratch)
    }
}

/// [`open_text`], in the pane beside the focused one that
/// [`scratch::open_beside`] opens in.
pub fn open_text_beside(title: &str, text: &str, language: Option<&str>, width: f32) {
    host::open_text_beside(title, text, language, width)
}

/// The programs it started ([`Command::spawn`]).
pub mod process {
    use super::host;

    /// Bytes for its stdin.
    pub fn write(process: u64, data: &[u8]) -> Result<(), String> {
        host::write_stdin(process, data)
    }

    /// Its stdin closed, so it reads it to its end.
    pub fn close_stdin(process: u64) {
        host::close_stdin(process)
    }

    pub fn kill(process: u64) {
        host::kill(process)
    }
}

/// Problems it found in a file -- a linter's -- shown with the language
/// servers'.
pub mod diagnostics {
    use super::{host, Problem, QuickFix, Severity, TextEdit};

    /// Its problems in the file on `path` (under `/project`, or a full path
    /// with `editor = "read"`), in place of those it gave before; none
    /// takes them away. Their fixes ([`Problem::fix`]) are offered in the
    /// code action list (`SPC c a`) on their lines while the file is as it
    /// was.
    pub fn set(path: &str, problems: &[Problem]) -> Result<(), String> {
        host::diagnostics_set(path, problems)
    }

    impl Problem {
        /// From (line, column) up to another, from 0 and in characters.
        pub fn new(severity: Severity, start: (u32, u32), end: (u32, u32), message: impl Into<String>) -> Problem {
            Problem { start, end, severity, message: message.into(), source: None, code: None, fixes: Vec::new() }
        }

        pub fn error(start: (u32, u32), end: (u32, u32), message: impl Into<String>) -> Problem {
            Problem::new(Severity::Error, start, end, message)
        }

        pub fn warning(start: (u32, u32), end: (u32, u32), message: impl Into<String>) -> Problem {
            Problem::new(Severity::Warning, start, end, message)
        }

        /// What found it (`ruff`), shown with it.
        pub fn source(mut self, source: &str) -> Problem {
            self.source = Some(source.into());
            self
        }

        /// Its rule (`E501`).
        pub fn code(mut self, code: &str) -> Problem {
            self.code = Some(code.into());
            self
        }

        /// A fix, `title`, its `edits` to the problem's file.
        pub fn fix(mut self, title: &str, edits: Vec<TextEdit>) -> Problem {
            self.fixes.push(QuickFix { title: title.into(), edits });
            self
        }
    }
}

/// Its tools folder, `/tools`: there when `permissions.process` names a
/// program in it (`/tools/*`). A server or an adapter is downloaded there
/// ([`HttpRequest::download`]), unpacked, made to run, and found
/// ([`find_program`]) or run by its path.
pub mod tools {
    use super::host;

    /// A file under `/tools` made one that runs (Linux, macOS).
    pub fn set_executable(path: &str) -> Result<(), String> {
        host::set_executable(path)
    }
}

/// Coding agents: one it brings, and the ones it asks.
///
/// ```no_run
/// use fenix_extension_api::{agents, run};
///
/// run::spawn(async {
///     match agents::ask("Name this branch's change in five words.", None).await {
///         Ok(answer) => fenix_extension_api::notify(&answer.text),
///         Err(e) => fenix_extension_api::notify(&e),
///     }
/// });
/// ```
pub mod agents {
    use super::{host, Command};

    /// The agent its manifest names `id` (`contributes.agents`), started
    /// by `command`: an ACP server on stdio, often one it installed in its
    /// tools folder. `None` takes it away.
    pub fn provide(id: &str, command: Option<&Command>) -> Result<(), String> {
        host::agent_provide(id, command)
    }

    /// What an agent answered (`agents.ask`).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Answer {
        /// What it said, as text.
        pub text: String,
        /// Why its turn ended: `end_turn`, `cancelled`, `max_tokens`,
        /// `max_turn_requests`, `refusal`.
        pub stop_reason: String,
        /// The agent that answered.
        pub agent: String,
    }

    /// `agents.ask`'s payload: the prompt, and the agent by id (`None`
    /// for the project's default one, `agents.default`).
    pub fn payload(prompt: &str, agent: Option<&str>) -> String {
        let mut p = serde_json::json!({ "prompt": prompt });
        if let Some(agent) = agent {
            p["agent"] = serde_json::Value::from(agent);
        }
        p.to_string()
    }

    /// `agents.ask`'s answer, read.
    pub fn answer(text: &str) -> Result<Answer, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("agents.ask: {e}"))?;
        let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        Ok(Answer { text: field("text"), stop_reason: field("stopReason"), agent: field("agent") })
    }

    /// Asks an agent once, in a thread of its own in the project, and
    /// waits for its answer (needs `permissions.agents`). It may read the
    /// project, not change it: whatever it asks permission for is refused.
    pub async fn ask(prompt: &str, agent: Option<&str>) -> Result<Answer, String> {
        answer(&crate::run::call("agents.ask", &payload(prompt, agent)).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_is_built() {
        let p = Problem::warning((1, 0), (1, 4), "unused").source("ruff").code("F401");
        assert_eq!((p.severity, p.start, p.source.as_deref(), p.code.as_deref()), (Severity::Warning, (1, 0), Some("ruff"), Some("F401")));
    }

    #[test]
    fn a_problem_takes_fixes_and_a_launch_is_built() {
        let edit = TextEdit { start: (1, 0), end: (1, 4), text: String::new() };
        let p = Problem::warning((1, 0), (1, 4), "unused").code("F401").fix("Remove the import", vec![edit.clone()]).fix("Ignore", vec![edit]);
        assert_eq!((p.code.as_deref(), p.fixes.len(), p.fixes[0].title.as_str()), (Some("F401"), 2, "Remove the import"));
        let l = LaunchConfig::launch("app", &["cpp"]).program("/project/build/app").arg("-v").env("X", "1").attributes(&serde_json::json!({"type": "lldb"})).pre_launch_task("cmake: build app");
        assert_eq!((l.request, l.program.as_deref(), l.args.len(), l.attributes.as_str(), l.pre_launch_task.as_deref()), (LaunchRequest::Launch, Some("/project/build/app"), 1, r#"{"type":"lldb"}"#, Some("cmake: build app")));
        assert_eq!(LaunchConfig::attach("pid", &["go"]).request, LaunchRequest::Attach);
    }

    #[test]
    fn a_page_is_built_a_line_at_a_time() {
        let p = Page::new().line([page::title("Containers"), page::muted("  2")]).rule().line([page::good("web")]).focus(2).key("s", "stop");
        assert_eq!(p.len(), 3);
        assert_eq!(p.lines[0].spans[1], Span { text: "  2".into(), role: Role::Muted });
        assert!(p.lines[1].rule);
        assert_eq!(p.focus, Some(2));
        assert_eq!(p.keys, [KeyHint { keys: "s".into(), what: "stop".into() }]);
        let p = p.popup(2, [vec![page::accent("s"), page::text(" stop")]]);
        assert_eq!(p.popup.as_ref().map(|m| (m.line, m.rows[0].len())), Some((2, 2)));
    }

    #[test]
    fn commands_and_requests_are_built() {
        let c = Command::new("docker").arg("ps").args(["-a", "--format", "json"]).cwd("/project").env("X", "1");
        assert_eq!((c.program.as_str(), c.args.len(), c.cwd.as_deref()), ("docker", 4, Some("/project")));
        let r = HttpRequest::post("https://api.example.com/x", "{}").header("Content-Type", "application/json");
        assert_eq!((r.method.as_str(), r.body.as_slice(), r.headers.len()), ("POST", &b"{}"[..], 1));
        assert_eq!(PickerItem::new("Red", "red").detail("#f00").detail, "#f00");
    }

    #[derive(Default)]
    struct Counting {
        n: u32,
    }

    impl Extension for Counting {}

    /// One instance, made once, kept between calls.
    #[test]
    fn an_agents_question_and_its_answer_are_json() {
        assert_eq!(agents::payload("hi", None), r#"{"prompt":"hi"}"#);
        assert!(agents::payload("hi", Some("codex")).contains(r#""agent":"codex""#));
        let a = agents::answer(r#"{"text":"Done.","stopReason":"end_turn","agent":"claude","thread":"t1"}"#).unwrap();
        assert_eq!((a.text.as_str(), a.stop_reason.as_str(), a.agent.as_str()), ("Done.", "end_turn", "claude"));
        assert!(agents::answer("nope").is_err());
    }

    #[test]
    fn the_instance_is_kept() {
        with::<Counting, _>(|c| c.n += 1);
        with::<Counting, _>(|c| c.n += 1);
        assert_eq!(with::<Counting, _>(|c| c.n), 2);
        assert_eq!(with::<Counting, _>(|c| c.run_command("x")), Err("no command x".into()));
    }
}
