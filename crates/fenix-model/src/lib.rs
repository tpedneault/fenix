//! 3D models for extensions' layers: a binary glTF (`.glb`) read into
//! what Fenix draws -- triangles, a colour at each corner, a texture when
//! it has one, a skeleton and named clips -- and posed: clips sampled at a
//! time, blended by weight, and the mesh skinned on the CPU.
//!
//! The models this is for are small (a pet is a few thousand vertices), so
//! skinning on the CPU costs nothing worth a compute shader, and keeps the
//! GPU side a plain lit-mesh pipeline. Nothing here knows about the GPU.
//!
//! Optional parts: a node named `opt:<name>` (a hat, a scarf) and
//! everything under it is drawn only when the pose asks for `<name>`.

use std::collections::HashMap;

use glam::{Mat3, Mat4, Quat, Vec3, Vec4};
use gltf::animation::util::ReadOutputs;
use gltf::animation::Interpolation;
use gltf::buffer::Source;
use gltf::mesh::Mode;

pub mod build;

/// The most a model file may be, in bytes.
pub const MAX_MODEL: u64 = 4 << 20;
/// The most vertices a model may have, all its parts together.
pub const MAX_VERTICES: usize = 65_536;
/// The widest or tallest a model's texture may be.
pub const MAX_TEXTURE: u32 = 1024;

/// A model, read and checked: ready to pose.
#[derive(Debug, Clone)]
pub struct Model {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    skins: Vec<Skin>,
    pub parts: Vec<Part>,
    pub textures: Vec<Texture>,
    pub clips: Vec<Clip>,
    /// Its rest pose's bounds, without its optional parts (a hat doesn't
    /// make it smaller): least, most.
    pub bounds: (Vec3, Vec3),
}

#[derive(Debug, Clone)]
struct Node {
    rest: Trs,
    children: Vec<usize>,
    /// `opt:<name>`'s name.
    optional: Option<String>,
}

/// A translation, a rotation and a scale: how glTF animates a node.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Trs {
    t: Vec3,
    r: Quat,
    s: Vec3,
}

impl Trs {
    fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }
}

#[derive(Debug, Clone)]
struct Skin {
    joints: Vec<usize>,
    inverse_bind: Vec<Mat4>,
}

/// One primitive of one mesh: its triangles, and what each corner is.
#[derive(Debug, Clone)]
pub struct Part {
    /// Three to a triangle, into this part's vertices.
    pub indices: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    /// The vertex's colour times its material's: white when it has none.
    pub colors: Vec<[f32; 4]>,
    /// Into `Model::textures`.
    pub texture: Option<usize>,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    /// The node it's drawn at.
    node: usize,
    skin: Option<usize>,
}

impl Part {
    pub fn vertices(&self) -> usize {
        self.positions.len()
    }
}

/// An image, straight-alpha RGBA.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A named animation.
#[derive(Debug, Clone)]
pub struct Clip {
    pub name: String,
    /// How long it lasts.
    pub seconds: f32,
    channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
struct Channel {
    node: usize,
    times: Vec<f32>,
    values: Values,
    step: bool,
}

#[derive(Debug, Clone)]
enum Values {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

/// A clip playing: which, how far in, and how much of the pose it makes.
/// A clip past the last ([`Playing::REST`]) is the rest pose, so a clip
/// can blend back to standing still.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playing {
    pub clip: usize,
    pub seconds: f32,
    pub weight: f32,
}

/// Every part's vertices where a pose puts them.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    pub parts: Vec<PosedPart>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PosedPart {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// False when it's an optional part not asked for.
    pub shown: bool,
}

impl Playing {
    pub const REST: usize = usize::MAX;
}

