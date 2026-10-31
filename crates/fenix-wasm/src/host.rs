//! Running one extension: its component compiled (once, then loaded from
//! the cache), instantiated in a store of its own, on a worker thread of
//! its own. Fenix sends it calls; what it says, what it asks of Fenix and
//! how each call ended come back as `Event`s. Nothing here runs on
//! Fenix's UI thread.
//!
//! What it may do is held to its manifest's `[permissions]` here, in the
//! host: programs by name, HTTP by host, secrets by key, files under
//! `/data` (its own) and, if it asks, `/project`. What takes time --
//! processes, HTTP, timers -- runs on threads of its own and comes back to
//! the extension as events, given to its `handle` on the worker.
//!
//! A trap -- a deadline missed, the stack or memory exhausted, a panic in
//! the guest -- ends that call with an error and stops the extension: its
//! state can't be trusted after one. Its processes are killed and later
//! calls refused with why, until it's started again.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use fenix_extensions::manifest::{Filesystem, ListenRule, Permissions, SocketRule};
use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits};
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::p2::{OutputStream, Pollable, StreamResult};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// What `wit/world.wit` makes: the extension's exports, the host's
/// imports, and their types.
mod bindings {
    // Every export of 1.0 is required. What a 1.x adds is looked up when
    // it's there (`get_typed_func`), so a guest built for 1.0 still loads.
    wasmtime::component::bindgen!({ path: "../fenix-extension-api/wit", world: "extension", additional_derives: [PartialEq] });
}

use bindings::{fenix, Extension};
pub use bindings::fenix::extension::types::{
    Anchor, Canvas, CanvasPopup, CellSpan, Cells, Check, Command, Ease, Event as Notice, Glide, Health, Here, HomeMark, HomeRow, HomeSection, HttpRequest, ImageInfo, Mark, TextEdit, EditorTheme, HttpResponse, Item, Key,
    KeyHint, LayerLevel, Layout, Level, Line, ModelInfo, Page, PaneBox, PickerItem, Picture, Place, Popup, Pose, ProcessOutput, ProjectInfo, Role, Section, Span, Style, Task,
    CursorImage, Datagram, KeyEvent, Modifiers, NamedKey, PointerEvent, SurfaceInput, SurfaceKey, SurfaceMark, SurfaceOptions, SurfaceScale, SurfaceSize, WheelEvent,
    Changed, CursorInfo, Focus, FocusedPage, FolderEntry,
    CellStyle, Decoration, DecorationLook, KeyMods, KeyPress, MouseAction, MouseButton, MouseEvent, MousePress, Rgb, Shade, TextBuffer, VerticalRule, VirtualText,
    Problem, Severity,
    ClientCert, DecorationRule, HttpOptions, HttpPart, HttpTiming, PlatformInfo, TlsOptions,
    CellColor, TextView,
    AgentTurn, DebugSession, DebugState, LaunchConfig, LaunchRequest, QuickFix,
};

/// The most pixels a surface may have (4096 x 4096), and either side.
pub const MAX_SURFACE: u32 = 4096;

/// The most a socket may have brought that its extension hasn't taken yet:
/// past it, its reading waits.
const MAX_UNREAD: usize = 8 << 20;

/// The most `file-read` gives.
pub const MAX_FILE: u64 = 64 << 20;

/// The most a sound (`play-sound`) may be.
pub const MAX_SOUND: u64 = 8 << 20;

/// The most files `folder-list` gives.
const MAX_LISTED: usize = 10_000;

/// The most decorations one `decorations-set` gives a buffer.
pub const MAX_DECORATIONS: usize = 10_000;

/// The most rules `decoration-rules-set` gives.
pub const MAX_RULES: usize = 64;

/// The most tasks of its own `run-task-background` runs at once.
pub const MAX_BACKGROUND: usize = 4;

/// The most problems one `diagnostics-set` gives a file.
pub const MAX_PROBLEMS: usize = 10_000;

/// The most launches `launches-set` gives, and choices a launch variable
/// has.
pub const MAX_LAUNCHES: usize = 256;

/// The most `http-download` writes.
const MAX_DOWNLOAD: u64 = 2 << 30;

/// A rectangle of a surface's pixels: x, y, width, height.
pub type Rect = (u32, u32, u32, u32);

fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some((x, y, w, h)) => {
            let (x0, y0) = (x.min(b.0), y.min(b.1));
            let (x1, y1) = ((x + w).max(b.0 + b.2), (y + h).max(b.1 + b.3));
            (x0, y0, x1 - x0, y1 - y0)
        }
    }
}

/// A surface's pixels, BGRA. The extension writes to its back buffer, on
/// its worker; `surface-present` copies what changed to the front, which
/// is what the UI uploads -- so a frame half written is never shown.
#[derive(Debug, Default)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    back: Vec<u8>,
    front: Vec<u8>,
    front_size: (u32, u32),
    /// Written in the back since the last present.
    pending: Option<Rect>,
    /// Sized again since the last present: all of it goes to the front.
    resized: bool,
    /// Presented and not yet taken by the UI.
    dirty: Option<Rect>,
    /// The pointer's picture, and whether it changed since the UI took it.
    cursor: Option<CursorImage>,
    cursor_changed: bool,
    marks: Vec<SurfaceMark>,
    /// The UI was told to look, and hasn't yet.
    woken: bool,
}

impl Framebuffer {
    /// The size of what's shown.
    pub fn size(&self) -> (u32, u32) {
        self.front_size
    }

    /// What's shown, BGRA, row after row.
    pub fn pixels(&self) -> &[u8] {
        &self.front
    }

    /// What was presented since the UI last looked: the rectangle to
    /// upload. The UI's next look will be asked for again.
    pub fn take_dirty(&mut self) -> Option<Rect> {
        self.woken = false;
        self.dirty.take()
    }

    /// The pointer's picture, when it changed since the last time.
    pub fn take_cursor(&mut self) -> Option<Option<CursorImage>> {
        std::mem::take(&mut self.cursor_changed).then(|| self.cursor.clone())
    }

    pub fn marks(&self) -> &[SurfaceMark] {
        &self.marks
    }

    /// `rect` of what's shown into `out`, tightly packed.
    pub fn copy_region(&self, (x, y, w, h): Rect, out: &mut Vec<u8>) {
        out.clear();
        let stride = self.front_size.0 as usize * 4;
        for row in y..y + h {
            let start = row as usize * stride + x as usize * 4;
            out.extend_from_slice(&self.front[start..start + w as usize * 4]);
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width > MAX_SURFACE || height > MAX_SURFACE {
            return Err(format!("a surface is at most {MAX_SURFACE} x {MAX_SURFACE}"));
        }
        self.width = width;
        self.height = height;
        self.back = vec![0; width as usize * height as usize * 4];
        // Opaque black.
        for px in self.back.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        self.resized = true;
        self.pending = None;
        Ok(())
    }

    fn inside(&self, (x, y, w, h): Rect) -> Result<(), String> {
        if u64::from(x) + u64::from(w) > u64::from(self.width) || u64::from(y) + u64::from(h) > u64::from(self.height) {
            return Err(format!("({x}, {y}) {w} x {h} isn't inside the surface's {} x {}", self.width, self.height));
        }
        Ok(())
    }

    fn written(&mut self, rect: Rect) {
        if rect.2 > 0 && rect.3 > 0 {
            self.pending = Some(union(self.pending, rect));
        }
    }

    fn write(&mut self, rect: Rect, bgra: &[u8]) -> Result<(), String> {
        self.inside(rect)?;
        let (x, y, w, h) = rect;
        if bgra.len() != w as usize * h as usize * 4 {
            return Err(format!("{w} x {h} pixels are {} bytes, not {}", w as usize * h as usize * 4, bgra.len()));
        }
        let stride = self.width as usize * 4;
        for row in 0..h as usize {
            let start = (y as usize + row) * stride + x as usize * 4;
            self.back[start..start + w as usize * 4].copy_from_slice(&bgra[row * w as usize * 4..(row + 1) * w as usize * 4]);
        }
        self.written(rect);
        Ok(())
    }

    fn fill(&mut self, rect: Rect, (r, g, b): (u8, u8, u8)) -> Result<(), String> {
        self.inside(rect)?;
        let (x, y, w, h) = rect;
        let stride = self.width as usize * 4;
        for row in y as usize..(y + h) as usize {
            for px in self.back[row * stride + x as usize * 4..row * stride + (x + w) as usize * 4].as_chunks_mut::<4>().0 {
                px.copy_from_slice(&[b, g, r, 255]);
            }
        }
        self.written(rect);
        Ok(())
    }

    fn copy(&mut self, from: Rect, to: (u32, u32)) -> Result<(), String> {
        self.inside(from)?;
        let (x, y, w, h) = from;
        self.inside((to.0, to.1, w, h))?;
        let stride = self.width as usize * 4;
        // Rows in the order that never reads one already overwritten.
        let rows: Vec<usize> = if to.1 > y { (0..h as usize).rev().collect() } else { (0..h as usize).collect() };
        for row in rows {
            let src = (y as usize + row) * stride + x as usize * 4;
            let dst = (to.1 as usize + row) * stride + to.0 as usize * 4;
            self.back.copy_within(src..src + w as usize * 4, dst);
        }
        self.written((to.0, to.1, w, h));
        Ok(())
    }

    /// What was written goes to the front. Whether the UI should be told.
    pub fn present(&mut self) -> bool {
        if self.resized {
            self.front = self.back.clone();
            self.front_size = (self.width, self.height);
            self.resized = false;
            self.pending = None;
            self.dirty = Some((0, 0, self.width, self.height));
        } else if let Some(rect) = self.pending.take() {
            let (x, y, w, h) = rect;
            let stride = self.width as usize * 4;
            for row in y as usize..(y + h) as usize {
                let span = row * stride + x as usize * 4..row * stride + (x + w) as usize * 4;
                self.front[span.clone()].copy_from_slice(&self.back[span]);
            }
            self.dirty = Some(union(self.dirty, rect));
        } else {
            return false;
        }
        !std::mem::replace(&mut self.woken, true)
    }

    /// Something besides the pixels changed. Whether the UI should be told.
    fn changed(&mut self) -> bool {
        !std::mem::replace(&mut self.woken, true)
    }
}

/// A surface's pixels, shared by its extension's worker and the UI.
#[derive(Debug, Clone, Default)]
pub struct SharedSurface(pub Arc<Mutex<Framebuffer>>);

impl PartialEq for SharedSurface {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl SharedSurface {
    /// Its framebuffer, locked; a lock a panic left is taken anyway.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Framebuffer> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The most a sprite sheet may be, in bytes.
pub const MAX_SHEET: u64 = 1 << 20;

/// The most an image (`image-load`, `image-file`) may be, in bytes.
pub const MAX_IMAGE: u64 = 16 << 20;

/// A model an extension loaded, read once on its worker and shared with
/// the UI: the same model is the same `Arc`.
#[derive(Debug, Clone)]
pub struct Shared(pub Arc<fenix_model::Model>);

impl PartialEq for Shared {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A program, already held to its permissions: what to start, its
/// arguments, and where.
pub type Checked = (String, Vec<String>, Option<PathBuf>);

/// A program's environment, as it was given: name, value.
pub type Env = Vec<(String, String)>;

/// Where a guest's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Log(Level),
    Stdout,
    Stderr,
}

/// Where the UI answers a question a worker waits on (`buffer-text`,
/// `clipboard-image`): once, or not at all if it can't.
#[derive(Debug, Clone)]
pub struct Answer<T>(Arc<Mutex<Option<mpsc::Sender<T>>>>);

impl<T> Answer<T> {
    /// Where an answer goes: `tx`.
    pub fn new(tx: mpsc::Sender<T>) -> Answer<T> {
        Answer(Arc::new(Mutex::new(Some(tx))))
    }

    /// The answer; a second one goes nowhere.
    pub fn send(&self, value: T) {
        if let Some(tx) = self.0.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(value);
        }
    }
}

impl<T> PartialEq for Answer<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// How long a worker waits for the UI's answer.
const UI_ANSWER: Duration = Duration::from_secs(2);

/// What an extension asks of Fenix: the things only the UI can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Notify(Level, String),
    RunCommand(String),
    /// A file opened: `sealed` when it's no file it may reach, opened for
    /// you to see -- its buffer, if it's opened now, doesn't reach the
    /// extensions (`open_only`). `text`: what's in it, read as it was
    /// checked (one it may reach, and text), for its buffer when one's made
    /// now -- not read again where a link could have taken its place.
    OpenFile { path: PathBuf, line: Option<u32>, sealed: bool, text: Option<String> },
    /// Answer with `Notice::Picked((id, choice))`.
    Picker { id: u64, title: String, items: Vec<PickerItem> },
    /// Answer with `Notice::PickedMany((id, marked))`.
    PickerMany { id: u64, title: String, items: Vec<PickerItem> },
    /// Answer with `Notice::Prompted((id, text))`. What's typed is shown
    /// as dots, and kept nowhere, when it's `masked` (`prompt-secret`,
    ///).
    Prompt { id: u64, label: String, initial: String, masked: bool },
    /// A compose pane; answer with `Notice::Composed((id, text))`.
    Compose { id: u64, title: String, verb: String, initial: String, allow_empty: bool },
    /// Its section on Home, or none.
    Home(Option<HomeSection>),
    /// One of its own settings, changed: save it (already in its map).
    SettingSet { key: String, value: Option<String> },
    /// A picture of its making, as image `id`.
    ImageLoad { id: u32, bytes: Vec<u8> },
    ImageFree(u32),
    /// The answer to service call `call` (`Call::Serve`), come later.
    ServiceAnswer { call: u64, answer: Result<String, String> },
    PageOpen { page: u64, kind: String, title: String },
    /// A page in a new pane beside the focused one, `width` of it.
    PageOpenBeside { page: u64, kind: String, title: String, width: f32 },
    PageRefresh(u64),
    PageClose(u64),
    /// A program in a terminal pane, already checked against its
    /// permissions: in a new pane under the focused one when `below`.
    /// Its end comes back as `Notice::TerminalClosed(id)`.
    Terminal { id: u64, program: String, args: Vec<String>, cwd: Option<PathBuf>, title: String, below: bool },
    /// What runs in its terminal `id`, stopped; the pane stays.
    TerminalStop(u64),
    /// Its terminal `id` started again: with this program when given.
    TerminalRestart { id: u64, program: Option<(String, Vec<String>, Option<PathBuf>)>, title: Option<String> },
    /// A program as a task, already checked. Its end comes back as
    /// `Notice::TaskFinished((id, success))`.
    Task { id: u64, name: String, program: String, args: Vec<String>, cwd: Option<PathBuf>, env: Vec<(String, String)> },
    /// Its project's language server for `languages` (with its
    /// environment), or none.
    LanguageServer { languages: Vec<String>, program: Option<(Checked, Env)> },
    /// Its project's debug adapter for `languages` (with its environment,
    ///), or none.
    DebugAdapter { languages: Vec<String>, program: Option<(Checked, Env)> },
    /// Its project's formatter for `languages`, or none; what it wrote
    /// taken even when it fails, when `keep_output`.
    Formatter { languages: Vec<String>, program: Option<Checked>, keep_output: bool },
    /// A coding agent of its manifest's (`contributes.agents`), as the
    /// command line that starts it, or taken away (`agent-provide`).
    AgentProvide { id: String, program: Option<(Checked, Env)> },
    /// Its problems in the file on `path` -- held as `BufferText`'s is
    /// when `open_only` -- in place of those it gave before; each one's
    /// fixes, when it gave them (`diagnostics-set-ex`), by its place in
    /// `problems`.
    Diagnostics { path: PathBuf, open_only: bool, problems: Vec<Problem> },
    LanguageServerRestart,
    /// Its checks for the project doctor.
    Doctor(Vec<Check>),
    /// The tasks it offers, each already checked, with its environment
    /// (`${tools}` filled in).
    Tasks(Vec<(String, Checked, Env)>),
    /// Where it found a program (`find-program`), for Fenix's own probes.
    Found { name: String, path: PathBuf },
    /// The new-project wizard, with `:project-new`'s arguments.
    NewProject(String),
    /// A PNG of its own (checked: in its folder, not too big, and read as
    /// it was), cut into frames, as sheet `id`.
    SheetLoad { id: u32, bytes: Vec<u8>, frame: (u32, u32) },
    /// A model of its own (checked: in its folder, read, posable), as
    /// model `id`.
    ModelLoad { id: u32, model: Shared },
    LayerOpen { id: u64, level: LayerLevel },
    LayerSet { layer: u64, items: Vec<Item> },
    LayerClose(u64),
    /// Answer with `Notice::ServiceReply((id, result))`.
    Service { id: u64, name: String, payload: String },
    /// Text in a read-only buffer, highlighted as `language`: in its pane
    /// beside the focused one, `beside` of it, when that's given.
    OpenText { title: String, text: String, language: Option<String>, beside: Option<f32> },
    /// An http(s) URL, already checked, for the system's browser.
    OpenUrl(String),
    Clipboard(String),
    ClipboardImage(Vec<u8>),
    /// The clipboard's picture, as a PNG: answered on `answer`.
    ClipboardImageRead(Answer<Option<Vec<u8>>>),
    /// The text of the buffer on `path`, if one has it. `open_only`: a
    /// path it may not reach itself, so only a buffer you have open on it
    /// -- not one sealed (`OpenFile`).
    BufferText { path: PathBuf, open_only: bool, answer: Answer<Option<String>> },
    /// Edits to the buffer on `path`, opened if need be -- not when
    /// `open_only`, held as `BufferText`'s is. `text`: the file's, read as
    /// it was checked, for the buffer if it's opened now (`OpenFile`'s).
    BufferEdit { path: PathBuf, open_only: bool, edits: Vec<TextEdit>, text: Option<String> },
    /// Text typed at the cursor, in the focused buffer.
    BufferInsert(String),
    BufferSave { path: PathBuf, open_only: bool },
    /// Its modeline item, or none.
    Status(Option<String>),
    /// A surface, as page `page`: its pixels in `surface`, which its
    /// worker writes and the UI draws from.
    /// `replaces`: the page being restored (`page-restore`) when it opened
    /// it then -- the surface goes where that page is, in its place.
    SurfaceOpen { page: u64, kind: String, title: String, options: SurfaceOptions, surface: SharedSurface, replaces: Option<u64> },
    /// One of its pages brought to the pane you're in (`page-focus`).
    PageFocus(u64),
    /// Surface `page` has a new frame, a pointer or marks: the UI looks
    /// at it on its next frame (`Framebuffer::take_dirty`).
    SurfaceChanged(u64),
    PageRetitle { page: u64, title: String },
    /// The clipboard's text, answered on `answer`.
    ClipboardText(Answer<Option<String>>),
    /// What's selected in the explorer, as a full path.
    ExplorerSelected(Answer<Option<String>>),
    /// Fenix's Git pages, read again.
    GitRefresh,
    /// A scratch buffer of its own, as its `id`, in the focused pane --
    /// or in its pane beside it, `beside` of it, when that's given.
    ScratchOpen { id: u64, title: String, text: String, language: Option<String>, beside: Option<f32> },
    /// Its scratch buffer `id`'s text, if it's open.
    ScratchText { id: u64, answer: Answer<Option<String>> },
    /// Its scratch buffer `id`'s text replaced, as one undo step.
    ScratchSet { id: u64, text: String },
    ScratchClose(u64),
    /// A WAV of its own, checked and read, to play.
    PlaySound(Vec<u8>),
    /// A full path it was given here (`explorer-selected`), for Fenix to
    /// keep with its pages and give it again when it starts.
    Granted(PathBuf),
    /// Its decorations of the buffer on `path` -- held as `BufferText`'s
    /// is when `open_only` -- in place of those it set before.
    /// Each with the id a click on it comes back with
    /// (`decorations-set-tagged`), if it has one.
    FileDecorations { path: PathBuf, open_only: bool, decorations: Vec<Decoration> },
    /// Its decorations of its scratch buffer `id`.
    ScratchDecorations { id: u64, decorations: Vec<Decoration> },
    /// The buffer on `path`'s last change undone (`redo`: done again),
    /// held as `BufferText`'s is when `open_only`.
    BufferUndo { path: PathBuf, open_only: bool, redo: bool },
    /// Its project's debug adapter for `languages` over a socket: `program`
    /// started (`${port}` in its args the port), then 127.0.0.1:`port`
    /// connected to -- 0, one Fenix picks; or, with no program, one
    /// already listening there.
    DebugAdapterSocket { languages: Vec<String>, program: Option<(Checked, Env)>, port: u16 },
    /// Its rules for decorating files, in place of those it gave before.
    DecorationRules(Vec<DecorationRule>),
    /// The file its page `page` shows now, or none.
    PageFile { page: u64, path: Option<PathBuf> },
    /// The buffer on `path` -- held as `BufferText`'s is when `open_only`
    /// -- highlighted as `language`, or as its name says.
    FileLanguage { path: PathBuf, open_only: bool, language: Option<String> },
    ScratchLanguage { id: u64, language: Option<String> },
    /// A task beside the task pane, as `Task` is checked: its output in a
    /// buffer of its own. Its end comes back as `Notice::TaskFinished`.
    BackgroundTask { id: u64, name: String, program: String, args: Vec<String>, cwd: Option<PathBuf>, env: Vec<(String, String)> },
    /// Its task `id` from `BackgroundTask`, stopped.
    TaskCancel(u64),
    /// Its task `id`'s output, in the focused pane.
    TaskShow(u64),
    /// A language server of its project's, `name`, beside the others for
    /// `languages` (checked, with its environment); none takes it away.
    LanguageServerAdd { name: String, languages: Vec<String>, program: Option<(Checked, Env)> },
    /// The settings its project's server `name` -- one it added, or a
    /// language's -- is given when it asks: a JSON object, or none.
    LanguageServerSettings { name: String, settings: Option<serde_json::Value> },
    /// The launch configurations it offers in its project, checked: each
    /// one's attributes a JSON object.
    Launches(Vec<LaunchConfig>),
    /// `${name}` in launches, and the choices it's filled in from (none:
    /// taken away).
    LaunchVariable { name: String, choices: Vec<PickerItem> },
    /// Secret `key` kept, in the keychain where there's one, or with none
    /// taken away (`secret-set`): held to `permissions.secrets`
    /// already.
    SecretSet { key: String, value: Option<Hidden> },
}

/// A secret, or what's typed at a masked prompt: shown as dots when it's
/// printed, so it's in no log.
#[derive(Clone, PartialEq, Eq)]
pub struct Hidden(pub String);

impl std::fmt::Debug for Hidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"••••\"")
    }
}

