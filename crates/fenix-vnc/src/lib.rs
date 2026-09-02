//! A real, interactive VNC (RFB) client -- connects to one VM, decodes
//! its framebuffer updates, and lets a host send keyboard/pointer/
//! clipboard input back. Deliberately has no `winit`/`wgpu` knowledge of
//! its own, same split this workspace already uses for `fenix-terminal`
//! (pure PTY/shell logic) versus `fenix-gui` (owns the actual background
//! reader thread, texture upload, and `FenixUserEvent` wiring).
//!
//! One deliberate asymmetry from `fenix-terminal`'s shape: the
//! underlying protocol crate (`vnc-rs`) is async-only, with no
//! synchronous "just give me a `Read`" escape hatch the way a PTY's
//! master fd is. So unlike `fenix-terminal` (which hands its caller a
//! plain `Box<dyn Read + Send>` and owns no thread itself), this crate
//! *does* own one dedicated OS thread per connection, running a small
//! `current_thread` tokio runtime for the connection's entire lifetime
//! -- tokio is fully contained here and never becomes a dependency of
//! `fenix-gui` itself, which only ever sees this crate's plain
//! synchronous-looking `connect`/`send_*` methods and reads decoded
//! `VncFrame`s off an ordinary `std::sync::mpsc::Receiver`.

pub mod coords;
pub mod framebuffer;
pub mod keysym;

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use vnc::{ClientKeyEvent, ClientMouseEvent, PixelFormat, VncConnector, VncEncoding, VncEvent, VncError, X11Event};

/// Minimum gap between one framebuffer update finishing and a
/// *visible/focused* session asking for the next -- a floor, not a
/// timer. `pump_events` keeps one request outstanding at a time, so the
/// real rate is whatever the server can actually produce; this only
/// stops a server that answers instantly (some reply to an incremental
/// request with an empty update rather than holding it) from turning
/// that into a busy loop. ~60Hz, i.e. below the point where the extra
/// wait is perceptible.
pub const ACTIVE_REFRESH_MILLIS: u64 = 16;

/// The same floor for a session no one is currently looking at -- still
/// frequent enough that switching to it never starts from a visibly
/// stale frame, but far cheaper for however long it stays unfocused.
/// Set via `VncClient::set_active(false)`.
pub const IDLE_REFRESH_MILLIS: u64 = 500;

/// One decoded update from the VM, handed to the caller's own background
/// reader thread (mirroring `fenix_terminal::Terminal::process`'s "caller
/// owns the reading" shape, just pre-decoded here since RFB frames are
/// structured messages, not an opaque byte stream).
#[derive(Debug, Clone, PartialEq)]
pub enum VncFrame {
    /// The server's framebuffer resolution -- sent once up front and
    /// again on any later resize.
    Resolution { width: u16, height: u16 },
    /// A dirty rectangle, tightly-packed BGRA (we always request
    /// `PixelFormat::bgra()`, see `do_handshake`, so this needs no
    /// channel-swizzle before it can go straight into a
    /// `wgpu::TextureFormat::Bgra8Unorm` texture).
    Rect { x: u16, y: u16, width: u16, height: u16, bgra: Vec<u8> },
    /// Copy already-decoded pixels from one region of the framebuffer to
    /// another (the `CopyRect` encoding) -- cheaper than the server
    /// resending pixels it knows the client already has.
    Copy { dst: (u16, u16, u16, u16), src: (u16, u16, u16, u16) },
    /// Every `Rect`/`Copy` since the last `UpdateEnd` formed one server
    /// `FramebufferUpdate` message, and the framebuffer is now
    /// self-consistent again.
    ///
    /// This is the *only* point at which it's correct to show the
    /// framebuffer. RFB defines `CopyRect` against the framebuffer as it
    /// was at the start of the update, so a frame presented partway
    /// through one legitimately shows content in two places at once, or
    /// regions that should already have been erased -- which is exactly
    /// what "the VNC pane renders in torn chunks" turned out to be.
    /// Requires the vendored `vnc-rs` patch (`vendor/vnc-rs/PATCH.md`);
    /// the published crate decodes the message's rectangle count but
    /// never surfaces the boundary.
    UpdateEnd,
    /// A new mouse cursor shape (the `CursorPseudo` encoding), as
    /// tightly-packed BGRA plus the hotspot offset within it. `bgra` is
    /// empty for a hidden/empty cursor.
    ///
    /// Requesting `CursorPseudo` is what stops the server from drawing
    /// the cursor *into* the framebuffer, where every pointer move would
    /// otherwise damage two regions (old position and new) and so keep a
    /// dragging session's update stream permanently busy. The caller
    /// draws this itself, at whatever position it last sent via
    /// `send_pointer`.
    Cursor { width: u16, height: u16, hotspot_x: u16, hotspot_y: u16, bgra: Vec<u8> },
    Bell,
    /// The server's clipboard changed. Only Latin-1 per RFB's own spec.
    ClipboardText(String),
    /// The connection ended, carrying why (a protocol/IO error, or the
    /// server's own `VncEvent::Error`). No more frames follow this one.
    Disconnected(String),
}

