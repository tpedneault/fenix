//! The test extension: each command exercises one thing the host must
//! get right.

wit_bindgen::generate!({ path: "../../../../fenix-extension-api/wit", world: "extension" });

use std::cell::RefCell;

use fenix::extension::host;
use fenix::extension::types::{
    Anchor, Canvas, CellSpan, Cells, Check, Command, CursorImage, Ease, Glide, Health, HomeMark, HomeRow, HomeSection, HttpRequest, Item, KeyHint, LayerLevel, Level, Line, PickerItem, Picture,
    Place, Popup, Pose, Role, Section, Span, Style, SurfaceKey, SurfaceMark, SurfaceOptions, SurfaceScale, Task,
    CellStyle, Decoration, DecorationLook, Shade, VerticalRule, VirtualText,
    Problem, Severity,
    DecorationRule, HttpOptions, TlsOptions,
    CellColor,
    LaunchConfig, LaunchRequest, QuickFix, TextEdit,
};

struct Hello;

thread_local! {
    /// What its page counts, by page.
    static COUNTS: RefCell<Vec<(u64, u32)>> = const { RefCell::new(Vec::new()) };
    /// A service call it answers later, and the timer it waits on.
    static LATER: RefCell<Option<(u64, u64, String)>> = const { RefCell::new(None) };
    /// The image it loaded (`hello.v6`).
    static IMAGE: RefCell<Option<u32>> = const { RefCell::new(None) };
}

fn log(message: &str) {
    host::log(Level::Info, message);
}

#[allow(unconditional_recursion)]
fn down(n: u64) -> u64 {
    // Not a tail call: each level keeps a frame.
    std::hint::black_box(down(n + 1)) + 1
}

fn count(page: u64) -> u32 {
    COUNTS.with(|c| c.borrow().iter().find(|(p, _)| *p == page).map_or(0, |(_, n)| *n))
}

fn set_count(page: u64, n: u32) {
    COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(p, _)| *p != page);
        c.push((page, n));
    });
}

thread_local! {
    /// Each page's kind, as it was opened.
    static KINDS: RefCell<Vec<(u64, String)>> = const { RefCell::new(Vec::new()) };
}

fn page_kind(page: u64) -> String {
    KINDS.with(|k| k.borrow().iter().find(|(p, _)| *p == page).map(|(_, kind)| kind.clone()).unwrap_or_default())
}

fn get(url: &str) -> HttpRequest {
    HttpRequest { method: "GET".into(), url: url.into(), headers: Vec::new(), body: Vec::new(), options: HttpOptions { timeout: None, follow_redirects: true, accept_invalid_certs: false, client_cert: None, proxy: None, stream: false } }
}