/// What a call into the extension gave back.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Done,
    Page(Box<Page>),
    State(Option<String>),
    Restored(bool),
    /// Its answer, or none when it comes later (`Request::ServiceAnswer`).
    Served(Option<Result<String, String>>),
}

/// What a running extension tells Fenix.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Loaded and activated: its commands can run.
    Ready,
    /// It said something: through its log, or on stdout/stderr.
    Said(Source, String),
    /// It asks Fenix for something.
    Asked(Request),
    /// Call `call` ended.
    Finished { call: u64, result: Result<Reply, String> },
    /// It stopped -- it couldn't load, or a call trapped -- and why.
    Stopped(String),
}

/// What Fenix asks of an extension.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    RunCommand(String),
    /// Something it asked for has come (a picker's choice, a reply).
    Deliver(Notice),
    /// Its page laid out for a pane `width` cells wide and `height`
    /// lines tall (`height` reaches only `page-render-styled`).
    Render { page: u64, width: u32, height: u32 },
    Key { page: u64, key: Key },
    PageState(u64),
    PageRestore { page: u64, kind: String, state: String },
    /// Service `name`, as call `call`: Fenix's own number for it.
    Serve { call: u64, name: String, payload: String },
    Paste { page: u64, text: String },
    /// A message for its language server `server` (`LspPort`).
    Lsp { server: u64, message: String },
    /// A key, the pointer or the wheel on its surface `surface`.
    SurfaceInput { surface: u64, input: SurfaceInput },
    /// A full path Fenix gives it -- a forge's repository: it may read
    /// files there (`file-read`) and run programs in it.
    Grant(PathBuf),
    /// You closed its page or surface `page`: told to it when it has
    /// `page-closed`.
    PageClosed(u64),
    /// A buffer's text changed (`buffer-changed`), when it has that.
    BufferChanged(Changed),
    /// You closed its scratch buffer (`scratch-closed`), when it has that.
    ScratchClosed(u64),
    /// Something else has the focus (`focus-moved`), when it has that.
    FocusMoved(Focus),
    /// A key `page-key` doesn't take, on a page that takes more keys
    /// (`page-key-press`), when it has that.
    KeyPress { page: u64, key: KeyPress },
    /// The mouse on its page (`page-mouse`), when it has that.
    Mouse { page: u64, event: MouseEvent },
    /// A file was saved (`file-saved`), when it has that: its path as it
    /// may know it.
    FileSaved(String),
    /// What came of its request (`http-event`): a thread of its own
    /// brings these.
    HttpEvent { request: u64, part: HttpPart },
    /// The lines the panes show of buffers it may know of
    /// (`view-changed`), when it has that.
    ViewChanged(Vec<TextView>),
    /// A click on one of its tagged decorations (`decoration-clicked`),
    /// when it has that.
    DecorationClicked { buffer: TextBuffer, id: u32 },
    /// A task started in the task pane (`task-started`), when it has that.
    TaskStarted(String),
    /// What became of a debug session (`debug-changed`), when it has that.
    DebugChanged(DebugState),
    /// A connection made to one of its listeners, its socket `socket` now
    /// (`socket-accepted`): made by the worker itself.
    SocketAccepted { listener: u64, socket: u64, peer: String },
    /// A coding agent's turn ended (`agent-turn-ended`), when it
    /// has that.
    AgentTurnEnded(AgentTurn),
}

/// Reads a secret, for `host.secret`; Fenix's keychain behind it.
pub type Secrets = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// One extension, to be run.
#[derive(Clone)]
pub struct Spec {
    pub id: String,
    /// Its `extension.wasm`.
    pub wasm: PathBuf,
    /// Where its compiled code is kept (`<extension>/.compiled`).
    pub cache: PathBuf,
    /// The most linear memory it may have.
    pub memory: usize,
    /// How many epoch ticks (`crate::TICK`) activating it may take.
    pub activate_ticks: u64,
    /// How many any other call may take.
    pub call_ticks: u64,
    pub permissions: Permissions,
    /// Its own folder, `/data`.
    pub data: PathBuf,
    /// The open project, `/project` when its permissions allow.
    pub project: Option<PathBuf>,
    /// Fenix's tools folder, where `find-program` looks first. Its own,
    /// `/tools`, is `<tools>/<id>` there.
    pub tools_dir: Option<PathBuf>,
    /// The prefix of its own settings (`docker.`), and their values.
    pub setting_prefix: String,
    pub settings: Arc<RwLock<HashMap<String, String>>>,
    /// The settings it contributes, by key: the ones `setting-set` may
    /// change.
    pub own_settings: Vec<String>,
    pub secrets: Secrets,
    /// Where you were when its command was picked (`here`), with
    /// `editor = "read"`.
    pub here: Arc<RwLock<Option<Here>>>,
    /// Where the cursor and the selection were then (`editor-cursor`).
    pub cursor: Arc<RwLock<Option<CursorInfo>>>,
    /// The files it opens itself (`contributes.opens`): in the folder of
    /// one it was given, `folder-list` gives it those.
    pub opens: Vec<String>,
    /// The editor theme's colours, as Fenix keeps them up to date.
    pub theme: Arc<RwLock<EditorTheme>>,
    /// Eight distinct colours of the theme's (`theme-palette`), kept with
    /// it.
    pub palette: Arc<RwLock<Vec<Rgb>>>,
    /// Its folders (`permissions.folders`), by name, where your settings
    /// put them: `/folders/<name>`.
    pub folders: Vec<(String, PathBuf)>,
    /// The system's font folders, at `/fonts/<n>`, when it may read them.
    pub fonts: Vec<PathBuf>,
}

impl Spec {
    /// `id`'s spec with nothing granted beyond its own folder: `wasm` is
    /// its code, `dir` where its `.compiled` and data go.
    pub fn new(id: &str, wasm: PathBuf, dir: &Path) -> Spec {
        Spec {
            id: id.to_string(),
            wasm,
            cache: dir.join(".compiled"),
            memory: 128 << 20,
            activate_ticks: 1000,
            call_ticks: 500,
            permissions: Permissions::default(),
            data: dir.join("data"),
            project: None,
            tools_dir: None,
            setting_prefix: format!("{}.", id.rsplit('.').next().unwrap_or(id)),
            settings: Arc::default(),
            own_settings: Vec::new(),
            secrets: Arc::new(|_| None),
            here: Arc::default(),
            cursor: Arc::default(),
            opens: Vec::new(),
            folders: Vec::new(),
            fonts: Vec::new(),
            palette: Arc::default(),
            theme: Arc::new(RwLock::new(EditorTheme { name: String::new(), background: (0, 0, 0), text: (255, 255, 255), panel: (0, 0, 0), accent: (255, 255, 255), muted: (128, 128, 128), tint: (0, 0, 0), warn: (255, 200, 0), dark: true })),
        }
    }
}

/// The compiled code's file in `cache`: tied to this engine (wasmtime's
/// version and settings) and to these very bytes.
fn cached_name(engine: &Engine, wasm: &[u8]) -> String {
    let mut h = Sha256::new();
    std::hash::Hash::hash(&engine.precompile_compatibility_hash(), &mut HashInto(&mut h));
    h.update(wasm);
    let digest = h.finalize();
    format!("{}.cwasm", digest.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>())
}

/// `std::hash::Hasher` over a SHA-256.
struct HashInto<'a>(&'a mut Sha256);

impl std::hash::Hasher for HashInto<'_> {
    fn finish(&self) -> u64 {
        0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

/// `wasm` as a component for `engine`: from `cache` when it was compiled
/// before, else compiled now and kept there (older files dropped).
pub fn load(engine: &Engine, wasm: &Path, cache: &Path) -> wasmtime::Result<Component> {
    let bytes = std::fs::read(wasm).map_err(|e| wasmtime::format_err!("{}: {e}", wasm.display()))?;
    let name = cached_name(engine, &bytes);
    let kept = cache.join(&name);
    if kept.is_file() {
        // SAFETY: Fenix wrote this file, from these bytes, with an engine
        // like this one (the name says so); it's in the extension's own
        // folder, which only you and Fenix write. One that won't load is
        // compiled again below.
        if let Ok(component) = unsafe { Component::deserialize_file(engine, &kept) } {
            return Ok(component);
        }
    }
    let component = Component::new(engine, &bytes)?;
    if std::fs::create_dir_all(cache).is_ok() {
        if let Ok(compiled) = component.serialize() {
            let partial = cache.join(format!("{name}.partial"));
            if std::fs::write(&partial, compiled).is_ok() && std::fs::rename(&partial, &kept).is_ok() {
                for old in std::fs::read_dir(cache).into_iter().flatten().flatten() {
                    if old.path() != kept && old.path().extension().is_some_and(|x| x == "cwasm" || x == "partial") {
                        let _ = std::fs::remove_file(old.path());
                    }
                }
            }
        }
    }
    Ok(component)
}

type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// A process it started.
struct Process {
    child: Arc<Mutex<std::process::Child>>,
    stdin: Option<std::process::ChildStdin>,
}

/// What goes to a stream socket's thread, in order: bytes, or TLS from
/// there on (`socket-start-tls`).
enum Out {
    Data(Vec<u8>),
    Tls(Arc<rustls::ClientConfig>, rustls::pki_types::ServerName<'static>),
}

/// How a socket's connection is ended, once it has one.
type Shut = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// A socket it opened: TCP or local (`ipc-connect`), its writes going to a
/// thread of its own; or UDP, sent from here.
struct Socket {
    writes: Option<mpsc::Sender<Out>>,
    udp: Option<UdpSocket>,
    /// How its connection is ended, once it has one: shut, so its reader
    /// stops.
    shut: Shut,
    /// Closed by the extension, or ended: nothing more is told of it.
    closed: Arc<AtomicBool>,
    /// Where a UDP socket sent to: datagrams from them come too.
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    /// A TCP socket's host: the name TLS checks by default.
    tcp: Option<String>,
    /// A listener's port (`tcp-listen`): what connects to it comes
    /// as sockets of their own.
    listening: Option<u16>,
}

impl Socket {
    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        if let Some(shut) = self.shut.lock().ok().and_then(|mut s| s.take()) {
            shut();
        }
    }
}

/// What a TCP socket speaks: plain bytes, TLS being set up (nothing's read
/// meanwhile, the handshake's), or TLS.
enum Link {
    Plain,
    Starting,
    Tls(Box<rustls::ClientConnection>),
}

/// How long a TLS handshake may take.
const HANDSHAKE: Duration = Duration::from_secs(15);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// TLS set up on `stream` as `config` says, for `name`: its link is TLS
/// from then on.
fn handshake(stream: &TcpStream, link: &Mutex<Link>, config: Arc<rustls::ClientConfig>, name: rustls::pki_types::ServerName<'static>) -> Result<(), String> {
    let mut conn = rustls::ClientConnection::new(config, name).map_err(|e| format!("TLS: {e}"))?;
    *lock(link) = Link::Starting;
    let _ = stream.set_read_timeout(Some(HANDSHAKE));
    let mut io = stream;
    let done = (|| -> std::io::Result<()> {
        while conn.is_handshaking() {
            conn.complete_io(&mut io)?;
        }
        Ok(())
    })();
    let _ = stream.set_read_timeout(None);
    done.map_err(|e| format!("TLS: {e}"))?;
    *lock(link) = Link::Tls(Box::new(conn));
    Ok(())
}

/// `data` sent on `stream`, through TLS when its link is.
fn send(mut stream: &TcpStream, link: &Mutex<Link>, data: &[u8]) -> std::io::Result<()> {
    let mut link = lock(link);
    match &mut *link {
        Link::Tls(conn) => {
            conn.writer().write_all(data)?;
            while conn.wants_write() {
                conn.write_tls(&mut stream)?;
            }
            Ok(())
        }
        _ => stream.write_all(data).and_then(|_| stream.flush()),
    }
}

/// What came on a TCP socket, plain or through TLS: `Ok(None)` when its
/// other end closed it; `Ok(Some(empty))` when nothing's to be told yet.
fn receive(mut reading: &TcpStream, link: &Mutex<Link>, buf: &mut [u8]) -> Result<Option<Vec<u8>>, String> {
    // Waits without taking anything, so a handshake that starts meanwhile
    // gets what's its.
    match reading.peek(&mut [0u8; 1]) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => return Ok(Some(Vec::new())),
        Err(e) => return Err(e.to_string()),
    }
    let mut link = lock(link);
    match &mut *link {
        Link::Starting => {
            drop(link);
            std::thread::sleep(Duration::from_millis(2));
            Ok(Some(Vec::new()))
        }
        Link::Plain => match reading.read(buf) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(e) => Err(e.to_string()),
        },
        Link::Tls(conn) => {
            if conn.read_tls(&mut reading).map_err(|e| e.to_string())? == 0 {
                return Ok(None);
            }
            let state = conn.process_new_packets().map_err(|e| {
                let _ = conn.write_tls(&mut reading);
                format!("TLS: {e}")
            })?;
            let mut plain = vec![0u8; state.plaintext_bytes_to_read()];
            conn.reader().read_exact(&mut plain).map_err(|e| e.to_string())?;
            while conn.wants_write() {
                conn.write_tls(&mut reading).map_err(|e| e.to_string())?;
            }
            if state.peer_has_closed() && plain.is_empty() {
                return Ok(None);
            }
            Ok(Some(plain))
        }
    }
}

/// A connected TCP stream served as socket `socket`, on this thread: what
/// comes on it (`reading`, a clone of it) told as `socket-data`, then its
/// end as `socket-closed`; `queued`'s writes sent in order until it's
/// closed.
fn serve_tcp(jobs: mpsc::Sender<Job>, socket: u64, stream: TcpStream, reading: TcpStream, closed: Arc<AtomicBool>, link: Arc<Mutex<Link>>, queued: mpsc::Receiver<Out>) {
    let (reader_jobs, reader_closed, reader_link) = (jobs.clone(), closed.clone(), link.clone());
    std::thread::spawn(move || {
        let credit = Arc::new(AtomicUsize::new(0));
        let mut buf = vec![0u8; 64 << 10];
        while wait_for_room(&credit, &reader_closed) {
            let why = match receive(&reading, &reader_link, &mut buf) {
                Ok(Some(data)) if data.is_empty() => continue,
                Ok(Some(data)) => {
                    let n = data.len();
                    if !bring(&reader_jobs, &credit, Notice::SocketData((socket, data)), n) {
                        return;
                    }
                    continue;
                }
                Ok(None) => None,
                Err(e) => Some(e),
            };
            if !reader_closed.swap(true, Ordering::Relaxed) {
                State::deliver(&reader_jobs, Notice::SocketClosed((socket, why)));
            }
            let _ = reading.shutdown(std::net::Shutdown::Both);
            return;
        }
    });
    // Its writes, in order, until it's closed.
    for out in queued {
        let sent = match out {
            Out::Data(data) => send(&stream, &link, &data).map_err(|e| e.to_string()),
            Out::Tls(config, name) => handshake(&stream, &link, config, name).map(|()| State::deliver(&jobs, Notice::SocketConnected((socket, Ok(()))))),
        };
        if let Err(e) = sent {
            if !closed.swap(true, Ordering::Relaxed) {
                State::deliver(&jobs, Notice::SocketClosed((socket, Some(e))));
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
    }
}

/// How long connecting to one address may take.
const CONNECT: Duration = Duration::from_secs(10);

/// `host`:`port`, connected: each address it names, in turn.
fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.collect();
    let mut last = format!("{host}: no address");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = format!("{host}:{port}: {e}"),
        }
    }
    Err(last)
}