/// A live connection to one VM. Cheap to hold onto indefinitely (per this
/// project's "each VM stays connected in the background" decision) --
/// all the real work happens on its own dedicated thread.
pub struct VncClient {
    handle: tokio::runtime::Handle,
    inner: vnc::VncClient,
    thread: Option<thread::JoinHandle<()>>,
    /// Current refresh-poll interval in milliseconds, shared with the
    /// background `refresh_loop` task -- `set_active` is the only way
    /// this changes.
    refresh_millis: Arc<AtomicU64>,
}

impl VncClient {
    /// Connects to `host:port` and performs the full RFB handshake
    /// (no-auth only -- see `do_handshake`) before returning, so the
    /// caller knows immediately whether the connection actually
    /// succeeded. Everything after that -- decoding framebuffer updates,
    /// keeping the connection alive -- happens on a dedicated background
    /// thread; decoded frames arrive on the returned `Receiver`.
    ///
    /// This call blocks the calling thread for as long as the handshake
    /// takes (DNS + TCP connect + RFB negotiation) -- callers that can't
    /// afford to block their own thread (e.g. `fenix-gui`'s main/render
    /// thread) should call this from their own background thread instead
    /// of inline, the same way `fenix_terminal::Terminal::spawn` is
    /// already backgrounded by `App::toggle_terminal` for the same
    /// reason (a slow/unreachable host can otherwise freeze the caller).
    pub fn connect(host: &str, port: u16) -> io::Result<(VncClient, std_mpsc::Receiver<VncFrame>)> {
        let (setup_tx, setup_rx) = std_mpsc::channel::<io::Result<(vnc::VncClient, tokio::runtime::Handle)>>();
        let (frame_tx, frame_rx) = std_mpsc::channel::<VncFrame>();
        let host = host.to_string();
        let refresh_millis = Arc::new(AtomicU64::new(ACTIVE_REFRESH_MILLIS));
        let refresh_millis_for_thread = refresh_millis.clone();

        let thread = thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = setup_tx.send(Err(err));
                    return;
                }
            };
            let handle = runtime.handle().clone();
            runtime.block_on(async move {
                let client = match do_handshake(&host, port).await {
                    Ok(client) => client,
                    Err(err) => {
                        let _ = setup_tx.send(Err(err));
                        return;
                    }
                };
                if setup_tx.send(Ok((client.clone(), handle))).is_err() {
                    // `connect`'s caller gave up (dropped its receiver)
                    // before the handshake finished -- nothing left to
                    // pump frames to.
                    return;
                }
                // The server only ever sends framebuffer data in
                // response to an explicit request: the one automatic
                // `FramebufferUpdateRequest` `vnc-rs` fires during the
                // handshake covers the very first frame only -- without
                // this, a session would render exactly one frame and
                // then go completely stale forever. Requesting an
                // *incremental* update on a fixed cadence (rather than
                // only after each update lands) is the same "poll at a
                // steady rate" shape `vnc-rs`'s own example client uses;
                // an idle screen costs the server nothing extra to
                // answer since incremental means "only send what
                // changed." Dropped when the connection ends (see
                // `refresh_loop`'s own doc comment). Starts at the full
                // active rate for every session regardless of whether
                // its pane is currently visible -- the caller (`fenix-
                // gui`) calls `set_active(false)` once it knows better,
                // via `refresh_millis_for_thread`.
                pump_events(client, frame_tx, refresh_millis_for_thread).await;
            });
        });

        match setup_rx.recv() {
            Ok(Ok((inner, handle))) => Ok((VncClient { handle, inner, thread: Some(thread), refresh_millis }, frame_rx)),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other("VNC connection thread exited without reporting a result"))
            }
        }
    }

    /// Adjusts how often this session polls for updates: `true` (the
    /// default) for the full-rate `ACTIVE_REFRESH_MILLIS` used while the
    /// pane is visible/focused, `false` for the much slower `IDLE_
    /// REFRESH_MILLIS` while it isn't -- a session no one is looking at
    /// still stays connected and current enough not to show a stale
    /// frame the moment it's switched back to, just at a fraction of the
    /// request/response overhead. Takes effect on the *next* tick of the
    /// background refresh loop, not immediately.
    pub fn set_active(&self, active: bool) {
        let millis = if active { ACTIVE_REFRESH_MILLIS } else { IDLE_REFRESH_MILLIS };
        self.refresh_millis.store(millis, Ordering::Relaxed);
    }

    /// Sends one key event. `keysym` is an X11 keysym (see the
    /// `keysym` module) and `down` is a real press (`true`) or release
    /// (`false`) -- RFB genuinely distinguishes these (held keys,
    /// auto-repeat, drag-select inside the guest), so this should not be
    /// called with an immediate synthetic down-then-up unless the caller
    /// truly has no way to observe the real key-up.
    pub fn send_key(&self, keysym: u32, down: bool) {
        let client = self.inner.clone();
        self.handle.spawn(async move {
            let _ = client.input(X11Event::KeyEvent(ClientKeyEvent { keycode: keysym, down })).await;
        });
    }

    /// Sends one pointer update. `button_mask` is the RFB pointer bitmask
    /// of *all currently-held* buttons (bit0=left, bit1=middle,
    /// bit2=right, bit3/4=wheel up/down) -- it must be resent in full on
    /// every move, not just on transitions, per RFB's stateful pointer
    /// semantics.
    pub fn send_pointer(&self, x: u16, y: u16, button_mask: u8) {
        let client = self.inner.clone();
        self.handle.spawn(async move {
            let _ = client.input(X11Event::PointerEvent(ClientMouseEvent { position_x: x, position_y: y, bottons: button_mask })).await;
        });
    }

    /// Pushes local clipboard text to the VM's clipboard (RFB `CutText`).
    /// Latin-1 only, per RFB's own spec -- non-Latin-1 text is the
    /// caller's problem to decide how to handle (drop, transliterate,
    /// etc.), not this crate's.
    pub fn send_clipboard(&self, text: String) {
        let client = self.inner.clone();
        self.handle.spawn(async move {
            let _ = client.input(X11Event::CopyText(text)).await;
        });
    }
}

