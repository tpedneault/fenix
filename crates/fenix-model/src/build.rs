//! Models made in code and written as `.glb`: a skeleton of joints, one
//! mesh made of simple shapes each bound whole to one joint, and clips
//! that turn, move and scale the joints. It's how the pets are modelled
//! (a toy's parts, rigidly jointed), and how this crate's tests make
//! models to read.

use glam::{Mat4, Quat, Vec3};
use serde_json::{json, Value};

/// A joint of the skeleton: its parent, and where it sits on it.
#[derive(Debug, Clone)]
struct Joint {
    name: String,
    parent: Option<usize>,
    translation: [f32; 3],
}

/// The mesh, a vertex at a time, each bound to one joint.
#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub joints: Vec<u16>,
    pub indices: Vec<u32>,
}

/// One joint's turn, move or scale over time.
#[derive(Debug, Clone)]
pub struct Track {
    joint: usize,
    path: &'static str,
    times: Vec<f32>,
    /// Flat: three or four numbers a key.
    values: Vec<f32>,
    step: bool,
}

impl Track {
    /// Quaternions, `[x, y, z, w]`.
    pub fn rotation(joint: usize, times: Vec<f32>, values: Vec<[f32; 4]>) -> Track {
        Track { joint, path: "rotation", times, values: values.into_iter().flatten().collect(), step: false }
    }

    /// Where the joint sits on its parent, in place of its rest.
    pub fn translation(joint: usize, times: Vec<f32>, values: Vec<[f32; 3]>) -> Track {
        Track { joint, path: "translation", times, values: values.into_iter().flatten().collect(), step: false }
    }

    pub fn scale(joint: usize, times: Vec<f32>, values: Vec<[f32; 3]>) -> Track {
        Track { joint, path: "scale", times, values: values.into_iter().flatten().collect(), step: false }
    }

    /// Jumping from key to key, not easing between them.
    pub fn stepped(mut self) -> Track {
        self.step = true;
        self
    }
}

impl Mesh {
    /// Adds triangles over `positions` (with their normals), all one
    /// colour, bound to `joint`.
    pub fn add(&mut self, positions: &[Vec3], normals: &[Vec3], indices: &[u32], color: [f32; 4], joint: usize) {
        let base = self.positions.len() as u32;
        self.positions.extend(positions.iter().map(|p| p.to_array()));
        self.normals.extend(normals.iter().map(|n| n.to_array()));
        self.colors.extend(std::iter::repeat_n(color, positions.len()));
        self.joints.extend(std::iter::repeat_n(joint as u16, positions.len()));
        self.indices.extend(indices.iter().map(|i| base + i));
    }

    /// A box around `center`, `half` its half-size each way.
    pub fn cuboid(&mut self, center: [f32; 3], half: [f32; 3], color: [f32; 4], joint: usize) {
        self.cuboid_turned(center, half, Quat::IDENTITY, color, joint);
    }