/// Waits while a socket's or process's reader is ahead of its extension
/// by `MAX_UNREAD`; false once it's closed.
fn wait_for_room(credit: &AtomicUsize, closed: &AtomicBool) -> bool {
    while credit.load(Ordering::Relaxed) > MAX_UNREAD {
        if closed.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    !closed.load(Ordering::Relaxed)
}

/// What a reader thread brings: its bytes delivered, counted against its
/// room until they are.
fn bring(jobs: &mpsc::Sender<Job>, credit: &Arc<AtomicUsize>, notice: Notice, len: usize) -> bool {
    credit.fetch_add(len, Ordering::Relaxed);
    jobs.send(Job::Data { what: Call::Deliver(notice), credit: credit.clone(), len }).is_ok()
}

/// One extension's store: WASI, its limits, what it may do, and what it
/// has going.
struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    sink: Sink,
    spec: Spec,
    /// Back onto its own worker: what threads bring it.
    jobs: mpsc::Sender<Job>,
    next: u64,
    processes: HashMap<u64, Process>,
    timers: HashMap<u64, Arc<AtomicBool>>,
    /// The paths `find-program` gave it: these may run.
    found: Vec<String>,
    /// Where its language servers' messages go, by server.
    lsp_out: HashMap<u64, Box<dyn Write + Send>>,
    sockets: HashMap<u64, Socket>,
    surfaces: HashMap<u64, SharedSurface>,
    /// Full paths Fenix gave it (`opened`, `explorer-selected`, a forge's
    /// repository): it may read files there, and work in them.
    given: Vec<PathBuf>,
    /// The page `page-restore` is opening again, while it runs: a surface
    /// it opens then takes that page's place.
    restoring: Option<u64>,
    /// Folders of files it was given that it listed (`folder-list`): the
    /// files there it opens itself, it may read.
    listed: Vec<PathBuf>,
    /// Its scratch buffers, open.
    scratches: HashSet<u64>,
    /// Its requests from `http-send`, while they go.
    requests: HashMap<u64, Arc<Cancel>>,
    /// Its tasks from `run-task-background`, while they run.
    background: HashSet<u64>,
    /// The secrets it kept or took away (`secret-set`) this run, read
    /// before Fenix's.
    secrets_set: HashMap<String, Option<String>>,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn ask(&self, request: Request) {
        (self.sink)(Event::Asked(request));
    }

    fn deliver(jobs: &mpsc::Sender<Job>, notice: Notice) {
        let _ = jobs.send(Job::Deliver(notice));
    }

    /// A full path of this machine's (`/home/me/x`, `C:\x`): absolute
    /// here, and not in one of its own folders (`/data/...` is absolute on
    /// Linux too). Not one it may reach for that.
    fn is_full_path(path: &str) -> bool {
        Path::new(path).is_absolute() && !is_guest_path(path)
    }

    /// A full path that's in one of its own folders all the same -- the
    /// project's, `/data`'s, a folder's of its own -- as Fenix spells
    /// them.
    fn own_full_path(&self, path: &str) -> Option<PathBuf> {
        if !State::is_full_path(path) {
            return None;
        }
        let path = PathBuf::from(path);
        if path.components().any(|c| c == std::path::Component::ParentDir) {
            return None;
        }
        let project = self.spec.project.as_deref().filter(|_| self.spec.permissions.filesystem != Filesystem::None);
        let mut roots = std::iter::once(self.spec.data.as_path()).chain(project).chain(self.spec.folders.iter().map(|(_, d)| d.as_path()));
        roots.any(|root| path.starts_with(root) && contained(root, &path).is_ok()).then_some(path)
    }

    /// Where `path` is, when it's somewhere it may be: in one of its own
    /// folders (`/project/...`, or the same as a full path), or a file
    /// Fenix gave it (`given_path`).
    fn reachable(&self, path: &str) -> Option<PathBuf> {
        self.given_path(path).or_else(|| self.host_path(path)).or_else(|| self.own_full_path(path))
    }

    /// A guest path (`/project/...`, `/data/...`) as a host one, if it's
    /// somewhere it may be.
    fn host_path(&self, path: &str) -> Option<PathBuf> {
        let path = path.replace('\\', "/");
        let under = |root: &Path, rest: &str| -> Option<PathBuf> {
            let rest = rest.trim_start_matches('/');
            let path = (!rest.split('/').any(|part| part == "..")).then(|| root.join(rest))?;
            contained(root, &path).is_ok().then_some(path)
        };
        if let Some(rest) = path.strip_prefix("/data") {
            return under(&self.spec.data, rest);
        }
        if let Some(rest) = path.strip_prefix("/tools").filter(|r| r.is_empty() || r.starts_with('/')) {
            return under(&self.own_tools()?, rest);
        }
        if let Some(rest) = path.strip_prefix("/folders/") {
            let (root, rest) = in_folder(&self.spec.folders, rest)?;
            return under(root, rest);
        }
        match (path.strip_prefix("/project"), &self.spec.project) {
            (Some(rest), Some(project)) if self.spec.permissions.filesystem != Filesystem::None => under(project, rest),
            _ => None,
        }
    }

    /// Its own tools folder, `/tools`, when its manifest names a program
    /// there (`permissions.process`) and Fenix has a tools folder: made
    /// when it's first asked for.
    fn own_tools(&self) -> Option<PathBuf> {
        let listed = self.spec.permissions.process.iter().any(|p| p.replace('\\', "/").starts_with("/tools/"));
        let dir = self.spec.tools_dir.as_ref().filter(|_| listed)?.join(&self.spec.id);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    /// `${tools}` in an argument or a variable's value: its tools folder,
    /// as a full path, left as it is when it has none; and `${data}`
    ///, its own folder.
    fn filled(&self, text: &str) -> String {
        let text = if text.contains("${data}") { text.replace("${data}", &self.spec.data.to_string_lossy()) } else { text.to_string() };
        if !text.contains("${tools}") {
            return text;
        }
        match self.own_tools() {
            Some(dir) => text.replace("${tools}", &dir.to_string_lossy()),
            None => text,
        }
    }

    /// The program `cmd` names, held to `permissions.process`, as what
    /// to start: a program by name; a path a setting it lists holds
    /// (`${setting:key}`); or a path under its own folders, when it lists
    /// it (`/data/...`, `/tools/...`, or a prefix ending in `*`).
    fn program(&self, cmd: &Command) -> Result<String, String> {
        let program = cmd.program.trim();
        if program.is_empty() {
            return Err("no program given".into());
        }
        let allowed = &self.spec.permissions.process;
        if !program.contains(['/', '\\']) {
            let stem = program.strip_suffix(".exe").unwrap_or(program);
            return if allowed.iter().any(|p| p.eq_ignore_ascii_case(stem)) { Ok(program.to_string()) } else { Err(format!("{stem} isn't a program this extension may run (permissions.process)")) };
        }
        let slashed = program.replace('\\', "/");
        if self.found.iter().any(|f| f.replace('\\', "/").eq_ignore_ascii_case(&slashed)) {
            return Ok(program.to_string());
        }
        if let Some(root) = ["/data/", "/tools/"].into_iter().find(|root| slashed.starts_with(root)) {
            let listed = allowed.iter().any(|p| match p.strip_suffix('*') {
                Some(prefix) => prefix.starts_with(root) && slashed.starts_with(prefix),
                None => p.replace('\\', "/") == slashed,
            });
            let host = self.host_path(&slashed).filter(|_| listed).ok_or_else(|| format!("{program} isn't a program this extension may run (permissions.process)"))?;
            return Ok(host.to_string_lossy().into_owned());
        }
        let settings = self.spec.settings.read().map(|s| s.clone()).unwrap_or_default();
        let same = |a: &str, b: &str| if cfg!(windows) { a.replace('\\', "/").eq_ignore_ascii_case(&b.replace('\\', "/")) } else { a == b };
        let by_setting = allowed.iter().filter_map(|p| p.strip_prefix("${setting:").and_then(|k| k.strip_suffix('}'))).filter_map(|key| settings.get(key)).any(|value| !value.trim().is_empty() && same(value.trim(), program));
        if by_setting {
            Ok(program.to_string())
        } else {
            Err(format!("{program}: a path runs only when one of its settings holds it (permissions.process)"))
        }
    }

    /// Whether it may draw over the editor (`permissions.draw`).
    /// `path`, a file in its own folder no bigger than `max`: `what` it
    /// is, and an `example` of a path that would do.
    fn own_file(&self, path: &str, what: &str, example: &str, max: u64) -> Result<PathBuf, String> {
        let rel = path.replace('\\', "/");
        if rel.is_empty() || rel.starts_with('/') || rel.contains(':') || rel.split('/').any(|p| p == "..") {
            return Err(format!("{path}: {what} is a file in its own folder ({example})"));
        }
        let dir = self.spec.wasm.parent().ok_or("no folder of its own")?;
        let file = dir.join(&rel);
        contained(dir, &file).map_err(|e| format!("{path}: {e}"))?;
        let size = std::fs::metadata(&file).map_err(|e| format!("{path}: {e}"))?.len();
        if size > max {
            return Err(format!("{path}: {what} is at most {} KiB", max >> 10));
        }
        Ok(file)
    }

    /// Whether it may know where you are in the editor
    /// (`permissions.editor`).
    fn may_read_editor(&self) -> bool {
        self.spec.permissions.editor.reads()
    }

    fn may_write_editor(&self) -> Result<(), String> {
        if self.spec.permissions.editor == fenix_extensions::manifest::Editor::Write {
            Ok(())
        } else {
            Err("it may not change what you edit (permissions.editor = \"write\")".into())
        }
    }

    /// A buffer's file (with `editor = "read"`): one it may reach
    /// (`reachable`); or any full path, but then only while you have a
    /// buffer open on it -- true, the UI's to hold it to -- and not one
    /// that's open only because an extension asked (`open-file`).
    fn buffer_path(&self, path: &str) -> Result<(PathBuf, bool), String> {
        if !self.may_read_editor() {
            return Err("it may not read what you edit (permissions.editor)".into());
        }
        if let Some(file) = self.reachable(path) {
            return Ok((file, false));
        }
        if State::is_full_path(path) {
            return Ok((PathBuf::from(path), true));
        }
        Err(format!("{path}: not a file it may reach"))
    }

    /// Its decorations of `buffer`, each with the id a click on it comes
    /// back with, if it has one, in place of those it set before.
    fn set_decorations(&mut self, buffer: TextBuffer, decorations: Vec<Decoration>) -> Result<(), String> {
        if decorations.len() > MAX_DECORATIONS {
            return Err(format!("{} decorations: a buffer has at most {MAX_DECORATIONS}", decorations.len()));
        }
        match buffer {
            TextBuffer::Scratch(id) => {
                if !self.scratches.contains(&id) {
                    return Err(format!("{id} isn't one of its scratch buffers, open"));
                }
                self.ask(Request::ScratchDecorations { id, decorations });
            }
            TextBuffer::File(path) => {
                let (path, open_only) = self.buffer_path(&path)?;
                self.ask(Request::FileDecorations { path, open_only, decorations });
            }
        }
        Ok(())
    }

    /// Asks the UI, and waits for its answer (`UI_ANSWER` at most).
    fn ask_and_wait<T>(&self, request: impl FnOnce(Answer<T>) -> Request) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        self.ask(request(Answer(Arc::new(Mutex::new(Some(tx))))));
        rx.recv_timeout(UI_ANSWER).ok()
    }

    /// File `file` -- one it may reach, as `reachable` or `own_file` found
    /// it -- read, at most `max` bytes, as what's opened: where the file it
    /// opened is, asked of the open file itself, must be somewhere it may
    /// read. A link put in its place after the check, or a folder's, is
    /// refused rather than followed out.
    fn read_held(&self, file: &Path, max: u64) -> Result<Vec<u8>, String> {
        let opened = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let at = opened_path(&opened).or_else(|| file.canonicalize().ok()).ok_or_else(|| format!("{}: not somewhere it may read", file.display()))?;
        if !self.may_hold(&at) {
            return Err(format!("{}: not somewhere it may read", file.display()));
        }
        let size = opened.metadata().map_err(|e| format!("{}: {e}", file.display()))?.len();
        if size > max {
            return Err(format!("{}: at most {} MiB", file.display(), (max >> 20).max(1)));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        opened.take(max + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        if bytes.len() as u64 > max {
            return Err(format!("{}: at most {} MiB", file.display(), (max >> 20).max(1)));
        }
        Ok(bytes)
    }

    /// `read_held`'s text of `file`, when it's a file of text.
    fn text_held(&self, file: &Path) -> Option<String> {
        file.is_file().then(|| self.read_held(file, MAX_FILE).ok().and_then(|b| String::from_utf8(b).ok())).flatten()
    }

    /// Whether `at` -- where an open file is, as the system spells it -- is
    /// somewhere it may read: under its own folders (its extension's, the
    /// project's, `/tools`, its folders), or a file or folder Fenix gave it.
    fn may_hold(&self, at: &Path) -> bool {
        let real = |p: &Path| p.canonicalize().ok();
        let project = self.spec.project.as_deref().filter(|_| self.spec.permissions.filesystem != Filesystem::None);
        let own = self.spec.wasm.parent();
        let tools = self.spec.tools_dir.as_ref().map(|d| d.join(&self.spec.id));
        let roots = own.into_iter().chain(std::iter::once(self.spec.data.as_path())).chain(project).chain(tools.as_deref()).chain(self.spec.folders.iter().map(|(_, d)| d.as_path())).chain(self.given.iter().map(PathBuf::as_path));
        if roots.filter_map(real).any(|root| at.starts_with(&root)) {
            return true;
        }
        if self.here_file().as_deref().and_then(real).is_some_and(|h| h == at) {
            return true;
        }
        let listed = at.parent().is_some_and(|dir| self.listed.iter().filter_map(|l| real(l)).any(|l| l == dir));
        listed && fenix_extensions::manifest::file_matches(&self.spec.opens, at, None)
    }

    fn may_draw(&self) -> Result<(), String> {
        if self.spec.permissions.draw == fenix_extensions::manifest::Draw::Over {
            Ok(())
        } else {
            Err("it may not draw over the editor (permissions.draw = \"over\")".into())
        }
    }

    /// `cmd` held to its permissions, as Fenix runs it.
    fn checked_program(&self, cmd: &Command) -> Result<Checked, String> {
        let (_, cwd) = self.checked(cmd)?;
        Ok((self.program(cmd)?, cmd.args.iter().map(|a| self.filled(a)).collect(), cwd))
    }

    /// `checked_program`, and its environment, `${tools}` filled in.
    fn checked_with_env(&self, cmd: &Command) -> Result<(Checked, Env), String> {
        Ok((self.checked_program(cmd)?, cmd.env.iter().map(|(k, v)| (k.clone(), self.filled(v))).collect()))
    }

    /// `cmd`, held to `permissions.process` (`program`) and to where it
    /// may work.
    fn checked(&self, cmd: &Command) -> Result<(std::process::Command, Option<PathBuf>), String> {
        let program = self.program(cmd)?;
        let cwd = match &cmd.cwd {
            Some(dir) => Some(self.host_path(dir).or_else(|| self.given_path(dir)).ok_or_else(|| format!("{dir}: not somewhere it may work"))?),
            None => self.spec.project.clone().filter(|_| self.spec.permissions.filesystem != Filesystem::None),
        };
        let mut command = std::process::Command::new(&program);
        command.args(cmd.args.iter().map(|a| self.filled(a)));
        for (k, v) in &cmd.env {
            command.env(k, self.filled(v));
        }
        if let Some(dir) = &cwd {
            command.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        Ok((command, cwd))
    }

    /// Whether `url` goes to a host `permissions.network` names: exactly,
    /// as `*.example.com`, as `${setting:key}` (the host in that setting's
    /// URL), or any with `*`.
    fn may_reach(&self, url: &str) -> Result<(), String> {
        may_reach(&self.spec.permissions.network, &self.spec.settings, url)
    }

    /// `may_reach`, for a thread of its own: what a request's redirects
    /// are held to.
    fn reach(&self) -> impl Fn(&str) -> Result<(), String> + Send + 'static {
        let (network, settings) = (self.spec.permissions.network.clone(), self.spec.settings.clone());
        move |url: &str| may_reach(&network, &settings, url)
    }

    /// `permissions.sockets`, read: `${setting:key}` as the addresses that
    /// setting holds now.
    fn socket_rules(&self) -> Vec<SocketRule> {
        let settings = self.spec.settings.read().map(|s| s.clone()).unwrap_or_default();
        self.spec
            .permissions
            .sockets
            .iter()
            .flat_map(|entry| match entry.trim().strip_prefix("${setting:").and_then(|e| e.strip_suffix('}')) {
                Some(key) => settings.get(key).map(|v| SocketRule::from_setting(v)).unwrap_or_default(),
                None => SocketRule::parse(entry).into_iter().collect(),
            })
            .collect()
    }

    /// Whether it may reach `host`:`port` (`udp`: with a datagram).
    fn may_connect(&self, udp: bool, host: &str, port: u16) -> Result<(), String> {
        if self.socket_rules().iter().any(|r| r.reaches(udp, host, port)) {
            Ok(())
        } else {
            Err(format!("{host}:{port} isn't somewhere this extension may open a {} socket to (permissions.sockets)", if udp { "UDP" } else { "TCP" }))
        }
    }

    /// Whether it may reach local socket `name` (`permissions.ipc`): a
    /// pattern (`discord-ipc-*`), or the name a setting of its holds
    /// (`${setting:key}`).
    fn may_ipc(&self, name: &str) -> Result<(), String> {
        let bare = name.strip_prefix(r"\\.\pipe\").unwrap_or(name);
        if bare.is_empty() || bare.replace('\\', "/").split('/').any(|part| part == "..") {
            return Err(format!("{name}: not a local socket's name"));
        }
        let settings = self.spec.settings.read().map(|s| s.clone()).unwrap_or_default();
        let allowed = self.spec.permissions.ipc.iter().any(|entry| match entry.strip_prefix("${setting:").and_then(|e| e.strip_suffix('}')) {
            Some(key) => settings.get(key).is_some_and(|v| !v.trim().is_empty() && v.trim() == bare),
            None => fenix_extensions::manifest::glob_matches(entry, bare),
        });
        if allowed {
            Ok(())
        } else {
            Err(format!("{name} isn't a local socket this extension may reach (permissions.ipc)"))
        }
    }

    /// A full path Fenix gave it, or one below such a path; or the file
    /// `editor-here` gives, with `editor = "read"`.
    fn given_path(&self, path: &str) -> Option<PathBuf> {
        if !Path::new(path).is_absolute() {
            return None;
        }
        let path = PathBuf::from(path);
        if path.components().any(|c| c == std::path::Component::ParentDir) {
            return None;
        }
        let here = self.here_file();
        let listed = path.parent().is_some_and(|dir| self.listed.iter().any(|l| l == dir)) && fenix_extensions::manifest::file_matches(&self.spec.opens, &path, None);
        // Below a folder it was given, not by way of a link out of it.
        let given = self.given.iter().any(|g| path.starts_with(g) && (path == *g || contained(g, &path).is_ok())) || here.is_some_and(|h| h == path) || listed;
        given.then_some(path)
    }

    /// The file `editor-here` gives, with `editor = "read"`.
    fn here_file(&self) -> Option<PathBuf> {
        self.may_read_editor().then(|| self.spec.here.read().ok().and_then(|h| h.as_ref().and_then(|h| h.path.clone()))).flatten().map(PathBuf::from)
    }

    fn surface(&self, surface: u64) -> Result<&SharedSurface, String> {
        self.surfaces.get(&surface).ok_or_else(|| format!("{surface} isn't one of its surfaces"))
    }

    /// Something of surface `surface`'s changed: the UI's told, once until
    /// it looks.
    fn surface_changed(&self, surface: u64, tell: bool) {
        if tell {
            self.ask(Request::SurfaceChanged(surface));
        }
    }

    /// A TCP connection to `host`:`port`, held to its permissions already:
    /// with TLS set up first when `tls` says, before it's told it's
    /// connected.
    fn open_tcp(&mut self, host: String, port: u16, tls: Option<(Arc<rustls::ClientConfig>, rustls::pki_types::ServerName<'static>)>) -> u64 {
        let socket = self.id();
        let (writes, queued) = mpsc::channel::<Out>();
        let closed = Arc::new(AtomicBool::new(false));
        let slot: Shut = Arc::default();
        let (jobs, closed2, slot2, named) = (self.jobs.clone(), closed.clone(), slot.clone(), host.clone());
        std::thread::spawn(move || {
            let (jobs, closed, slot, host) = (jobs, closed2, slot2, named);
            let failed = |why: String| {
                if !closed.swap(true, Ordering::Relaxed) {
                    State::deliver(&jobs, Notice::SocketConnected((socket, Err(why))));
                }
            };
            let stream = match connect(&host, port) {
                Ok(stream) => stream,
                Err(e) => return failed(e),
            };
            let _ = stream.set_nodelay(true);
            let (Ok(reading), Ok(kept)) = (stream.try_clone(), stream.try_clone()) else { return };
            if let Ok(mut s) = slot.lock() {
                *s = Some(Box::new(move || {
                    let _ = kept.shutdown(std::net::Shutdown::Both);
                }));
            }
            let link = Arc::new(Mutex::new(Link::Plain));
            if let Some((config, name)) = tls {
                if let Err(e) = handshake(&stream, &link, config, name) {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return failed(e);
                }
            }
            if closed.load(Ordering::Relaxed) {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
            State::deliver(&jobs, Notice::SocketConnected((socket, Ok(()))));
            serve_tcp(jobs, socket, stream, reading, closed, link, queued);
        });
        self.sockets.insert(socket, Socket { writes: Some(writes), udp: None, shut: slot, closed, peers: Arc::default(), tcp: Some(host), listening: None });
        socket
    }

    /// A connection made to one of its listeners (`tcp-listen`): a TCP
    /// socket of its own from now on, served as one it connected. Its id.
    fn adopt_tcp(&mut self, stream: TcpStream) -> Option<u64> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        let (reading, kept) = (stream.try_clone().ok()?, stream.try_clone().ok()?);
        let socket = self.id();
        let (writes, queued) = mpsc::channel::<Out>();
        let closed = Arc::new(AtomicBool::new(false));
        let slot: Shut = Arc::new(Mutex::new(Some(Box::new(move || {
            let _ = kept.shutdown(std::net::Shutdown::Both);
        }))));
        let (jobs, serving) = (self.jobs.clone(), closed.clone());
        std::thread::spawn(move || serve_tcp(jobs, socket, stream, reading, serving, Arc::new(Mutex::new(Link::Plain)), queued));
        self.sockets.insert(socket, Socket { writes: Some(writes), udp: None, shut: slot, closed, peers: Arc::default(), tcp: Some("127.0.0.1".into()), listening: None });
        Some(socket)
    }

    /// Starts `cmd`: its stdout as lines (`process-output`), or as bytes
    /// (`process-bytes`) when `raw`.
    fn start_process(&mut self, cmd: &Command, raw: bool) -> Result<u64, String> {
        let (mut command, _) = self.checked(cmd)?;
        command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("{}: {e}", cmd.program))?;
        let process = self.id();
        let (stdout, stderr, stdin) = (child.stdout.take(), child.stderr.take(), child.stdin.take());
        let child = Arc::new(Mutex::new(child));
        let mut readers = Vec::new();
        if let Some(stdout) = stdout {
            let jobs = self.jobs.clone();
            readers.push(if raw {
                std::thread::spawn(move || {
                    let (credit, never) = (Arc::new(AtomicUsize::new(0)), AtomicBool::new(false));
                    let mut stdout = stdout;
                    let mut buf = vec![0u8; 256 << 10];
                    while wait_for_room(&credit, &never) {
                        match stdout.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if !bring(&jobs, &credit, Notice::ProcessBytes((process, buf[..n].to_vec())), n) {
                                    break;
                                }
                            }
                        }
                    }
                })
            } else {
                std::thread::spawn(move || {
                    for line in std::io::BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        State::deliver(&jobs, Notice::ProcessOutput(ProcessOutput { process, stderr: false, line }));
                    }
                })
            });
        }
        if let Some(stderr) = stderr {
            let jobs = self.jobs.clone();
            readers.push(std::thread::spawn(move || {
                for line in std::io::BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    State::deliver(&jobs, Notice::ProcessOutput(ProcessOutput { process, stderr: true, line }));
                }
            }));
        }
        let (jobs, waited) = (self.jobs.clone(), child.clone());
        std::thread::spawn(move || {
            for reader in readers {
                let _ = reader.join();
            }
            let code = loop {
                let status = waited.lock().ok().and_then(|mut c| c.try_wait().ok()).flatten();
                if let Some(status) = status {
                    break status.code();
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            State::deliver(&jobs, Notice::ProcessExit((process, code)));
        });
        self.processes.insert(process, Process { child, stdin });
        Ok(process)
    }
}

/// Where this machine keeps its fonts, for `permissions.fonts`.
pub fn font_dirs() -> Vec<PathBuf> {
    let home = |rest: &str| std::env::var_os(if cfg!(windows) { "LOCALAPPDATA" } else { "HOME" }).map(|h| PathBuf::from(h).join(rest));
    if cfg!(windows) {
        let windows = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        [Some(windows.join("Fonts")), home("Microsoft\\Windows\\Fonts")].into_iter().flatten().collect()
    } else if cfg!(target_os = "macos") {
        [Some(PathBuf::from("/System/Library/Fonts")), Some(PathBuf::from("/Library/Fonts")), home("Library/Fonts")].into_iter().flatten().collect()
    } else {
        [Some(PathBuf::from("/usr/share/fonts")), Some(PathBuf::from("/usr/local/share/fonts")), home(".local/share/fonts")].into_iter().flatten().collect()
    }
}

/// Which of `folders` a path under `/folders/` is in, and the rest of it:
/// a map's folders are named `<name>/<its name>`, so the longest name it
/// starts with.
fn in_folder<'a>(folders: &'a [(String, PathBuf)], rest: &'a str) -> Option<(&'a Path, &'a str)> {
    let (name, root) = folders.iter().filter(|(n, _)| rest == n.as_str() || rest.starts_with(&format!("{n}/"))).max_by_key(|(n, _)| n.len())?;
    Some((root.as_path(), &rest[name.len()..]))
}

