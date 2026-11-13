//! An extension rebuilt on a later 1.x API crate that uses nothing that
//! minor added still starts on the Fenix before it (the test explorer's
//! plan, F15): `guests/plain16.wasm`, built on the 1.6 crate with `api =
//! "1.5"` in its manifest, instantiated the way Fenix 1.5 instantiates an
//! extension -- WASI and 1.5's `extension-base` imports in its `Linker`
//! (`host.rs`'s `run`), bound from a copy of v1.5.2's `wit/` -- then
//! activated, and a command run. What it shows, and all it shows: a
//! component built on the 1.6 crate whose code has no call to
//! `tests::provider` reachable from an export -- so no `test-provider-set`
//! import, which the test checks along with its five `tests-*` exports --
//! starts on Fenix 1.5. A later minor's crate on the Fenix before it is
//! promised only once a test like this one shows it.

use std::path::PathBuf;

use fenix_extensions::manifest::Manifest;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::Store;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// Fenix 1.5's bindings: what its host links, and the exports it looks up.
mod v15 {
    wasmtime::component::bindgen!({ path: "tests/wit-1.5", world: "extension-base" });
}

use v15::fenix::extension::types as T;

/// What a 1.5 host keeps of an extension, here: what it said.
struct Host15 {
    wasi: WasiCtx,
    table: ResourceTable,
    said: Vec<String>,
}

impl WasiView for Host15 {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl T::Host for Host15 {}

include!("wit-1.5/host.rs");

fn guest(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests").join(name)
}

/// The names of what `component` imports from Fenix's `host`.
fn host_imports(engine: &wasmtime::Engine, component: &Component) -> Vec<String> {
    let ty = component.component_type();
    let (_, host) = ty.imports(engine).find(|(name, _)| name.starts_with("fenix:extension/host")).expect("it imports host");
    let wasmtime::component::types::ComponentItem::ComponentInstance(host) = host.ty else { panic!("host is an instance") };
    host.exports(engine).map(|(name, _)| name.to_string()).collect()
}

#[test]
fn a_guest_built_on_the_1_6_crate_that_uses_nothing_new_starts_on_fenix_1_5() {
    // Its manifest says 1.5 and names no provider: it installs on 1.5.
    let manifest = Manifest::parse(
        "[extension]\nid = \"test.plain\"\nname = \"Plain\"\nversion = \"1.0.0\"\ndescription = \"x\"\nruntime = \"wasm\"\n\n[engine]\nfenix = \">=1.5, <2\"\napi = \"1.5\"\n",
    )
    .unwrap();
    assert!(manifest.contributes.test_providers.is_empty());

    let engine = fenix_wasm::engine(&fenix_wasm::Options::default()).unwrap();
    let component = Component::from_file(&engine, guest("plain16.wasm")).unwrap();
    // Built on the 1.6 crate, it exports the five `tests-*` (else this
    // test would pass for a guest built on 1.5, proving nothing); it
    // imports only what it calls, and nothing reachable from its exports
    // calls `tests::provider`: no `test-provider-set`.
    let exports: Vec<String> = component.component_type().exports(&engine).map(|(name, _)| name.to_string()).collect();
    for export in ["tests-scan", "tests-plan", "tests-read", "tests-step-ended", "tests-launch"] {
        assert!(exports.iter().any(|e| e == export), "{export} among {exports:?}");
    }
    let imports = host_imports(&engine, &component);
    assert!(imports.contains(&"log".to_string()), "{imports:?}");
    assert!(!imports.iter().any(|i| i == "test-provider-set"), "{imports:?}");

    let mut linker: Linker<Host15> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    v15::ExtensionBase::add_to_linker::<Host15, HasSelf<Host15>>(&mut linker, |s| s).unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut wasi = WasiCtxBuilder::new();
    wasi.preopened_dir(data.path(), "/data", wasmtime_wasi::FsPerms::ReadWrite).unwrap();
    let mut store = Store::new(&engine, Host15 { wasi: wasi.build(), table: ResourceTable::new(), said: Vec::new() });
    store.set_epoch_deadline(u64::MAX);
    let instance = match linker.instantiate(&mut store, &component) {
        Ok(instance) => instance,
        Err(e) => panic!("Fenix 1.5 can't instantiate it: {e:?}"),
    };
    let ext = v15::ExtensionBase::new(&mut store, &instance).expect("1.5 finds every export it needs");
    assert_eq!(ext.call_activate(&mut store).unwrap(), Ok(()));
    assert_eq!(ext.call_run_command(&mut store, "plain.say").unwrap(), Ok(()));
    assert_eq!(store.data().said, ["plain16 activated", "plain16 said 1"]);
}

/// And the provider guest, which calls `test-provider-set`, doesn't: 1.5
/// says what it lacks, so an extension that uses 1.6 must say `api =
/// "1.6"`, and 1.5 never installs it.
#[test]
fn a_guest_that_uses_1_6_says_what_fenix_1_5_lacks() {
    let engine = fenix_wasm::engine(&fenix_wasm::Options::default()).unwrap();
    let component = Component::from_file(&engine, guest("provider.wasm")).unwrap();
    assert!(host_imports(&engine, &component).contains(&"test-provider-set".to_string()));
    let mut linker: Linker<Host15> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    v15::ExtensionBase::add_to_linker::<Host15, HasSelf<Host15>>(&mut linker, |s| s).unwrap();
    let mut store = Store::new(&engine, Host15 { wasi: WasiCtxBuilder::new().build(), table: ResourceTable::new(), said: Vec::new() });
    let err = linker.instantiate(&mut store, &component).map(|_| ()).unwrap_err();
    assert!(format!("{err:#}").contains("test-provider-set"), "{err:#}");
}