impl Drop for VncClient {
    /// Asks the connection to close, which makes the background thread's
    /// event loop see its next receive fail and exit on its own, then
    /// joins that thread -- same "Drop stops the background thread, then
    /// waits for it to actually be gone" shape as `DockerLogFollower`/
    /// `TerminalReader` in `fenix-gui`, just without a bounded retry loop
    /// since there's no external process to force-kill here, only a
    /// socket close to wait out.
    fn drop(&mut self) {
        let inner = self.inner.clone();
        let _ = self.handle.spawn(async move {
            let _ = inner.close().await;
        });
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Connects and negotiates the RFB session. No-auth only for v1 (the
/// trivial auth callback is never actually polled unless the server asks
/// for a password -- `vnc-rs` only calls it when a security type other
/// than `None` is negotiated). Deliberately does not request `Tight`
/// encoding: its JPEG subtype would need a JPEG-decode dependency this
/// crate doesn't otherwise need; `Zrle`/`CopyRect`/`Raw` alone are
/// already a good fit for a LAN-local VM console and `vnc-rs` decodes all
/// three down to plain `VncEvent::RawImage`/`Copy` events regardless.
///
/// The three pseudo-encodings are each load-bearing, not nice-to-haves:
///
/// * `CursorPseudo` moves the mouse cursor out of the framebuffer and
///   into a `VncFrame::Cursor` the caller draws itself. Without it the
///   server composites the cursor into the image, so *every* pointer
///   move damages two regions (old position and new) -- which on a
///   desktop being dragged around means the update stream never goes
///   idle, and every one of those updates costs a full decode and
///   re-upload for a few cursor pixels.
/// * `DesktopSizePseudo` is how a server announces a resolution change
///   after connect. Without it `VncEvent::SetResolution` only ever fires
///   once, from the initial handshake, and a guest that changes
///   resolution mid-session would keep being decoded against a
///   stale-sized framebuffer.
/// * `LastRectPseudo` lets the server end an update without knowing its
///   rectangle count up front; `vnc-rs` closes the update either way, so
///   this only widens what `VncFrame::UpdateEnd` works against.
async fn do_handshake(host: &str, port: u16) -> io::Result<vnc::VncClient> {
    let stream = TcpStream::connect((host, port)).await?;
    let connector = VncConnector::new(stream)
        .set_auth_method(async move { Ok(String::new()) })
        .add_encoding(VncEncoding::Zrle)
        .add_encoding(VncEncoding::CopyRect)
        .add_encoding(VncEncoding::Raw)
        .add_encoding(VncEncoding::CursorPseudo)
        .add_encoding(VncEncoding::DesktopSizePseudo)
        .add_encoding(VncEncoding::LastRectPseudo)
        .allow_shared(true)
        .set_pixel_format(PixelFormat::bgra())
        .build()
        .map_err(vnc_err_to_io)?;
    let state = connector.try_start().await.map_err(vnc_err_to_io)?;
    state.finish().map_err(vnc_err_to_io)
}

fn vnc_err_to_io(err: VncError) -> io::Error {
    io::Error::other(err.to_string())
}

/// The connection's entire background lifetime: forwards every decoded
/// frame until the connection ends (either side), then returns, letting
/// its caller's `runtime.block_on` (and with it, the dedicated thread)
/// finish naturally. Also drives the periodic incremental-refresh
/// request in this same loop, at whatever rate `refresh_millis` says
/// (`VncClient::set_active` is the only thing that ever changes it) --
/// see this function's own doc comment on why that can't be a separate
/// task.
///
/// Deliberately `poll_event` (non-blocking), not `recv_event`
/// (blocking), and deliberately *one* loop rather than this plus a
/// separate refresh task: `vnc-rs`'s `VncClient` guards its internal
/// state with one `tokio::sync::Mutex`, and `recv_event`/`input` both
/// lock it -- `recv_event` holds that lock for as long as it's waiting
/// for the *next* event, which after the very first frame is
/// indefinite (the server sends nothing more until a new
/// `FramebufferUpdateRequest` arrives). A separate task calling
/// `client.input(X11Event::Refresh)` to send that very request would
/// then block forever trying to acquire a lock `recv_event` never lets
/// go of -- a self-inflicted deadlock where the first frame renders and
/// nothing (updates, key/pointer input) ever gets through again. This
/// is also exactly the shape `vnc-rs`'s own doc example uses: one loop,
/// `poll_event` plus a periodic `input(Refresh)`, never `recv_event`
/// racing another task's `input`.
///
/// Keeps exactly *one* `FramebufferUpdateRequest` outstanding at a time,
/// the same request/response flow control every other RFB client uses:
/// ask once, wait for that update to arrive in full (`VncFrame::
/// UpdateEnd`), then ask again. `refresh_millis` is a floor on how often
/// to ask -- a minimum gap between one update completing and the next
/// request going out -- not a timer that fires regardless.
///
/// This is deliberately not a fixed-interval poll. Firing every
/// `ACTIVE_REFRESH_MILLIS` no matter whether the last request had been
/// answered lets several overlapping requests pile up against a server
/// that takes longer than that to answer, and gives this loop no idea
/// which arriving rectangles belong to which update. Both matter: the
/// pile-up wastes the server's time re-scanning, and the missing
/// boundary is what let `fenix-gui` present half-finished frames (see
/// `VncFrame::UpdateEnd`).
///
/// The one thing to be careful about: an *incremental* request goes
/// unanswered for as long as nothing on the guest changes, which is
/// correct and is what keeps an idle session cheap -- but it means the
/// only thing that can un-stick this loop is the server, so there is no
/// timeout here to "retry" with. A genuinely dead connection surfaces as
/// a read error from `poll_event` instead.
async fn pump_events(client: vnc::VncClient, frame_tx: std_mpsc::Sender<VncFrame>, refresh_millis: Arc<AtomicU64>) {
    /// Poll granularity: fine enough to keep decode latency well under a
    /// frame, coarse enough not to spin the thread.
    const POLL_MILLIS: u64 = 5;
    // `vnc-rs` sends one `FramebufferUpdateRequest` of its own during the
    // handshake, so an update is already in flight before this loop
    // starts -- starting at `true` avoids immediately stacking a second
    // request on top of it.
    let mut awaiting_response = true;
    let mut last_update_end = Instant::now();
    loop {
        match client.poll_event().await {
            Ok(Some(event)) => {
                let Some(frame) = map_event(event) else { continue };
                if matches!(frame, VncFrame::UpdateEnd) {
                    awaiting_response = false;
                    last_update_end = Instant::now();
                }
                let disconnected = matches!(frame, VncFrame::Disconnected(_));
                if frame_tx.send(frame).is_err() || disconnected {
                    return;
                }
                // Keep draining: one `FramebufferUpdate` decodes into many
                // events, and sleeping between each would stretch a single
                // update across many poll intervals for no reason.
                continue;
            }
            Ok(None) => {}
            Err(err) => {
                let _ = frame_tx.send(VncFrame::Disconnected(err.to_string()));
                return;
            }
        }

        if !awaiting_response && last_update_end.elapsed() >= Duration::from_millis(refresh_millis.load(Ordering::Relaxed)) {
            if client.input(X11Event::Refresh).await.is_err() {
                return;
            }
            awaiting_response = true;
        }

        tokio::time::sleep(Duration::from_millis(POLL_MILLIS)).await;
    }
}

/// `None` for events this client never expects to receive, given the
/// encodings/pixel format `do_handshake` requests: `SetPixelFormat` (we
/// always set our own) and `JpegImage` (we don't request `Tight`).
///
/// `SetCursor`'s rect carries the hotspot in its `x`/`y` (not a position
/// -- see `vnc-rs`'s own cursor decoder), and its image is already BGRA
/// with the RFB bitmask folded into the alpha channel, so it maps
/// straight onto `VncFrame::Cursor`.
fn map_event(event: VncEvent) -> Option<VncFrame> {
    match event {
        VncEvent::SetResolution(screen) => Some(VncFrame::Resolution { width: screen.width, height: screen.height }),
        VncEvent::RawImage(rect, data) => Some(VncFrame::Rect { x: rect.x, y: rect.y, width: rect.width, height: rect.height, bgra: data }),
        VncEvent::Copy(dst, src) => Some(VncFrame::Copy { dst: (dst.x, dst.y, dst.width, dst.height), src: (src.x, src.y, src.width, src.height) }),
        VncEvent::FramebufferUpdateEnd => Some(VncFrame::UpdateEnd),
        VncEvent::SetCursor(rect, image) => {
            Some(VncFrame::Cursor { width: rect.width, height: rect.height, hotspot_x: rect.x, hotspot_y: rect.y, bgra: image })
        }
        VncEvent::Bell => Some(VncFrame::Bell),
        VncEvent::Text(text) => Some(VncFrame::ClipboardText(text)),
        VncEvent::Error(msg) => Some(VncFrame::Disconnected(msg)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_event_translates_resolution_rect_copy_bell_and_text() {
        assert_eq!(map_event(VncEvent::SetResolution(vnc::Screen { width: 800, height: 600 })), Some(VncFrame::Resolution { width: 800, height: 600 }));
        assert_eq!(
            map_event(VncEvent::RawImage(vnc::Rect { x: 1, y: 2, width: 3, height: 4 }, vec![9, 9, 9, 9])),
            Some(VncFrame::Rect { x: 1, y: 2, width: 3, height: 4, bgra: vec![9, 9, 9, 9] })
        );
        assert_eq!(
            map_event(VncEvent::Copy(vnc::Rect { x: 0, y: 0, width: 10, height: 10 }, vnc::Rect { x: 5, y: 5, width: 10, height: 10 })),
            Some(VncFrame::Copy { dst: (0, 0, 10, 10), src: (5, 5, 10, 10) })
        );
        assert_eq!(map_event(VncEvent::Bell), Some(VncFrame::Bell));
        assert_eq!(map_event(VncEvent::Text("hello".to_string())), Some(VncFrame::ClipboardText("hello".to_string())));
    }

    #[test]
    fn map_event_translates_a_server_error_to_disconnected() {
        assert_eq!(map_event(VncEvent::Error("socket reset".to_string())), Some(VncFrame::Disconnected("socket reset".to_string())));
    }

    #[test]
    fn map_event_ignores_pixel_format_and_jpeg_events() {
        assert_eq!(map_event(VncEvent::SetPixelFormat(PixelFormat::bgra())), None);
        assert_eq!(map_event(VncEvent::JpegImage(vnc::Rect { x: 0, y: 0, width: 1, height: 1 }, vec![])), None);
    }

    #[test]
    fn map_event_translates_the_end_of_a_framebuffer_update() {
        // The whole point of the vendored `vnc-rs` patch -- without this
        // reaching the caller there's no way to know which rects formed
        // one whole frame. See `VncFrame::UpdateEnd`.
        assert_eq!(map_event(VncEvent::FramebufferUpdateEnd), Some(VncFrame::UpdateEnd));
    }

    #[test]
    fn map_event_translates_a_cursor_taking_the_hotspot_from_the_rects_position() {
        // `SetCursor`'s rect carries the hotspot in `x`/`y` rather than a
        // screen position (see `vnc-rs`'s cursor decoder) -- getting this
        // mapping backwards would offset every drawn cursor.
        assert_eq!(
            map_event(VncEvent::SetCursor(vnc::Rect { x: 3, y: 4, width: 16, height: 16 }, vec![7; 16 * 16 * 4])),
            Some(VncFrame::Cursor { width: 16, height: 16, hotspot_x: 3, hotspot_y: 4, bgra: vec![7; 16 * 16 * 4] })
        );
    }

    #[test]
    fn connect_to_an_unreachable_host_fails_cleanly_instead_of_hanging() {
        // Port 0 never accepts a real connection -- this is the one
        // "real I/O" case worth covering automatically (a fast, local,
        // guaranteed-to-fail path), as opposed to anything requiring an
        // actual RFB server, which per this crate's own test strategy
        // is manual-only (see the workspace plan).
        let result = VncClient::connect("127.0.0.1", 0);
        assert!(result.is_err());
    }
}