/// Whether `path` is in one of the folders an extension has of its own --
/// `/data`, `/project`, `/folders/...`, `/fonts/...`, `/tools` -- rather
/// than a full path of this machine's.
pub fn is_guest_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    ["/data", "/project", "/folders", "/fonts", "/tools"].iter().any(|root| path == *root || path.starts_with(&format!("{root}/")))
}

/// The executable's file name for `stem` here.
fn exe(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// The file names program `stem` may have here, in the order they're
/// looked for: on Windows `.exe`, then a script, `.cmd` and `.bat`
/// (`gradlew.bat`, `mvnw.cmd`).
fn exes(stem: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"].iter().map(|x| format!("{stem}.{x}")).collect()
    } else {
        vec![stem.to_string()]
    }
}

/// Program `stem` at `path`: the file itself, or in it when it's a
/// folder; a script beside it on Windows (`./gradlew` is `gradlew.bat`
/// there).
fn program_at(path: &Path, stem: &str) -> Option<PathBuf> {
    if path.is_dir() {
        return exes(stem).into_iter().map(|name| path.join(name)).find(|p| p.is_file());
    }
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let dir = path.parent()?;
    (cfg!(windows) && path.extension().is_none()).then(|| exes(&name).into_iter().map(|n| dir.join(n)).find(|p| p.is_file())).flatten()
}

/// `%VAR%` and `$VAR` in `path`, from the environment.
fn expand(path: &str) -> String {
    let mut out = path.to_string();
    for (key, value) in std::env::vars() {
        out = out.replace(&format!("%{key}%"), &value).replace(&format!("${key}"), &value);
    }
    out
}

/// The first file called `name` at most `depth` folders under `dir`: a
/// release archive unpacks into a versioned folder of its own.
fn find_below(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    children.sort();
    children.into_iter().find_map(|child| find_below(&child, name, depth - 1))
}

/// Program `stem`: beside Fenix itself (a companion it ships, as
/// `fenix-pdfium`), in the extension's own tools folder (`own`, a few
/// folders down: what it unpacked there), in Fenix's tools folder
/// (`<tools>/<stem>/...`), on `PATH`, or at one of `hints` -- a hint
/// naming a folder means the program in it.
fn locate(stem: &str, own: Option<&Path>, tools: Option<&Path>, hints: &[String]) -> Option<PathBuf> {
    let name = exe(stem);
    let names = exes(stem);
    let beside = std::env::current_exe().ok().and_then(|me| me.parent().map(|dir| dir.join(&name))).filter(|p| p.is_file());
    beside
        .or_else(|| own.and_then(|dir| names.iter().find_map(|n| find_below(dir, n, 4))))
        .or_else(|| tools.and_then(|dir| names.iter().find_map(|n| find_below(&dir.join(stem), n, 3))))
        .or_else(|| std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))))
        .or_else(|| hints.iter().find_map(|h| program_at(&PathBuf::from(expand(h)), stem)))
}

/// Whether `url` goes to a host `network` (`permissions.network`) names,
/// `settings` filling in `${setting:key}`.
fn may_reach(network: &[String], settings: &RwLock<HashMap<String, String>>, url: &str) -> Result<(), String> {
    let host = host_of(url).ok_or_else(|| format!("{url}: not an http(s) URL"))?;
    let settings = settings.read().map(|s| s.clone()).unwrap_or_default();
    let allowed = network.iter().any(|entry| {
        let entry = match entry.strip_prefix("${setting:").and_then(|e| e.strip_suffix('}')) {
            Some(key) => settings.get(key).and_then(|v| host_of(v)).unwrap_or_default(),
            None => entry.to_ascii_lowercase(),
        };
        match entry.strip_prefix("*.") {
            Some(domain) => host.ends_with(&format!(".{domain}")),
            None => entry == "*" || (!entry.is_empty() && host == entry),
        }
    });
    if allowed {
        Ok(())
    } else {
        Err(format!("{host} isn't a host this extension may reach (permissions.network)"))
    }
}

/// `https://Host:443/x` as `host`.
fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]"))? } else { host.split(':').next()?.to_string() };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