impl Model {
    /// A `.glb` read: its meshes, skeleton, clips and textures.
    pub fn from_glb(bytes: &[u8]) -> Result<Model, String> {
        if bytes.len() as u64 > MAX_MODEL {
            return Err(format!("a model is at most {} MiB", MAX_MODEL >> 20));
        }
        let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
        let blob = gltf.blob.as_deref();
        let get = |buffer: gltf::Buffer| match buffer.source() {
            Source::Bin => blob,
            Source::Uri(_) => None,
        };
        let doc = &gltf.document;
        if doc.buffers().any(|b| matches!(b.source(), Source::Uri(_))) {
            return Err("a model is one .glb file: its buffers can't be other files".into());
        }

        let mut nodes: Vec<Node> = doc
            .nodes()
            .map(|n| {
                let (t, r, s) = n.transform().decomposed();
                Node {
                    rest: Trs { t: Vec3::from(t), r: Quat::from_array(r).normalize(), s: Vec3::from(s) },
                    children: n.children().map(|c| c.index()).collect(),
                    optional: n.name().and_then(|name| name.strip_prefix("opt:")).map(str::to_string),
                }
            })
            .collect();
        let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or("the model has no scene")?;
        let roots: Vec<usize> = scene.nodes().map(|n| n.index()).collect();
        if has_cycle(&nodes, &roots) {
            return Err("the model's nodes go round in a circle".into());
        }

        let mut textures = Vec::new();
        let mut texture_of: HashMap<usize, usize> = HashMap::new();
        let mut parts = Vec::new();
        for node in doc.nodes() {
            let Some(mesh) = node.mesh() else { continue };
            for prim in mesh.primitives() {
                if prim.mode() != Mode::Triangles {
                    continue;
                }
                let reader = prim.reader(get);
                let positions: Vec<Vec3> = reader.read_positions().ok_or("a mesh without positions")?.map(Vec3::from).collect();
                let n = positions.len();
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..n as u32).collect(),
                };
                if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= n) {
                    return Err("a mesh's indices don't make whole triangles of its vertices".into());
                }
                let normals = match reader.read_normals() {
                    Some(normals) => normals.map(|v| Vec3::from(v).normalize_or_zero()).collect(),
                    None => smooth_normals(&positions, &indices),
                };
                let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).map(|t| t.into_f32().collect()).unwrap_or_else(|| vec![[0.0; 2]; n]);
                let material = prim.material();
                let pbr = material.pbr_metallic_roughness();
                let factor = pbr.base_color_factor();
                let colors: Vec<[f32; 4]> = match reader.read_colors(0) {
                    Some(c) => c.into_rgba_f32().map(|c| [c[0] * factor[0], c[1] * factor[1], c[2] * factor[2], c[3] * factor[3]]).collect(),
                    None => vec![factor; n],
                };
                let (joints, weights) = match (reader.read_joints(0), reader.read_weights(0)) {
                    (Some(j), Some(w)) => (j.into_u16().collect(), w.into_f32().collect()),
                    _ => (Vec::new(), Vec::new()),
                };
                if [uvs.len(), colors.len(), normals.len()].iter().any(|&len| len != n) || (!joints.is_empty() && (joints.len() != n || weights.len() != n)) {
                    return Err("a mesh's attributes don't all have one value a vertex".into());
                }
                let texture = match pbr.base_color_texture() {
                    Some(info) => {
                        let image = info.texture().source();
                        let index = image.index();
                        match texture_of.get(&index) {
                            Some(&t) => Some(t),
                            None => {
                                textures.push(decode(&image, blob)?);
                                texture_of.insert(index, textures.len() - 1);
                                Some(textures.len() - 1)
                            }
                        }
                    }
                    None => None,
                };
                let skin = node.skin().map(|s| s.index()).filter(|_| !joints.is_empty());
                parts.push(Part { indices, uvs, colors, texture, positions, normals, joints, weights, node: node.index(), skin });
            }
        }
        if parts.is_empty() {
            return Err("the model has no triangles".into());
        }
        if parts.iter().map(Part::vertices).sum::<usize>() > MAX_VERTICES {
            return Err(format!("a model has at most {MAX_VERTICES} vertices"));
        }

        let mut skins = Vec::new();
        for skin in doc.skins() {
            let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
            let inverse_bind: Vec<Mat4> = match skin.reader(get).read_inverse_bind_matrices() {
                Some(m) => m.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
                None => vec![Mat4::IDENTITY; joints.len()],
            };
            if inverse_bind.len() != joints.len() {
                return Err("a skin has a different number of joints and bind matrices".into());
            }
            skins.push(Skin { joints, inverse_bind });
        }
        for part in &parts {
            if let Some(skin) = part.skin {
                let count = skins[skin].joints.len();
                if part.joints.iter().flatten().any(|&j| j as usize >= count) {
                    return Err("a mesh names a joint its skin doesn't have".into());
                }
            }
        }

        let mut clips = Vec::new();
        for (i, anim) in doc.animations().enumerate() {
            let mut channels = Vec::new();
            for channel in anim.channels() {
                let reader = channel.reader(get);
                let Some(times) = reader.read_inputs() else { continue };
                let times: Vec<f32> = times.collect();
                let cubic = channel.sampler().interpolation() == Interpolation::CubicSpline;
                // A cubic spline keys an in-tangent, the value and an
                // out-tangent: the value is the middle of each three.
                let keep = |i: usize| !cubic || i % 3 == 1;
                let values = match reader.read_outputs() {
                    Some(ReadOutputs::Translations(v)) => Values::Translation(v.enumerate().filter(|(i, _)| keep(*i)).map(|(_, v)| Vec3::from(v)).collect()),
                    Some(ReadOutputs::Rotations(v)) => Values::Rotation(v.into_f32().enumerate().filter(|(i, _)| keep(*i)).map(|(_, q)| Quat::from_array(q).normalize()).collect()),
                    Some(ReadOutputs::Scales(v)) => Values::Scale(v.enumerate().filter(|(i, _)| keep(*i)).map(|(_, v)| Vec3::from(v)).collect()),
                    _ => continue,
                };
                let len = match &values {
                    Values::Translation(v) | Values::Scale(v) => v.len(),
                    Values::Rotation(v) => v.len(),
                };
                if times.is_empty() || len != times.len() || times.windows(2).any(|w| w[1] < w[0]) {
                    return Err(format!("animation {}: a channel's keys don't match its times", anim.name().unwrap_or("")));
                }
                channels.push(Channel { node: channel.target().node().index(), times, values, step: channel.sampler().interpolation() == Interpolation::Step });
            }
            let seconds = channels.iter().filter_map(|c| c.times.last().copied()).fold(0.0, f32::max);
            clips.push(Clip { name: anim.name().map(str::to_string).unwrap_or_else(|| format!("clip{i}")), seconds, channels });
        }

        let mut model = Model { nodes: std::mem::take(&mut nodes), roots, skins, parts, textures, clips, bounds: (Vec3::ZERO, Vec3::ZERO) };
        // What's drawn: the triangles not folded away with a hidden part.
        let rest = model.pose(&[], &|_| false);
        let mut least = Vec3::splat(f32::INFINITY);
        let mut most = Vec3::splat(f32::NEG_INFINITY);
        for (part, posed) in model.parts.iter().zip(&rest.parts).filter(|(_, p)| p.shown) {
            for tri in part.indices.as_chunks::<3>().0 {
                let [a, b, c] = tri.map(|i| Vec3::from(posed.positions[i as usize]));
                if (b - a).cross(c - a).length_squared() > 0.0 {
                    least = least.min(a).min(b).min(c);
                    most = most.max(a).max(b).max(c);
                }
            }
        }
        if !least.is_finite() {
            return Err("the model has nothing drawn without its optional parts".into());
        }
        model.bounds = (least, most);
        Ok(model)
    }

    /// Clip `name`'s index.
    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|c| c.name == name)
    }

    /// The model posed by `playing` (no clips: its rest pose), with the
    /// optional parts `shown` says yes to.
    pub fn pose(&self, playing: &[Playing], shown: &dyn Fn(&str) -> bool) -> Pose {
        let locals = self.locals(playing);
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        let mut visible = vec![false; self.nodes.len()];
        let mut stack: Vec<(usize, Mat4, bool)> = self.roots.iter().map(|&r| (r, Mat4::IDENTITY, true)).collect();
        while let Some((i, parent, vis)) = stack.pop() {
            world[i] = parent * locals[i].matrix();
            visible[i] = vis && self.nodes[i].optional.as_deref().is_none_or(shown);
            for &c in &self.nodes[i].children {
                stack.push((c, world[i], visible[i]));
            }
        }
        // A hidden joint (an optional part not asked for) folds its
        // vertices to a point, so its triangles vanish.
        let joint_mats: Vec<Vec<Mat4>> = self
            .skins
            .iter()
            .map(|s| s.joints.iter().zip(&s.inverse_bind).map(|(&j, ib)| if visible[j] { world[j] * *ib } else { Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, world[j].w_axis) }).collect())
            .collect();
        let parts = self
            .parts
            .iter()
            .map(|part| {
                let mut positions = Vec::with_capacity(part.positions.len());
                let mut normals = Vec::with_capacity(part.normals.len());
                match part.skin {
                    Some(skin) => {
                        let mats = &joint_mats[skin];
                        for (k, (p, n)) in part.positions.iter().zip(&part.normals).enumerate() {
                            let (j, w) = (part.joints[k], part.weights[k]);
                            let total: f32 = w.iter().sum();
                            let mut m = Mat4::ZERO;
                            for x in 0..4 {
                                if w[x] > 0.0 {
                                    m += mats[j[x] as usize] * (w[x] / total.max(1e-6));
                                }
                            }
                            if total <= 0.0 {
                                m = Mat4::IDENTITY;
                            }
                            positions.push(m.transform_point3(*p).to_array());
                            normals.push((Mat3::from_mat4(m) * *n).normalize_or_zero().to_array());
                        }
                    }
                    None => {
                        let m = world[part.node];
                        let nm = Mat3::from_mat4(m).inverse().transpose();
                        positions.extend(part.positions.iter().map(|p| m.transform_point3(*p).to_array()));
                        normals.extend(part.normals.iter().map(|n| (nm * *n).normalize_or_zero().to_array()));
                    }
                }
                // A skinned mesh follows its joints, not its node: the
                // node's own visibility is what an `opt:` parent decides.
                PosedPart { positions, normals, shown: visible[part.node] }
            })
            .collect();
        Pose { parts }
    }

    /// Each node's own transform: its rest, with what the clips playing
    /// make of it, blended by their weights.
    fn locals(&self, playing: &[Playing]) -> Vec<Trs> {
        let rest: Vec<Trs> = self.nodes.iter().map(|n| n.rest).collect();
        let playing: Vec<&Playing> = playing.iter().filter(|p| p.weight > 0.0).collect();
        let total: f32 = playing.iter().map(|p| p.weight).sum();
        if playing.is_empty() {
            return rest;
        }
        let mut t = vec![Vec3::ZERO; rest.len()];
        let mut s = vec![Vec3::ZERO; rest.len()];
        let mut r = vec![[0.0f32; 4]; rest.len()];
        for p in &playing {
            let w = p.weight / total;
            let mut one = rest.clone();
            let channels = self.clips.get(p.clip).map(|c| c.channels.as_slice()).unwrap_or_default();
            for ch in channels {
                let Some(node) = one.get_mut(ch.node) else { continue };
                let (i, f) = key(&ch.times, p.seconds, ch.step);
                match &ch.values {
                    Values::Translation(v) => node.t = v[i].lerp(v[(i + 1).min(v.len() - 1)], f),
                    Values::Scale(v) => node.s = v[i].lerp(v[(i + 1).min(v.len() - 1)], f),
                    Values::Rotation(v) => node.r = v[i].slerp(v[(i + 1).min(v.len() - 1)], f),
                }
            }
            for (n, trs) in one.iter().enumerate() {
                t[n] += trs.t * w;
                s[n] += trs.s * w;
                // Quaternions summed on one hemisphere, then normalised:
                // close enough to a slerp for blending poses.
                let q = if Quat::from_array(r[n]).dot(trs.r) < 0.0 { -trs.r } else { trs.r };
                for (acc, v) in r[n].iter_mut().zip(q.to_array()) {
                    *acc += v * w;
                }
            }
        }
        (0..rest.len()).map(|n| Trs { t: t[n], r: Quat::from_array(r[n]).normalize(), s: s[n] }).collect()
    }
}