    pub fn cuboid_turned(&mut self, center: [f32; 3], half: [f32; 3], turn: Quat, color: [f32; 4], joint: usize) {
        let (c, h) = (Vec3::from(center), Vec3::from(half));
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let mut n = Vec3::ZERO;
                n[axis] = sign;
                let (u, v) = (Vec3::from([n.y.abs(), n.z.abs(), n.x.abs()]), Vec3::from([n.z.abs(), n.x.abs(), n.y.abs()]));
                let base = positions.len() as u32;
                for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    positions.push(c + turn * ((n + u * a + v * b) * h));
                    normals.push(turn * n);
                }
                // Wound outward, counter-clockwise seen from outside: the
                // corners go round counter-clockwise seen from -axis.
                if sign > 0.0 {
                    indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
                } else {
                    indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        }
        self.add(&positions, &normals, &indices, color, joint);
    }

    /// An ellipsoid around `center` with radii `radii`, turned by `turn`:
    /// `rings` bands from pole to pole, `segments` round.
    #[allow(clippy::too_many_arguments)]
    pub fn ellipsoid(&mut self, center: [f32; 3], radii: [f32; 3], turn: Quat, color: [f32; 4], joint: usize, rings: u32, segments: u32) {
        let (c, r) = (Vec3::from(center), Vec3::from(radii));
        let (rings, segments) = (rings.max(2), segments.max(3));
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        for i in 0..=rings {
            let phi = std::f32::consts::PI * i as f32 / rings as f32;
            for j in 0..=segments {
                let theta = std::f32::consts::TAU * j as f32 / segments as f32;
                let unit = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
                positions.push(c + turn * (unit * r));
                // An ellipsoid's normal: the unit sphere's, divided by the radii.
                normals.push((turn * (unit / r)).normalize());
            }
        }
        let mut indices = Vec::new();
        let row = segments + 1;
        for i in 0..rings {
            for j in 0..segments {
                let (a, b) = (i * row + j, (i + 1) * row + j);
                indices.extend([a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        self.add(&positions, &normals, &indices, color, joint);
    }

    /// A cone from a round base at `base` (`radius` wide) to a point at
    /// `tip`.
    pub fn cone(&mut self, base: [f32; 3], tip: [f32; 3], radius: f32, color: [f32; 4], joint: usize, segments: u32) {
        self.frustum(base, tip, radius, 0.0, color, joint, segments);
    }

    /// A tube from `a` to `b`, `ra` wide at `a` and `rb` at `b`, capped.
    #[allow(clippy::too_many_arguments)]
    pub fn frustum(&mut self, a: [f32; 3], b: [f32; 3], ra: f32, rb: f32, color: [f32; 4], joint: usize, segments: u32) {
        let (a, b) = (Vec3::from(a), Vec3::from(b));
        let axis = (b - a).normalize_or(Vec3::Y);
        let turn = Quat::from_rotation_arc(Vec3::Y, axis);
        let len = (b - a).length();
        let segments = segments.max(3);
        let slope = (ra - rb) / len.max(1e-6);
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        for j in 0..=segments {
            let theta = std::f32::consts::TAU * j as f32 / segments as f32;
            let round = Vec3::new(theta.cos(), 0.0, theta.sin());
            let n = (turn * (round + Vec3::Y * slope)).normalize();
            positions.push(a + turn * (round * ra));
            normals.push(n);
            positions.push(b + turn * (round * rb));
            normals.push(n);
        }
        for j in 0..segments {
            let (s, t) = (j * 2, j * 2 + 2);
            indices.extend([s, s + 1, t, t, s + 1, t + 1]);
        }
        // The caps, flat.
        for (center, radius, n) in [(a, ra, -axis), (b, rb, axis)] {
            if radius <= 0.0 {
                continue;
            }
            let base = positions.len() as u32;
            positions.push(center);
            normals.push(n);
            for j in 0..=segments {
                let theta = std::f32::consts::TAU * j as f32 / segments as f32;
                positions.push(center + turn * (Vec3::new(theta.cos(), 0.0, theta.sin()) * radius));
                normals.push(n);
            }
            for j in 0..segments {
                let (p, q) = (base + 1 + j, base + 2 + j);
                if n.dot(axis) > 0.0 {
                    indices.extend([base, q, p]);
                } else {
                    indices.extend([base, p, q]);
                }
            }
        }
        self.add(&positions, &normals, &indices, color, joint);
    }
}

/// A model being made.
#[derive(Debug, Clone, Default)]
pub struct Builder {
    joints: Vec<Joint>,
    mesh: Option<Mesh>,
    clips: Vec<(String, Vec<Track>)>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    /// A joint at `translation` from its parent (from the origin when it
    /// has none). Name it `opt:<name>` for an optional part.
    pub fn joint(&mut self, name: &str, parent: Option<usize>, translation: [f32; 3]) -> usize {
        self.joints.push(Joint { name: name.into(), parent, translation });
        self.joints.len() - 1
    }

    /// Where joint `joint` is at rest, from the origin.
    pub fn rest(&self, joint: usize) -> Vec3 {
        let mut at = Vec3::ZERO;
        let mut j = Some(joint);
        while let Some(i) = j {
            at += Vec3::from(self.joints[i].translation);
            j = self.joints[i].parent;
        }
        at
    }

    pub fn mesh(&mut self, mesh: Mesh) {
        self.mesh = Some(mesh);
    }

    pub fn clip(&mut self, name: &str, tracks: Vec<Track>) {
        self.clips.push((name.into(), tracks));
    }

    /// The model as a `.glb`.
    pub fn glb(&self) -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let mut views = Vec::new();
        let mut accessors = Vec::new();
        let mut push = |bytes: &[u8], accessor: Value| -> usize {
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len()}));
            bin.extend_from_slice(bytes);
            let mut accessor = accessor;
            accessor["bufferView"] = json!(views.len() - 1);
            accessors.push(accessor);
            accessors.len() - 1
        };
        let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();

        // Joints, then the mesh's own node at the root.
        let mut nodes: Vec<Value> = self
            .joints
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let children: Vec<usize> = self.joints.iter().enumerate().filter(|(_, c)| c.parent == Some(i)).map(|(k, _)| k).collect();
                let mut node = json!({"name": j.name, "translation": j.translation});
                if !children.is_empty() {
                    node["children"] = json!(children);
                }
                node
            })
            .collect();
        let mut roots: Vec<usize> = self.joints.iter().enumerate().filter(|(_, j)| j.parent.is_none()).map(|(i, _)| i).collect();

        let mut meshes = Vec::new();
        let mut skins = Vec::new();
        if let Some(mesh) = self.mesh.as_ref().filter(|m| !m.positions.is_empty()) {
            let (mut least, mut most) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for p in &mesh.positions {
                for k in 0..3 {
                    least[k] = least[k].min(p[k]);
                    most[k] = most[k].max(p[k]);
                }
            }
            let n = mesh.positions.len();
            let position = push(&floats(&mesh.positions.concat()), json!({"componentType": 5126, "count": n, "type": "VEC3", "min": least, "max": most}));
            let normal = push(&floats(&mesh.normals.concat()), json!({"componentType": 5126, "count": n, "type": "VEC3"}));
            let color = push(&floats(&mesh.colors.concat()), json!({"componentType": 5126, "count": n, "type": "VEC4"}));
            let joints: Vec<u8> = mesh.joints.iter().flat_map(|&j| [j, 0, 0, 0]).flat_map(u16::to_le_bytes).collect();
            let joint = push(&joints, json!({"componentType": 5123, "count": n, "type": "VEC4"}));
            let weights: Vec<f32> = std::iter::repeat_n([1.0, 0.0, 0.0, 0.0], n).flatten().collect();
            let weight = push(&floats(&weights), json!({"componentType": 5126, "count": n, "type": "VEC4"}));
            let indices: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let index = push(&indices, json!({"componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR"}));
            meshes.push(json!({"primitives": [{
                "attributes": {"POSITION": position, "NORMAL": normal, "COLOR_0": color, "JOINTS_0": joint, "WEIGHTS_0": weight},
                "indices": index, "material": 0, "mode": 4
            }]}));
            let inverse: Vec<f32> = (0..self.joints.len()).flat_map(|j| Mat4::from_translation(self.rest(j)).inverse().to_cols_array()).collect();
            let ibm = push(&floats(&inverse), json!({"componentType": 5126, "count": self.joints.len(), "type": "MAT4"}));
            skins.push(json!({"joints": (0..self.joints.len()).collect::<Vec<_>>(), "inverseBindMatrices": ibm}));
            nodes.push(json!({"name": "body", "mesh": 0, "skin": 0}));
            roots.push(nodes.len() - 1);
        }

        let mut animations = Vec::new();
        for (name, tracks) in &self.clips {
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            for t in tracks {
                let (least, most) = (t.times.iter().copied().fold(f32::INFINITY, f32::min), t.times.iter().copied().fold(f32::NEG_INFINITY, f32::max));
                let input = push(&floats(&t.times), json!({"componentType": 5126, "count": t.times.len(), "type": "SCALAR", "min": [least], "max": [most]}));
                let kind = if t.path == "rotation" { "VEC4" } else { "VEC3" };
                let output = push(&floats(&t.values), json!({"componentType": 5126, "count": t.times.len(), "type": kind}));
                samplers.push(json!({"input": input, "output": output, "interpolation": if t.step { "STEP" } else { "LINEAR" }}));
                channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": t.joint, "path": t.path}}));
            }
            animations.push(json!({"name": name, "samplers": samplers, "channels": channels}));
        }

        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "fenix-model"},
            "scene": 0,
            "scenes": [{"nodes": roots}],
            "nodes": nodes,
            "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0], "metallicFactor": 0.0, "roughnessFactor": 1.0}}],
        });
        for (key, list) in [("meshes", meshes), ("skins", skins), ("animations", animations), ("accessors", accessors), ("bufferViews", views)] {
            if !list.is_empty() {
                doc[key] = Value::Array(list);
            }
        }
        if !bin.is_empty() {
            doc["buffers"] = json!([{"byteLength": bin.len()}]);
        }
        let mut text = serde_json::to_vec(&doc).unwrap_or_default();
        while !text.len().is_multiple_of(4) {
            text.push(b' ');
        }
        let total = 12 + 8 + text.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&text);
        if !bin.is_empty() {
            out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&bin);
        }
        out
    }
}