impl Drop for State {
    fn drop(&mut self) {
        for timer in self.timers.values() {
            timer.store(true, Ordering::Relaxed);
        }
        for process in self.processes.values() {
            if let Ok(mut child) = process.child.lock() {
                let _ = child.kill();
            }
        }
        for socket in self.sockets.values() {
            socket.close();
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl fenix::extension::types::Host for State {}

impl fenix::extension::host::Host for State {
    fn log(&mut self, level: Level, message: String) {
        (self.sink)(Event::Said(Source::Log(level), message));
    }

    fn notify(&mut self, level: Level, message: String) {
        self.ask(Request::Notify(level, message));
    }

    fn run_command(&mut self, id: String) {
        self.ask(Request::RunCommand(id));
    }

    fn open_file(&mut self, path: String, line: Option<u32>) {
        if let Some(path) = self.reachable(&path) {
            let text = self.text_held(&path);
            return self.ask(Request::OpenFile { path, line, sealed: false, text });
        }
        // Any other full path, with `editor = "read"` (a task's link to
        // code, a tag outside the project): opened for you to see, and
        // sealed -- what's in it doesn't reach the extensions.
        if self.may_read_editor() && State::is_full_path(&path) {
            return self.ask(Request::OpenFile { path: PathBuf::from(path), line, sealed: true, text: None });
        }
        (self.sink)(Event::Said(Source::Log(Level::Warn), format!("open-file: {path} isn't somewhere it may open")));
    }

    fn show_picker(&mut self, title: String, items: Vec<PickerItem>) -> u64 {
        let id = self.id();
        self.ask(Request::Picker { id, title, items });
        id
    }

    fn show_picker_many(&mut self, title: String, items: Vec<PickerItem>) -> u64 {
        let id = self.id();
        self.ask(Request::PickerMany { id, title, items });
        id
    }

    fn prompt(&mut self, label: String, initial: String) -> u64 {
        let id = self.id();
        self.ask(Request::Prompt { id, label, initial, masked: false });
        id
    }

    fn compose(&mut self, title: String, verb: String, initial: String, allow_empty: bool) -> u64 {
        let id = self.id();
        self.ask(Request::Compose { id, title, verb, initial, allow_empty });
        id
    }

    fn editor_here(&mut self) -> Option<Here> {
        if !self.may_read_editor() {
            (self.sink)(Event::Said(Source::Log(Level::Warn), "editor-here: it may not read the editor (permissions.editor = \"read\")".into()));
            return None;
        }
        self.spec.here.read().ok()?.clone()
    }

    fn home_set(&mut self, section: Option<HomeSection>) {
        self.ask(Request::Home(section));
    }

    fn page_open(&mut self, kind: String, title: String) -> u64 {
        let page = self.id();
        self.ask(Request::PageOpen { page, kind, title });
        page
    }

    fn page_open_beside(&mut self, kind: String, title: String, width: f32) -> u64 {
        let page = self.id();
        self.ask(Request::PageOpenBeside { page, kind, title, width: width.clamp(0.1, 0.9) });
        page
    }

    fn page_refresh(&mut self, page: u64) {
        self.ask(Request::PageRefresh(page));
    }

    fn page_close(&mut self, page: u64) {
        // A surface's pixels go with it: one opened again in its place
        // (a remote desktop brought to another pane) is a new one.
        self.surfaces.remove(&page);
        self.ask(Request::PageClose(page));
    }

    fn setting(&mut self, key: String) -> Option<String> {
        // Its own name's, one it contributes under a namespace of its own
        // (`reader.zoom`, a 1.x feature's), or another's it may read
        // (`permissions.settings`, `ccsds.pus`).
        let readable = key.split_once('.').is_some_and(|(namespace, _)| self.spec.permissions.settings.iter().any(|n| n == namespace));
        if !key.starts_with(&self.spec.setting_prefix) && !self.spec.own_settings.contains(&key) && !readable {
            return None;
        }
        self.spec.settings.read().ok()?.get(&key).cloned()
    }

    fn theme_colors(&mut self) -> EditorTheme {
        self.spec.theme.read().map(|t| t.clone()).unwrap_or_else(|p| p.into_inner().clone())
    }

    fn theme_palette(&mut self) -> Vec<Rgb> {
        self.spec.palette.read().map(|p| p.clone()).unwrap_or_else(|p| p.into_inner().clone())
    }

    fn decorations_set(&mut self, buffer: TextBuffer, decorations: Vec<Decoration>) -> Result<(), String> {
        self.set_decorations(buffer, decorations)
    }

    fn buffer_language_set(&mut self, buffer: TextBuffer, language: Option<String>) -> Result<(), String> {
        let language = language.map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty());
        match buffer {
            TextBuffer::Scratch(id) => {
                if !self.scratches.contains(&id) {
                    return Err(format!("{id} isn't one of its scratch buffers, open"));
                }
                self.ask(Request::ScratchLanguage { id, language });
            }
            TextBuffer::File(path) => {
                let (path, open_only) = self.buffer_path(&path)?;
                self.ask(Request::FileLanguage { path, open_only, language });
            }
        }
        Ok(())
    }

    fn run_task_background(&mut self, name: String, cmd: Command) -> Result<u64, String> {
        let (_, cwd) = self.checked(&cmd)?;
        let program = self.program(&cmd)?;
        if self.background.len() >= MAX_BACKGROUND {
            return Err(format!("{} of its tasks run already: at most {MAX_BACKGROUND} at once", self.background.len()));
        }
        let id = self.id();
        let (args, env) = (cmd.args.iter().map(|a| self.filled(a)).collect(), cmd.env.iter().map(|(k, v)| (k.clone(), self.filled(v))).collect());
        self.background.insert(id);
        self.ask(Request::BackgroundTask { id, name, program, args, cwd, env });
        Ok(id)
    }

    fn task_cancel(&mut self, task: u64) {
        if self.background.contains(&task) {
            self.ask(Request::TaskCancel(task));
        }
    }

    fn task_show(&mut self, task: u64) -> Result<(), String> {
        // Its output stays once it's ended, so any of its ids will do.
        if task == 0 || task > self.next {
            return Err(format!("{task} isn't one of its tasks"));
        }
        self.ask(Request::TaskShow(task));
        Ok(())
    }

    fn buffer_undo(&mut self, path: String) -> Result<(), String> {
        self.may_write_editor()?;
        let (path, open_only) = self.buffer_path(&path)?;
        self.ask(Request::BufferUndo { path, open_only, redo: false });
        Ok(())
    }

    fn buffer_redo(&mut self, path: String) -> Result<(), String> {
        self.may_write_editor()?;
        let (path, open_only) = self.buffer_path(&path)?;
        self.ask(Request::BufferUndo { path, open_only, redo: true });
        Ok(())
    }

    fn setting_set(&mut self, key: String, value: Option<String>) -> Result<(), String> {
        if !self.spec.own_settings.contains(&key) {
            return Err(format!("{key} isn't one of its settings (contributes.settings)"));
        }
        if let Ok(mut settings) = self.spec.settings.write() {
            match &value {
                Some(v) => settings.insert(key.clone(), v.clone()),
                None => settings.remove(&key),
            };
        }
        self.ask(Request::SettingSet { key, value });
        Ok(())
    }

    fn secret(&mut self, key: String) -> Option<String> {
        if !fenix_extensions::manifest::secret_allowed(&self.spec.permissions.secrets, &key) {
            return None;
        }
        if let Some(set) = self.secrets_set.get(&key) {
            return set.clone();
        }
        (self.spec.secrets)(&key)
    }

    fn spawn(&mut self, cmd: Command) -> Result<u64, String> {
        self.start_process(&cmd, false)
    }

    fn spawn_raw(&mut self, cmd: Command) -> Result<u64, String> {
        self.start_process(&cmd, true)
    }

    fn socket_connect(&mut self, host: String, port: u16) -> Result<u64, String> {
        self.may_connect(false, &host, port)?;
        Ok(self.open_tcp(host, port, None))
    }

    fn socket_connect_tls(&mut self, host: String, port: u16, tls: TlsOptions) -> Result<u64, String> {
        self.may_connect(false, &host, port)?;
        let config = tls_config(&tls.pinned, tls.accept_invalid, tls.client_cert.as_ref())?;
        let name = server_name(tls.server_name.as_deref().unwrap_or(&host))?;
        Ok(self.open_tcp(host, port, Some((config, name))))
    }

    fn socket_start_tls(&mut self, socket: u64, tls: TlsOptions) -> Result<(), String> {
        let s = self.sockets.get(&socket).ok_or_else(|| format!("{socket} isn't one of its sockets"))?;
        let host = s.tcp.clone().ok_or_else(|| format!("socket {socket} isn't a TCP socket"))?;
        let writes = s.writes.as_ref().filter(|_| !s.closed.load(Ordering::Relaxed)).ok_or_else(|| format!("socket {socket} is closed"))?;
        let config = tls_config(&tls.pinned, tls.accept_invalid, tls.client_cert.as_ref())?;
        let name = server_name(tls.server_name.as_deref().unwrap_or(&host))?;
        writes.send(Out::Tls(config, name)).map_err(|_| format!("socket {socket} is closed"))
    }

    fn ipc_connect(&mut self, name: String) -> Result<u64, String> {
        self.may_ipc(&name)?;
        let socket = self.id();
        let closed = Arc::new(AtomicBool::new(false));
        let (reading, writing, shut) = ipc_open(&name, closed.clone())?;
        let (writes, queued) = mpsc::channel::<Out>();
        let slot: Shut = Arc::new(Mutex::new(Some(shut)));
        State::deliver(&self.jobs, Notice::SocketConnected((socket, Ok(()))));
        let (jobs, reader_closed) = (self.jobs.clone(), closed.clone());
        std::thread::spawn(move || {
            let (credit, mut reading) = (Arc::new(AtomicUsize::new(0)), reading);
            let mut buf = vec![0u8; 64 << 10];
            while wait_for_room(&credit, &reader_closed) {
                let why = match reading.read(&mut buf) {
                    Ok(0) => None,
                    Ok(n) => {
                        if !bring(&jobs, &credit, Notice::SocketData((socket, buf[..n].to_vec())), n) {
                            return;
                        }
                        continue;
                    }
                    Err(e) => Some(e.to_string()),
                };
                if !reader_closed.swap(true, Ordering::Relaxed) {
                    State::deliver(&jobs, Notice::SocketClosed((socket, why)));
                }
                return;
            }
        });
        let (jobs, writer_closed) = (self.jobs.clone(), closed.clone());
        std::thread::spawn(move || {
            let mut writing = writing;
            for out in queued {
                let Out::Data(data) = out else { continue };
                if let Err(e) = writing.write_all(&data).and_then(|_| writing.flush()) {
                    if !writer_closed.swap(true, Ordering::Relaxed) {
                        State::deliver(&jobs, Notice::SocketClosed((socket, Some(e.to_string()))));
                    }
                    return;
                }
            }
        });
        self.sockets.insert(socket, Socket { writes: Some(writes), udp: None, shut: slot, closed, peers: Arc::default(), tcp: None, listening: None });
        Ok(socket)
    }

    fn socket_write(&mut self, socket: u64, data: Vec<u8>) -> Result<(), String> {
        let s = self.sockets.get(&socket).ok_or_else(|| format!("{socket} isn't one of its sockets"))?;
        if s.closed.load(Ordering::Relaxed) {
            return Err(format!("socket {socket} is closed"));
        }
        match (&s.writes, &s.udp) {
            (Some(writes), _) => writes.send(Out::Data(data)).map_err(|_| format!("socket {socket} is closed")),
            (None, Some(_)) => Err(format!("socket {socket} is UDP: udp-send says where a datagram goes")),
            (None, None) if s.listening.is_some() => Err(format!("socket {socket} listens: what connects to it comes to socket-accepted, as a socket of its own")),
            (None, None) => Err(format!("socket {socket} is closed")),
        }
    }

    fn udp_bind(&mut self, port: u16) -> Result<u64, String> {
        let rules = self.socket_rules();
        if rules.is_empty() {
            return Err("it may open no sockets (permissions.sockets)".into());
        }
        let open = port != 0;
        if open && !rules.iter().any(|r| r.binds(port)) {
            return Err(format!("port {port} isn't one this extension may take (permissions.sockets: \":{port}\" or \"udp://:{port}\")"));
        }
        let udp = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| format!("port {port}: {e}"))?;
        let _ = udp.set_read_timeout(Some(Duration::from_millis(250)));
        let reading = udp.try_clone().map_err(|e| e.to_string())?;
        let socket = self.id();
        let (closed, peers) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(HashSet::<SocketAddr>::new())));
        let (jobs, reader_closed, reader_peers) = (self.jobs.clone(), closed.clone(), peers.clone());
        std::thread::spawn(move || {
            let credit = Arc::new(AtomicUsize::new(0));
            let mut buf = vec![0u8; 65_536];
            while wait_for_room(&credit, &reader_closed) {
                match reading.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        // On a port of its own, from anyone; else only from
                        // where it may reach, or sent to.
                        let welcome = open || reader_peers.lock().is_ok_and(|p| p.contains(&from)) || rules.iter().any(|r| r.reaches(true, &from.ip().to_string(), from.port()));
                        if welcome && !bring(&jobs, &credit, Notice::Datagram(Datagram { socket, from: from.to_string(), data: buf[..n].to_vec() }), n) {
                            return;
                        }
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    // Windows says so of a datagram an earlier send's
                    // host refused: not the socket's end.
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    Err(e) => {
                        if !reader_closed.swap(true, Ordering::Relaxed) {
                            State::deliver(&jobs, Notice::SocketClosed((socket, Some(e.to_string()))));
                        }
                        return;
                    }
                }
            }
        });
        self.sockets.insert(socket, Socket { writes: None, udp: Some(udp), shut: Arc::default(), closed, peers, tcp: None, listening: None });
        Ok(socket)
    }

    fn udp_send(&mut self, socket: u64, host: String, port: u16, data: Vec<u8>) -> Result<(), String> {
        self.may_connect(true, &host, port)?;
        let s = self.sockets.get(&socket).ok_or_else(|| format!("{socket} isn't one of its sockets"))?;
        let udp = s.udp.as_ref().filter(|_| !s.closed.load(Ordering::Relaxed)).ok_or_else(|| format!("socket {socket} isn't an open UDP socket"))?;
        let to = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.find(|a| a.is_ipv4()).ok_or_else(|| format!("{host}: no IPv4 address"))?;
        if let Ok(mut peers) = s.peers.lock() {
            peers.insert(to);
        }
        udp.send_to(&data, to).map(|_| ()).map_err(|e| format!("{host}:{port}: {e}"))
    }

    fn socket_close(&mut self, socket: u64) {
        if let Some(s) = self.sockets.remove(&socket) {
            s.close();
        }
    }

    fn surface_open(&mut self, kind: String, title: String, options: SurfaceOptions) -> Result<u64, String> {
        let page = self.id();
        let surface = SharedSurface::default();
        self.surfaces.insert(page, surface.clone());
        let options = SurfaceOptions { beside: options.beside.map(|w| w.clamp(0.1, 0.9)), ..options };
        let replaces = self.restoring.take();
        self.ask(Request::SurfaceOpen { page, kind, title, options, surface, replaces });
        Ok(page)
    }

    fn surface_resize(&mut self, surface: u64, width: u32, height: u32) -> Result<(), String> {
        self.surface(surface)?.lock().resize(width, height)
    }

    fn surface_write(&mut self, surface: u64, x: u32, y: u32, width: u32, height: u32, bgra: Vec<u8>) -> Result<(), String> {
        self.surface(surface)?.lock().write((x, y, width, height), &bgra)
    }

    fn surface_fill(&mut self, surface: u64, x: u32, y: u32, width: u32, height: u32, color: (u8, u8, u8)) -> Result<(), String> {
        self.surface(surface)?.lock().fill((x, y, width, height), color)
    }

    fn surface_copy(&mut self, surface: u64, x: u32, y: u32, width: u32, height: u32, to_x: u32, to_y: u32) -> Result<(), String> {
        self.surface(surface)?.lock().copy((x, y, width, height), (to_x, to_y))
    }

    fn surface_present(&mut self, surface: u64) {
        let Ok(shared) = self.surface(surface) else { return };
        let tell = shared.lock().present();
        self.surface_changed(surface, tell);
    }

    fn surface_cursor(&mut self, surface: u64, cursor: Option<CursorImage>) {
        let Ok(shared) = self.surface(surface) else { return };
        let cursor = cursor.filter(|c| c.width > 0 && c.height > 0 && c.width <= 256 && c.height <= 256 && c.bgra.len() == c.width as usize * c.height as usize * 4);
        let tell = {
            let mut fb = shared.lock();
            fb.cursor = cursor;
            fb.cursor_changed = true;
            fb.changed()
        };
        self.surface_changed(surface, tell);
    }

    fn surface_marks(&mut self, surface: u64, marks: Vec<SurfaceMark>) {
        let Ok(shared) = self.surface(surface) else { return };
        let tell = {
            let mut fb = shared.lock();
            fb.marks = marks.into_iter().take(4096).collect();
            fb.changed()
        };
        self.surface_changed(surface, tell);
    }

    fn page_retitle(&mut self, page: u64, title: String) {
        self.ask(Request::PageRetitle { page, title });
    }

    fn clipboard_text(&mut self) -> Option<String> {
        if !self.spec.permissions.clipboard {
            (self.sink)(Event::Said(Source::Log(Level::Warn), "clipboard-text: it may not read the clipboard (permissions.clipboard = true)".into()));
            return None;
        }
        self.ask_and_wait(Request::ClipboardText).flatten()
    }

    fn file_read(&mut self, path: String) -> Result<Vec<u8>, String> {
        let file = self.given_path(&path).or_else(|| self.host_path(&path)).ok_or_else(|| format!("{path}: not somewhere it may read"))?;
        let size = std::fs::metadata(&file).map_err(|e| format!("{path}: {e}"))?.len();
        if size > MAX_FILE {
            return Err(format!("{path}: file-read gives at most {} MiB", MAX_FILE >> 20));
        }
        self.read_held(&file, MAX_FILE).map_err(|e| e.replace(&file.display().to_string(), &path))
    }

    fn explorer_selected(&mut self) -> Option<String> {
        if !self.may_read_editor() {
            (self.sink)(Event::Said(Source::Log(Level::Warn), "explorer-selected: it may not read the editor (permissions.editor = \"read\")".into()));
            return None;
        }
        let path = self.ask_and_wait(Request::ExplorerSelected).flatten()?;
        self.given.push(PathBuf::from(&path));
        self.ask(Request::Granted(PathBuf::from(&path)));
        Some(path)
    }

    fn editor_cursor(&mut self) -> Option<CursorInfo> {
        if !self.may_read_editor() {
            (self.sink)(Event::Said(Source::Log(Level::Warn), "editor-cursor: it may not read the editor (permissions.editor = \"read\")".into()));
            return None;
        }
        *self.spec.cursor.read().ok()?
    }

    fn scratch_open(&mut self, title: String, text: String, language: Option<String>) -> u64 {
        let id = self.id();
        self.scratches.insert(id);
        self.ask(Request::ScratchOpen { id, title, text, language, beside: None });
        id
    }

    fn scratch_open_beside(&mut self, title: String, text: String, language: Option<String>, width: f32) -> u64 {
        let id = self.id();
        self.scratches.insert(id);
        self.ask(Request::ScratchOpen { id, title, text, language, beside: Some(beside_share(width)) });
        id
    }

    fn scratch_text(&mut self, scratch: u64) -> Option<String> {
        if !self.scratches.contains(&scratch) {
            return None;
        }
        self.ask_and_wait(|answer| Request::ScratchText { id: scratch, answer }).flatten()
    }

    fn scratch_set(&mut self, scratch: u64, text: String) -> Result<(), String> {
        if !self.scratches.contains(&scratch) {
            return Err(format!("{scratch} isn't one of its scratch buffers, open"));
        }
        self.ask(Request::ScratchSet { id: scratch, text });
        Ok(())
    }

    fn scratch_close(&mut self, scratch: u64) {
        if self.scratches.remove(&scratch) {
            self.ask(Request::ScratchClose(scratch));
        }
    }

    fn folder_list(&mut self, path: String) -> Result<Vec<FolderEntry>, String> {
        if !State::is_full_path(&path) {
            return Err(format!("{path}: a full path, of a folder Fenix gave it or a file it was given is in"));
        }
        let dir = PathBuf::from(&path);
        if dir.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(format!("{path}: not a folder it may list"));
        }
        // In a folder it was given, all of it; beside a file it was given,
        // the files it opens itself.
        let whole = self.given.iter().any(|g| dir.starts_with(g) && g.is_dir());
        let files: Vec<PathBuf> = self.given.iter().filter(|g| !g.is_dir()).cloned().chain(self.here_file()).collect();
        let beside = files.iter().any(|f| f.parent() == Some(dir.as_path())) || self.listed.contains(&dir);
        if !whole && !beside {
            return Err(format!("{path}: not a folder Fenix gave it, nor one a file it was given is in"));
        }
        let mut entries: Vec<FolderEntry> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{path}: {e}"))?
            .flatten()
            .filter_map(|e| {
                let meta = e.metadata().ok().filter(|m| m.is_file())?;
                let file = e.path();
                let mine = whole || files.contains(&file) || fenix_extensions::manifest::file_matches(&self.spec.opens, &file, None);
                mine.then(|| FolderEntry { name: e.file_name().to_string_lossy().into_owned(), size: meta.len() })
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries.truncate(MAX_LISTED);
        if !whole && !self.listed.contains(&dir) {
            self.listed.push(dir);
        }
        Ok(entries)
    }

    fn play_sound(&mut self, path: String) -> Result<(), String> {
        if !self.spec.permissions.sound {
            return Err("it may not play sounds (permissions.sound = true)".into());
        }
        let file = if path.replace('\\', "/").starts_with("/data/") {
            let file = self.host_path(&path).ok_or_else(|| format!("{path}: not somewhere it may read"))?;
            if std::fs::metadata(&file).map_err(|e| format!("{path}: {e}"))?.len() > MAX_SOUND {
                return Err(format!("{path}: a sound is at most {} MiB", MAX_SOUND >> 20));
            }
            file
        } else {
            self.own_file(&path, "a sound", "sounds/chime.wav", MAX_SOUND)?
        };
        // Read now, as it's checked: played from these bytes.
        let bytes = self.read_held(&file, MAX_SOUND).map_err(|e| e.replace(&file.display().to_string(), &path))?;
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(format!("{path}: a sound is a WAV file"));
        }
        self.ask(Request::PlaySound(bytes));
        Ok(())
    }

    fn git_refresh(&mut self) {
        self.ask(Request::GitRefresh);
    }

    fn page_focus(&mut self, page: u64) {
        self.ask(Request::PageFocus(page));
    }

    fn close_stdin(&mut self, process: u64) {
        if let Some(p) = self.processes.get_mut(&process) {
            p.stdin = None;
        }
    }

    fn write_stdin(&mut self, process: u64, data: Vec<u8>) -> Result<(), String> {
        let stdin = self.processes.get_mut(&process).and_then(|p| p.stdin.as_mut()).ok_or("no such process, or its input is closed")?;
        stdin.write_all(&data).and_then(|_| stdin.flush()).map_err(|e| e.to_string())
    }

    fn kill(&mut self, process: u64) {
        if let Some(p) = self.processes.get(&process) {
            if let Ok(mut child) = p.child.lock() {
                let _ = child.kill();
            }
        }
    }

    fn run_in_terminal(&mut self, cmd: Command, title: String, below: bool) -> Result<u64, String> {
        let (_, cwd) = self.checked(&cmd)?;
        let program = self.program(&cmd)?;
        let id = self.id();
        self.ask(Request::Terminal { id, program, args: cmd.args, cwd, title, below });
        Ok(id)
    }

    fn terminal_stop(&mut self, terminal: u64) {
        self.ask(Request::TerminalStop(terminal));
    }

    fn terminal_restart(&mut self, terminal: u64, cmd: Option<Command>, title: Option<String>) -> Result<(), String> {
        let program = match cmd {
            Some(cmd) => {
                let (_, cwd) = self.checked(&cmd)?;
                Some((self.program(&cmd)?, cmd.args, cwd))
            }
            None => None,
        };
        self.ask(Request::TerminalRestart { id: terminal, program, title });
        Ok(())
    }

    fn run_task(&mut self, name: String, cmd: Command) -> Result<u64, String> {
        let (_, cwd) = self.checked(&cmd)?;
        let program = self.program(&cmd)?;
        let id = self.id();
        let (args, env) = (cmd.args.iter().map(|a| self.filled(a)).collect(), cmd.env.iter().map(|(k, v)| (k.clone(), self.filled(v))).collect());
        self.ask(Request::Task { id, name, program, args, cwd, env });
        Ok(id)
    }

    fn http(&mut self, req: HttpRequest) -> Result<u64, String> {
        self.may_reach(&req.url)?;
        if let Some(proxy) = &req.options.proxy {
            if !proxy.starts_with("http://") {
                return Err(format!("{proxy}: a proxy is http://host:port"));
            }
            self.may_reach(proxy)?;
        }
        let tls = tls_config(&[], req.options.accept_invalid_certs, req.options.client_cert.as_ref())?;
        let id = self.id();
        let cancel = Arc::new(Cancel::default());
        self.requests.retain(|_, c| !c.done.load(Ordering::Relaxed));
        self.requests.insert(id, cancel.clone());
        // Redirects are followed here, each held to its permissions as the
        // first was.
        let (jobs, reach) = (self.jobs.clone(), self.reach());
        std::thread::spawn(move || {
            send_http(id, req, tls, &cancel, &reach, &jobs);
            cancel.done.store(true, Ordering::Relaxed);
        });
        Ok(id)
    }

    fn http_cancel(&mut self, request: u64) {
        if let Some(cancel) = self.requests.get(&request) {
            cancel.cancel();
        }
    }

    fn set_timer(&mut self, millis: u32, repeat: bool) -> u64 {
        let id = self.id();
        let stop = Arc::new(AtomicBool::new(false));
        let (jobs, flag) = (self.jobs.clone(), stop.clone());
        let every = Duration::from_millis(u64::from(millis.max(10)));
        std::thread::spawn(move || loop {
            std::thread::sleep(every);
            if flag.load(Ordering::Relaxed) || jobs.send(Job::Deliver(Notice::Timer(id))).is_err() || !repeat {
                break;
            }
        });
        self.timers.insert(id, stop);
        id
    }

    fn utc_offset(&mut self, at: i64) -> i32 {
        use chrono::{Offset, TimeZone};
        chrono::Local.timestamp_opt(at, 0).earliest().map(|t| t.offset().fix().local_minus_utc()).unwrap_or(0)
    }

    fn cancel_timer(&mut self, timer: u64) {
        if let Some(stop) = self.timers.remove(&timer) {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn open_text(&mut self, title: String, text: String, language: Option<String>) {
        self.ask(Request::OpenText { title, text, language, beside: None });
    }

    fn open_text_beside(&mut self, title: String, text: String, language: Option<String>, width: f32) {
        self.ask(Request::OpenText { title, text, language, beside: Some(beside_share(width)) });
    }

    fn open_url(&mut self, url: String) -> Result<(), String> {
        host_of(&url).ok_or_else(|| format!("{url}: only http(s) URLs open"))?;
        self.ask(Request::OpenUrl(url));
        Ok(())
    }

    fn clipboard_set(&mut self, text: String) {
        self.ask(Request::Clipboard(text));
    }

    fn clipboard_set_image(&mut self, png: Vec<u8>) -> Result<(), String> {
        if !png.starts_with(b"\x89PNG") {
            return Err("a picture for the clipboard is a PNG".into());
        }
        self.ask(Request::ClipboardImage(png));
        Ok(())
    }

    fn clipboard_image(&mut self) -> Option<Vec<u8>> {
        self.ask_and_wait(Request::ClipboardImageRead).flatten()
    }

    fn buffer_text(&mut self, path: String) -> Option<String> {
        let (path, open_only) = self.buffer_path(&path).ok()?;
        self.ask_and_wait(|answer| Request::BufferText { path, open_only, answer }).flatten()
    }

    fn buffer_edit(&mut self, path: String, edits: Vec<TextEdit>) -> Result<(), String> {
        self.may_write_editor()?;
        let (path, open_only) = self.buffer_path(&path)?;
        // Should it be opened for this, from what's read as it's checked.
        let text = if open_only { None } else { self.text_held(&path) };
        self.ask(Request::BufferEdit { path, open_only, edits, text });
        Ok(())
    }

    fn buffer_insert(&mut self, text: String) -> Result<(), String> {
        self.may_write_editor()?;
        self.ask(Request::BufferInsert(text));
        Ok(())
    }

    fn buffer_save(&mut self, path: String) -> Result<(), String> {
        self.may_write_editor()?;
        let (path, open_only) = self.buffer_path(&path)?;
        self.ask(Request::BufferSave { path, open_only });
        Ok(())
    }

    fn status_set(&mut self, text: Option<String>) {
        self.ask(Request::Status(text));
    }

    fn find_program(&mut self, name: String, hints: Vec<String>) -> Option<String> {
        let stem = name.trim().strip_suffix(".exe").unwrap_or(name.trim()).to_string();
        if stem.is_empty() || stem.contains(['/', '\\']) || !self.spec.permissions.process.iter().any(|p| p.eq_ignore_ascii_case(&stem)) {
            (self.sink)(Event::Said(Source::Log(Level::Warn), format!("find-program: {name} isn't a program this extension may run (permissions.process)")));
            return None;
        }
        // A path a setting of its holds (`${setting:python}`): yours,
        // so it comes before what's on `PATH`.
        let settings = self.spec.settings.read().map(|s| s.clone()).unwrap_or_default();
        let set = hints.iter().filter_map(|h| h.trim().strip_prefix("${setting:").and_then(|k| k.strip_suffix('}'))).filter_map(|k| settings.get(k)).map(|v| PathBuf::from(v.trim())).filter(|v| !v.as_os_str().is_empty()).find_map(|p| program_at(&p, &stem));
        if let Some(found) = set {
            let text = found.to_string_lossy().into_owned();
            self.found.push(text.clone());
            self.ask(Request::Found { name: stem, path: found });
            return Some(text);
        }
        // A hint under its project (`./gradlew`, `/project/gradlew`,
        //): the project's own wrapper, before what's on `PATH`.
        let project = hints.iter().map(|h| h.trim().replace('\\', "/")).filter_map(|h| {
            let rest = h.strip_prefix("./").or_else(|| h.strip_prefix("/project/"))?.to_string();
            self.host_path(&format!("/project/{rest}"))
        });
        if let Some(found) = project.into_iter().find_map(|p| program_at(&p, &stem)) {
            let text = found.to_string_lossy().into_owned();
            self.found.push(text.clone());
            self.ask(Request::Found { name: stem, path: found });
            return Some(text);
        }
        // A hint in its tools folder, as it spells it (`/tools/x/bin`).
        let hints: Vec<String> = hints.iter().map(|h| if h.replace('\\', "/").starts_with("/tools/") { self.host_path(h).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default() } else { h.clone() }).collect();
        let found = locate(&stem, self.own_tools().as_deref(), self.spec.tools_dir.as_deref(), &hints)?;
        let text = found.to_string_lossy().into_owned();
        self.found.push(text.clone());
        self.ask(Request::Found { name: stem, path: found });
        Some(text)
    }

    fn new_project(&mut self, args: String) {
        self.ask(Request::NewProject(args));
    }

    fn sheet_load(&mut self, path: String, frame_width: u32, frame_height: u32) -> Result<u32, String> {
        self.may_draw()?;
        let file = self.own_file(&path, "a sheet", "sprites/cat.png", MAX_SHEET)?;
        if frame_width == 0 || frame_height == 0 {
            return Err("a frame is at least a pixel".into());
        }
        let bytes = self.read_held(&file, MAX_SHEET).map_err(|e| e.replace(&file.display().to_string(), &path))?;
        let id = self.id() as u32;
        self.ask(Request::SheetLoad { id, bytes, frame: (frame_width, frame_height) });
        Ok(id)
    }

    fn model_load(&mut self, path: String) -> Result<ModelInfo, String> {
        self.may_draw()?;
        let file = self.own_file(&path, "a model", "models/cat.glb", fenix_model::MAX_MODEL)?;
        let bytes = self.read_held(&file, fenix_model::MAX_MODEL).map_err(|e| e.replace(&file.display().to_string(), &path))?;
        let model = fenix_model::Model::from_glb(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let (least, most) = model.bounds;
        let height = (most.y - least.y).max(1e-6);
        let aspect = (most.x - least.x).max(most.z - least.z) / height;
        let clips = model.clips.iter().map(|c| (c.name.clone(), c.seconds)).collect();
        let id = self.id() as u32;
        self.ask(Request::ModelLoad { id, model: Shared(Arc::new(model)) });
        Ok(ModelInfo { id, clips, aspect })
    }

    fn layer_open(&mut self, level: LayerLevel) -> Result<u64, String> {
        self.may_draw()?;
        let id = self.id();
        self.ask(Request::LayerOpen { id, level });
        Ok(id)
    }

    fn layer_set(&mut self, layer: u64, items: Vec<Item>) {
        self.ask(Request::LayerSet { layer, items });
    }

    fn layer_close(&mut self, layer: u64) {
        self.ask(Request::LayerClose(layer));
    }

    fn language_server_set(&mut self, languages: Vec<String>, cmd: Option<Command>) -> Result<(), String> {
        let program = match cmd {
            Some(cmd) => Some(self.checked_with_env(&cmd)?),
            None => None,
        };
        self.ask(Request::LanguageServer { languages, program });
        Ok(())
    }

    fn language_server_add(&mut self, name: String, languages: Vec<String>, cmd: Option<Command>) -> Result<(), String> {
        let name = name.trim().to_string();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err(format!("\"{name}\" isn't a server's name: letters, digits, -, _ and ."));
        }
        let program = match cmd {
            Some(cmd) => {
                if languages.iter().all(|l| l.trim().is_empty()) {
                    return Err(format!("{name}: no languages named"));
                }
                Some(self.checked_with_env(&cmd)?)
            }
            None => None,
        };
        self.ask(Request::LanguageServerAdd { name, languages, program });
        Ok(())
    }

    fn debug_adapter_set(&mut self, languages: Vec<String>, cmd: Option<Command>) -> Result<(), String> {
        let program = match cmd {
            Some(cmd) => Some(self.checked_with_env(&cmd)?),
            None => None,
        };
        self.ask(Request::DebugAdapter { languages, program });
        Ok(())
    }

    fn formatter_set(&mut self, languages: Vec<String>, cmd: Option<Command>, keep_output: bool) -> Result<(), String> {
        let program = match cmd {
            Some(cmd) => Some(self.checked_program(&cmd)?),
            None => None,
        };
        self.ask(Request::Formatter { languages, program, keep_output });
        Ok(())
    }

    fn diagnostics_set(&mut self, path: String, problems: Vec<Problem>) -> Result<(), String> {
        if problems.len() > MAX_PROBLEMS {
            return Err(format!("{} problems: a file has at most {MAX_PROBLEMS}", problems.len()));
        }
        if problems.iter().any(|p| p.fixes.iter().any(|f| f.title.trim().is_empty() || f.edits.is_empty())) {
            return Err("a fix has a title and edits".into());
        }
        let (path, open_only) = match self.reachable(&path) {
            Some(file) => (file, false),
            None if self.may_read_editor() && State::is_full_path(&path) => (PathBuf::from(&path), true),
            None => return Err(format!("{path}: not a file it may reach (or, with editor = \"read\", a full path)")),
        };
        self.ask(Request::Diagnostics { path, open_only, problems });
        Ok(())
    }

    fn http_download(&mut self, req: HttpRequest, path: String, sha256: Option<String>) -> Result<u64, String> {
        self.may_reach(&req.url)?;
        let slashed = path.replace('\\', "/");
        let root = match slashed.split('/').nth(1) {
            Some("tools") => self.own_tools(),
            Some("data") => Some(self.spec.data.clone()),
            _ => None,
        };
        let (root, file) = root.zip(self.host_path(&slashed)).filter(|(root, file)| file != root).ok_or_else(|| format!("{path}: a download goes under /tools or /data"))?;
        let want = match sha256 {
            Some(hex) if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) => Some(hex.to_ascii_lowercase()),
            Some(hex) => return Err(format!("{hex}: a SHA-256 is 64 hex digits")),
            None => None,
        };
        let id = self.id();
        let (jobs, reach) = (self.jobs.clone(), self.reach());
        std::thread::spawn(move || {
            let result = download(&req, &root, &file, want.as_deref(), &reach);
            State::deliver(&jobs, Notice::HttpResponse((id, result)));
        });
        Ok(id)
    }

    fn set_executable(&mut self, path: String) -> Result<(), String> {
        let slashed = path.replace('\\', "/");
        let file = self.host_path(&slashed).filter(|_| slashed.starts_with("/tools/")).ok_or_else(|| format!("{path}: only a file under /tools runs"))?;
        confined(&self.own_tools().ok_or("no tools folder")?, &file)?;
        // Not by way of a link: that could reach out of the folder.
        let meta = std::fs::symlink_metadata(&file).map_err(|e| format!("{path}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{path}: not a file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            // Read and run for you, and for others as they may read it.
            let runs = mode | 0o100 | ((mode & 0o044) >> 2);
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(runs)).map_err(|e| format!("{path}: {e}"))?;
        }
        Ok(())
    }

    fn debug_adapter_connect(&mut self, languages: Vec<String>, cmd: Option<Command>, port: u16) -> Result<(), String> {
        let program = match cmd {
            Some(cmd) => Some(self.checked_with_env(&cmd)?),
            None if port == 0 => return Err("a port to connect to, or a program to start".into()),
            None => {
                if !["127.0.0.1", "localhost"].iter().any(|host| self.may_connect(false, host, port).is_ok()) {
                    return Err(format!("localhost:{port} isn't somewhere this extension may connect to (permissions.sockets)"));
                }
                None
            }
        };
        self.ask(Request::DebugAdapterSocket { languages, program, port });
        Ok(())
    }

    fn decoration_rules_set(&mut self, rules: Vec<DecorationRule>) -> Result<(), String> {
        if !self.may_read_editor() {
            return Err("it may not read what you edit (permissions.editor)".into());
        }
        if rules.len() > MAX_RULES {
            return Err(format!("{} rules: at most {MAX_RULES}", rules.len()));
        }
        for rule in &rules {
            let pattern = regex::Regex::new(&rule.pattern).map_err(|e| format!("{}: {e}", rule.pattern))?;
            if rule.group as usize >= pattern.captures_len() {
                return Err(format!("{}: it has no group {}", rule.pattern, rule.group));
            }
            if rule.files.iter().all(|f| f.trim().is_empty()) {
                return Err(format!("{}: no files named", rule.pattern));
            }
        }
        self.ask(Request::DecorationRules(rules));
        Ok(())
    }

    fn page_file(&mut self, page: u64, path: Option<String>) -> Result<(), String> {
        let path = match path {
            Some(path) => Some(self.reachable(&path).ok_or_else(|| format!("{path}: not a file it may read"))?),
            None => None,
        };
        self.ask(Request::PageFile { page, path });
        Ok(())
    }

    fn platform(&mut self) -> PlatformInfo {
        let env = if cfg!(target_env = "musl") {
            "musl"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else if cfg!(target_env = "msvc") {
            "msvc"
        } else {
            ""
        };
        PlatformInfo { os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into(), env: env.into() }
    }

    fn language_server_restart(&mut self) {
        self.ask(Request::LanguageServerRestart);
    }

    fn process_id(&mut self) -> u32 {
        std::process::id()
    }

    fn language_server_settings(&mut self, name: String, settings: Option<String>) -> Result<(), String> {
        let name = name.trim().to_string();
        if !is_name(&name) {
            return Err(format!("\"{name}\" isn't a server's name or a language: letters, digits, -, _ and ."));
        }
        let settings = match settings {
            Some(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(value @ serde_json::Value::Object(_)) => Some(value),
                Ok(_) => return Err(format!("{name}: its settings are a JSON object")),
                Err(e) => return Err(format!("{name}: {e}")),
            },
            None => None,
        };
        self.ask(Request::LanguageServerSettings { name, settings });
        Ok(())
    }

    fn launches_set(&mut self, launches: Vec<LaunchConfig>) -> Result<(), String> {
        if launches.len() > MAX_LAUNCHES {
            return Err(format!("{} launches: at most {MAX_LAUNCHES}", launches.len()));
        }
        let mut checked = Vec::with_capacity(launches.len());
        for mut launch in launches {
            if launch.name.trim().is_empty() {
                return Err("a launch has a name".into());
            }
            if launch.languages.iter().all(|l| l.trim().is_empty()) {
                return Err(format!("{}: it names no languages", launch.name));
            }
            match serde_json::from_str::<serde_json::Value>(if launch.attributes.trim().is_empty() { "{}" } else { &launch.attributes }) {
                Ok(serde_json::Value::Object(_)) => {}
                Ok(_) => return Err(format!("{}: its attributes are a JSON object", launch.name)),
                Err(e) => return Err(format!("{}: its attributes: {e}", launch.name)),
            }
            // Its own paths (`/project/build/app`) as Fenix's.
            let own = |me: &Self, text: Option<String>| text.map(|t| if t.starts_with('/') { me.host_path(&t).map(|p| p.to_string_lossy().into_owned()).unwrap_or(t) } else { t });
            launch.program = own(self, launch.program.take());
            launch.cwd = own(self, launch.cwd.take());
            launch.env = launch.env.iter().map(|(k, v)| (k.clone(), self.filled(v))).collect();
            launch.args = launch.args.iter().map(|a| self.filled(a)).collect();
            checked.push(launch);
        }
        self.ask(Request::Launches(checked));
        Ok(())
    }

    fn launch_variable_set(&mut self, name: String, choices: Vec<PickerItem>) -> Result<(), String> {
        let name = name.trim().to_string();
        if !is_name(&name) || name.starts_with(['-', '.']) {
            return Err(format!("\"{name}\" isn't a variable's name: letters, digits, _, - and ."));
        }
        if LAUNCH_VARIABLES.iter().any(|own| own.eq_ignore_ascii_case(&name)) || name.contains(':') {
            return Err(format!("${{{name}}} is Fenix's own"));
        }
        if choices.len() > MAX_LAUNCHES {
            return Err(format!("{} choices: at most {MAX_LAUNCHES}", choices.len()));
        }
        self.ask(Request::LaunchVariable { name, choices });
        Ok(())
    }

    fn secret_set(&mut self, key: String, value: Option<String>) -> Result<(), String> {
        if !fenix_extensions::manifest::secret_allowed(&self.spec.permissions.secrets, &key) {
            return Err(format!("{key} isn't a secret this extension may keep (permissions.secrets)"));
        }
        if FENIX_TOKENS.contains(&key.as_str()) {
            return Err(format!("{key} is Fenix's own: it's set on the settings page"));
        }
        if value.as_ref().is_some_and(|v| v.len() > MAX_SECRET) {
            return Err(format!("a secret is at most {} KiB", MAX_SECRET >> 10));
        }
        let value = value.filter(|v| !v.is_empty());
        self.secrets_set.insert(key.clone(), value.clone());
        self.ask(Request::SecretSet { key, value: value.map(Hidden) });
        Ok(())
    }

    fn prompt_secret(&mut self, label: String) -> u64 {
        let id = self.id();
        self.ask(Request::Prompt { id, label, initial: String::new(), masked: true });
        id
    }

    fn tcp_listen(&mut self, port: u16) -> Result<(u64, u16), String> {
        let rules: Vec<ListenRule> = self.spec.permissions.listen.iter().filter_map(|e| ListenRule::parse(e).ok()).collect();
        if rules.is_empty() {
            return Err("it may listen on no port (permissions.listen)".into());
        }
        if !rules.iter().any(|r| r.takes(port)) {
            return Err(format!("port {port} isn't one this extension may listen on (permissions.listen: \"127.0.0.1:{port}\")"));
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
        let got = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let id = self.id();
        let closed = Arc::new(AtomicBool::new(false));
        let (jobs, stop) = (self.jobs.clone(), closed.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if jobs.send(Job::Accepted { listener: id, stream, peer }).is_err() {
                            return;
                        }
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => std::thread::sleep(Duration::from_millis(20)),
                    Err(e) => {
                        if !stop.swap(true, Ordering::Relaxed) {
                            State::deliver(&jobs, Notice::SocketClosed((id, Some(e.to_string()))));
                        }
                        return;
                    }
                }
            }
        });
        self.sockets.insert(id, Socket { writes: None, udp: None, shut: Arc::default(), closed, peers: Arc::default(), tcp: None, listening: Some(got) });
        Ok((id, got))
    }

    fn env_var(&mut self, name: String) -> Option<String> {
        let settings = self.spec.settings.read().map(|s| s.clone()).unwrap_or_default();
        // Its secrets' overrides are Fenix's: never by this way.
        let fenixs = name.len() >= 6 && name[..6].eq_ignore_ascii_case("FENIX_");
        if fenixs || !fenix_extensions::manifest::env_allowed(&self.spec.permissions.env, &name, &|k| settings.get(k).cloned()) {
            (self.sink)(Event::Said(Source::Log(Level::Warn), format!("env-var: {name} isn't a variable this extension may read (permissions.env)")));
            return None;
        }
        std::env::var(&name).ok()
    }

    fn agent_provide(&mut self, id: String, cmd: Option<Command>) -> Result<(), String> {
        let id = id.trim().to_string();
        if id.is_empty() {
            return Err("an agent's id is the one its manifest gives it (contributes.agents)".into());
        }
        let program = match cmd {
            Some(cmd) => Some(self.checked_with_env(&cmd)?),
            None => None,
        };
        self.ask(Request::AgentProvide { id, program });
        Ok(())
    }

    fn doctor_set(&mut self, checks: Vec<Check>) {
        self.ask(Request::Doctor(checks));
    }

    fn tasks_set(&mut self, tasks: Vec<Task>) -> Result<(), String> {
        let tasks = tasks.into_iter().map(|t| Ok((t.name, self.checked_program(&t.cmd)?, t.cmd.env.iter().map(|(k, v)| (k.clone(), self.filled(v))).collect()))).collect::<Result<Vec<_>, String>>()?;
        self.ask(Request::Tasks(tasks));
        Ok(())
    }

    fn call_service(&mut self, name: String, payload: String) -> u64 {
        let id = self.id();
        self.ask(Request::Service { id, name, payload });
        id
    }

    fn service_answer(&mut self, call: u64, answer: Result<String, String>) {
        self.ask(Request::ServiceAnswer { call, answer });
    }

    fn image_load(&mut self, bytes: Vec<u8>) -> Result<ImageInfo, String> {
        if bytes.len() as u64 > MAX_IMAGE {
            return Err(format!("an image is at most {} MiB", MAX_IMAGE >> 20));
        }
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().map_err(|e| e.to_string())?.into_dimensions().map_err(|e| format!("not a picture Fenix reads: {e}"))?;
        let id = self.id() as u32;
        self.ask(Request::ImageLoad { id, bytes });
        Ok(ImageInfo { id, width, height })
    }

    fn image_file(&mut self, path: String) -> Result<ImageInfo, String> {
        let file = self.reachable(&path).ok_or_else(|| format!("{path}: not somewhere it may read"))?;
        let size = std::fs::metadata(&file).map_err(|e| format!("{path}: {e}"))?.len();
        if size > MAX_IMAGE {
            return Err(format!("{path}: an image is at most {} MiB", MAX_IMAGE >> 20));
        }
        // Read now, as what's checked: the UI decodes it later, when a link
        // could have taken its place.
        let bytes = self.read_held(&file, MAX_IMAGE).map_err(|e| e.replace(&file.display().to_string(), &path))?;
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().map_err(|e| format!("{path}: {e}"))?.into_dimensions().map_err(|e| format!("{path}: {e}"))?;
        let id = self.id() as u32;
        self.ask(Request::ImageLoad { id, bytes });
        Ok(ImageInfo { id, width, height })
    }

    fn image_free(&mut self, image: u32) {
        self.ask(Request::ImageFree(image));
    }

    fn lsp_send(&mut self, server: u64, message: String) {
        let Some(out) = self.lsp_out.get_mut(&server) else { return };
        // Framed as a language server frames it on stdout.
        let framed = format!("Content-Length: {}\r\n\r\n{message}", message.len());
        if out.write_all(framed.as_bytes()).and_then(|_| out.flush()).is_err() {
            self.lsp_out.remove(&server);
        }
    }
}

