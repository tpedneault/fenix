//! The epoch ticker sleeps while no call runs (#74). In a test binary of
//! its own: whether a call is running is the whole process's, and the
//! other tests' calls would wake it.

use std::time::{Duration, Instant};

use fenix_wasm::{calling, engine, Options, Ticker};

/// Waits until `done` or `within` has passed.
fn eventually(within: Duration, done: impl Fn() -> bool) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    done()
}

#[test]
fn the_ticker_sleeps_while_no_call_runs_and_ticks_while_one_does() {
    let engine = engine(&Options::default()).unwrap();
    let ticker = Ticker::start(&engine);
    // Idle: it used to wake every 10 ms, 30 times here.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ticker.wakes(), 0, "it woke with nothing running");

    // A call: the epoch moves on, so its deadline comes.
    let call = calling();
    assert!(
        eventually(Duration::from_secs(10), || ticker.wakes() >= 3),
        "it didn't tick for a call: {}",
        ticker.wakes()
    );
    drop(call);

    // Done: at most the tick already under way, then nothing.
    std::thread::sleep(Duration::from_millis(50));
    let settled = ticker.wakes();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        ticker.wakes() <= settled + 1,
        "it went on waking: {settled} then {}",
        ticker.wakes()
    );
    drop(ticker);
}

#[test]
fn dropping_an_idle_ticker_doesnt_wait_for_a_call() {
    let engine = engine(&Options::default()).unwrap();
    let ticker = Ticker::start(&engine);
    std::thread::sleep(Duration::from_millis(20));
    let started = Instant::now();
    drop(ticker);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
