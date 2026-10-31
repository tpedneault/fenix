//! Embeds the app icon into the Windows executable. The `.ico` is drawn
//! here from `fenix-brand`'s geometry rather than read from a checked-in
//! file, so every build carries the current icon -- there is no copy to
//! forget to update.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("fenix.ico");
        std::fs::write(&out, fenix_brand::ico_bytes()).expect("failed to write the generated fenix.ico");
        winresource::WindowsResource::new()
            .set_icon(out.to_str().expect("OUT_DIR is valid UTF-8"))
            .compile()
            .expect("failed to embed fenix.ico into the Windows exe resources");
    }
}
