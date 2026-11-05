//! Single instance, and the control channel other tools use: makes a
//! second `fenix` launch (Explorer's "Open With" on a double-clicked
//! file, `fenix src/main.rs:12` in a terminal, a tool like Flint) hand
//! what it was asked for to the Fenix already running instead of
//! opening a second editor -- the "one real process, every later launch
//! is a thin client" shape `emacsclient` has.
//!
//! The endpoint, the protocol and its checks are `fenix-control`'s; this
//! module is the glue to the event loop. The first launch takes the
//! endpoint and serves it from a background thread; each request that
//! needs the editor becomes a `FenixUserEvent::Control` carrying a reply
//! channel, answered on the UI thread like every other cross-thread
//! event, so nothing new is locked. A later launch connects, says
//! `fenix/hello`, lets Fenix take the foreground (Windows only lets the
//! foreground process hand that on), and sends `fenix/open` and
//! `fenix/openProject`.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use fenix_control::protocol::OpenPath;
use fenix_control::{
    BindError, Call, Client, ClientError, Endpoint, Listener, OpenParams, RpcError, ServerInfo,
};
use fenix_tasks::PathPosition;
use serde_json::Value;
use winit::event_loop::EventLoopProxy;

use crate::app::FenixUserEvent;
use crate::cli::Invocation;

/// How long a request waits for the editor to answer it. Generous: a
/// request arriving during startup waits for the editor to load. Past
/// it the client gets an error rather than hanging, which a frozen UI
/// thread would otherwise make it do.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// What this launch turned out to be, once single-instance negotiation
/// settles.
pub enum Role {
    /// The first or only instance: it holds the endpoint for as long as
    /// it runs. Pass the listener to `spawn_accept_loop`.
    Server(Listener),
    /// A running Fenix took this launch's request; exit without opening
    /// a window.
    HandedOff,
    /// Neither: the endpoint couldn't be used (an unsafe directory, a
    /// Fenix that wouldn't answer). Run as an ordinary editor, without
    /// serving later launches.
    Standalone,
}

/// Hands `invocation` to the Fenix serving this user's endpoint, or
/// takes the endpoint if nobody is.
pub fn negotiate(invocation: &Invocation) -> Role {
    negotiate_at(&Endpoint::current(), invocation)
}

fn negotiate_at(endpoint: &Endpoint, invocation: &Invocation) -> Role {
    // Two launches at once (Explorer opening a selection of files can
    // start one per file) race for the endpoint; the loser connects to
    // the winner, which may need a moment to start listening.
    for _ in 0..20 {
        match Client::connect(endpoint) {
            Ok(Some(mut client)) => {
                return match hand_off(&mut client, invocation) {
                    Ok(()) => Role::HandedOff,
                    Err(e) => {
                        eprintln!("fenix: the running Fenix didn't take this launch ({e}); starting another");
                        Role::Standalone
                    }
                };
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("fenix: can't reach the control endpoint ({e}); running on its own");
                return Role::Standalone;
            }
        }
        match endpoint.bind() {
            Ok(listener) => return Role::Server(listener),
            Err(BindError::InUse) => std::thread::sleep(Duration::from_millis(50)),
            Err(BindError::Io(e)) => {
                eprintln!("fenix: can't serve the control endpoint ({e}); running on its own");
                return Role::Standalone;
            }
        }
    }
    Role::Standalone
}

/// Set on the Fenix a restart (`SPC q r`) starts: the one it replaces is
/// still quitting, so this one waits for its endpoint to go before
/// taking it, rather than handing itself to the Fenix that's leaving.
pub const RESTARTED_ENV: &str = "FENIX_RESTARTED";

/// Waits, up to `timeout`, until nobody serves `endpoint`: true once
/// it's free.
pub fn wait_until_free(timeout: Duration) -> bool {
    wait_until_free_at(&Endpoint::current(), timeout)
}

fn wait_until_free_at(endpoint: &Endpoint, timeout: Duration) -> bool {
    let until = std::time::Instant::now() + timeout;
    loop {
        // Only "nobody serves it" is free. A connection that fails (a
        // Windows pipe whose one instance is taken answers "busy") means
        // the old Fenix still holds it.
        match Client::connect(endpoint) {
            Ok(None) => return true,
            _ if std::time::Instant::now() < until => std::thread::sleep(Duration::from_millis(100)),
            _ => return false,
        }
    }
}

/// The client half: `fenix/hello`, the foreground handed on, then the
/// project and the files. A bare relaunch still sends an empty
/// `fenix/open`, which brings Fenix forward.
fn hand_off<R: std::io::Read, W: std::io::Write>(
    client: &mut Client<R, W>,
    invocation: &Invocation,
) -> Result<(), ClientError> {
    let hello = client.hello("fenix", env!("CARGO_PKG_VERSION"))?;
    allow_foreground(hello.pid);
    if let Some(project) = &invocation.project {
        client.open_project(&plain(project))?;
        if invocation.files.is_empty() {
            return Ok(());
        }
    }
    let paths = invocation
        .files
        .iter()
        .map(|f| OpenPath {
            path: plain(&f.path),
            line: f.line,
            column: f.column,
        })
        .collect();
    client.open(&OpenParams {
        paths,
        new_window: invocation.new_window,
        focus: true,
    })
}

