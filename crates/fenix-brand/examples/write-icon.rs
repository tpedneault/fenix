//! Regenerates the committed `fenix.ico` at the repository root from the
//! brand geometry, plus PNG previews of the icon and lockup next to it
//! under `docs/brand/`. Builds never need this -- `fenix-gui`'s build
//! script draws the icon it embeds itself -- it keeps the checked-in
//! copies (for installers, the README, anything outside a build) current.
//!
//! cargo run -p fenix-brand --example write-icon --features ico

use std::path::Path;

fn main() -> std::io::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::write(root.join("fenix.ico"), fenix_brand::ico_bytes())?;

    let docs = root.join("docs/brand");
    std::fs::create_dir_all(&docs)?;
    let save = |name: &str, image: fenix_brand::Image| {
        image::save_buffer(docs.join(name), &image.rgba, image.width, image.height, image::ExtendedColorType::Rgba8)
            .map_err(std::io::Error::other)
    };
    save("fenix-icon-256.png", fenix_brand::app_icon(256))?;
    save("fenix-mark-512.png", fenix_brand::mark(512))?;
    save("fenix-lockup-dark-bg.png", fenix_brand::lockup(160, [0xF4, 0xF4, 0xF6]))?;
    save("fenix-lockup-light-bg.png", fenix_brand::lockup(160, [0x17, 0x18, 0x1B]))?;
    println!("wrote fenix.ico and docs/brand/*.png");
    Ok(())
}
