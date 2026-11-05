//! The program itself, driven as the extension drives it: requests on its
//! stdin, replies read off its stdout, on `fixtures/two-pages.pdf` -- two
//! US Letter pages, the first with text, a red square, a link to page 2
//! and a web link; two bookmarks.
//!
//! PDFium is a library you put beside Fenix (docs/BUILDING.md), so it may
//! not be on the machine the tests run on. Where it is
//! (`FENIX_PDFIUM_PATH`, or beside the program), the pages are rendered,
//! read and searched; where it isn't, opening says so, naming where it
//! looked, and the program carries on.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use fenix_pdfium::protocol::{Frames, Reply, Request, Target, VERSION};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf")
}

fn library_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Whether the program will find PDFium: where `FENIX_PDFIUM_PATH` says,
/// or beside it.
fn pdfium_here() -> bool {
    let program = PathBuf::from(env!("CARGO_BIN_EXE_fenix-pdfium"));
    let beside = program.parent().map(|d| d.join(library_name()));
    let named = std::env::var_os("FENIX_PDFIUM_PATH").map(|d| PathBuf::from(d).join(library_name()));
    [beside, named].into_iter().flatten().any(|p| p.is_file())
}

struct Companion {
    child: Child,
    stdin: ChildStdin,
    replies: mpsc::Receiver<Reply>,
}

