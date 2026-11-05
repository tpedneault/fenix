//! Writes the two-bone stick fenix-wasm's tests load as a model:
//! `cargo run -p fenix-model --example stick -- <path.glb>`.

use fenix_model::build::{Builder, Mesh, Track};

fn main() {
    let path = std::env::args().nth(1).expect("where to write it");
    let mut b = Builder::new();
    let root = b.joint("root", None, [0.0, 0.0, 0.0]);
    let tip = b.joint("tip", Some(root), [0.0, 1.0, 0.0]);
    let hat = b.joint("opt:hat", Some(tip), [0.0, 1.0, 0.0]);
    let mut mesh = Mesh::default();
    mesh.cuboid([0.0, 0.5, 0.0], [0.2, 0.5, 0.2], [0.9, 0.3, 0.1, 1.0], root);
    mesh.cuboid([0.0, 1.5, 0.0], [0.2, 0.5, 0.2], [0.2, 0.7, 0.3, 1.0], tip);
    mesh.cuboid([0.0, 2.1, 0.0], [0.1, 0.1, 0.1], [0.2, 0.3, 0.9, 1.0], hat);
    b.mesh(mesh);
    let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
    b.clip("bend", vec![Track::rotation(tip, vec![0.0, 0.5, 1.0], vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, s, c], [0.0, 0.0, 0.0, 1.0]])]);
    std::fs::write(&path, b.glb()).expect("written");
}
