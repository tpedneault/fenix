//! Phase 3's spike: what running extensions in wasmtime costs on this
//! machine. `cargo run --release -p fenix-wasm --features pulley --example
//! spike`; the numbers go into `docs/adr/0001-wasm-runtime.md`.

use std::time::{Duration, Instant};

use fenix_wasm::{engine, limits, Backend, Options};
use wasmtime::component::{Component, Linker as ComponentLinker};
use wasmtime::{Engine, Instance, Linker, Module, Store};

/// Private bytes and working set of this process, in MiB.
#[cfg(windows)]
fn memory() -> (f64, f64) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb) };
    (c.PrivateUsage as f64 / 1048576.0, c.WorkingSetSize as f64 / 1048576.0)
}

#[cfg(not(windows))]
fn memory() -> (f64, f64) {
    (0.0, 0.0)
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

/// A module about the size of a real extension: `n` functions, each a
/// little arithmetic and a branch or two.
fn big_module(n: usize) -> Vec<u8> {
    let mut wat = String::from("(module (memory (export \"memory\") 1)\n");
    for i in 0..n {
        wat += &format!(
            "(func $f{i} (export \"f{i}\") (param i32 i32) (result i32) (local i32)
               (local.set 2 (i32.add (local.get 0) (i32.const {i})))
               (if (i32.gt_s (local.get 2) (local.get 1))
                 (then (local.set 2 (i32.mul (local.get 2) (i32.const 3))))
                 (else (local.set 2 (i32.sub (local.get 1) (local.get 2)))))
               (i32.store (i32.and (local.get 2) (i32.const 1020)) (local.get 2))
               (i32.add (i32.load (i32.const 0)) (local.get 2)))\n"
        );
    }
    wat += ")";
    wat::parse_str(&wat).expect("valid")
}

const GUEST: &str = r#"
(module
  (import "host" "tick" (func $tick))
  (memory (export "memory") 130)
  (func $fib (export "fib") (param i32) (result i32)
    (if (result i32) (i32.lt_u (local.get 0) (i32.const 2))
      (then (local.get 0))
      (else (i32.add (call $fib (i32.sub (local.get 0) (i32.const 1))) (call $fib (i32.sub (local.get 0) (i32.const 2)))))))
  (func (export "nop") (param i32) (result i32) (local.get 0))
  (func (export "ticks") (param $n i32)
    (loop $l (call $tick) (local.set $n (i32.sub (local.get $n) (i32.const 1))) (br_if $l (local.get $n))))
  ;; A 1920x1080 RGBA frame at 0, a pixel at a time.
  (func (export "paint") (param $seed i32) (local $i i32)
    (loop $l
      (i32.store (local.get $i) (i32.add (local.get $seed) (local.get $i)))
      (local.set $i (i32.add (local.get $i) (i32.const 4)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 8294400))))))
"#;

const COMPONENT: &str = r#"
(component
  (import "host-add" (func $host_add (param "a" u32) (param "b" u32) (result u32)))
  (core module $m
    (import "h" "add" (func $add (param i32 i32) (result i32)))
    (func (export "run") (param i32) (result i32) (call $add (local.get 0) (i32.const 1))))
  (core func $add (canon lower (func $host_add)))
  (core instance $h (export "add" (func $add)))
  (core instance $i (instantiate $m (with "h" (instance $h))))
  (func (export "run") (param "x" u32) (result u32) (canon lift (core func $i "run"))))
"#;

fn store(engine: &Engine) -> Store<wasmtime::StoreLimits> {
    let mut store = Store::new(engine, limits(256 << 20));
    store.limiter(|l| l);
    store.set_epoch_deadline(u64::MAX);
    store
}

