//! The icon as an `.iconset` folder, which macOS's `iconutil` makes an
//! `.icns` of (`packaging/macos/build-app.sh`):
//!
//! cargo run -p fenix-brand --example write-iconset --features ico -- Fenix.iconset

/// Apple's grid: the tile is 824 of 1024, with clear space around it.
const TILE: f32 = 824.0 / 1024.0;

fn main() -> std::io::Result<()> {
    let folder = std::path::PathBuf::from(std::env::args_os().nth(1).expect("the folder to write"));
    std::fs::create_dir_all(&folder)?;
    for points in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let size = points * scale;
            let tile = ((size as f32 * TILE / 2.0).round() as u32 * 2).max(1);
            // 16 px is the mark alone, which needs all of it.
            let tile = if size <= 16 { size } else { tile.max(17) };
            let icon = fenix_brand::app_icon(tile);
            let mut canvas = image::RgbaImage::new(size, size);
            let drawn = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba).expect("its own size");
            let at = i64::from((size - tile) / 2);
            image::imageops::replace(&mut canvas, &drawn, at, at);
            let suffix = if scale == 2 { "@2x" } else { "" };
            canvas.save(folder.join(format!("icon_{points}x{points}{suffix}.png"))).map_err(std::io::Error::other)?;
        }
    }
    Ok(())
}