impl Clip {
    /// How far into it `seconds` after it started: round again when
    /// it loops, held at its end when not.
    pub fn at(&self, seconds: f32, looping: bool) -> f32 {
        if self.seconds <= 0.0 {
            return 0.0;
        }
        if looping {
            seconds.rem_euclid(self.seconds)
        } else {
            seconds.clamp(0.0, self.seconds)
        }
    }
}

/// The key at or before `at`, and how far towards the next one.
fn key(times: &[f32], at: f32, step: bool) -> (usize, f32) {
    if at <= times[0] {
        return (0, 0.0);
    }
    let last = times.len() - 1;
    if at >= times[last] {
        return (last, 0.0);
    }
    let i = times.partition_point(|&t| t <= at) - 1;
    let span = times[i + 1] - times[i];
    let f = if step || span <= 0.0 { 0.0 } else { (at - times[i]) / span };
    (i, f)
}

/// Normals for a mesh that has none: each vertex the average of its
/// triangles' faces.
fn smooth_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| i as usize);
        let face = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        for v in [a, b, c] {
            normals[v] += face;
        }
    }
    normals.into_iter().map(|n| n.normalize_or(Vec3::Y)).collect()
}

fn has_cycle(nodes: &[Node], roots: &[usize]) -> bool {
    let mut state = vec![0u8; nodes.len()];
    fn visit(i: usize, nodes: &[Node], state: &mut [u8]) -> bool {
        match state.get(i) {
            None => return true,
            Some(1) => return true,
            Some(2) => return false,
            _ => {}
        }
        state[i] = 1;
        for &c in &nodes[i].children {
            if visit(c, nodes, state) {
                return true;
            }
        }
        state[i] = 2;
        false
    }
    roots.iter().any(|&r| visit(r, nodes, &mut state))
}