impl Companion {
    fn start() -> Companion {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fenix-pdfium")).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("fenix-pdfium starts");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            let mut frames = Frames::default();
            let mut buf = vec![0u8; 1 << 16];
            // Read a few bytes at a time, as a pipe may give them.
            while let Ok(n) = stdout.read(&mut buf[..997]) {
                if n == 0 {
                    break;
                }
                for body in frames.push(&buf[..n]).expect("whole frames") {
                    if tx.send(Reply::decode(&body).expect("a reply")).is_err() {
                        return;
                    }
                }
            }
        });
        let mut c = Companion { child, stdin, replies };
        assert_eq!(c.next(), Reply::Hello { version: VERSION }, "it says which protocol first");
        c
    }

    fn ask(&mut self, request: Request) {
        self.stdin.write_all(&request.encode()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn next(&mut self) -> Reply {
        self.replies.recv_timeout(Duration::from_secs(30)).expect("a reply")
    }

    /// Replies until one `done` says is the one, which it gives back.
    fn until(&mut self, mut done: impl FnMut(&Reply) -> bool) -> Reply {
        loop {
            let reply = self.next();
            if done(&reply) {
                return reply;
            }
        }
    }

    /// Stdin closed: it ends by itself.
    fn finish(mut self) {
        drop(self.stdin);
        let start = std::time::Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "it ends when its stdin closes");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_document_is_rendered_read_and_searched() {
    let mut c = Companion::start();
    if !pdfium_here() {
        // No PDFium: opening says so, naming where it looked, and the
        // program carries on for the next.
        c.ask(Request::Open { doc: 1, path: fixture().display().to_string() });
        match c.next() {
            Reply::OpenFailed { doc: 1, message } => assert!(message.contains("pdfium") && message.contains("FENIX_PDFIUM_PATH"), "{message}"),
            other => panic!("{other:?}"),
        }
        c.ask(Request::Open { doc: 2, path: fixture().display().to_string() });
        assert!(matches!(c.next(), Reply::OpenFailed { doc: 2, .. }), "and again, still running");
        return c.finish();
    }
    c.ask(Request::Open { doc: 7, path: fixture().display().to_string() });
    assert_eq!(c.next(), Reply::Opened { doc: 7, pages: vec![(612.0, 792.0), (612.0, 792.0)] });

    // A strip of page 1 at 100%, across the red square (72..172 points
    // from the left, 620..720 from the top).
    c.ask(Request::Render { doc: 7, view: 1, id: 1, page: 0, page_w: 612, page_h: 792, x: 0, y: 650, w: 612, h: 20 });
    let Reply::Rendered { doc: 7, view: 1, id: 1, page: 0, x: 0, y: 650, w: 612, h: 20, bgra } = c.next() else { panic!("a render") };
    assert_eq!(bgra.len(), 612 * 20 * 4);
    let px = |x: usize, y: usize| &bgra[(y * 612 + x) * 4..(y * 612 + x) * 4 + 4];
    let red = px(120, 10);
    assert!(red[2] > 200 && red[1] < 60 && red[0] < 60, "BGRA, red inside the square: {red:?}");
    let paper = px(400, 10);
    assert!(paper.iter().take(3).all(|&c| c > 240), "white beside it: {paper:?}");

    // The same strip at 200%: the square's twice as far in.
    c.ask(Request::Render { doc: 7, view: 1, id: 2, page: 0, page_w: 1224, page_h: 1584, x: 200, y: 1300, w: 100, h: 10 });
    let Reply::Rendered { id: 2, w: 100, h: 10, bgra, .. } = c.next() else { panic!("a render") };
    assert!(bgra[2] > 200 && bgra[1] < 60, "red at (200, 1300) of 200%");

    // Renders asked before a cancel aren't all made.
    for id in 10..40 {
        c.ask(Request::Render { doc: 7, view: 2, id, page: 1, page_w: 1224, page_h: 1584, x: 0, y: 0, w: 1224, h: 1584 });
    }
    c.ask(Request::CancelBefore { doc: 7, view: 2, before: 40 });
    c.ask(Request::Outline { doc: 7 });
    let mut renders = 0;
    let outline = c.until(|r| {
        renders += matches!(r, Reply::Rendered { view: 2, .. }) as usize;
        matches!(r, Reply::Outline { .. })
    });
    assert!(renders < 30, "{renders} of 30 rendered after the cancel");
    let Reply::Outline { doc: 7, headings } = outline else { unreachable!() };
    assert_eq!(headings.iter().map(|h| (h.title.as_str(), h.page, h.depth)).collect::<Vec<_>>(), [("Chapter 1", 0, 0), ("Chapter 2", 1, 0)]);

    c.ask(Request::Text { doc: 7, page: 0 });
    let Reply::Text { doc: 7, page: 0, chars } = c.next() else { panic!("the text") };
    let text: String = chars.iter().map(|(c, _)| *c).collect();
    assert!(text.contains("Hello PDF reader") && text.contains("Go to page two"), "{text:?}");
    let h = chars.iter().find(|(c, _)| *c == 'H').unwrap().1;
    assert!(h[0] > 70.0 && h[0] < 80.0 && h[1] > 60.0 && h[3] < 100.0, "boxes from the page's top left: {h:?}");

    c.ask(Request::Links { doc: 7, page: 0 });
    let Reply::Links { doc: 7, page: 0, links } = c.next() else { panic!("the links") };
    assert!(links.iter().any(|l| matches!(l.to, Target::Page { page: 1, .. })), "{links:?}");
    assert!(links.iter().any(|l| l.to == Target::Uri("https://example.com/".into())), "{links:?}");

    c.ask(Request::Search { doc: 7, id: 3, query: "second".into(), match_case: false });
    let Reply::Found { doc: 7, id: 3, done: true, matches } = c.until(|r| matches!(r, Reply::Found { done: true, .. })) else { unreachable!() };
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].page, 1);
    assert!(matches[0].context.contains("Second page"), "{:?}", matches[0].context);
    c.ask(Request::Search { doc: 7, id: 4, query: "second".into(), match_case: true });
    let Reply::Found { matches, .. } = c.until(|r| matches!(r, Reply::Found { id: 4, done: true, .. })) else { unreachable!() };
    assert!(matches.is_empty(), "match case: it's \"Second\"");

    c.ask(Request::Open { doc: 8, path: Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml").display().to_string() });
    assert!(matches!(c.next(), Reply::OpenFailed { doc: 8, .. }), "not a PDF");
    c.ask(Request::Close { doc: 7 });
    c.ask(Request::Render { doc: 7, view: 1, id: 50, page: 0, page_w: 10, page_h: 10, x: 0, y: 0, w: 10, h: 10 });
    assert!(matches!(c.next(), Reply::RenderFailed { doc: 7, id: 50, .. }), "closed");
    c.finish();
}

#[test]
fn a_document_written_again_is_said_to_have_changed() {
    let dir = std::env::temp_dir().join(format!("fenix-pdfium-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("copy.pdf");
    std::fs::copy(fixture(), &copy).unwrap();
    let mut c = Companion::start();
    c.ask(Request::Open { doc: 3, path: copy.display().to_string() });
    assert!(matches!(c.next(), Reply::Opened { doc: 3, .. } | Reply::OpenFailed { doc: 3, .. }));
    // Its time moved on, as a build writing it again would.
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options().write(true).open(&copy).unwrap().set_modified(later).unwrap();
    assert_eq!(c.until(|r| matches!(r, Reply::Changed { .. })), Reply::Changed { doc: 3 });
    c.ask(Request::Close { doc: 3 });
    c.finish();
    let _ = std::fs::remove_dir_all(&dir);
}