/// Fenix's own tokens, which an extension may read when it asks for them
/// but never write (`secret-set`).
pub const FENIX_TOKENS: &[&str] = &["gitlab.token", "github.token", "jira.token"];

/// The most a secret may be (`secret-set`).
const MAX_SECRET: usize = 64 << 10;

/// What Fenix fills in in launch files itself: an extension's variables
/// can't be called these.
pub const LAUNCH_VARIABLES: &[&str] = &["file", "fileDir", "project", "module", "port", "tools", "pickProcess"];

/// A server's, a language's or a variable's name: letters, digits, `-`,
/// `_` and `.`.
fn is_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The most an HTTP response may be.
const MAX_BODY: u64 = 16 << 20;

/// The most redirects `http` and `http-download` follow, as ureq did.
const REDIRECTS: u32 = 5;

/// Whether a host `permissions.network` doesn't name may be reached: what
/// each redirect is held to.
type Reach<'a> = &'a dyn Fn(&str) -> Result<(), String>;

/// `req` made, its redirects followed here rather than by ureq (`agent`
/// makes each hop's, its own redirects off), up to `most` (with none, a
/// 3xx is the answer): each held to
/// `reach` as the first URL was, so a server can't send it on to a host
/// its manifest doesn't name. As browsers do, a 303 -- and a 301 or 302 of
/// anything but a GET or a HEAD -- goes on as a GET with no body, and what
/// vouches for it (`Authorization`, `Cookie`) isn't sent on to another
/// host. The answer, an error's status among them, and where it was.
fn follow(agent: &dyn Fn() -> ureq::Agent, req: &HttpRequest, most: u32, reach: Reach) -> Result<(ureq::Response, String), String> {
    let (mut method, mut url, mut body, mut headers) = (req.method.clone(), req.url.clone(), req.body.clone(), req.headers.clone());
    for _ in 0..=most {
        let mut request = agent().request(&method, &url);
        for (k, v) in &headers {
            request = request.set(k, v);
        }
        let response = match if body.is_empty() { request.call() } else { request.send_bytes(&body) } {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(e.to_string()),
        };
        let status = response.status();
        let Some(to) = response.header("location").filter(|_| most > 0 && matches!(status, 301 | 302 | 303 | 307 | 308)) else { return Ok((response, url)) };
        let next = url::Url::parse(&url).and_then(|base| base.join(to)).map_err(|e| format!("redirected to {to}: {e}"))?.to_string();
        reach(&next).map_err(|e| format!("redirected to {next}: {e}"))?;
        if status == 303 || (matches!(status, 301 | 302) && method != "GET" && method != "HEAD") {
            method = "GET".into();
            body = Vec::new();
        }
        if host_of(&next) != host_of(&url) {
            headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("cookie"));
        }
        url = next;
    }
    Err(format!("more than {most} redirects"))
}