fn backend(name: &str, options: Options, big: &[u8]) -> wasmtime::Result<()> {
    println!("\n== {name}");
    let (engine, t) = time(|| engine(&options));
    let engine = engine?;
    println!("engine:                 {}", ms(t));

    let (module, t) = time(|| Module::new(&engine, big));
    let module = module?;
    println!("compile {:>5} KiB wasm:  {}", big.len() / 1024, ms(t));
    let (bytes, t) = time(|| engine.precompile_module(big));
    let bytes = bytes?;
    println!("precompile:             {}  ({} KiB artifact)", ms(t), bytes.len() / 1024);
    // SAFETY: bytes this engine just produced.
    let (loaded, t) = time(|| unsafe { Module::deserialize(&engine, &bytes) });
    loaded?;
    println!("load precompiled:       {}", ms(t));
    let mut s = store(&engine);
    let (inst, t) = time(|| Instance::new(&mut s, &module, &[]));
    inst?;
    println!("instantiate big:        {}", ms(t));

    let guest = Module::new(&engine, GUEST)?;
    let mut linker: Linker<wasmtime::StoreLimits> = Linker::new(&engine);
    linker.func_wrap("host", "tick", || {})?;
    let mut s = store(&engine);
    let (inst, t) = time(|| linker.instantiate(&mut s, &guest));
    let inst = inst?;
    println!("instantiate guest:      {}", ms(t));

    let nop = inst.get_typed_func::<i32, i32>(&mut s, "nop")?;
    let n = 1_000_000;
    let (_, t) = time(|| (0..n).for_each(|i| drop(nop.call(&mut s, i))));
    println!("host -> guest call:     {:.0} ns", t.as_nanos() as f64 / n as f64);
    let ticks = inst.get_typed_func::<i32, ()>(&mut s, "ticks")?;
    let (r, t) = time(|| ticks.call(&mut s, n));
    r?;
    println!("guest -> host call:     {:.0} ns", t.as_nanos() as f64 / n as f64);
    let fib = inst.get_typed_func::<i32, i32>(&mut s, "fib")?;
    let (r, t) = time(|| fib.call(&mut s, 30));
    println!("fib(30) = {}:       {}", r?, ms(t));

    let paint = inst.get_typed_func::<i32, ()>(&mut s, "paint")?;
    let memory = inst.get_memory(&mut s, "memory").expect("memory");
    let frames = 30;
    let (mut painted, mut copied) = (Duration::ZERO, Duration::ZERO);
    let mut frame = vec![0u8; 8_294_400];
    for seed in 0..frames {
        let (r, t) = time(|| paint.call(&mut s, seed));
        r?;
        painted += t;
        let (_, t) = time(|| frame.copy_from_slice(&memory.data(&s)[..8_294_400]));
        copied += t;
    }
    println!("1080p frame, painted:   {} a frame (in the guest, a pixel at a time)", ms(painted / frames as u32));
    println!("1080p frame, copied:    {} a frame (guest memory to the host)", ms(copied / frames as u32));

    let component = Component::new(&engine, COMPONENT)?;
    let mut linker: ComponentLinker<wasmtime::StoreLimits> = ComponentLinker::new(&engine);
    linker.root().func_wrap("host-add", |_, (a, b): (u32, u32)| Ok((a + b,)))?;
    let mut s = store(&engine);
    let inst = linker.instantiate(&mut s, &component)?;
    let run = inst.get_typed_func::<(u32,), (u32,)>(&mut s, "run")?;
    let n = 200_000;
    let (_, t) = time(|| (0..n).for_each(|i| drop(run.call(&mut s, (i,)))));
    println!("component call (+host): {:.0} ns", t.as_nanos() as f64 / n as f64);

    // What an extension costs to keep: twenty of the guest, each in its
    // own store, as twenty extensions would be.
    let before = memory_now();
    let mut kept = Vec::new();
    for _ in 0..20 {
        let mut s = store(&engine);
        let small = Module::new(&engine, "(module (memory 1) (func (export \"f\")))")?;
        let i = Instance::new(&mut s, &small, &[])?;
        kept.push((s, i));
    }
    let after = memory_now();
    println!("20 small instances:     +{:.1} MiB private, +{:.1} MiB working set", after.0 - before.0, after.1 - before.1);
    drop(kept);
    Ok(())
}

fn memory_now() -> (f64, f64) {
    memory()
}

fn main() -> wasmtime::Result<()> {
    let start = memory();
    println!("process at start:       {:.1} MiB private, {:.1} MiB working set", start.0, start.1);
    let big = big_module(4000);
    backend("native (Cranelift)", Options::default(), &big)?;
    #[cfg(feature = "pulley")]
    backend("pulley (interpreted)", Options { backend: Backend::Pulley, ..Options::default() }, &big)?;
    #[cfg(feature = "winch")]
    backend("winch (baseline compiler)", Options { backend: Backend::Winch, ..Options::default() }, &big)?;
    let _ = Backend::Native;
    let end = memory();
    println!("\nprocess at end:         {:.1} MiB private, {:.1} MiB working set", end.0, end.1);
    Ok(())
}