fn decode(image: &gltf::Image, blob: Option<&[u8]>) -> Result<Texture, String> {
    let gltf::image::Source::View { view, .. } = image.source() else {
        return Err("a model's textures are inside it, not other files".into());
    };
    let blob = blob.ok_or("the model has no binary chunk")?;
    let bytes = blob.get(view.offset()..view.offset() + view.length()).ok_or("a texture runs past the model's end")?;
    let decoded = image::load_from_memory(bytes).map_err(|e| format!("a texture: {e}"))?.to_rgba8();
    let (width, height) = decoded.dimensions();
    if width > MAX_TEXTURE || height > MAX_TEXTURE {
        return Err(format!("a texture is at most {MAX_TEXTURE}x{MAX_TEXTURE}"));
    }
    Ok(Texture { width, height, rgba: decoded.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::build::{Builder, Mesh, Track};
    use super::*;

    /// A two-bone stick: a box on the root, a box on a child bone, and a
    /// clip that turns the child a quarter round.
    fn stick() -> Vec<u8> {
        let mut b = Builder::new();
        let root = b.joint("root", None, [0.0, 0.0, 0.0]);
        let tip = b.joint("tip", Some(root), [0.0, 1.0, 0.0]);
        let hat = b.joint("opt:hat", Some(tip), [0.0, 1.0, 0.0]);
        let mut mesh = Mesh::default();
        mesh.cuboid([0.0, 0.5, 0.0], [0.2, 0.5, 0.2], [1.0, 0.0, 0.0, 1.0], root);
        mesh.cuboid([0.0, 1.5, 0.0], [0.2, 0.5, 0.2], [0.0, 1.0, 0.0, 1.0], tip);
        mesh.cuboid([0.0, 2.1, 0.0], [0.1, 0.1, 0.1], [0.0, 0.0, 1.0, 1.0], hat);
        b.mesh(mesh);
        let quarter = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array();
        b.clip("bend", vec![Track::rotation(tip, vec![0.0, 1.0], vec![Quat::IDENTITY.to_array(), quarter])]);
        b.glb()
    }

    /// The top of what's drawn: triangles folded to a point aren't.
    fn highest(pose: &Pose) -> f32 {
        let model = Model::from_glb(&stick()).unwrap();
        let mut top = f32::MIN;
        for (part, posed) in model.parts.iter().zip(&pose.parts).filter(|(_, p)| p.shown) {
            for tri in part.indices.as_chunks::<3>().0 {
                let [a, b, c] = tri.map(|i| Vec3::from(posed.positions[i as usize]));
                if (b - a).cross(c - a).length() > 1e-9 {
                    top = top.max(a.y).max(b.y).max(c.y);
                }
            }
        }
        top
    }

    #[test]
    fn a_built_model_reads_back() {
        let model = Model::from_glb(&stick()).unwrap();
        assert_eq!(model.clips.len(), 1);
        assert_eq!((model.clips[0].name.as_str(), model.clips[0].seconds), ("bend", 1.0));
        assert_eq!(model.clip("bend"), Some(0));
        assert!((model.bounds.1.y - 2.0).abs() < 1e-4, "the hat's left out: {:?}", model.bounds);
        assert!(model.bounds.0.y.abs() < 1e-4);
    }

    #[test]
    fn a_clip_moves_the_skin() {
        let model = Model::from_glb(&stick()).unwrap();
        let rest = model.pose(&[], &|_| true);
        assert!((highest(&rest) - 2.2).abs() < 1e-4);
        // Bent a quarter round at the tip, the top box lies on its side:
        // nothing's above the joint by more than half the box's width.
        let bent = model.pose(&[Playing { clip: 0, seconds: 1.0, weight: 1.0 }], &|_| false);
        assert!(highest(&bent) < 1.3, "{}", highest(&bent));
        // Halfway between, blended half and half.
        let half = model.pose(&[Playing { clip: 0, seconds: 1.0, weight: 1.0 }, Playing { clip: 0, seconds: 0.0, weight: 1.0 }], &|_| false);
        let h = highest(&half);
        assert!(h > 1.3 && h < 2.0, "{h}");
        // Blending back to rest: the rest pose is a clip of its own.
        let back = model.pose(&[Playing { clip: 0, seconds: 1.0, weight: 1.0 }, Playing { clip: Playing::REST, seconds: 0.0, weight: 1.0 }], &|_| false);
        assert_eq!(highest(&back), h);
    }

    #[test]
    fn optional_parts_show_only_when_asked() {
        let model = Model::from_glb(&stick()).unwrap();
        let with = model.pose(&[], &|name| name == "hat");
        let without = model.pose(&[], &|_| false);
        assert!((highest(&with) - 2.2).abs() < 1e-4);
        assert!((highest(&without) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn clips_loop_or_hold() {
        let model = Model::from_glb(&stick()).unwrap();
        let clip = &model.clips[0];
        assert!((clip.at(1.25, true) - 0.25).abs() < 1e-6);
        assert_eq!(clip.at(3.0, false), 1.0);
        assert_eq!(key(&[0.0, 1.0, 2.0], 1.5, false), (1, 0.5));
        assert_eq!(key(&[0.0, 1.0, 2.0], 1.5, true), (1, 0.0));
        assert_eq!(key(&[0.0, 1.0], 5.0, false), (1, 0.0));
    }

    #[test]
    fn what_isnt_a_model_is_refused() {
        assert!(Model::from_glb(b"not a model").is_err());
        assert!(Model::from_glb(&vec![0; (MAX_MODEL + 1) as usize]).unwrap_err().contains("at most"));
        assert!(Model::from_glb(&Builder::new().glb()).unwrap_err().contains("no triangles"));
    }
}
