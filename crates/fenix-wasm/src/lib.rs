//! The WASM extension host's runtime: a wasmtime `Engine` set up the way
//! Fenix runs extensions. Every call into an extension has a deadline
//! (epoch interruption, ticked by `Ticker`), the wasm stack is bounded so
//! runaway recursion traps instead of taking Fenix down, and each
//! extension's memory is capped (`limits`).
//!
//! With `cranelift`, extensions are compiled here, to native code or to
//! Pulley bytecode (`Backend`); without it, only precompiled ones load.
//! See `docs/adr/0001-wasm-runtime.md` for why it's set up this way.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use wasmtime;

pub mod host;

/// What an extension's code runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Compiled to this machine's code: the fastest.
    Native,
    /// Compiled to Pulley bytecode and interpreted: portable, slower.
    Pulley,
    /// Compiled by Winch, wasmtime's baseline compiler: quick to
    /// compile, slower code than Cranelift's.
    Winch,
}

/// How the engine is set up.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub backend: Backend,
    /// The most wasm stack a call may use; past it, the call traps.
    pub max_stack: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { backend: Backend::Native, max_stack: 512 * 1024 }
    }
}

/// How often `Ticker` moves the epoch on: a deadline is counted in ticks.
pub const TICK: Duration = Duration::from_millis(10);

/// The engine every extension shares.
pub fn engine(options: &Options) -> wasmtime::Result<wasmtime::Engine> {
    let mut config = wasmtime::Config::new();
    config.epoch_interruption(true);
    config.max_wasm_stack(options.max_stack);
    config.wasm_component_model(true);
    match options.backend {
        Backend::Native => {}
        Backend::Pulley => {
            config.target("pulley64")?;
        }
        Backend::Winch => {
            config.strategy(wasmtime::Strategy::Winch);
        }
    }
    wasmtime::Engine::new(&config)
}

/// What one extension may use: `memory` bytes of linear memory, and a
/// handful of instances, tables and memories.
pub fn limits(memory: usize) -> wasmtime::StoreLimits {
    wasmtime::StoreLimitsBuilder::new().memory_size(memory).instances(16).tables(16).memories(4).trap_on_grow_failure(true).build()
}

/// `Ticker`'s sleep running this far over is the machine having slept.
const SLEPT_AFTER: Duration = Duration::from_secs(5);

/// Moves `engine`'s epoch on every `TICK` until dropped, so a call given
/// a deadline of `n` ticks traps about `n * TICK` after it starts.
pub struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// How many ticks to make up, `due` being where the clock says the count
/// should be. A run far past that is the machine having slept, not a call
/// having used the time: a call caught mid-way by the lid closing mustn't
/// be trapped for a deadline it had all of on waking.
fn ticks_owed(ticked: u128, due: u128) -> u128 {
    let owed = due.saturating_sub(ticked);
    if owed * TICK.as_nanos() > SLEPT_AFTER.as_nanos() { 1 } else { owed }
}

impl Ticker {
    pub fn start(engine: &wasmtime::Engine) -> Ticker {
        let stop = Arc::new(AtomicBool::new(false));
        let (engine, flag) = (engine.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("fenix-wasm-epoch".into())
            .spawn(move || {
                // By the clock, not by how many sleeps there were: a sleep
                // runs long (Windows' timer, a busy machine), and a deadline
                // counted in sleeps would then run long with it.
                let started = std::time::Instant::now();
                let mut ticked: u128 = 0;
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    let due = started.elapsed().as_nanos() / TICK.as_nanos();
                    let owed = ticks_owed(ticked, due);
                    for _ in 0..owed {
                        engine.increment_epoch();
                    }
                    ticked = due;
                }
            })
            .expect("a thread for the epoch");
        Ticker { stop, thread: Some(thread) }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The epoch counts time a call had, so a call mustn't be charged for
    /// the hours the lid was shut.
    #[test]
    fn the_machine_sleeping_isnt_time_a_call_used() {
        assert_eq!(ticks_owed(100, 100), 0);
        assert_eq!(ticks_owed(100, 103), 3, "a sleep that ran long is made up");
        let hours = 8 * 3600 * 1000 / TICK.as_millis();
        assert_eq!(ticks_owed(100, 100 + hours), 1);
    }
    use wasmtime::{Instance, Module, Store};

    fn run(engine: &wasmtime::Engine, wat: &str, export: &str, deadline: u64) -> wasmtime::Result<i32> {
        let module = Module::new(engine, wat)?;
        let mut store = Store::new(engine, limits(16 << 20));
        store.limiter(|l| l);
        store.set_epoch_deadline(deadline);
        let instance = Instance::new(&mut store, &module, &[])?;
        instance.get_typed_func::<(), i32>(&mut store, export)?.call(&mut store, ())
    }

    /// A call that never returns traps at its deadline; Fenix goes on.
    ///
    /// Only the call is timed, not compiling the module or starting it,
    /// and the bound is loose: a loaded runner (Windows CI) can starve
    /// the epoch thread for a second or two, and the deadline is counted
    /// by the clock, so the trap comes late but it comes. What it guards
    /// is that the call ends at all, not how close to the 50 ms it is.
    #[test]
    fn a_runaway_call_traps_at_its_deadline() {
        let engine = engine(&Options::default()).unwrap();
        let _ticker = Ticker::start(&engine);
        let module = Module::new(&engine, r#"(module (func (export "spin") (result i32) (loop $l (br $l)) (i32.const 0)))"#).unwrap();
        let mut store = Store::new(&engine, limits(16 << 20));
        store.limiter(|l| l);
        store.set_epoch_deadline(u64::MAX);
        let instance = Instance::new(&mut store, &module, &[]).unwrap();
        let spin = instance.get_typed_func::<(), i32>(&mut store, "spin").unwrap();
        store.set_epoch_deadline(5);
        let started = std::time::Instant::now();
        let err = spin.call(&mut store, ()).unwrap_err();
        assert_eq!(err.downcast_ref::<wasmtime::Trap>(), Some(&wasmtime::Trap::Interrupt), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
    }

    /// Recursion with no end traps on the stack bound instead of
    /// overflowing Fenix's own stack.
    #[test]
    fn runaway_recursion_traps_on_the_stack_bound() {
        let engine = engine(&Options::default()).unwrap();
        let err = run(&engine, r#"(module (func $f (export "down") (result i32) (i32.add (call $f) (i32.const 1))))"#, "down", u64::MAX).unwrap_err();
        assert_eq!(err.downcast_ref::<wasmtime::Trap>(), Some(&wasmtime::Trap::StackOverflow), "{err:?}");
    }

    /// Memory past the limit can't be had: growing it traps.
    #[test]
    fn memory_past_the_limit_traps() {
        let engine = engine(&Options::default()).unwrap();
        // 16 MiB allowed; 512 pages is 32 MiB.
        let err = run(&engine, r#"(module (memory 1) (func (export "grow") (result i32) (memory.grow (i32.const 512))))"#, "grow", u64::MAX).unwrap_err();
        assert!(format!("{err:?}").contains("growing memory"), "{err:?}");
    }

    /// Pulley runs the same code, interpreted.
    #[cfg(feature = "pulley")]
    #[test]
    fn pulley_runs_the_same_code() {
        let engine = engine(&Options { backend: Backend::Pulley, ..Options::default() }).unwrap();
        assert_eq!(run(&engine, r#"(module (func (export "answer") (result i32) (i32.mul (i32.const 6) (i32.const 7))))"#, "answer", u64::MAX).unwrap(), 42);
    }
}