/// A page with nothing on it.
fn empty_page() -> Page {
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

fn cmd(program: &str, args: &[&str]) -> Command {
    Command { program: program.into(), args: args.iter().map(|a| a.to_string()).collect(), cwd: None, env: Vec::new() }
}

thread_local! {
    /// How many bytes its raw process wrote.
    static RAW: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The page or surface it opened last, for `hello.focus`.
    static LAST: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// A socket waiting for its server's yes to STARTTLS (`hello.starttls`),
    /// and the TLS it then asks for.
    static STARTING: RefCell<Option<(u64, TlsOptions)>> = const { RefCell::new(None) };
}

extern "C" {
    /// wasi-libc's: a link, made the way WASI lets a guest make one.
    fn symlink(target: *const std::ffi::c_char, path: *const std::ffi::c_char) -> i32;
}

/// The TLS its settings ask for: the certificate `hello.cert` pinned, or
/// any with `hello.any`.
fn tls() -> TlsOptions {
    TlsOptions { server_name: Some("localhost".into()), pinned: host::setting("hello.cert").into_iter().collect(), accept_invalid: host::setting("hello.any").is_some(), client_cert: None }
}

/// The echo server its setting `hello.echo` names, `host:port`.
fn echo_address() -> Result<(String, u16), String> {
    let address = host::setting("hello.echo").ok_or("no hello.echo")?;
    let (host_name, port) = address.rsplit_once(':').ok_or("hello.echo is host:port")?;
    Ok((host_name.to_string(), port.parse().map_err(|_| "hello.echo's port")?))
}

impl Guest for Hello {
    fn activate() -> Result<(), String> {
        log("hello is up");
        println!("and printing to stdout");
        Ok(())
    }

    fn run_command(id: String) -> Result<(), String> {
        match id.as_str() {
            "hello.greet" => {
                log("hello, Fenix");
                host::notify(Level::Info, "Hello says hello");
                Ok(())
            }
            "hello.fail" => Err("asked to fail".into()),
            "hello.spin" => loop {
                std::hint::spin_loop();
            },
            "hello.panic" => panic!("asked to panic"),
            "hello.recurse" => {
                down(0);
                Ok(())
            }
            "hello.hog" => {
                let mut kept: Vec<Vec<u8>> = Vec::new();
                loop {
                    kept.push(vec![1; 16 << 20]);
                    std::hint::black_box(&kept);
                }
            }
            // Nothing outside /data: no file of Windows' to read.
            "hello.read-file" => match std::fs::read_to_string("C:/Windows/win.ini") {
                Ok(_) => Err("read a file it shouldn't have".into()),
                Err(e) => {
                    host::log(Level::Warn, &format!("no file: {e}"));
                    Ok(())
                }
            },
            // Its own folder, kept between runs.
            "hello.data" => {
                let before = std::fs::read_to_string("/data/note.txt").unwrap_or_default();
                std::fs::write("/data/note.txt", format!("{before}x")).map_err(|e| e.to_string())?;
                log(&format!("note: {}", std::fs::read_to_string("/data/note.txt").map_err(|e| e.to_string())?));
                Ok(())
            }
            "hello.env" => {
                log(&format!("{} environment variables", std::env::vars().count()));
                Ok(())
            }
            "hello.timer" => {
                let id = host::set_timer(30, false);
                log(&format!("timer {id} set"));
                Ok(())
            }
            "hello.spawn" => {
                let id = host::spawn(&cmd("cmd", &["/c", "echo", "hi", "from", "cmd"]))?;
                log(&format!("process {id} started"));
                Ok(())
            }
            "hello.spawn-denied" => match host::spawn(&cmd("powershell", &["-c", "1"])) {
                Ok(_) => Err("ran a program it may not".into()),
                Err(e) => {
                    host::log(Level::Warn, &e);
                    Ok(())
                }
            },
            "hello.spawn-path" => match host::spawn(&cmd("C:/Windows/System32/cmd.exe", &[])) {
                Ok(_) => Err("ran a program by path".into()),
                Err(e) => {
                    host::log(Level::Warn, &e);
                    Ok(())
                }
            },
            "hello.http" => {
                let url = host::setting("hello.url").ok_or("no hello.url")?;
                let id = host::http(&get(&url))?;
                log(&format!("request {id} sent"));
                Ok(())
            }
            "hello.http-denied" => match host::http(&get("https://example.org/")) {
                Ok(_) => Err("reached a host it may not".into()),
                Err(e) => {
                    host::log(Level::Warn, &e);
                    Ok(())
                }
            },
            "hello.pick" => {
                let items = ["red", "green"].iter().map(|c| PickerItem { label: c.to_string(), detail: String::new(), value: c.to_string() }).collect::<Vec<_>>();
                log(&format!("picker {}", host::show_picker("A colour", &items)));
                Ok(())
            }
            "hello.page" => {
                let page = host::page_open("counter", "Hello");
                LAST.with(|l| l.set(page));
                set_count(page, 0);
                log(&format!("page {page}"));
                Ok(())
            }
            "hello.secret" => {
                log(&format!("token: {:?}", host::secret("hello.token")));
                log(&format!("other: {:?}", host::secret("other.token")));
                log(&format!("their setting: {:?}", host::setting("other.url")));
                Ok(())
            }
            "hello.extras" => {
                host::open_text("Hello JSON", "{\"hello\": 1}", Some("json"));
                host::clipboard_set("copied by hello");
                host::status_set(Some("hello · ok"));
                host::open_url("https://example.com/")?;
                match host::open_url("file:///C:/Windows/win.ini") {
                    Ok(()) => Err("opened a file: URL".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        Ok(())
                    }
                }
            }
            // A terminal under the focused pane, stopped, and run again.
            "hello.terminal" => {
                let id = host::run_in_terminal(&cmd("cmd", &["/c", "echo", "monitor"]), "Hello monitor", true)?;
                host::terminal_stop(id);
                host::terminal_restart(id, Some(&cmd("cmd", &["/c", "echo", "again"])), Some("Hello again"))?;
                log(&format!("terminal {id}"));
                Ok(())
            }
            "hello.task" => {
                log(&format!("task {}", host::run_task("hello build", &cmd("cmd", &["/c", "exit", "0"]))?));
                Ok(())
            }
            // Programs by path: under its own folder where its manifest
            // says, and the one a setting it names holds.
            "hello.spawn-data" => {
                for program in ["/data/tools/missing.exe", "/data/elsewhere/x.exe"] {
                    match host::spawn(&cmd(program, &[])) {
                        Ok(_) => log(&format!("{program}: ran")),
                        Err(e) => host::log(Level::Warn, &format!("{program}: {e}")),
                    }
                }
                Ok(())
            }
            "hello.spawn-setting" => {
                let program = host::setting("hello.cli").ok_or("no hello.cli")?;
                let id = host::spawn(&cmd(&program, &["/c", "echo", "by", "path"]))?;
                log(&format!("process {id} started"));
                Ok(())
            }
            // What it says about its project: a language server, a
            // doctor's check with a fix, a task.
            "hello.project" => {
                host::language_server_set(&["cpp".to_string()], Some(&cmd("cmd", &["/c", "hello-ls"])))?;
                host::doctor_set(&[Check { section: Section::Hardware, label: "board".into(), health: Health::Warn, detail: "none chosen".into(), fix: Some(("pick a board".into(), "hello.greet".into())) }]);
                host::tasks_set(&[Task { name: "hello: build".into(), cmd: cmd("cmd", &["/c", "exit", "0"]) }])?;
                match host::tasks_set(&[Task { name: "bad".into(), cmd: cmd("powershell", &[]) }]) {
                    Ok(()) => Err("offered a task it may not run".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        Ok(())
                    }
                }
            }
            // Found where it is, then run by that path; one it may not run
            // isn't even looked for.
            "hello.find" => {
                let found = host::find_program("cmd", &["%SystemRoot%/System32".to_string()]).ok_or("cmd not found")?;
                log(&format!("found {}", found.rsplit(['/', '\\']).next().unwrap_or_default()));
                host::spawn(&cmd(&found, &["/c", "echo", "found", "it"]))?;
                match host::find_program("powershell", &[]) {
                    Some(_) => Err("looked for a program it may not run".into()),
                    None => Ok(()),
                }
            }
            // A layer over the editor: a sheet, an item gliding along the
            // modeline, clickable.
            "hello.layer" => {
                let sheet = host::sheet_load("hello-sheet.png", 2, 2)?;
                let layer = host::layer_open(LayerLevel::AboveText)?;
                let at = |x: f32| Place { anchor: Anchor::Modeline, x, y: 0.0 };
                host::layer_set(layer, &[Item { id: 1, sheet, frames: vec![0, 1], fps: 4, at: at(0.0), size: 1.0, glide: Some(Glide { to: at(10.0), millis: 50, ease: Ease::Smooth, arc: 1.0 }), flip: false, opacity: 1.0, clickable: true, keep_clear: true, pose: None }]);
                match host::sheet_load("../escape.png", 2, 2) {
                    Ok(_) => Err("loaded a sheet outside its folder".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        log(&format!("layer {layer}, sheet {sheet}"));
                        Ok(())
                    }
                }
            }
            // A model: loaded, its clips told, and posed on a layer.
            "hello.model" => {
                let info = host::model_load("hello-model.glb")?;
                let clips: Vec<String> = info.clips.iter().map(|(name, seconds)| format!("{name} {seconds}")).collect();
                log(&format!("model {} clips {}", info.id, clips.join(", ")));
                let layer = host::layer_open(LayerLevel::AboveText)?;
                let pose = Pose { model: info.id, clip: "bend".into(), once: false, speed: 1.0, yaw: 30.0, style: Style::Pixel, parts: vec!["hat".into()], tint: (1.0, 1.0, 1.0), glow: 0.0 };
                let at = Place { anchor: Anchor::Modeline, x: 4.0, y: 0.0 };
                host::layer_set(layer, &[Item { id: 2, sheet: 0, frames: Vec::new(), fps: 12, at, size: 2.0, glide: None, flip: false, opacity: 1.0, clickable: false, keep_clear: false, pose: Some(pose) }]);
                match host::model_load("missing.glb") {
                    Ok(_) => Err("loaded a model that isn't there".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        Ok(())
                    }
                }
            }
            "hello.here" => {
                match host::editor_here() {
                    Some(here) => log(&format!("here: {:?} line {} {:?} in {:?}", here.path, here.line, here.text, here.project.map(|p| p.name))),
                    None => log("here: nowhere"),
                }
                Ok(())
            }
            "hello.v6" => {
                let png = std::fs::read("/data/dot.png").map_err(|e| e.to_string())?;
                let image = host::image_load(&png)?;
                IMAGE.with(|i| *i.borrow_mut() = Some(image.id));
                log(&format!("image {}x{}", image.width, image.height));
                host::setting_set("hello.url", Some("https://changed.example/"))?;
                log(&format!("set: {}", host::setting_set("other.key", Some("x")).unwrap_err()));
                host::home_set(Some(&HomeSection {
                    title: "hello".into(),
                    key: "SPC h h".into(),
                    rows: vec![HomeRow { name: "a row".into(), detail: "now".into(), mark: HomeMark::Live("0:05".into()), value: "row-1".into() }],
                    most: 3,
                }));
                let marked = host::show_picker_many("Colours", &[PickerItem { label: "Red".into(), detail: String::new(), value: "red".into() }, PickerItem { label: "Blue".into(), detail: String::new(), value: "blue".into() }]);
                log(&format!("picker-many {marked}"));
                Ok(())
            }
            "hello.compose" => {
                let id = host::compose("A note", "save", "seed", false);
                log(&format!("compose {id}"));
                Ok(())
            }
            "hello.canvas" => {
                let page = host::page_open("canvas", "Canvas");
                KINDS.with(|k| k.borrow_mut().push((page, "canvas".into())));
                Ok(())
            }
            "hello.service" => {
                log(&format!("service call {}", host::call_service("echo", "ping")));
                Ok(())
            }
            // A socket to the echo server a setting names; one to a
            // port it may not reach is refused.
            "hello.socket" => {
                let (host_name, port) = echo_address()?;
                let socket = host::socket_connect(&host_name, port)?;
                host::socket_write(socket, b"ping")?;
                log(&format!("socket {socket} to {port}"));
                match host::socket_connect(&host_name, port.wrapping_add(1).max(1)) {
                    Ok(_) => Err("connected where it may not".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        Ok(())
                    }
                }
            }
            "hello.udp" => {
                let (host_name, port) = echo_address()?;
                let socket = host::udp_bind(0)?;
                host::udp_send(socket, &host_name, port, b"ping")?;
                log(&format!("udp {socket} to {port}"));
                match host::udp_bind(9) {
                    Ok(_) => Err("took a port it may not".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        Ok(())
                    }
                }
            }
            // A surface: 4 x 2, red on the left, a white pixel, presented.
            "hello.surface" => {
                let surface = host::surface_open("screen", "Screen", SurfaceOptions { capture: true, scale: SurfaceScale::Stretch, beside: None })?;
                LAST.with(|l| l.set(surface));
                host::surface_resize(surface, 4, 2)?;
                host::surface_fill(surface, 0, 0, 2, 2, (255, 0, 0))?;
                host::surface_write(surface, 3, 1, 1, 1, &[255, 255, 255, 255])?;
                host::surface_present(surface);
                host::surface_cursor(surface, Some(&CursorImage { width: 1, height: 1, hot_x: 0, hot_y: 0, bgra: vec![0, 0, 0, 255] }));
                host::surface_marks(surface, &[SurfaceMark { x: 0, y: 0, width: 1, height: 1, role: Role::Accent }]);
                match host::surface_write(surface, 3, 1, 2, 1, &[0; 8]) {
                    Ok(()) => Err("wrote outside its surface".into()),
                    Err(e) => {
                        host::log(Level::Warn, &e);
                        log(&format!("surface {surface}"));
                        Ok(())
                    }
                }
            }
            // A program whose stdout comes as bytes: the path a setting
            // holds, with the arguments another holds.
            "hello.raw" => {
                let program = host::setting("hello.cli").ok_or("no hello.cli")?;
                let args = host::setting("hello.args").unwrap_or_default();
                let id = host::spawn_raw(&cmd(&program, &args.split(' ').filter(|a| !a.is_empty()).collect::<Vec<_>>()))?;
                log(&format!("raw {id} started"));
                Ok(())
            }
            "hello.read" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                match host::file_read(&path) {
                    Ok(bytes) => log(&format!("read {} bytes", bytes.len())),
                    Err(e) => host::log(Level::Warn, &format!("read: {e}")),
                }
                log(&format!("clipboard: {:?}", host::clipboard_text()));
                host::page_retitle(1, "Renamed");
                host::git_refresh();
                Ok(())
            }
            // A file it may not reach: opened for you, and nothing more.
            "hello.escape" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                host::open_file(&path, None);
                log(&format!("escape text: {:?}", host::buffer_text(&path)));
                log(&format!("escape image: {:?}", host::image_file(&path).map(|i| i.width)));
                log(&format!("escape edit: {:?}", host::buffer_edit(&path, &[])));
                Ok(())
            }
            "hello.cursor" => {
                log(&format!("cursor: {:?}", host::editor_cursor()));
                Ok(())
            }
            "hello.scratch" => {
                let scratch = host::scratch_open("Notes", "one\n", Some("md"));
                log(&format!("scratch {scratch}"));
                host::scratch_set(scratch, "two\n")?;
                log(&format!("scratch text: {:?}", host::scratch_text(scratch)));
                log(&format!("not its own: {:?}", host::scratch_set(scratch + 100, "x")));
                Ok(())
            }
            "hello.list" => {
                let dir = host::setting("hello.dir").ok_or("no hello.dir")?;
                match host::folder_list(&dir) {
                    Ok(entries) => log(&format!("listed: {}", entries.iter().map(|e| format!("{} {}", e.name, e.size)).collect::<Vec<_>>().join(", "))),
                    Err(e) => log(&format!("list: {e}")),
                }
                let next = host::setting("hello.next").ok_or("no hello.next")?;
                log(&format!("next read: {:?}", host::file_read(&next).map(|b| b.len())));
                Ok(())
            }
            "hello.sound" => {
                // A WAV of its own making, and something that isn't one.
                let mut wav = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\0\0\0\0".to_vec();
                wav.truncate(44);
                std::fs::write("/data/chime.wav", &wav).map_err(|e| e.to_string())?;
                std::fs::write("/data/chime.ogg", b"OggS").map_err(|e| e.to_string())?;
                log(&format!("sound: {:?}", host::play_sound("/data/chime.wav")));
                log(&format!("not a wav: {:?}", host::play_sound("/data/chime.ogg")));
                log(&format!("not its own: {:?}", host::play_sound("../x.wav")));
                Ok(())
            }
            "hello.styled" => {
                let page = host::page_open("styled", "Styled");
                KINDS.with(|k| k.borrow_mut().push((page, "styled".into())));
                LAST.with(|l| l.set(page));
                log(&format!("page {page}"));
                Ok(())
            }
            "hello.decorate" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                let decorations = [
                    Decoration { start: (0, 3), end: (0, 3), look: DecorationLook::Text(VirtualText { text: ": i32".into(), role: Role::Muted, after: false }), id: None },
                    Decoration { start: (1, 0), end: (1, 0), look: DecorationLook::Swatch((255, 0, 0)), id: None },
                    Decoration { start: (1, 4), end: (1, 10), look: DecorationLook::Hide("••••".into()), id: None },
                    Decoration { start: (0, 0), end: (0, 2), look: DecorationLook::Tint(Shade::Rgb((1, 2, 3))), id: None },
                    Decoration { start: (0, 0), end: (0, 2), look: DecorationLook::Underline(Role::Bad), id: None },
                ];
                log(&format!("decorate: {:?}", host::decorations_set(&TextBuffer::File(path), &decorations)));
                let scratch = host::scratch_open("Decorated", "x\n", None);
                log(&format!("decorate scratch: {:?}", host::decorations_set(&TextBuffer::Scratch(scratch), &decorations[..1])));
                log(&format!("not its own: {:?}", host::decorations_set(&TextBuffer::Scratch(scratch + 100), &[])));
                let many: Vec<Decoration> = (0..10_001).map(|_| decorations[0].clone()).collect();
                log(&format!("too many: {:?}", host::decorations_set(&TextBuffer::Scratch(scratch), &many)));
                Ok(())
            }
            "hello.undo" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                log(&format!("undo: {:?}", host::buffer_undo(&path)));
                log(&format!("redo: {:?}", host::buffer_redo(&path)));
                Ok(())
            }
            "hello.palette" => {
                let palette = host::theme_palette();
                log(&format!("palette: {}", palette.len()));
                Ok(())
            }
            "hello.pack" => {
                let langs = ["python".to_string()];
                log(&format!("adapter: {:?}", host::debug_adapter_set(&langs, Some(&cmd("sort", &["-r"])))));
                log(&format!("formatter: {:?}", host::formatter_set(&langs, Some(&cmd("sort", &["${file}"])), false)));
                log(&format!("not its own: {:?}", host::formatter_set(&langs, Some(&cmd("powershell", &[])), false)));
                log(&format!("no formatter: {:?}", host::formatter_set(&langs, None, false)));
                Ok(())
            }
            "hello.problems" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                let problem = Problem { start: (0, 4), end: (0, 5), severity: Severity::Warning, message: "x is unused".into(), source: Some("lint".into()), code: Some("W1".into()), fixes: Vec::new() };
                log(&format!("problems: {:?}", host::diagnostics_set(&path, std::slice::from_ref(&problem))));
                log(&format!("in its project: {:?}", host::diagnostics_set("/project/a.py", &[])));
                let many: Vec<Problem> = (0..10_001).map(|_| problem.clone()).collect();
                log(&format!("too many: {:?}", host::diagnostics_set("/project/a.py", &many)));
                Ok(())
            }
            // A program that reads its stdin to the end: `sort`.
            "hello.stdin" => {
                let id = host::spawn(&cmd("sort", &[]))?;
                host::write_stdin(id, b"pear\napple\n")?;
                host::close_stdin(id);
                log(&format!("after close: {:?}", host::write_stdin(id, b"x")));
                Ok(())
            }
            "hello.beside" => {
                let scratch = host::scratch_open_beside("Result", "{}", Some("json"), 0.4);
                log(&format!("beside {scratch}"));
                host::open_text_beside("Raw", "raw", None, 7.0);
                Ok(())
            }
            // Its tools folder: a program put there, made to run, found.
            "hello.tools" => {
                std::fs::create_dir_all("/tools/hello-tool/bin").map_err(|e| e.to_string())?;
                std::fs::write("/tools/hello-tool/bin/hello-tool", b"#!/bin/sh\necho tool\n").map_err(|e| e.to_string())?;
                std::fs::write("/tools/hello-tool/bin/hello-tool.exe", b"MZ").map_err(|e| e.to_string())?;
                log(&format!("executable: {:?}", host::set_executable("/tools/hello-tool/bin/hello-tool")));
                log(&format!("not in tools: {:?}", host::set_executable("/data/x")));
                let found = host::find_program("hello-tool", &[]).ok_or("hello-tool not found")?;
                log(&format!("found in tools: {}", found.replace('\\', "/").contains("/hello-tool/bin/")));
                log(&format!("adapter by path: {:?}", host::debug_adapter_set(&["go".to_string()], Some(&cmd("/tools/hello-tool/bin/hello-tool", &["${tools}/x"])))));
                log(&format!("download outside: {:?}", host::http_download(&get("https://x.example/a"), "/project/a", None).map(|_| ())));
                log(&format!("download bad sum: {:?}", host::http_download(&get("https://x.example/a"), "/tools/a", Some("abc")).map(|_| ())));
                log(&format!("download elsewhere: {:?}", host::http_download(&get("https://elsewhere.example/a"), "/tools/a", None).map(|_| ())));
                Ok(())
            }
            // A link out of its folder, made through WASI, and read through.
            "hello.link" => {
                let made = unsafe { symlink(c"..".as_ptr(), c"/data/up".as_ptr()) };
                log(&format!("link made: {}", made == 0));
                log(&format!("through it: {:?}", host::file_read("/data/up/secret.txt").map(|b| b.len())));
                log(&format!("by wasi: {:?}", std::fs::read("/data/up/secret.txt").map(|b| b.len()).map_err(|e| e.kind())));
                Ok(())
            }
            "hello.send" => {
                let url = host::setting("hello.url").ok_or("no hello.url")?;
                let options = HttpOptions { timeout: Some(5000), follow_redirects: host::setting("hello.stay").is_none(), accept_invalid_certs: host::setting("hello.any").is_some(), client_cert: None, proxy: None, stream: host::setting("hello.stream").is_some() };
                let id = host::http(&HttpRequest { options: options.clone(), ..get(&url) })?;
                log(&format!("sent {id}"));
                log(&format!("elsewhere: {:?}", host::http(&HttpRequest { options: options.clone(), ..get("https://elsewhere.example/") })));
                log(&format!("by a proxy elsewhere: {:?}", host::http(&HttpRequest { options: HttpOptions { proxy: Some("http://elsewhere.example:3128".into()), ..options }, ..get(&url) })));
                Ok(())
            }
            "hello.cancel" => {
                let url = host::setting("hello.url").ok_or("no hello.url")?;
                let options = HttpOptions { timeout: None, follow_redirects: true, accept_invalid_certs: false, client_cert: None, proxy: None, stream: true };
                let id = host::http(&HttpRequest { options, ..get(&url) })?;
                host::http_cancel(id);
                log(&format!("cancelled {id}"));
                Ok(())
            }
            "hello.tls" => {
                let (host_name, port) = echo_address()?;
                let socket = host::socket_connect_tls(&host_name, port, &tls())?;
                host::socket_write(socket, b"ping")?;
                log(&format!("tls socket {socket}"));
                Ok(())
            }
            "hello.starttls" => {
                let (host_name, port) = echo_address()?;
                let socket = host::socket_connect(&host_name, port)?;
                STARTING.with(|s| *s.borrow_mut() = Some((socket, tls())));
                host::socket_write(socket, b"STARTTLS\n")?;
                log(&format!("starting tls on {socket}"));
                log(&format!("not tcp: {:?}", host::socket_start_tls(socket + 100, &tls())));
                Ok(())
            }
            "hello.ipc" => {
                let name = host::setting("hello.ipc").ok_or("no hello.ipc")?;
                let socket = host::ipc_connect(&name)?;
                host::socket_write(socket, b"ping")?;
                log(&format!("ipc socket {socket}"));
                log(&format!("not listed: {:?}", host::ipc_connect("other-ipc-0")));
                Ok(())
            }
            "hello.adapter-port" => {
                log(&format!("spawned: {:?}", host::debug_adapter_connect(&["python".to_string()], Some(&cmd("sort", &["--port", "${port}"])), 0)));
                log(&format!("running: {:?}", host::debug_adapter_connect(&["python".to_string()], None, 4711)));
                log(&format!("nowhere: {:?}", host::debug_adapter_connect(&["python".to_string()], None, 0)));
                Ok(())
            }
            "hello.rules" => {
                let rule = |pattern: &str, group| DecorationRule { files: vec![".env".into(), ".env.*".into()], pattern: pattern.into(), group, look: DecorationLook::Hide("••••".into()) };
                log(&format!("rules: {:?}", host::decoration_rules_set(&[rule("^[A-Z_]+=(.*)$", 1)])));
                log(&format!("bad pattern: {:?}", host::decoration_rules_set(&[rule("(", 0)])));
                log(&format!("no such group: {:?}", host::decoration_rules_set(&[rule("^x$", 1)])));
                Ok(())
            }
            "hello.platform" => {
                let p = host::platform();
                log(&format!("platform: {} {} {}", p.os, p.arch, p.env));
                Ok(())
            }
            "hello.page-file" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                let page = host::page_open("viewer", "Viewer");
                log(&format!("page file: {:?}", host::page_file(page, Some(&path))));
                log(&format!("not its own: {:?}", host::page_file(page, Some("/project/x"))));
                log(&format!("none: {:?}", host::page_file(page, None)));
                Ok(())
            }
            "hello.language" => {
                let scratch = host::scratch_open("Data", "{\"a\": 1}\n", None);
                log(&format!("language: {:?}", host::buffer_language_set(&TextBuffer::Scratch(scratch), Some("JSON"))));
                log(&format!("by its name again: {:?}", host::buffer_language_set(&TextBuffer::Scratch(scratch), None)));
                log(&format!("not its own: {:?}", host::buffer_language_set(&TextBuffer::Scratch(scratch + 100), Some("json"))));
                log(&format!("a file: {:?}", host::buffer_language_set(&TextBuffer::File("/project/a.txt".into()), Some("json"))));
                Ok(())
            }
            "hello.tagged" => {
                let scratch = host::scratch_open("Swatches", "#ff0000\n", None);
                let swatch = Decoration { start: (0, 0), end: (0, 0), look: DecorationLook::Swatch((255, 0, 0)), id: Some(5) };
                log(&format!("tagged: {:?}", host::decorations_set(&TextBuffer::Scratch(scratch), &[swatch])));
                Ok(())
            }
            "hello.background" => {
                for i in 0..5 {
                    log(&format!("background {i}: {:?}", host::run_task_background(&format!("job {i}"), &cmd("cmd", &["/c", "exit", "0"]))));
                }
                log(&format!("denied: {:?}", host::run_task_background("x", &cmd("powershell", &[]))));
                host::task_cancel(1);
                log(&format!("show: {:?}", host::task_show(1)));
                log(&format!("show none: {:?}", host::task_show(999)));
                Ok(())
            }
            "hello.add-server" => {
                log(&format!("add: {:?}", host::language_server_add("ruff", &["python".into()], Some(&cmd("cmd", &["/c", "ruff"])))));
                log(&format!("gone: {:?}", host::language_server_add("ruff", &[], None)));
                log(&format!("bad name: {:?}", host::language_server_add("a b", &["python".into()], None)));
                log(&format!("denied: {:?}", host::language_server_add("ps", &["python".into()], Some(&cmd("powershell", &[])))));
                Ok(())
            }
            "hello.find-setting" => {
                log(&format!("found by setting: {:?}", host::find_program("cmd", &["${setting:hello.cmd}".into()]).map(|p| p.rsplit(['/', '\\']).next().unwrap_or_default().to_string())));
                Ok(())
            }
            "hello.pid" => {
                log(&format!("pid: {}", host::process_id()));
                Ok(())
            }
            "hello.settings" => {
                log(&format!("settings: {:?}", host::language_server_settings("typescript", Some(r#"{"typescript": {"format": {"semicolons": "remove"}}}"#))));
                log(&format!("gone: {:?}", host::language_server_settings("eslint", None)));
                log(&format!("not json: {:?}", host::language_server_settings("eslint", Some("{"))));
                log(&format!("not an object: {:?}", host::language_server_settings("eslint", Some("[1]"))));
                log(&format!("bad name: {:?}", host::language_server_settings("a b", None)));
                Ok(())
            }
            "hello.fixes" => {
                let path = host::setting("hello.file").ok_or("no hello.file")?;
                let problem = Problem { start: (0, 0), end: (0, 6), severity: Severity::Warning, message: "unused import".into(), source: Some("ruff".into()), code: Some("F401".into()), fixes: Vec::new() };
                let fix = QuickFix { title: "Remove the import".into(), edits: vec![TextEdit { start: (0, 0), end: (1, 0), text: String::new() }] };
                let with = Problem { fixes: vec![fix], ..problem.clone() };
                log(&format!("fixes: {:?}", host::diagnostics_set(&path, &[with, problem.clone()])));
                let untitled = Problem { fixes: vec![QuickFix { title: " ".into(), edits: Vec::new() }], ..problem };
                log(&format!("untitled: {:?}", host::diagnostics_set(&path, &[untitled])));
                Ok(())
            }
            "hello.launches" => {
                let launch = |name: &str, attributes: &str| LaunchConfig {
                    name: name.into(),
                    languages: vec!["cpp".into()],
                    request: LaunchRequest::Launch,
                    program: Some("/data/app".into()),
                    args: vec!["${tools}/x".into()],
                    cwd: None,
                    env: Vec::new(),
                    attributes: attributes.into(),
                    pre_launch_task: Some("build app".into()),
                };
                log(&format!("launches: {:?}", host::launches_set(&[launch("app", r#"{"type": "lldb"}"#), launch("empty", "")])));
                log(&format!("bad attributes: {:?}", host::launches_set(&[launch("x", "[1]")])));
                log(&format!("no languages: {:?}", host::launches_set(&[LaunchConfig { languages: Vec::new(), ..launch("y", "{}") }])));
                let target = PickerItem { label: "app".into(), detail: "build/app".into(), value: "build/app".into() };
                log(&format!("variable: {:?}", host::launch_variable_set("target", std::slice::from_ref(&target))));
                log(&format!("fenix's own: {:?}", host::launch_variable_set("pickProcess", &[])));
                log(&format!("bad variable: {:?}", host::launch_variable_set("a b", &[])));
                Ok(())
            }
            "hello.adapter-env" => {
                let mut sort = cmd("sort", &[]);
                sort.env.push(("WHERE".into(), "${tools}/here".into()));
                log(&format!("adapter env: {:?}", host::debug_adapter_set(&["python".to_string()], Some(&sort))));
                log(&format!("server env: {:?}", host::language_server_set(&["python".to_string()], Some(&sort))));
                Ok(())
            }
            "hello.secrets" => {
                log(&format!("by pattern: {:?}", host::secret("hello.secret.api")));
                log(&format!("kept: {:?}", host::secret_set("hello.secret.pw", Some("hunter2"))));
                log(&format!("read back: {:?}", host::secret("hello.secret.pw")));
                log(&format!("gone: {:?} {:?}", host::secret_set("hello.secret.pw", None), host::secret("hello.secret.pw")));
                log(&format!("not its: {:?}", host::secret_set("other.secret", Some("x"))));
                log(&format!("fenix's: {:?}", host::secret_set("gitlab.token", Some("x"))));
                Ok(())
            }
            "hello.prompt-secret" => {
                log(&format!("asked: {}", host::prompt_secret("Password")));
                Ok(())
            }
            "hello.listen" => {
                match host::tcp_listen(0) {
                    Ok((listener, port)) => log(&format!("listening: {listener} {port}")),
                    Err(e) => log(&format!("listen: Err({e})")),
                }
                log(&format!("taken port: {:?}", host::tcp_listen(80)));
                Ok(())
            }
            "hello.env-var" => {
                log(&format!("env: {:?}", host::env_var("FENIX_HELLO_TEST").or(host::env_var("HELLO_TEST_VAR"))));
                log(&format!("by prefix: {:?}", host::env_var("HELLO_TEST_OTHER")));
                log(&format!("not its: {:?}", host::env_var("PATH")));
                Ok(())
            }
            "hello.formatter-ex" => {
                log(&format!("formatter: {:?}", host::formatter_set(&["python".to_string()], Some(&cmd("sort", &["${data}/x"])), true)));
                Ok(())
            }
            "hello.wrapper" => {
                log(&format!("wrapper: {:?}", host::find_program("gradlew", &["./gradlew".into()]).map(|p| p.rsplit(['/', '\\']).next().unwrap_or_default().to_string())));
                Ok(())
            }
            // What it opened last, brought to the pane you're in.
            "hello.focus" => {
                host::page_focus(LAST.with(|l| l.get()));
                Ok(())
            }
            "hello.agent" => {
                log(&format!("provide: {:?}", host::agent_provide("hello", Some(&cmd("cmd", &["/c", "agent", "--acp"])))));
                log(&format!("gone: {:?}", host::agent_provide("hello", None)));
                log(&format!("no id: {:?}", host::agent_provide(" ", None)));
                log(&format!("denied: {:?}", host::agent_provide("hello", Some(&cmd("powershell", &[])))));
                log(&format!("ask {}", host::call_service("agents.ask", r#"{"prompt":"hi"}"#)));
                Ok(())
            }
            other => Err(format!("no command {other}")),
        }
    }

    fn page_closed(page: u64) {
        log(&format!("you closed {page}"));
    }

    fn http_event(request: u64, part: HttpPart) {
        match part {
            HttpPart::Response(Ok(r)) => {
                let t = &r.timing;
                log(&format!("http {request}: {} {} {} dns {} connect {} tls {} total {}", r.status, r.url.rsplit('/').next().unwrap_or_default(), String::from_utf8_lossy(&r.body), t.dns.is_some(), t.connect.is_some(), t.tls.is_some(), t.total >= t.first_byte));
            }
            HttpPart::Response(Err(e)) => log(&format!("http {request} failed: {e}")),
            HttpPart::Chunk(data) => log(&format!("http {request} chunk: {}", String::from_utf8_lossy(&data))),
            HttpPart::End(why) => log(&format!("http {request} end: {why:?}")),
        }
    }

    fn file_saved(path: String) {
        log(&format!("file saved: {path}"));
    }

    fn socket_accepted(listener: u64, socket: u64, peer: String) {
        log(&format!("accepted on {listener}: {socket} from {}", if peer.starts_with("127.0.0.1:") { "127.0.0.1" } else { &peer }));
        let _ = host::socket_write(socket, b"hello\n");
    }

    fn agent_turn_ended(turn: AgentTurn) {
        log(&format!("turn ended: {} {} {} {}", turn.agent, turn.thread, turn.stop_reason, turn.text));
    }

    fn task_started(name: String) {
        log(&format!("task started: {name}"));
    }

    fn debug_changed(state: DebugState) {
        match state {
            DebugState::Started(s) => log(&format!("debug started: {} {} {} {:?}", s.id, s.name, s.language, s.parent)),
            DebugState::Paused((id, why)) => log(&format!("debug paused: {id} {why}")),
            DebugState::Resumed(id) => log(&format!("debug resumed: {id}")),
            DebugState::Ended(id) => log(&format!("debug ended: {id}")),
        }
    }

    fn view_changed(views: Vec<TextView>) {
        for v in views {
            log(&format!("view {:?} {}..={}", v.buffer, v.first, v.last));
        }
    }

    fn decoration_clicked(buffer: TextBuffer, id: u32) {
        log(&format!("clicked {id} in {buffer:?}"));
    }

    fn buffer_changed(what: Changed) {
        log(&format!("changed {what:?}"));
    }

    fn scratch_closed(scratch: u64) {
        log(&format!("scratch {scratch} closed"));
    }

    fn focus_moved(focus: Focus) {
        log(&format!("focus {focus:?}"));
        // Where you are, read as it moved: not only while a command runs.
        if let Some(here) = host::editor_here() {
            log(&format!("here at focus: {:?} {:?}", here.path.map(|p| p.rsplit(['/', '\\']).next().unwrap_or_default().to_string()), host::editor_cursor().map(|c| c.at)));
        }
    }

    fn page_key_press(page: u64, key: KeyPress) {
        log(&format!("key press on {page}: {key:?}"));
    }

    fn page_mouse(page: u64, event: MouseEvent) {
        log(&format!("mouse on {page}: {event:?}"));
    }

    fn handle(event: Event) {
        if let Event::Timer(timer) = &event {
            if let Some((call, _, payload)) = LATER.with(|l| l.borrow_mut().take_if(|(_, t, _)| t == timer)) {
                host::service_answer(call, Ok(&format!("later: {payload}")));
                return;
            }
        }
        match event {
            Event::Timer(id) => log(&format!("timer {id} went off")),
            Event::Picked((id, choice)) => log(&format!("picked {id}: {choice:?}")),
            Event::Prompted((id, text)) => log(&format!("prompted {id}: {text:?}")),
            Event::ProcessOutput(out) => log(&format!("process {} said: {}", out.process, out.line.trim())),
            Event::ProcessExit((id, code)) => log(&format!("process {id} ended: {code:?}")),
            // A whole answer: as `http` gave it, and timed, as a stream's head is.
            Event::HttpResponse((id, Ok(r))) => {
                log(&format!("response {id}: {} {}", r.status, String::from_utf8_lossy(&r.body)));
                let t = &r.timing;
                log(&format!("http {id}: {} {} {} dns {} connect {} tls {} total {}", r.status, r.url.rsplit('/').next().unwrap_or_default(), String::from_utf8_lossy(&r.body), t.dns.is_some(), t.connect.is_some(), t.tls.is_some(), t.total >= t.first_byte));
            }
            Event::HttpResponse((id, Err(e))) => {
                log(&format!("response {id} failed: {e}"));
                log(&format!("http {id} failed: {e}"));
            }
            Event::ServiceReply((id, r)) => log(&format!("reply {id}: {r:?}")),
            Event::SettingChanged(key) => log(&format!("{key} changed")),
            Event::PageShown((page, shown)) => log(&format!("page {page} shown: {shown}")),
            Event::TaskFinished((id, ok)) => log(&format!("task {id} finished: {ok}")),
            Event::TerminalClosed(id) => log(&format!("terminal {id} closed")),
            Event::FocusChanged(path) => log(&format!("in {path:?}")),
            Event::Layout(layout) => log(&format!("layout {}x{}", layout.columns, layout.rows)),
            Event::LayerClicked((layer, item)) => log(&format!("clicked {layer}/{item}")),
            Event::Arrived((layer, item)) => log(&format!("arrived {layer}/{item}")),
            Event::Saved(name) => log(&format!("saved {name}")),
            Event::Diagnostics((errors, warnings)) => log(&format!("{errors} errors, {warnings} warnings")),
            Event::TaskEnded(ok) => log(&format!("a task ended: {ok}")),
            Event::Typing => log("typing"),
            Event::Active => log("active"),
            Event::PickedMany((id, marked)) => log(&format!("picked-many {id}: {marked:?}")),
            Event::Composed((id, text)) => log(&format!("composed {id}: {text:?}")),
            Event::HomePicked(value) => log(&format!("home picked {value}")),
            Event::ThemeChanged => log("theme changed"),
            Event::SocketConnected((socket, result)) => log(&format!("socket {socket} connected: {result:?}")),
            Event::SocketData((socket, data)) => {
                log(&format!("socket {socket} said: {}", String::from_utf8_lossy(&data)));
                // Its server said yes to STARTTLS: TLS from here on, and the
                // ping through it.
                if let Some((_, tls)) = STARTING.with(|s| s.borrow_mut().take_if(|(s, _)| *s == socket)) {
                    log(&format!("start tls: {:?}", host::socket_start_tls(socket, &tls)));
                    let _ = host::socket_write(socket, b"ping");
                    return;
                }
                host::socket_close(socket);
            }
            Event::Datagram(d) => log(&format!("datagram on {}: {}", d.socket, String::from_utf8_lossy(&d.data))),
            Event::SocketClosed((socket, why)) => log(&format!("socket {socket} closed: {why:?}")),
            Event::SurfaceResized(size) => log(&format!("surface {} is {}x{} at {}", size.surface, size.width, size.height, size.scale)),
            Event::ProcessBytes((id, bytes)) => {
                let before = RAW.with(|r| r.get());
                RAW.with(|r| r.set(before + bytes.len()));
                if before == 0 {
                    log(&format!("raw {id} wrote bytes"));
                }
            }
            Event::Opened(path) => {
                let read = host::file_read(&path).map(|b| b.len());
                let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
                log(&format!("opened {name}: {read:?}"));
                // A `.view` file, shown on a page of its own.
                if name.ends_with(".view") {
                    host::page_open("viewer", name);
                }
            }
        }
    }

    fn surface_input(surface: u64, input: SurfaceInput) {
        match input {
            SurfaceInput::Key(key) => {
                let mods = if key.mods.ctrl { " ctrl" } else { "" };
                log(&format!("key {:?} {}{mods}", key.key, if key.down { "down" } else { "up" }));
                // `c` moves the red half right, as a scroll would.
                if matches!(key.key, SurfaceKey::Char('c')) && key.down {
                    let moved = host::surface_copy(surface, 0, 0, 2, 2, 2, 0);
                    host::surface_present(surface);
                    log(&format!("copied: {moved:?}"));
                }
            }
            SurfaceInput::Pointer(p) => log(&format!("pointer {},{} buttons {}", p.x, p.y, p.buttons)),
            SurfaceInput::Wheel(w) => log(&format!("wheel {} at {},{}", w.lines, w.x, w.y)),
            SurfaceInput::Capture(on) => log(&format!("capture {on}")),
        }
    }

    fn page_render(page: u64, width: u32, height: u32) -> Page {
        // A styled canvas: its cells' styles, a rule down it, another
        // focused row, the keys and the mouse, and colours of its own.
        if page_kind(page) == "styled" {
            let cells = |line, start, end| Cells { line, start, end };
            let canvas = Canvas {
                text: format!("日本 ok {width}x{height}\nrow two"),
                spans: Vec::new(),
                rules: Vec::new(),
                focus: Some(cells(1, 0, 7)),
                panels: Vec::new(),
                pictures: Vec::new(),
                popup: None,
            };
            return Page {
                canvas: Some(canvas),
                styles: vec![
                    CellStyle { cells: cells(0, 3, 5), background: Some(Shade::Selection), underline: Some(Role::Accent) },
                    CellStyle { cells: cells(1, 0, 3), background: Some(Shade::Rgb((10, 20, 30))), underline: None },
                ],
                vertical_rules: vec![VerticalRule { column: 9, top: 0, bottom: 2 }],
                focuses: vec![cells(0, 0, 2)],
                more_keys: true,
                mouse: true,
                colors: vec![CellColor { cells: cells(1, 0, 3), color: (200, 100, 50) }],
                ..empty_page()
            };
        }
        let span = |text: String, role: Role| Span { text, role };
        // A canvas page: laid out cell by cell, typing while it counts odd.
        if count(page) >= 100 || page_kind(page) == "canvas" {
            let n = count(page);
            let image = IMAGE.with(|i| *i.borrow());
            let text = format!("  Canvas {width}\n\n    counted {n}");
            return Page {
                lines: Vec::new(),
                focus: None,
                keys: Vec::new(),
                popup: None,
                canvas: Some(Canvas {
                    text,
                    spans: vec![CellSpan { cells: Cells { line: 0, start: 2, end: 8 }, role: Role::Syntax("keyword".into()) }],
                    rules: vec![Cells { line: 1, start: 2, end: 20 }],
                    focus: Some(Cells { line: 2, start: 2, end: 20 }),
                    panels: Vec::new(),
                    pictures: image.map(|image| Picture { cells: Cells { line: 3, start: 2, end: 6 }, rows: 2, image, view: None, marks: Vec::new() }).into_iter().collect(),
                    popup: None,
                }),
                typing: n % 2 == 1,
                space: true,
                ..empty_page()
            };
        }
        Page {
            lines: vec![
                Line { spans: vec![span("Hello".into(), Role::Title), span(format!("  {width} wide"), Role::Muted)], rule: false },
                Line { spans: Vec::new(), rule: true },
                Line { spans: vec![span(format!("counted {}", count(page)), Role::Text)], rule: false },
            ],
            focus: Some(2),
            keys: vec![KeyHint { keys: "j".into(), what: "count".into() }],
            // Past two counts, a menu over the count line.
            popup: (count(page) >= 2).then(|| Popup { line: 2, rows: vec![vec![span("r".into(), Role::Accent), span("  reset".into(), Role::Text)]] }),
            ..empty_page()
        }
    }

    // A language server, its JSON by hand: `initialize` says it hovers,
    // a hover says hello, and opening a document says its problem.
    fn lsp_message(server: u64, message: String) {
        let id = message.split("\"id\":").nth(1).map(|rest| rest.chars().take_while(|c| c.is_ascii_digit()).collect::<String>()).filter(|id| !id.is_empty());
        let reply = |result: &str| {
            if let Some(id) = &id {
                host::lsp_send(server, &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"));
            }
        };
        if message.contains("\"method\":\"initialize\"") {
            reply("{\"capabilities\":{\"hoverProvider\":true}}");
        } else if message.contains("\"method\":\"textDocument/hover\"") {
            reply("{\"contents\":\"hello from the guest\"}");
        } else if message.contains("\"method\":\"textDocument/didOpen\"") {
            let uri = message.split("\"uri\":\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or_default();
            let language = message.split("\"languageId\":\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or_default();
            log(&format!("lsp opened {} as {language}", uri.rsplit('/').next().unwrap_or_default()));
            host::lsp_send(
                server,
                &format!("{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{{\"uri\":\"{uri}\",\"diagnostics\":[{{\"range\":{{\"start\":{{\"line\":0,\"character\":0}},\"end\":{{\"line\":0,\"character\":1}}}},\"severity\":2,\"message\":\"a guest's problem\"}}]}}}}"),
            );
        } else if id.is_some() {
            reply("null");
        }
    }

    fn page_paste(page: u64, text: String) {
        set_count(page, count(page) + text.chars().count() as u32);
        host::page_refresh(page);
    }

    fn page_key(page: u64, key: Key) {
        if matches!(key, Key::Char('j') | Key::Char(' ')) {
            set_count(page, count(page) + 1);
            host::page_refresh(page);
        }
    }

    fn page_state(page: u64) -> Option<String> {
        Some(count(page).to_string())
    }

    fn page_restore(page: u64, kind: String, state: String) -> bool {
        match (kind.as_str(), state.parse()) {
            ("counter", Ok(n)) => {
                set_count(page, n);
                true
            }
            // A surface opened again, in the restored page's place.
            ("screen", _) => match host::surface_open("screen", "Screen again", SurfaceOptions { capture: false, scale: SurfaceScale::Fit, beside: None }) {
                Ok(surface) => {
                    log(&format!("restored {page} as surface {surface}"));
                    true
                }
                Err(_) => false,
            },
            _ => false,
        }
    }

    fn serve(call: u64, name: String, payload: String) -> Option<Result<String, String>> {
        match name.as_str() {
            "hello.upper" => Some(Ok(payload.to_uppercase())),
            // As the PDF reader answers Fenix: a file at a page, read as
            // one it's given; a file searched.
            "pdf.open" => {
                let path = payload.split("\"path\":\"").nth(1).and_then(|p| p.split('"').next()).map(|p| p.replace("\\\\", "\\")).unwrap_or_default();
                let read = host::file_read(&path).map(|b| b.len());
                let page = payload.split("\"page\":").nth(1).and_then(|p| p.split([',', '}']).next()).unwrap_or("none").to_string();
                log(&format!("pdf.open at page {page}, read {read:?}"));
                Some(Ok(String::new()))
            }
            "pdf.search" => Some(Ok(r#"[{"page": 2, "context": "a packet here"}]"#.into())),
            // As a forge answers Fenix's Git pages.
            "forge.branch" => Some(Ok(r##"{"number": 7, "reference": "#7", "title": "From the guest", "checks": "passed", "approved_by": ["ana"], "unresolved": 1}"##.into())),
            "forge.open" => {
                let git = payload.split("\"root\":\"").nth(1).and_then(|r| r.split('"').next()).map(|root| host::file_read(&format!("{}/.git/HEAD", root.replace("\\\\", "/"))).is_ok());
                log(&format!("forge.open, its HEAD read: {git:?}"));
                Some(Ok(String::new()))
            }
            // As the agenda answers the control channel.
            "agenda.tasks" => Some(Ok(r#"{"tasks": [{"id": 7, "title": "From the guest", "status": "inProgress", "jiraKey": "FEN-1", "elapsedMinutes": 30}]}"#.into())),
            "agenda.timer" => Some(Ok(r#"{"taskId": 7, "title": "From the guest", "since": "2026-10-28T09:00:00+00:00"}"#.into())),
            // As the notebook answers the control channel and the reading view.
            "notebook.capture" => Some(Ok("/nb/notes/inbox.md".into())),
            "notebook.link" if payload.contains("Nowhere") => Some(Ok(r#"[{"exists": false, "kind": "missing"}]"#.into())),
            "notebook.link" => Some(Ok(r#"[{"exists": true, "kind": "note", "name": "Ground station", "text": "Antenna notes\n"}]"#.into())),
            // As the diagrams extension draws one: a pixel, or its problem.
            "diagrams.render" if payload.contains("broken") => Some(Err("line 1: not a diagram".into())),
            "diagrams.render" => Some(Ok(r#"{"png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==", "width": 1, "height": 1}"#.into())),
            // Answered once a timer goes off.
            "hello.later" => {
                let timer = host::set_timer(30, false);
                LATER.with(|l| *l.borrow_mut() = Some((call, timer, payload)));
                None
            }
            _ => Some(Err(format!("no service {name}"))),
        }
    }
}

export!(Hello);