fn plain(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Lets the running Fenix take the foreground. Windows refuses
/// `SetForegroundWindow` to a process the user isn't using, so the one
/// they are -- this launch, which Explorer or a terminal just started --
/// has to pass the right on before asking. Only that pid, and only
/// until the user's next input.
#[cfg(windows)]
fn allow_foreground(pid: u32) {
    // SAFETY: a plain call; failing just means Fenix may only flash.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
}

#[cfg(not(windows))]
fn allow_foreground(_pid: u32) {}

/// The reply to one control request, sent back from the UI thread.
pub type Reply = Result<Value, RpcError>;

/// Spawns the background thread that serves the endpoint for as long as
/// this process runs, with a thread per connection so a client that
/// keeps one open doesn't hold up the next. Each request the editor has
/// to answer goes to it as `FenixUserEvent::Control`.
pub fn spawn_accept_loop(mut listener: Listener, proxy: EventLoopProxy<FenixUserEvent>) {
    let info = ServerInfo::new(env!("CARGO_PKG_VERSION"), std::process::id());
    std::thread::spawn(move || loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("fenix: control endpoint: {e}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let proxy = proxy.clone();
        let info = info.clone();
        std::thread::spawn(move || {
            let Ok(writer) = stream.try_clone() else {
                return;
            };
            let _ = fenix_control::serve(stream, writer, &info, |call| ask_editor(&proxy, call));
        });
    });
}

/// Hands `call` to the UI thread and waits for its answer.
fn ask_editor(proxy: &EventLoopProxy<FenixUserEvent>, call: Call) -> Reply {
    let (reply, answer) = mpsc::channel();
    proxy
        .send_event(FenixUserEvent::Control { call, reply })
        .map_err(|_| RpcError::internal("Fenix is closing"))?;
    answer
        .recv_timeout(ANSWER_TIMEOUT)
        .map_err(|_| RpcError::internal("Fenix didn't answer in time"))?
}

/// `fenix/open`'s paths as the editor opens them.
pub fn positions(paths: &[OpenPath]) -> Vec<PathPosition> {
    paths
        .iter()
        .map(|p| PathPosition {
            path: p.path.clone().into(),
            line: p.line,
            column: p.column,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn unique() -> Endpoint {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        Endpoint::instance(&format!(
            "gui-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Serves one connection with a recording editor; its calls come
    /// back when the client hangs up.
    fn serve_one(listener: Listener) -> mpsc::Receiver<Vec<Call>> {
        let mut listener = listener;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            let writer = stream.try_clone().unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&calls);
            let info = ServerInfo::new("1", 1);
            let _ = fenix_control::serve(stream, writer, &info, move |call| {
                seen.lock().unwrap().push(call);
                Ok(serde_json::json!({}))
            });
            tx.send(calls.lock().unwrap().clone()).unwrap();
        });
        rx
    }

    /// A restarted Fenix waits for the one quitting to let go of the
    /// endpoint, and not forever.
    #[test]
    fn a_restart_waits_for_the_endpoint_to_be_free() {
        let endpoint = unique();
        assert!(wait_until_free_at(&endpoint, Duration::from_millis(10)), "nobody serves it yet");
        let Role::Server(listener) = negotiate_at(&endpoint, &Invocation::default()) else { panic!("should serve") };
        assert!(!wait_until_free_at(&endpoint, Duration::from_millis(300)), "still served");
        drop(listener);
    }

    #[test]
    fn the_first_launch_becomes_the_server() {
        assert!(matches!(
            negotiate_at(&unique(), &Invocation::default()),
            Role::Server(_)
        ));
    }

    #[test]
    fn a_later_launch_hands_its_files_and_positions_over() {
        let endpoint = unique();
        let Role::Server(listener) = negotiate_at(&endpoint, &Invocation::default()) else {
            panic!("first launch should serve")
        };
        let calls = serve_one(listener);
        let invocation = Invocation {
            new_window: true,
            files: vec![
                PathPosition {
                    path: PathBuf::from(r"C:\x.rs"),
                    line: Some(12),
                    column: Some(3),
                },
                PathPosition {
                    path: PathBuf::from(r"C:\y.rs"),
                    line: None,
                    column: None,
                },
            ],
            project: Some(PathBuf::from(r"C:\repo")),
        };
        assert!(matches!(
            negotiate_at(&endpoint, &invocation),
            Role::HandedOff
        ));
        let calls = calls.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(matches!(&calls[0], Call::OpenProject(p) if p.root == r"C:\repo"));
        let Call::Open(open) = &calls[1] else {
            panic!("{calls:?}")
        };
        assert!(open.new_window && open.focus);
        assert_eq!(positions(&open.paths), invocation.files);
    }

    #[test]
    fn a_bare_relaunch_asks_to_be_brought_forward() {
        let endpoint = unique();
        let Role::Server(listener) = negotiate_at(&endpoint, &Invocation::default()) else {
            panic!("first launch should serve")
        };
        let calls = serve_one(listener);
        assert!(matches!(
            negotiate_at(&endpoint, &Invocation::default()),
            Role::HandedOff
        ));
        let calls = calls.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            calls,
            vec![Call::Open(OpenParams {
                paths: vec![],
                new_window: false,
                focus: true
            })]
        );
    }
}
