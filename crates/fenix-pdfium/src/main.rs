//! `fenix-pdfium`: requests on stdin, replies on stdout, until stdin
//! closes (`fenix_pdfium::protocol`). What goes wrong that isn't a reply
//! goes to stderr, which the extension logs.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fenix_pdfium::protocol::{Frames, Reply, Request, VERSION};

/// How often the open documents' files are looked at.
const WATCH_EVERY: Duration = Duration::from_millis(1000);

type Out = Arc<Mutex<std::io::Stdout>>;

/// Each open document's file, by the extension's number for it, and when
/// it was last written.
type Watched = Arc<Mutex<HashMap<u64, (PathBuf, Option<SystemTime>)>>>;

/// A reply, framed and flushed; a pipe gone means the extension is too.
fn send(out: &Out, reply: &Reply) {
    let frame = reply.encode();
    let Ok(mut out) = out.lock() else { return };
    if out.write_all(&frame).and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

fn modified(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn main() {
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));
    send(&out, &Reply::Hello { version: VERSION });
    let worker = {
        let out = out.clone();
        fenix_pdf::PdfWorker::spawn(move |response| {
            if let Some(reply) = fenix_pdfium::from_worker(response) {
                send(&out, &reply);
            }
        })
    };
    // Each open document's file, and when it was last written: one that's
    // written again (a build, a download) is said to have changed.
    let watched: Watched = Arc::default();
    {
        let (watched, out) = (watched.clone(), out.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(WATCH_EVERY);
            let changed: Vec<u64> = match watched.lock() {
                Ok(mut docs) => docs
                    .iter_mut()
                    .filter_map(|(doc, (path, seen))| {
                        let now = modified(path);
                        (now.is_some() && now != *seen).then(|| {
                            *seen = now;
                            *doc
                        })
                    })
                    .collect(),
                Err(_) => return,
            };
            for doc in changed {
                send(&out, &Reply::Changed { doc });
            }
        });
    }
    let mut stdin = std::io::stdin().lock();
    let mut frames = Frames::default();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let bodies = match frames.push(&buf[..n]) {
            Ok(bodies) => bodies,
            Err(e) => {
                eprintln!("fenix-pdfium: {e}");
                break;
            }
        };
        for body in bodies {
            let request = match Request::decode(&body) {
                Ok(request) => request,
                Err(e) => {
                    eprintln!("fenix-pdfium: {e}");
                    continue;
                }
            };
            if let Ok(mut docs) = watched.lock() {
                match &request {
                    Request::Open { doc, path } => {
                        let path = PathBuf::from(path);
                        let seen = modified(&path);
                        docs.insert(*doc, (path, seen));
                    }
                    Request::Close { doc } => {
                        docs.remove(doc);
                    }
                    _ => {}
                }
            }
            worker.send(fenix_pdfium::to_worker(request));
        }
    }
    // Stdin closed: the extension's gone. The worker finishes what it
    // was asked, then ends.
    drop(worker);
}
