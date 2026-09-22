//! A tiny in-process RFB server for tests: no auth, a 4x4 framebuffer,
//! raw encoding. Exists so what happens when a server goes away -- and
//! comes back -- can be staged exactly, without a real VNC server.
//! Plain `std`, one thread per server; not part of the client's API.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const W: u16 = 4;
const H: u16 = 4;

/// How the server answers incremental update requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Traffic {
    /// Never -- an idle desktop, the common case.
    Idle,
    /// Every one, immediately -- a busy desktop, so the client always
    /// has a request in flight.
    Busy,
}

/// A running fake server on `127.0.0.1:port()`. It can be taken down --
/// the live connection closed and new ones hung up on, the way a closed
/// VNC server behaves -- and brought back up on the same port.
pub struct TestServer {
    port: u16,
    up: Arc<AtomicBool>,
    /// Bumped on every `go_down`; a connection serving an older
    /// generation closes itself.
    generation: Arc<AtomicU64>,
}

impl TestServer {
    pub fn start(traffic: Traffic) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let port = listener.local_addr().unwrap().port();
        let up = Arc::new(AtomicBool::new(true));
        let generation = Arc::new(AtomicU64::new(0));
        let (up_for_thread, generation_for_thread) = (up.clone(), generation.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if !up_for_thread.load(Ordering::SeqCst) {
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                let generation = generation_for_thread.clone();
                let mine = generation.load(Ordering::SeqCst);
                thread::spawn(move || serve(stream, traffic, &generation, mine));
            }
        });
        TestServer { port, up, generation }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Closes every live connection and hangs up on new ones.
    pub fn go_down(&self) {
        self.up.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Accepts connections again.
    pub fn come_back(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.go_down();
    }
}

fn read_exact(s: &mut TcpStream, n: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0; n];
    s.read_exact(&mut buf).ok().map(|_| buf)
}

fn send_update(s: &mut TcpStream) -> std::io::Result<()> {
    let mut msg = vec![0u8, 0, 0, 1];
    for v in [0u16, 0, W, H] {
        msg.extend_from_slice(&v.to_be_bytes());
    }
    msg.extend_from_slice(&0i32.to_be_bytes());
    msg.extend(std::iter::repeat_n(0x80u8, W as usize * H as usize * 4));
    s.write_all(&msg)
}

fn handshake(s: &mut TcpStream) -> Option<()> {
    s.write_all(b"RFB 003.008\n").ok()?;
    read_exact(s, 12)?;
    s.write_all(&[1, 1]).ok()?; // one security type: None
    read_exact(s, 1)?;
    s.write_all(&0u32.to_be_bytes()).ok()?; // SecurityResult OK
    read_exact(s, 1)?; // ClientInit
    let mut init = Vec::new();
    init.extend_from_slice(&W.to_be_bytes());
    init.extend_from_slice(&H.to_be_bytes());
    // 32bpp, depth 24, little-endian, true colour, BGRA shifts.
    init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    init.extend_from_slice(&4u32.to_be_bytes());
    init.extend_from_slice(b"fake");
    s.write_all(&init).ok()
}

fn serve(mut s: TcpStream, traffic: Traffic, generation: &AtomicU64, mine: u64) {
    if handshake(&mut s).is_none() {
        return;
    }
    let _ = s.set_read_timeout(Some(Duration::from_millis(20)));
    let mut first = true;
    while generation.load(Ordering::SeqCst) == mine {
        let mut ty = [0u8; 1];
        match s.read(&mut ty) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => continue, // read timeout: check the generation again
        }
        let _ = s.set_read_timeout(None);
        let answer = match ty[0] {
            0 => read_exact(&mut s, 19).map(|_| false),
            2 => read_exact(&mut s, 3).and_then(|h| read_exact(&mut s, 4 * u16::from_be_bytes([h[1], h[2]]) as usize)).map(|_| false),
            3 => read_exact(&mut s, 9).map(|req| first || req[0] == 0 || traffic == Traffic::Busy),
            4 => read_exact(&mut s, 7).map(|_| false),
            5 => read_exact(&mut s, 5).map(|_| false),
            6 => read_exact(&mut s, 7).and_then(|h| read_exact(&mut s, u32::from_be_bytes([h[3], h[4], h[5], h[6]]) as usize)).map(|_| false),
            _ => None,
        };
        let _ = s.set_read_timeout(Some(Duration::from_millis(20)));
        match answer {
            Some(true) => {
                first = false;
                if send_update(&mut s).is_err() {
                    return;
                }
            }
            Some(false) => {}
            None => return,
        }
    }
    let _ = s.shutdown(Shutdown::Both);
}