/// `req`, made with Fenix's own client: system certificates, proxies
/// from the environment; its redirects held to `reach`.
#[cfg(test)]
fn fetch(req: &HttpRequest, reach: Reach) -> Result<HttpResponse, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).redirects(0).build();
    let (response, url) = follow(&|| agent.clone(), req, REDIRECTS, reach)?;
    let status = response.status();
    let headers = response.headers_names().into_iter().filter_map(|n| response.header(&n).map(|v| (n.clone(), v.to_string()))).collect();
    let mut body = Vec::new();
    response.into_reader().take(MAX_BODY + 1).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() as u64 > MAX_BODY {
        return Err("the response is bigger than an extension may take".into());
    }
    Ok(HttpResponse { status, headers, body, url, timing: untimed() })
}

/// The system's certificates, read once; the ones Fenix ships when it
/// has none.
fn roots() -> Arc<rustls::RootCertStore> {
    static ROOTS: std::sync::OnceLock<Arc<rustls::RootCertStore>> = std::sync::OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.add_parsable_certificates(rustls_native_certs::load_native_certs().unwrap_or_default());
            if roots.is_empty() {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            Arc::new(roots)
        })
        .clone()
}

/// Certificates in PEM.
fn pem_certs(pem: &str) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let certs = rustls_pemfile::certs(&mut pem.as_bytes()).collect::<Result<Vec<_>, _>>().map_err(|e| format!("a certificate: {e}"))?;
    if certs.is_empty() {
        return Err("no certificate in it (PEM, BEGIN CERTIFICATE)".into());
    }
    Ok(certs)
}

/// A server's certificate taken as `pinned` says, or any when `any`;
/// else checked against the system's, as always. Its signature on the
/// handshake is checked either way.
#[derive(Debug)]
struct Lenient {
    checked: Arc<rustls::client::WebPkiServerVerifier>,
    pinned: Vec<rustls::pki_types::CertificateDer<'static>>,
    any: bool,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for Lenient {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if self.any || self.pinned.iter().any(|p| p.as_ref() == end_entity.as_ref()) {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }
        self.checked.verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &rustls::pki_types::CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &rustls::pki_types::CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// TLS as an extension asks for it: the system's certificates, and
/// `pinned` (PEM) taken as they are, or any with `any`; a client
/// certificate when given.
fn tls_config(pinned: &[String], any: bool, client: Option<&ClientCert>) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone()).with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13]).map_err(|e| e.to_string())?;
    let pinned = pinned.iter().map(|p| pem_certs(p)).collect::<Result<Vec<_>, _>>()?.concat();
    let builder = if pinned.is_empty() && !any {
        builder.with_root_certificates(roots())
    } else {
        let checked = rustls::client::WebPkiServerVerifier::builder_with_provider(roots(), provider.clone()).build().map_err(|e| e.to_string())?;
        builder.dangerous().with_custom_certificate_verifier(Arc::new(Lenient { checked, pinned, any, provider }))
    };
    let config = match client {
        Some(client) => {
            let key = rustls_pemfile::private_key(&mut client.key.as_bytes()).map_err(|e| format!("the client key: {e}"))?.ok_or("no key in the client key (PEM, BEGIN PRIVATE KEY)")?;
            builder.with_client_auth_cert(pem_certs(&client.cert)?, key).map_err(|e| format!("the client certificate: {e}"))?
        }
        None => builder.with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

/// `host` as TLS names a server: a name, or an address.
fn server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>, String> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| format!("{host}: {e}"))
}

/// A local socket's two ends -- what's read, what's written -- and how
/// it's shut.
type Local = (Box<dyn Read + Send>, Box<dyn Write + Send>, Box<dyn Fn() + Send>);

/// The Unix socket `name` names: that path, or one of that name where
/// programs keep theirs.
#[cfg(unix)]
fn ipc_path(name: &str) -> Result<PathBuf, String> {
    if Path::new(name).is_absolute() {
        return Ok(PathBuf::from(name));
    }
    let dirs = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(std::env::var_os).map(PathBuf::from).chain([PathBuf::from("/tmp")]);
    dirs.map(|d| d.join(name)).find(|p| p.exists()).ok_or_else(|| format!("{name}: no such socket in $XDG_RUNTIME_DIR, $TMPDIR or /tmp"))
}

#[cfg(unix)]
fn ipc_open(name: &str, _closed: Arc<AtomicBool>) -> Result<Local, String> {
    let path = ipc_path(name)?;
    let stream = std::os::unix::net::UnixStream::connect(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (reading, kept) = (stream.try_clone().map_err(|e| e.to_string())?, stream.try_clone().map_err(|e| e.to_string())?);
    Ok((Box::new(reading), Box::new(stream), Box::new(move || {
        let _ = kept.shutdown(std::net::Shutdown::Both);
    })))
}

/// A named pipe, read only once there's something to read: a pipe
/// opened for both reads its handle's writes wait behind a read that
/// waits.
#[cfg(windows)]
struct PipeReader {
    pipe: std::fs::File,
    closed: Arc<AtomicBool>,
}

#[cfg(windows)]
impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        loop {
            if self.closed.load(Ordering::Relaxed) {
                return Ok(0);
            }
            let mut ready: u32 = 0;
            // SAFETY: the handle is this pipe's, open while `self` is; the
            // rest are a count it writes and nulls.
            let ok = unsafe { windows_sys::Win32::System::Pipes::PeekNamedPipe(self.pipe.as_raw_handle() as _, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut ready, std::ptr::null_mut()) };
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                // ERROR_BROKEN_PIPE: the other end closed it.
                return if e.raw_os_error() == Some(109) { Ok(0) } else { Err(e) };
            }
            if ready > 0 {
                let n = buf.len().min(ready as usize);
                return self.pipe.read(&mut buf[..n]);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(windows)]
fn ipc_open(name: &str, closed: Arc<AtomicBool>) -> Result<Local, String> {
    let path = format!(r"\\.\pipe\{}", name.strip_prefix(r"\\.\pipe\").unwrap_or(name));
    let pipe = std::fs::OpenOptions::new().read(true).write(true).open(&path).map_err(|e| format!("{path}: {e}"))?;
    let reading = pipe.try_clone().map_err(|e| e.to_string())?;
    let flag = closed.clone();
    Ok((Box::new(PipeReader { pipe: reading, closed }), Box::new(pipe), Box::new(move || flag.store(true, Ordering::Relaxed))))
}

/// How long `http-send` waits when it isn't told.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// A request `http-send` made, while it goes: given up (`http-cancel`),
/// it tells nothing more, and its connection is shut when Fenix has it
/// (https).
#[derive(Default)]
struct Cancel {
    cancelled: AtomicBool,
    done: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
}

impl Cancel {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(socket) = lock(&self.socket).take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// When each part of a request's last connection ended, as its resolver
/// and its TLS saw them.
#[derive(Debug, Default, Clone, Copy)]
struct Clock {
    dns: Option<Duration>,
    looked_up: Option<Instant>,
    tls_from: Option<Instant>,
    tls_done: Option<Instant>,
    first_byte: Option<Instant>,
}

/// TLS for `http-send`, timed.
struct Timed {
    config: Arc<rustls::ClientConfig>,
    clock: Arc<Mutex<Clock>>,
    cancel: Arc<Cancel>,
}

impl ureq::TlsConnector for Timed {
    fn connect(&self, dns_name: &str, mut io: Box<dyn ureq::ReadWrite>) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
        lock(&self.clock).tls_from = Some(Instant::now());
        if let Some(socket) = io.socket().and_then(|s| s.try_clone().ok()) {
            *lock(&self.cancel.socket) = Some(socket);
        }
        let name = server_name(dns_name).map_err(std::io::Error::other)?;
        let mut conn = rustls::ClientConnection::new(self.config.clone(), name).map_err(std::io::Error::other)?;
        while conn.is_handshaking() {
            conn.complete_io(&mut io)?;
        }
        lock(&self.clock).tls_done = Some(Instant::now());
        Ok(Box::new(TimedStream { tls: rustls::StreamOwned::new(conn, io), clock: self.clock.clone() }))
    }
}

/// A TLS stream that notes when the answer's first byte came.
struct TimedStream {
    tls: rustls::StreamOwned<rustls::ClientConnection, Box<dyn ureq::ReadWrite>>,
    clock: Arc<Mutex<Clock>>,
}

impl Read for TimedStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.tls.read(buf)?;
        if n > 0 {
            lock(&self.clock).first_byte.get_or_insert_with(Instant::now);
        }
        Ok(n)
    }
}

impl Write for TimedStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tls.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tls.flush()
    }
}

impl std::fmt::Debug for TimedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TimedStream")
    }
}

impl ureq::ReadWrite for TimedStream {
    fn socket(&self) -> Option<&TcpStream> {
        self.tls.get_ref().socket()
    }
}

/// The timing of an answer that wasn't timed: a download's.
fn untimed() -> HttpTiming {
    HttpTiming { dns: None, connect: None, tls: None, first_byte: 0, total: 0 }
}

fn micros(d: Duration) -> u64 {
    d.as_micros().min(u128::from(u64::MAX)) as u64
}

/// The headers of `response`, as an extension is given them.
fn headers_of(response: &ureq::Response) -> Vec<(String, String)> {
    response.headers_names().into_iter().flat_map(|n| response.all(&n).into_iter().map(|v| (n.clone(), v.to_string())).collect::<Vec<_>>()).collect()
}

/// `http`'s request `id` made, redirects followed here, each held to
/// `reach`: its answer delivered whole, or, streamed, told in parts as
/// it comes.
fn send_http(id: u64, request: HttpRequest, tls: Arc<rustls::ClientConfig>, cancel: &Arc<Cancel>, reach: &dyn Fn(&str) -> Result<(), String>, jobs: &mpsc::Sender<Job>) {
    let tell = |part: HttpPart| {
        if !cancel.cancelled.load(Ordering::Relaxed) {
            let _ = jobs.send(Job::Tell(Call::HttpEvent { request: id, part }));
        }
    };
    let options = request.options.clone();
    let answer = |result: Result<HttpResponse, String>| {
        if options.stream {
            tell(HttpPart::Response(result));
        } else if !cancel.cancelled.load(Ordering::Relaxed) {
            State::deliver(jobs, Notice::HttpResponse((id, result)));
        }
    };
    let failed = |why: String| answer(Err(why));
    let started = Instant::now();
    let limit = options.timeout.map(|ms| Duration::from_millis(u64::from(ms.max(1))));
    let clock = Arc::new(Mutex::new(Clock::default()));
    let proxy = match options.proxy.as_deref().map(ureq::Proxy::new).transpose() {
        Ok(proxy) => proxy,
        Err(e) => return failed(format!("{}: {e}", options.proxy.as_deref().unwrap_or_default())),
    };
    // Each hop's agent, its parts timed afresh.
    let agent = || {
        *lock(&clock) = Clock::default();
        let resolver = {
            let clock = clock.clone();
            move |netloc: &str| {
                let from = Instant::now();
                let found = netloc.to_socket_addrs().map(|a| a.collect());
                let mut c = lock(&clock);
                c.dns = Some(from.elapsed());
                c.looked_up = Some(Instant::now());
                found
            }
        };
        let timed = Timed { config: tls.clone(), clock: clock.clone(), cancel: cancel.clone() };
        let mut agent = ureq::AgentBuilder::new().redirects(0).tls_connector(Arc::new(timed)).resolver(resolver);
        if let Some(proxy) = &proxy {
            agent = agent.proxy(proxy.clone());
        }
        let left = |whole: Duration| whole.saturating_sub(started.elapsed()).max(Duration::from_millis(1));
        match (options.stream, limit) {
            (false, limit) => agent.timeout(left(limit.unwrap_or(HTTP_TIMEOUT))),
            (true, Some(wait)) => agent.timeout_connect(wait).timeout_read(wait),
            (true, None) => agent.timeout_connect(HTTP_TIMEOUT),
        }
        .build()
    };
    let (response, url) = match follow(&agent, &request, if options.follow_redirects { 10 } else { 0 }, reach) {
        Ok(answer) => answer,
        Err(e) => return failed(e),
    };
    let head = Instant::now();
    let timing = |end: Instant| {
        let c = *lock(&clock);
        let connected = c.tls_done.or(c.looked_up).unwrap_or(started);
        HttpTiming {
            dns: c.dns.map(micros),
            connect: c.tls_from.zip(c.looked_up).map(|(t, l)| micros(t.saturating_duration_since(l))),
            tls: c.tls_done.zip(c.tls_from).map(|(d, f)| micros(d.saturating_duration_since(f))),
            first_byte: micros(c.first_byte.unwrap_or(head).saturating_duration_since(connected)),
            total: micros(end.saturating_duration_since(started)),
        }
    };
    let (status, headers) = (response.status(), headers_of(&response));
    let mut reader = response.into_reader();
    if !options.stream {
        let mut body = Vec::new();
        if let Err(e) = reader.take(MAX_BODY + 1).read_to_end(&mut body) {
            return failed(e.to_string());
        }
        if body.len() as u64 > MAX_BODY {
            return failed("the response is bigger than an extension may take".into());
        }
        return answer(Ok(HttpResponse { status, headers, body, url, timing: timing(Instant::now()) }));
    }
    tell(HttpPart::Response(Ok(HttpResponse { status, headers, body: Vec::new(), url, timing: timing(head) })));
    let credit = Arc::new(AtomicUsize::new(0));
    let mut buf = vec![0u8; 64 << 10];
    while wait_for_room(&credit, &cancel.cancelled) {
        match reader.read(&mut buf) {
            Ok(0) => return tell(HttpPart::End(None)),
            Ok(n) => {
                credit.fetch_add(n, Ordering::Relaxed);
                if jobs.send(Job::Data { what: Call::HttpEvent { request: id, part: HttpPart::Chunk(buf[..n].to_vec()) }, credit: credit.clone(), len: n }).is_err() {
                    return;
                }
            }
            Err(e) => return tell(HttpPart::End(Some(e.to_string()))),
        }
    }
}

/// Where the file `file` has open is now, as the system spells it (as
/// `canonicalize` does): asked of the open file, so no link between is
/// followed. None where the system can't say.
fn opened_path(file: &std::fs::File) -> Option<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd;
        std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: `buf` is PATH_MAX bytes, what F_GETPATH writes at most.
        let done = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
        let len = buf.iter().position(|&b| b == 0)?;
        (done != -1).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
        let mut buf = vec![0u16; 32_768];
        // SAFETY: the handle is `file`'s, open; `buf` holds as many units
        // as it's said to. 0 is FILE_NAME_NORMALIZED | VOLUME_NAME_DOS: a
        // `\\?\C:\...` path, as `canonicalize` gives.
        let len = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle() as _, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        (len > 0 && len < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
    {
        let _ = file;
        None
    }
}

/// Whether `path` is in `root` as it is on disk too, links followed: a
/// link the extension made in its folders (WASI lets it point anywhere
/// with `..`) doesn't take what Fenix reads or writes for it out of them.
/// What isn't there yet is held to the nearest folder that is; a link to
/// nothing is refused, since what it names could be made later.
fn contained(root: &Path, path: &Path) -> Result<(), String> {
    let outside = || format!("{}: not in its own folder", path.display());
    if path.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(outside());
    }
    let real_root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let (mut at, mut rest) = (path, Vec::new());
    let real = loop {
        match at.canonicalize() {
            Ok(real) => break real,
            Err(_) if std::fs::symlink_metadata(at).is_ok() => return Err(outside()),
            Err(_) => {
                rest.push(at.file_name().ok_or_else(outside)?);
                at = at.parent().ok_or_else(outside)?;
            }
        }
    };
    // Both as the system spells them (`\\?\C:\...`, its case), so
    // they compare.
    let real = rest.into_iter().rev().fold(real, |p, part| p.join(part));
    if real.starts_with(&real_root) {
        Ok(())
    } else {
        Err(outside())
    }
}

/// Whether `file`'s folder is in `root` itself, not somewhere a link the
/// extension made there leads: what it writes or changes from the host's
/// side stays in its own folders.
fn confined(root: &Path, file: &Path) -> Result<(), String> {
    let (Ok(root), Some(Ok(dir))) = (root.canonicalize(), file.parent().map(Path::canonicalize)) else { return Err(format!("{}: no such folder", file.display())) };
    if dir.starts_with(&root) {
        Ok(())
    } else {
        Err(format!("{}: not in its own folder", file.display()))
    }
}

