//! What a `VncClient` does when the server on the other end goes away --
//! the VNC server process closed, the VM shut down -- staged against the
//! crate's fake RFB server (`fenix_vnc::test_server`).

use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use fenix_vnc::test_server::{TestServer, Traffic};
use fenix_vnc::{VncClient, VncFrame};

/// Connects, lets the session run until the first frame has been drawn,
/// takes the server down, and returns every frame that arrived afterwards
/// up to the point the channel closed (`None` if `timeout` ran out first).
fn frames_after_server_dies(traffic: Traffic, timeout: Duration) -> (VncClient, Option<Vec<VncFrame>>) {
    let server = TestServer::start(traffic);
    let (client, frames) = VncClient::connect("127.0.0.1", server.port()).expect("handshake with the fake server");
    // Wait for the first whole frame, then let a little steady-state
    // traffic flow so a busy server really has requests in flight.
    let deadline = Instant::now() + Duration::from_secs(5);
    while frames.recv_timeout(Duration::from_millis(50)) != Ok(VncFrame::UpdateEnd) {
        assert!(Instant::now() < deadline, "no first frame from the fake server");
    }
    thread::sleep(Duration::from_millis(200));
    while frames.try_recv().is_ok() {}

    server.go_down();

    let deadline = Instant::now() + timeout;
    let mut after = Vec::new();
    loop {
        match frames.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(frame) => after.push(frame),
            Err(RecvTimeoutError::Disconnected) => return (client, Some(after)),
            Err(RecvTimeoutError::Timeout) => return (client, None),
        }
    }
}

fn assert_reports_disconnect(traffic: Traffic) {
    let (client, after) = frames_after_server_dies(traffic, Duration::from_secs(5));
    let after = after.expect("the frame channel should close once the server is gone, not stay open forever");
    assert!(
        matches!(after.last(), Some(VncFrame::Disconnected(reason)) if reason == fenix_vnc::SERVER_CLOSED),
        "the last thing the caller hears must be Disconnected -- it's the only signal to reconnect -- got {after:?}"
    );
    // Dropping a dead client must not hang the caller.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        drop(client);
        let _ = done_tx.send(());
    });
    assert!(done_rx.recv_timeout(Duration::from_secs(5)).is_ok(), "dropping a client whose server died hung");
}

#[test]
fn an_idle_session_reports_the_server_going_away() {
    assert_reports_disconnect(Traffic::Idle);
}

#[test]
fn a_busy_session_reports_the_server_going_away() {
    // Repeated, since a busy session is where a periodic update request
    // can race the socket closing.
    for _ in 0..10 {
        assert_reports_disconnect(Traffic::Busy);
    }
}

#[test]
fn a_server_that_comes_back_on_the_same_port_accepts_a_new_connection() {
    let server = TestServer::start(Traffic::Idle);
    let first = VncClient::connect("127.0.0.1", server.port());
    assert!(first.is_ok());
    server.go_down();
    assert!(VncClient::connect("127.0.0.1", server.port()).is_err(), "a down server hangs up");
    server.come_back();
    assert!(VncClient::connect("127.0.0.1", server.port()).is_ok());
}