/// `req`'s body written to `file` in `root` (by way of `<file>.partial`),
/// checked against `sha256` when given; the answer's body is empty.
fn download(req: &HttpRequest, root: &Path, file: &Path, sha256: Option<&str>, reach: Reach) -> Result<HttpResponse, String> {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(30)).timeout_read(Duration::from_secs(60)).redirects(0).build();
    let (response, url) = follow(&|| agent.clone(), req, REDIRECTS, reach)?;
    let status = response.status();
    let headers = response.headers_names().into_iter().filter_map(|n| response.header(&n).map(|v| (n.clone(), v.to_string()))).collect();
    // An error's answer, and nothing written.
    if status >= 400 {
        return Ok(HttpResponse { status, headers, body: Vec::new(), url, timing: untimed() });
    }
    confined(root, file)?;
    let partial = file.with_file_name(format!("{}.partial", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let _ = std::fs::remove_file(&partial);
    let written = (|| {
        // A new file, never one that's there (a link to elsewhere).
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let (mut reader, mut hash, mut buf, mut total) = (response.into_reader().take(MAX_DOWNLOAD + 1), Sha256::new(), vec![0u8; 256 << 10], 0u64);
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_DOWNLOAD {
                return Err(format!("the download is bigger than {} GiB", MAX_DOWNLOAD >> 30));
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        match sha256 {
            Some(want) if want != got => Err(format!("its SHA-256 is {got}, not {want}")),
            _ => Ok(()),
        }
    })();
    match written.and_then(|_| std::fs::rename(&partial, file).map_err(|e| format!("{}: {e}", file.display()))) {
        Ok(()) => Ok(HttpResponse { status, headers, body: Vec::new(), url, timing: untimed() }),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// A pane beside's share of the focused one, as `page-open-beside` takes
/// it: a fraction, kept sensible.
fn beside_share(width: f32) -> f32 {
    if width.is_finite() { width.clamp(0.1, 0.9) } else { 0.5 }
}

/// A guest's stdout or stderr, a write at a time, as `Event::Said`.
#[derive(Clone)]
struct Output {
    source: Source,
    sink: Sink,
}

impl Output {
    fn send(&self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\r', '\n']);
        if !text.is_empty() {
            (self.sink)(Event::Said(self.source, text.to_string()));
        }
    }
}

impl IsTerminal for Output {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for Output {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Pollable for Output {
    async fn ready(&mut self) {}
}

impl OutputStream for Output {
    fn write(&mut self, bytes: bytes::Bytes) -> StreamResult<()> {
        self.send(&bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(64 * 1024)
    }
}

impl tokio::io::AsyncWrite for Output {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        self.send(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

enum Job {
    Call { call: u64, what: Call, ticks: u64 },
    /// What a thread of its own brings it, given to `handle`.
    Deliver(Notice),
    /// Bytes a socket, a process or a stream brings it: `len` of them,
    /// given back to `credit` once delivered, so its reader waits while
    /// it's behind.
    Data { what: Call, credit: Arc<AtomicUsize>, len: usize },
    /// A call of Fenix's own a thread makes (`http-event`).
    Tell(Call),
    /// Its language server `server` started: its messages go to `out`.
    LspOpen { server: u64, out: Box<dyn Write + Send> },
    /// A message for its language server `server`.
    Lsp { server: u64, message: String },
    LspClose(u64),
    /// A connection made to its listener `listener` (`tcp-listen`).
    Accepted { listener: u64, stream: TcpStream, peer: SocketAddr },
    Stop,
}

/// Where Fenix sends an extension's language servers their messages, from
/// any thread: its `lsp-message`, in order with its other calls.
#[derive(Clone)]
pub struct LspPort {
    jobs: mpsc::Sender<Job>,
}

impl LspPort {
    /// Language server `server` starts: what it sends goes to `out`.
    pub fn open(&self, server: u64, out: impl Write + Send + 'static) {
        let _ = self.jobs.send(Job::LspOpen { server, out: Box::new(out) });
    }

    /// One message for language server `server`. False once the
    /// extension is gone.
    pub fn send(&self, server: u64, message: String) -> bool {
        self.jobs.send(Job::Lsp { server, message }).is_ok()
    }

    pub fn close(&self, server: u64) {
        let _ = self.jobs.send(Job::LspClose(server));
    }
}

/// A running extension: a worker thread with its store.
pub struct Worker {
    jobs: mpsc::Sender<Job>,
    next: u64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// Starts `spec` on a thread of its own: loads it, activates it, then
    /// takes calls. Every event goes to `events`, on that thread or one of
    /// the extension's own.
    pub fn start(engine: &Engine, spec: Spec, events: impl Fn(Event) + Send + Sync + 'static) -> Worker {
        let (jobs, rx) = mpsc::channel();
        let engine = engine.clone();
        let sink: Sink = Arc::new(events);
        let back = jobs.clone();
        let thread = std::thread::Builder::new()
            .name(format!("fenix-ext-{}", spec.id))
            .spawn(move || run(engine, spec, sink, back, rx))
            .expect("a thread for the extension");
        Worker { jobs, next: 0, thread: Some(thread) }
    }

    /// Where its language servers' messages are sent.
    pub fn lsp_port(&self) -> LspPort {
        LspPort { jobs: self.jobs.clone() }
    }

    /// Asks for `what`, with `ticks` epoch ticks to finish it in (`0`: the
    /// extension's usual); its end comes back as `Event::Finished` with the
    /// number returned here.
    pub fn call(&mut self, what: Call, ticks: u64) -> u64 {
        self.next += 1;
        let _ = self.jobs.send(Job::Call { call: self.next, what, ticks });
        self.next
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The worker thread.
fn run(engine: Engine, spec: Spec, sink: Sink, back: mpsc::Sender<Job>, jobs: mpsc::Receiver<Job>) {
    let id = spec.id.clone();
    let call_ticks = spec.call_ticks;
    let started = (|| -> wasmtime::Result<(Store<State>, Extension)> {
        let component = load(&engine, &spec.wasm, &spec.cache)?;
        let mut linker: Linker<State> = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Extension::add_to_linker::<State, wasmtime::component::HasSelf<State>>(&mut linker, |s| s)?;
        // Its own folder, always; the project if it asks. No network, no
        // environment, no arguments. What it prints goes to its output
        // page.
        std::fs::create_dir_all(&spec.data).map_err(|e| wasmtime::format_err!("{}: {e}", spec.data.display()))?;
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdout(Output { source: Source::Stdout, sink: sink.clone() }).stderr(Output { source: Source::Stderr, sink: sink.clone() });
        wasi.preopened_dir(&spec.data, "/data", FsPerms::ReadWrite)?;
        let listed = spec.permissions.process.iter().any(|p| p.replace('\\', "/").starts_with("/tools/"));
        if let Some(tools) = spec.tools_dir.as_ref().filter(|_| listed).map(|d| d.join(&spec.id)) {
            if std::fs::create_dir_all(&tools).is_ok() {
                wasi.preopened_dir(&tools, "/tools", FsPerms::ReadWrite)?;
            }
        }
        for (name, dir) in spec.folders.iter().filter(|(_, d)| d.is_dir()) {
            wasi.preopened_dir(dir, format!("/folders/{name}"), FsPerms::ReadWrite)?;
        }
        if spec.permissions.fonts {
            for (i, dir) in spec.fonts.iter().filter(|d| d.is_dir()).enumerate() {
                wasi.preopened_dir(dir, format!("/fonts/{i}"), FsPerms::ReadOnly)?;
            }
        }
        if let Some(project) = spec.project.as_ref().filter(|p| p.is_dir()) {
            match spec.permissions.filesystem {
                Filesystem::ProjectRead => {
                    wasi.preopened_dir(project, "/project", FsPerms::ReadOnly)?;
                }
                Filesystem::ProjectWrite => {
                    wasi.preopened_dir(project, "/project", FsPerms::ReadWrite)?;
                }
                Filesystem::None | Filesystem::UserPicked => {}
            }
        }
        let limits = crate::limits(spec.memory);
        let activate_ticks = spec.activate_ticks;
        let state = State {
            wasi: wasi.build(), table: ResourceTable::new(), limits, sink: sink.clone(), spec, jobs: back, next: 0, processes: HashMap::new(), timers: HashMap::new(), found: Vec::new(), lsp_out: HashMap::new(),
            sockets: HashMap::new(), surfaces: HashMap::new(), given: Vec::new(), restoring: None, listed: Vec::new(), scratches: HashSet::new(), requests: HashMap::new(),
            background: HashSet::new(), secrets_set: HashMap::new(),
        };
        let mut store = Store::new(&engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(activate_ticks);
        let instance = linker.instantiate(&mut store, &component)?;
        let ext = Extension::new(&mut store, &instance)?;
        ext.call_activate(&mut store)?.map_err(|e| wasmtime::format_err!("{e}"))?;
        Ok((store, ext))
    })();
    let mut running = match started {
        Ok(running) => {
            sink(Event::Ready);
            Some(running)
        }
        Err(e) => {
            sink(Event::Stopped(format!("{id} couldn't start: {}", describe(&e))));
            None
        }
    };
    let mut stopped_why = format!("{id} isn't running");
    while let Ok(job) = jobs.recv() {
        // Bytes handed back to their reader once delivered (or dropped).
        let mut taken: Option<(Arc<AtomicUsize>, usize)> = None;
        let (call, what, ticks) = match job {
            Job::Stop => break,
            Job::Call { call, what, ticks } => (Some(call), what, ticks),
            Job::Deliver(notice) => (None, Call::Deliver(notice), 0),
            Job::Data { what, credit, len } => {
                taken = Some((credit, len));
                (None, what, 0)
            }
            Job::Tell(what) => (None, what, 0),
            Job::Lsp { server, message } => (None, Call::Lsp { server, message }, 0),
            Job::LspOpen { server, out } => {
                if let Some((store, _)) = running.as_mut() {
                    store.data_mut().lsp_out.insert(server, out);
                }
                continue;
            }
            Job::LspClose(server) => {
                if let Some((store, _)) = running.as_mut() {
                    store.data_mut().lsp_out.remove(&server);
                }
                continue;
            }
            // Its own socket from now on, if it still listens and hears
            // of it; else let go.
            Job::Accepted { listener, stream, peer } => {
                let Some((store, _)) = running.as_mut() else { continue };
                let listens = store.data().sockets.get(&listener).is_some_and(|s| s.listening.is_some() && !s.closed.load(Ordering::Relaxed));
                if !listens {
                    continue;
                }
                let Some(socket) = store.data_mut().adopt_tcp(stream) else { continue };
                (None, Call::SocketAccepted { listener, socket, peer: peer.to_string() }, 0)
            }
        };
        let _release = Release(taken);
        let Some((store, ext)) = running.as_mut() else {
            if let Some(call) = call {
                sink(Event::Finished { call, result: Err(stopped_why.clone()) });
            }
            continue;
        };
        // What Fenix gives it to read: a file it opens, a repository.
        match &what {
            Call::Grant(path) => {
                store.data_mut().given.push(path.clone());
                if let Some(call) = call {
                    sink(Event::Finished { call, result: Ok(Reply::Done) });
                }
                continue;
            }
            Call::Deliver(Notice::Opened(path)) => store.data_mut().given.push(PathBuf::from(path)),
            Call::Deliver(Notice::TaskFinished((task, _))) => {
                store.data_mut().background.remove(task);
            }
            _ => {}
        }
        store.set_epoch_deadline(if ticks == 0 { call_ticks } else { ticks });
        let result: wasmtime::Result<Result<Reply, String>> = match what {
            Call::SurfaceInput { surface, input } => ext.call_surface_input(&mut *store, surface, input).map(|()| Ok(Reply::Done)),
            Call::Grant(_) => Ok(Ok(Reply::Done)),
            Call::RunCommand(id) => ext.call_run_command(&mut *store, &id).map(|r| r.map(|()| Reply::Done)),
            Call::Deliver(notice) => ext.call_handle(&mut *store, &notice).map(|()| Ok(Reply::Done)),
            Call::Render { page, width, height } => ext.call_page_render(&mut *store, page, width, height).map(|p| Ok(Reply::Page(Box::new(p)))),
            Call::KeyPress { page, key } => ext.call_page_key_press(&mut *store, page, key).map(|()| Ok(Reply::Done)),
            Call::Mouse { page, event } => ext.call_page_mouse(&mut *store, page, event).map(|()| Ok(Reply::Done)),
            Call::Key { page, key } => ext.call_page_key(&mut *store, page, key).map(|()| Ok(Reply::Done)),
            Call::PageState(page) => ext.call_page_state(&mut *store, page).map(|s| Ok(Reply::State(s))),
            Call::PageRestore { page, kind, state } => {
                store.data_mut().restoring = Some(page);
                let restored = ext.call_page_restore(&mut *store, page, &kind, &state).map(|ok| Ok(Reply::Restored(ok)));
                store.data_mut().restoring = None;
                restored
            }
            Call::PageClosed(page) => {
                // Its pixels go with it, as with `page-close`.
                store.data_mut().surfaces.remove(&page);
                ext.call_page_closed(&mut *store, page).map(|()| Ok(Reply::Done))
            }
            Call::BufferChanged(what) => ext.call_buffer_changed(&mut *store, &what).map(|()| Ok(Reply::Done)),
            Call::ScratchClosed(scratch) => {
                store.data_mut().scratches.remove(&scratch);
                ext.call_scratch_closed(&mut *store, scratch).map(|()| Ok(Reply::Done))
            }
            Call::FocusMoved(focus) => ext.call_focus_moved(&mut *store, &focus).map(|()| Ok(Reply::Done)),
            Call::HttpEvent { request, part } => {
                let given_up = store.data().requests.get(&request).is_some_and(|c| c.cancelled.load(Ordering::Relaxed));
                if given_up { Ok(Ok(Reply::Done)) } else { ext.call_http_event(&mut *store, request, &part).map(|()| Ok(Reply::Done)) }
            }
            Call::FileSaved(path) => ext.call_file_saved(&mut *store, &path).map(|()| Ok(Reply::Done)),
            Call::ViewChanged(views) => ext.call_view_changed(&mut *store, &views).map(|()| Ok(Reply::Done)),
            Call::DecorationClicked { buffer, id } => ext.call_decoration_clicked(&mut *store, &buffer, id).map(|()| Ok(Reply::Done)),
            Call::TaskStarted(name) => ext.call_task_started(&mut *store, &name).map(|()| Ok(Reply::Done)),
            Call::DebugChanged(state) => ext.call_debug_changed(&mut *store, &state).map(|()| Ok(Reply::Done)),
            Call::SocketAccepted { listener, socket, peer } => ext.call_socket_accepted(&mut *store, listener, socket, &peer).map(|()| Ok(Reply::Done)),
            Call::AgentTurnEnded(turn) => ext.call_agent_turn_ended(&mut *store, &turn).map(|()| Ok(Reply::Done)),
            Call::Serve { call, name, payload } => ext.call_serve(&mut *store, call, &name, &payload).map(|r| Ok(Reply::Served(r))),
            Call::Paste { page, text } => ext.call_page_paste(&mut *store, page, &text).map(|()| Ok(Reply::Done)),
            Call::Lsp { server, message } => ext.call_lsp_message(&mut *store, server, &message).map(|()| Ok(Reply::Done)),
        };
        match result {
            Ok(said) => {
                if let Some(call) = call {
                    sink(Event::Finished { call, result: said });
                }
            }
            // A trap: this call's over, and so is the extension.
            Err(e) => {
                let why = describe(&e);
                stopped_why = format!("{id} stopped: {why}");
                if let Some(call) = call {
                    sink(Event::Finished { call, result: Err(why) });
                }
                sink(Event::Stopped(stopped_why.clone()));
                running = None;
            }
        }
    }
}

/// Bytes a reader brought, given back to it when this goes.
struct Release(Option<(Arc<AtomicUsize>, usize)>);

impl Drop for Release {
    fn drop(&mut self) {
        if let Some((credit, len)) = self.0.take() {
            credit.fetch_sub(len, Ordering::Relaxed);
        }
    }
}

/// A trap or load error, said plainly.
fn describe(e: &wasmtime::Error) -> String {
    // The store's limit refused it more memory (`limits`).
    if format!("{e:#}").contains("forcing trap when growing memory") {
        return "it ran out of memory (it's over its limit)".into();
    }
    match e.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::Interrupt) => "it took too long and was stopped".into(),
        Some(wasmtime::Trap::StackOverflow) => "it ran out of stack (endless recursion?)".into(),
        Some(wasmtime::Trap::UnreachableCodeReached) => "it crashed (a panic, or unreachable code)".into(),
        Some(trap) => format!("it crashed: {trap}"),
        None => format!("{e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server on this machine that answers one request with `body`.
    fn serve_once(body: &'static [u8]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}/server.tar.gz")
    }

    #[test]
    fn a_download_is_written_where_it_was_asked_and_checked() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("server.tar.gz");
        let req = |url: String| HttpRequest { method: "GET".into(), url, headers: Vec::new(), body: Vec::new(), options: HttpOptions { timeout: None, follow_redirects: true, accept_invalid_certs: false, client_cert: None, proxy: None, stream: false } };
        let anywhere = |_: &str| Ok(());
        let sum: String = Sha256::digest(b"release").iter().map(|b| format!("{b:02x}")).collect();
        let answer = download(&req(serve_once(b"release")), root.path(), &file, Some(&sum), &anywhere).unwrap();
        assert_eq!((answer.status, answer.body.len()), (200, 0));
        assert_eq!(std::fs::read(&file).unwrap(), b"release");
        let wrong = "0".repeat(64);
        let err = download(&req(serve_once(b"tampered")), root.path(), &file, Some(&wrong), &anywhere).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert_eq!(std::fs::read(&file).unwrap(), b"release", "kept as it was");
        assert!(!root.path().join("server.tar.gz.partial").exists());
        assert!(download(&req(serve_once(b"x")), root.path(), &root.path().join("missing/x"), None, &anywhere).is_err(), "into a folder that's there");
    }

    /// A link it made in its folder doesn't take Fenix's writes out of it.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_its_folder_is_not_followed() {
        let (root, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join("out")).unwrap();
        assert!(confined(root.path(), &root.path().join("out/x")).is_err());
        std::fs::create_dir(root.path().join("in")).unwrap();
        assert!(confined(root.path(), &root.path().join("in/x")).is_ok());
    }

    /// What Fenix reads or writes for it stays in its folders on disk:
    /// not by a link out (`..` is a target WASI lets it make), nor one to
    /// nothing; a link within, and what isn't there yet, are fine.
    #[cfg(unix)]
    #[test]
    fn a_path_is_held_to_its_folder_as_it_is_on_disk() {
        let (root, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = root.path().join("data");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(elsewhere.path().join("secret"), "x").unwrap();
        std::fs::write(root.join("mine"), "x").unwrap();
        std::os::unix::fs::symlink("../..", root.join("up")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path().join("secret"), root.join("far")).unwrap();
        std::os::unix::fs::symlink("../nothing-yet", root.join("dangling")).unwrap();
        std::os::unix::fs::symlink("mine", root.join("near")).unwrap();
        for out in ["up", "up/x", "far", "dangling", "dangling/x"] {
            assert!(contained(&root, &root.join(out)).is_err(), "{out}");
        }
        for fine in ["", "mine", "near", "new/file", "new"] {
            assert!(contained(&root, &root.join(fine)).is_ok(), "{fine}");
        }
        assert!(contained(&root, &root.join("new/../../x")).is_err());
    }

    /// where a file opened is, asked of the open file: a link
    /// that led to it doesn't show -- what `read_held` holds to its
    /// folders, however the path was swapped after it was checked.
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn an_open_file_says_where_it_is() {
        let (root, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let secret = elsewhere.path().join("secret");
        std::fs::write(&secret, "x").unwrap();
        std::fs::write(root.path().join("mine"), "x").unwrap();
        let mine = std::fs::File::open(root.path().join("mine")).unwrap();
        assert_eq!(opened_path(&mine), Some(root.path().join("mine").canonicalize().unwrap()));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret, root.path().join("link")).unwrap();
            let through = std::fs::File::open(root.path().join("link")).unwrap();
            let at = opened_path(&through).unwrap();
            assert_eq!(at, secret.canonicalize().unwrap());
            assert!(!at.starts_with(root.path().canonicalize().unwrap()));
        }
    }

    /// A server on this machine that answers each of `answers` in turn,
    /// one a connection: its URL.
    fn serve(answers: Vec<String>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}/a")
    }

    fn redirect(to: &str) -> String {
        format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// A redirect is followed only to a host it may reach: the first URL
    /// is checked before the request, and each one it's sent on to here.
    #[test]
    fn a_redirect_goes_only_where_it_may_reach() {
        let req = |url: String| HttpRequest { method: "GET".into(), url, headers: vec![("Authorization".into(), "token".into())], body: Vec::new(), options: HttpOptions { timeout: None, follow_redirects: true, accept_invalid_certs: false, client_cert: None, proxy: None, stream: false } };
        let network = vec!["127.0.0.1".to_string()];
        let settings = RwLock::new(HashMap::new());
        let reach = |url: &str| may_reach(&network, &settings, url);
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nhere".to_string();
        let answer = fetch(&req(serve(vec![redirect("/b"), ok.clone()])), &reach).unwrap();
        assert_eq!((answer.status, answer.body.as_slice()), (200, &b"here"[..]), "on the same host");
        let err = fetch(&req(serve(vec![redirect("http://elsewhere.example/x")])), &reach).unwrap_err();
        assert!(err.starts_with("redirected to http://elsewhere.example/x") && err.contains("permissions.network"), "{err}");
        let root = tempfile::tempdir().unwrap();
        let err = download(&req(serve(vec![redirect("https://evil.example/x")])), root.path(), &root.path().join("x"), None, &reach).unwrap_err();
        assert!(err.contains("permissions.network"), "{err}");
        assert!(!root.path().join("x").exists());
        let err = fetch(&req(serve((0..7).map(|_| redirect("/again")).collect())), &reach).unwrap_err();
        assert!(err.contains("more than 5 redirects"), "{err}");
    }

    #[test]
    fn its_own_folders_are_told_from_full_paths_of_this_machine() {
        for guest in ["/data", "/data/places.tsv", "/project/a.pdf", "/folders/notes/x.md", "/fonts/0/a.ttf", "\\data\\x"] {
            assert!(is_guest_path(guest), "{guest}");
        }
        // A full path on Linux starts with `/` too: it's not one of them.
        for full in ["/home/me/refs/a.pdf", "/database/x", "/projects/a.pdf", "C:/refs/a.pdf", "/"] {
            assert!(!is_guest_path(full), "{full}");
        }
    }

    #[test]
    fn a_path_in_a_folder_finds_the_longest_name_it_starts_with() {
        let folders = vec![("notes".to_string(), PathBuf::from("N")), ("mib/OPS".to_string(), PathBuf::from("O")), ("mib/OPS2".to_string(), PathBuf::from("P"))];
        assert_eq!(in_folder(&folders, "notes/a.md"), Some((Path::new("N"), "/a.md")));
        assert_eq!(in_folder(&folders, "mib/OPS/ccf.dat"), Some((Path::new("O"), "/ccf.dat")));
        assert_eq!(in_folder(&folders, "mib/OPS2/ccf.dat"), Some((Path::new("P"), "/ccf.dat")), "not OPS's");
        assert_eq!(in_folder(&folders, "mib/OPS"), Some((Path::new("O"), "")));
        assert_eq!(in_folder(&folders, "mib/x.dat"), None, "mib alone isn't one");
        assert_eq!(in_folder(&folders, "notesx/a.md"), None);
    }

    #[test]
    fn a_urls_host_is_read() {
        assert_eq!(host_of("https://Jira.Example.com:8443/rest?x=1").as_deref(), Some("jira.example.com"));
        assert_eq!(host_of("http://user@host/x").as_deref(), Some("host"));
        assert_eq!(host_of("ftp://x"), None);
    }
}
