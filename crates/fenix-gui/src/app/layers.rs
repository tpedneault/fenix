//! What extensions draw over the editor: their layers (`layer-open`,
//! `layer-set`), each a level of the frame's order holding items -- a
//! frame of a sprite sheet, or a 3D model posed, placed by an anchor.
//! Fenix animates them: a glide is eased (and bowed into an arc) here
//! every frame, a sheet's frames cycled at the item's rate, a model's
//! clips played and blended into one another and its turns eased; the
//! extension is only told when a glide ends, an item is clicked, or the
//! panes move (`layout`).
//!
//! While something glides or a model moves, the frame is drawn at the
//! display's own rate (it's one of the animations `about_to_wait` keeps
//! going); a sheet cycling its frames, or a model drawn a few times a
//! second, wakes only when its next frame is due. Nothing redraws for a
//! layer while all on it is still.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use fenix_model::{Model, Playing};
use fenix_wasm::host::{Anchor, Ease, Item, LayerLevel, Layout, Notice, PaneBox, Place, Pose, Style};

use super::*;
use crate::model3d::{Lighting, ModelRenderer, Shot};
use crate::sprite::{Sprite, SpriteRenderer};

/// How long a model takes to blend from one clip into the next.
const BLEND: f32 = 0.2;
/// How fast a model turns to face where it's told, degrees a second.
const TURN_SPEED: f32 = 720.0;

/// A sprite sheet, decoded: straight-alpha RGBA, cut into frames.
pub(crate) struct Sheet {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub frame: (u32, u32),
}

/// An item as it's drawn, and the clocks it's drawn by.
pub(crate) struct Live {
    pub item: Item,
    /// When its glide started: kept while it's given the same one.
    pub since: Instant,
    /// When its frames started cycling: kept while it cycles the same ones.
    pub frames_since: Instant,
    pub arrived: bool,
    /// A model's clips and turn, as they're played.
    pub acting: Option<Acting>,
}

impl Live {
    fn new(item: Item, now: Instant) -> Live {
        let acting = item.pose.as_ref().map(|p| Acting::new(p, now));
        Live { item, since: now, frames_since: now, arrived: false, acting }
    }
}

/// A model's clip as Fenix plays it: how far in, what it's blending from,
/// and which way it faces now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Acting {
    clip: String,
    once: bool,
    speed: f32,
    /// How far into the clip it had played at `played_at`, in seconds.
    played: f32,
    played_at: Instant,
    /// The clip it's blending out of: its name, how far in, whether it
    /// was played once, and when the blend began.
    from: Option<(String, f32, bool, Instant)>,
    /// Which way it faces now, and when that was worked out.
    yaw: f32,
    yaw_at: Instant,
}

impl Acting {
    fn new(pose: &Pose, now: Instant) -> Acting {
        Acting { clip: pose.clip.clone(), once: pose.once, speed: pose.speed, played: 0.0, played_at: now, from: None, yaw: pose.yaw, yaw_at: now }
    }

    /// How far into its clip at `now`.
    fn time(&self, now: Instant) -> f32 {
        self.played + now.saturating_duration_since(self.played_at).as_secs_f32() * self.speed
    }

    /// Given `pose` again: the same clip keeps playing (at its new
    /// speed), another starts, blending out of this one.
    fn update(&mut self, pose: &Pose, now: Instant) {
        if pose.clip != self.clip || pose.once != self.once {
            self.from = Some((std::mem::take(&mut self.clip), self.time(now), self.once, now));
            self.clip = pose.clip.clone();
            self.once = pose.once;
            self.played = 0.0;
            self.played_at = now;
        } else if pose.speed != self.speed {
            self.played = self.time(now);
            self.played_at = now;
        }
        self.speed = pose.speed;
    }

    /// Turned towards `target` by `now`, the short way round, at
    /// `TURN_SPEED`; at once with motion off.
    fn turn(&mut self, target: f32, now: Instant, motion: bool) {
        let gap = (target - self.yaw + 180.0).rem_euclid(360.0) - 180.0;
        let step = TURN_SPEED * now.saturating_duration_since(self.yaw_at).as_secs_f32();
        self.yaw = if !motion || gap.abs() <= step { target } else { self.yaw + step * gap.signum() };
        self.yaw_at = now;
    }

    /// The clips that make its pose at `now`, by weight.
    fn playing(&self, model: &Model, now: Instant) -> Vec<Playing> {
        let at = |clip: &str, seconds: f32, once: bool| match model.clip(clip) {
            Some(i) => (i, model.clips[i].at(seconds, !once)),
            None => (Playing::REST, 0.0),
        };
        let (clip, seconds) = at(&self.clip, self.time(now), self.once);
        let mut out = vec![Playing { clip, seconds, weight: 1.0 }];
        if let Some((from, played, once, began)) = &self.from {
            let k = smooth((now.saturating_duration_since(*began).as_secs_f32() / BLEND).min(1.0));
            if k < 1.0 {
                let (clip, seconds) = at(from, *played, *once);
                out[0].weight = k;
                out.push(Playing { clip, seconds, weight: 1.0 - k });
            }
        }
        out
    }

    /// Whether it looks different from one moment to the next: a clip
    /// going round, or not yet at its end, a blend, a turn.
    fn moving(&self, model: &Model, target_yaw: f32, now: Instant) -> bool {
        let playing = model.clip(&self.clip).is_some_and(|i| !self.once || self.time(now) < model.clips[i].seconds);
        let blending = self.from.as_ref().is_some_and(|(_, _, _, began)| now.saturating_duration_since(*began).as_secs_f32() < BLEND);
        let turning = ((target_yaw - self.yaw + 180.0).rem_euclid(360.0) - 180.0).abs() > 0.01;
        playing || blending || turning
    }
}

/// Smoothstep: starts and stops gently.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// How far along a glide `ease` puts it when its clock says `t`.
pub(crate) fn ease(ease: Ease, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match ease {
        Ease::Linear => t,
        Ease::Smooth => smooth(t),
        Ease::Out => 1.0 - (1.0 - t).powi(3),
        Ease::In => t * t * t,
        // Overshoots a little, then settles (an ease-out-back).
        Ease::Spring => {
            let (c1, c3) = (1.70158, 2.70158);
            1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
        }
    }
}

pub(crate) struct Layer {
    pub level: LayerLevel,
    pub items: Vec<Live>,
}

/// One pane, as the frame laid it out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GeoPane {
    pub id: u64,
    pub rect: fenix_window::Rect,
    /// Where its text starts: column 0, and its first row's top.
    pub text_x: f32,
    pub text_y: f32,
    pub base_line: usize,
    pub scroll_col: usize,
    pub rows: usize,
    pub focused: bool,
}

/// Where things were on screen in the last frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Geometry {
    pub cell: (f32, f32),
    pub window: (f32, f32),
    pub modeline_top: f32,
    pub panes: Vec<GeoPane>,
    /// The caret's cell, its top left, in pixels.
    pub caret: Option<(f32, f32)>,
}

/// The layers of every code extension, and what drawing them found.
#[derive(Default)]
pub(crate) struct Layers {
    pub sheets: HashMap<(String, u32), Sheet>,
    pub models: HashMap<(String, u32), Arc<Model>>,
    pub layers: BTreeMap<(String, u64), Layer>,
    /// The last frame's geometry, and the clickable items' rects.
    pub geometry: Geometry,
    pub hits: Vec<(String, u64, u32, fenix_window::Rect)>,
    /// What each extension was last told of the layout.
    pub told: HashMap<String, Layout>,
}

/// The glide's progress by its clock, 0 to 1, `motion` off: there at once.
fn progress(live: &Live, now: Instant, motion: bool) -> f32 {
    match &live.item.glide {
        Some(glide) if motion && glide.millis > 0 => (now.saturating_duration_since(live.since).as_secs_f32() * 1000.0 / glide.millis as f32).min(1.0),
        _ => 1.0,
    }
}

/// Where an item's foot is at `now`, in pixels: along its glide, eased,
/// and lifted by its arc.
fn foot(live: &Live, geometry: &Geometry, now: Instant, motion: bool) -> Option<(f32, f32)> {
    let from = geometry.resolve(&live.item.at)?;
    let Some(glide) = &live.item.glide else { return Some(from) };
    let t = progress(live, now, motion);
    let to = geometry.resolve(&glide.to)?;
    let e = ease(glide.ease, t);
    let lift = glide.arc * geometry.cell.1 * 4.0 * t * (1.0 - t);
    Some((from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e - lift))
}

/// The moment a model is posed at: `now`, or with `fps` set, the last
/// of its `fps` steps a second; its clip's start, with motion off.
fn sampled(live: &Live, now: Instant, motion: bool) -> Instant {
    if !motion {
        return live.acting.as_ref().map_or(now, |a| a.played_at);
    }
    match live.item.fps {
        0 => now,
        fps => {
            let step = Duration::from_secs(1) / fps as u32;
            let since = now.saturating_duration_since(live.frames_since);
            live.frames_since + step * (since.as_nanos() / step.as_nanos().max(1)) as u32
        }
    }
}

impl Geometry {
    fn pane(&self, id: u64) -> Option<&GeoPane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// `place`'s point, in pixels: none when its anchor isn't on screen.
    pub fn resolve(&self, place: &Place) -> Option<(f32, f32)> {
        let (cw, lh) = self.cell;
        let (ox, oy) = match place.anchor {
            Anchor::Window => (0.0, 0.0),
            Anchor::Modeline => (0.0, self.modeline_top),
            Anchor::Pane(id) => {
                let pane = self.pane(id)?;
                (pane.text_x, pane.text_y)
            }
            Anchor::Text((id, line, col)) => {
                let pane = self.pane(id)?;
                let (line, col) = (line as usize, col as usize);
                if line < pane.base_line || line >= pane.base_line + pane.rows || col < pane.scroll_col {
                    return None;
                }
                (pane.text_x + (col - pane.scroll_col) as f32 * cw, pane.text_y + (line - pane.base_line) as f32 * lh)
            }
            Anchor::Caret => self.caret?,
        };
        Some((ox + place.x * cw, oy + place.y * lh))
    }

    /// Where things are, in cells, as an extension is told.
    pub fn layout(&self) -> Layout {
        let (cw, lh) = (self.cell.0.max(1.0), self.cell.1.max(1.0));
        let cells = |v: f32, per: f32| (v / per).round().max(0.0) as u32;
        Layout {
            columns: cells(self.window.0, cw),
            rows: cells(self.window.1, lh),
            modeline: cells(self.modeline_top, lh),
            panes: self
                .panes
                .iter()
                .map(|p| PaneBox { id: p.id, x: cells(p.rect.x, cw), y: cells(p.rect.y, lh), width: cells(p.rect.w, cw), height: cells(p.rect.h, lh), focused: p.focused })
                .collect(),
            caret: self.caret.map(|(x, y)| (cells(x, cw), cells(y, lh))),
        }
    }
}

impl Layers {
    /// Layer `layer` of `ext` now draws `items`. One given again keeps
    /// its clocks: its glide unless that changed, its frames unless
    /// those did, and a model's clip, blending into a new one.
    pub fn set(&mut self, ext: &str, layer: u64, items: Vec<Item>, now: Instant) {
        let Some(l) = self.layers.get_mut(&(ext.to_string(), layer)) else { return };
        let mut old: HashMap<u32, Live> = l.items.drain(..).map(|live| (live.item.id, live)).collect();
        l.items = items
            .into_iter()
            .map(|item| match old.remove(&item.id) {
                Some(mut live) => {
                    if live.item.at != item.at || live.item.glide != item.glide {
                        live.since = now;
                        live.arrived = false;
                    }
                    if live.item.sheet != item.sheet || live.item.frames != item.frames || live.item.fps != item.fps {
                        live.frames_since = now;
                    }
                    live.acting = match (live.acting.take(), &item.pose) {
                        (Some(mut acting), Some(pose)) if live.item.pose.as_ref().is_some_and(|p| p.model == pose.model) => {
                            acting.update(pose, now);
                            Some(acting)
                        }
                        (_, pose) => pose.as_ref().map(|p| Acting::new(p, now)),
                    };
                    live.item = item;
                    live
                }
                None => Live::new(item, now),
            })
            .collect();
    }

    /// Everything of `ext`'s gone: it stopped, or started again.
    pub fn forget(&mut self, ext: &str) {
        self.layers.retain(|(e, _), _| e != ext);
        self.sheets.retain(|(e, _), _| e != ext);
        self.models.retain(|(e, _), _| e != ext);
        self.told.remove(ext);
    }

    /// The glides that ended since the last frame: (ext, layer, item).
    pub fn arrivals(&mut self, now: Instant, motion: bool) -> Vec<(String, u64, u32)> {
        let mut out = Vec::new();
        for ((ext, layer), l) in &mut self.layers {
            for live in &mut l.items {
                if live.item.glide.is_some() && !live.arrived && progress(live, now, motion) >= 1.0 {
                    live.arrived = true;
                    out.push((ext.clone(), *layer, live.item.id));
                }
            }
        }
        out
    }

    /// Whether something needs every frame the display shows: a glide
    /// under way, or a model moving that's drawn every frame.
    pub fn moving(&self, now: Instant, motion: bool) -> bool {
        motion
            && self.layers.iter().any(|((ext, _), l)| {
                l.items.iter().any(|live| {
                    progress(live, now, motion) < 1.0
                        || (live.item.fps == 0
                            && live.item.pose.as_ref().zip(live.acting.as_ref()).is_some_and(|(pose, acting)| self.models.get(&(ext.clone(), pose.model)).is_some_and(|m| acting.moving(m, pose.yaw, now))))
                })
            })
    }

    /// When a layer next needs a frame, beyond what `moving` keeps going:
    /// a sheet's next frame, or a model's next step, at its rate.
    pub fn next_wake(&self, now: Instant, motion: bool) -> Option<Instant> {
        if !motion {
            return None;
        }
        let mut wake: Option<Instant> = None;
        for ((ext, _), l) in &self.layers {
            for live in &l.items {
                let cycling = match &live.item.pose {
                    Some(pose) => live.acting.as_ref().zip(self.models.get(&(ext.clone(), pose.model))).is_some_and(|(acting, m)| acting.moving(m, pose.yaw, now)),
                    None => live.item.frames.len() > 1,
                };
                if live.item.fps == 0 || !cycling {
                    continue;
                }
                let step = Duration::from_secs(1) / live.item.fps as u32;
                let since = now.saturating_duration_since(live.frames_since);
                let next = live.frames_since + step * (since.as_nanos() / step.as_nanos().max(1) + 1) as u32;
                wake = Some(wake.map_or(next, |w| w.min(next)));
            }
        }
        wake
    }

    /// Queues `level`'s items into `renderer`, uploading sheets into
    /// `sprites` the first time they're drawn and drawing models with
    /// `models3d`; notes the clickable ones.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, level: LayerLevel, renderer: &mut SpriteRenderer, sprites: &mut HashMap<(String, u32), Sprite>, models3d: &mut ModelRenderer, lighting: Lighting, now: Instant, motion: bool) {
        if level == LayerLevel::BelowText {
            self.hits.clear();
        }
        let Layers { sheets, models, layers, geometry, hits, .. } = self;
        let (_, lh) = geometry.cell;
        let caret_row = geometry.caret.map(|(_, y)| (y, y + lh));
        for ((ext, layer), l) in layers.iter_mut() {
            if l.level != level {
                continue;
            }
            for live in &mut l.items {
                let Some((fx, fy)) = foot(live, geometry, now, motion) else { continue };
                let at = sampled(live, now, motion);
                let item = &live.item;
                // Where it's drawn, and the box that's it (for clicks and
                // keeping clear of the caret).
                let (sprite, x, y, w, h, uv, body) = match &item.pose {
                    Some(pose) => {
                        let Some(model) = models.get(&(ext.clone(), pose.model)) else { continue };
                        let Some(acting) = live.acting.as_mut() else { continue };
                        acting.turn(pose.yaw, now, motion);
                        let playing = acting.playing(model, at);
                        let shot = Shot {
                            height: item.size * lh,
                            yaw: acting.yaw,
                            pixel: pose.style == Style::Pixel,
                            line: lh,
                            tint: [pose.tint.0, pose.tint.1, pose.tint.2],
                            glow: pose.glow,
                            lighting,
                        };
                        let Some(placed) = models3d.draw((ext.clone(), *layer, item.id), model, &playing, &pose.parts, &shot, renderer) else { continue };
                        let (x, y) = ((fx - placed.feet.0).round(), (fy - placed.feet.1).round());
                        let (bx, by, bw, bh) = placed.body;
                        let uv = if item.flip { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 0.0, 1.0, 1.0] };
                        (placed.sprite, x, y, placed.size.0, placed.size.1, uv, (x + bx, y + by, bw, bh))
                    }
                    None => {
                        let Some(sheet) = sheets.get(&(ext.clone(), item.sheet)) else { continue };
                        let key = (ext.clone(), item.sheet);
                        if !sprites.contains_key(&key) {
                            sprites.insert(key.clone(), renderer.upload(sheet.width, sheet.height, &sheet.rgba));
                        }
                        let Some(sprite) = sprites.get(&key) else { continue };
                        // A whole number of its pixels, as near its size as that goes.
                        let (frame_w, frame_h) = sheet.frame;
                        let scale = ((item.size * lh) / frame_h as f32).round().max(1.0);
                        let (w, h) = (frame_w as f32 * scale, frame_h as f32 * scale);
                        let (x, y) = (fx.round(), (fy - h).round());
                        let columns = (sheet.width / frame_w).max(1);
                        let frame = match item.frames.len() {
                            0 => 0,
                            1 => item.frames[0],
                            n if motion && item.fps > 0 => item.frames[(now.saturating_duration_since(live.frames_since).as_millis() as u64 * item.fps as u64 / 1000) as usize % n],
                            _ => item.frames[0],
                        };
                        let (col, row) = (frame % columns, frame / columns);
                        let (sw, sh) = (sheet.width as f32, sheet.height as f32);
                        let (u0, v0) = (col as f32 * frame_w as f32 / sw, row as f32 * frame_h as f32 / sh);
                        let (u1, v1) = (u0 + frame_w as f32 / sw, v0 + frame_h as f32 / sh);
                        let uv = if item.flip { [u1, v0, u0, v1] } else { [u0, v0, u1, v1] };
                        (sprite, x, y, w, h, uv, (x, y, w, h))
                    }
                };
                let (bx, by, bw, bh) = body;
                let over_caret = caret_row.is_some_and(|(top, bottom)| by < bottom && by + bh > top);
                let alpha = item.opacity.clamp(0.0, 1.0) * if item.keep_clear && over_caret { 0.3 } else { 1.0 };
                renderer.push_part(sprite, x, y, w, h, uv, [1.0, 1.0, 1.0, alpha]);
                if item.clickable {
                    hits.push((ext.clone(), *layer, item.id, fenix_window::Rect { x: bx, y: by, w: bw, h: bh }));
                }
            }
        }
    }

    /// The clickable item at `pos`, topmost first.
    pub fn hit(&self, pos: (f32, f32)) -> Option<(String, u64, u32)> {
        self.hits.iter().rev().find(|(_, _, _, r)| pos.0 >= r.x && pos.0 < r.x + r.w && pos.1 >= r.y && pos.1 < r.y + r.h).map(|(e, l, i, _)| (e.clone(), *l, *i))
    }
}

impl App {
    fn motion_on(&self) -> bool {
        crate::motion::level(&self.config) != crate::motion::Level::Off
    }

    /// When a layer next needs a frame.
    pub(super) fn layers_next_wake(&self, now: Instant) -> Option<Instant> {
        self.wasm.layers.next_wake(now, self.motion_on())
    }

    /// Whether a layer needs every frame the display shows.
    pub(super) fn layers_moving(&self, now: Instant) -> bool {
        self.wasm.layers.moving(now, self.motion_on())
    }

    /// After a frame: glides that ended, and the layout if it moved, told
    /// to the extensions that draw.
    pub(super) fn layers_after_frame(&mut self) {
        let now = Instant::now();
        let motion = self.motion_on();
        for (ext, layer, item) in self.wasm.layers.arrivals(now, motion) {
            self.code_deliver(&ext, Notice::Arrived((layer, item)));
        }
        let layout = self.wasm.layers.geometry.layout();
        if layout.columns == 0 {
            return;
        }
        let drawing: Vec<String> = self
            .wasm
            .installed
            .iter()
            .filter(|e| e.manifest.permissions.draw == fenix_extensions::manifest::Draw::Over && self.code_ready(&e.id))
            .map(|e| e.id.clone())
            .filter(|id| self.wasm.layers.told.get(id) != Some(&layout))
            .collect();
        for ext in drawing {
            self.wasm.layers.told.insert(ext.clone(), layout.clone());
            self.code_deliver(&ext, Notice::Layout(layout.clone()));
        }
    }

    /// A left click: an extension's, when it's on one of its clickable
    /// items (and nothing else gets it).
    pub(super) fn layer_click(&mut self, pos: (f32, f32)) -> bool {
        let Some((ext, layer, item)) = self.wasm.layers.hit(pos) else { return false };
        self.code_deliver(&ext, Notice::LayerClicked((layer, item)));
        true
    }

    /// A sheet it asked for, read as it was checked: decoded, kept for
    /// drawing.
    pub(super) fn load_sheet(&mut self, ext: &str, id: u32, bytes: &[u8], frame: (u32, u32)) {
        match image::load_from_memory(bytes) {
            Ok(image) => {
                let rgba = image.to_rgba8();
                let (width, height) = rgba.dimensions();
                if width < frame.0 || height < frame.1 {
                    return self.note_code_output(ext, format!("  warn  sheet {id}: smaller than a frame"));
                }
                self.wasm.layers.sheets.insert((ext.to_string(), id), Sheet { rgba: rgba.into_raw(), width, height, frame });
            }
            Err(e) => self.note_code_output(ext, format!("  warn  sheet {id}: {e}")),
        }
    }

    /// An editor event, for the extensions that asked for it
    /// (`contributes.events`).
    pub(super) fn editor_event(&mut self, name: &str, notice: Notice) {
        let to: Vec<String> = self.wasm.installed.iter().filter(|e| e.manifest.contributes.events.iter().any(|n| n == name) && self.code_ready(&e.id)).map(|e| e.id.clone()).collect();
        for ext in to {
            self.code_deliver(&ext, notice.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Geometry {
        Geometry {
            cell: (10.0, 20.0),
            window: (800.0, 600.0),
            modeline_top: 580.0,
            panes: vec![GeoPane { id: 7, rect: fenix_window::Rect { x: 0.0, y: 40.0, w: 800.0, h: 540.0 }, text_x: 50.0, text_y: 44.0, base_line: 10, scroll_col: 0, rows: 20, focused: true }],
            caret: Some((90.0, 104.0)),
        }
    }

    fn item(id: u32, at: Place) -> Item {
        Item { id, sheet: 1, frames: vec![0, 1], fps: 4, at, size: 1.0, glide: None, flip: false, opacity: 1.0, clickable: true, keep_clear: false, pose: None }
    }

    fn glide(x: f32, millis: u32) -> fenix_wasm::host::Glide {
        fenix_wasm::host::Glide { to: Place { anchor: Anchor::Modeline, x, y: 0.0 }, millis, ease: Ease::Linear, arc: 0.0 }
    }

    /// The stick from `fenix-model`'s tests: a clip `bend` a second long.
    fn stick() -> Arc<Model> {
        use fenix_model::build::{Builder, Mesh, Track};
        let mut b = Builder::new();
        let root = b.joint("root", None, [0.0, 0.0, 0.0]);
        let tip = b.joint("tip", Some(root), [0.0, 1.0, 0.0]);
        let mut mesh = Mesh::default();
        mesh.cuboid([0.0, 0.5, 0.0], [0.2, 0.5, 0.2], [1.0; 4], root);
        mesh.cuboid([0.0, 1.5, 0.0], [0.2, 0.5, 0.2], [1.0; 4], tip);
        b.mesh(mesh);
        b.clip("bend", vec![Track::rotation(tip, vec![0.0, 1.0], vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2]])]);
        Arc::new(Model::from_glb(&b.glb()).unwrap())
    }

    fn pose(clip: &str) -> Pose {
        Pose { model: 3, clip: clip.into(), once: false, speed: 1.0, yaw: 0.0, style: Style::Pixel, parts: Vec::new(), tint: (1.0, 1.0, 1.0), glow: 0.0 }
    }

    #[test]
    fn anchors_resolve_to_pixels_and_text_scrolls() {
        let g = geometry();
        assert_eq!(g.resolve(&Place { anchor: Anchor::Window, x: 2.0, y: 1.0 }), Some((20.0, 20.0)));
        assert_eq!(g.resolve(&Place { anchor: Anchor::Modeline, x: 3.0, y: 0.0 }), Some((30.0, 580.0)), "the floor");
        assert_eq!(g.resolve(&Place { anchor: Anchor::Pane(7), x: 0.0, y: 1.0 }), Some((50.0, 64.0)));
        assert_eq!(g.resolve(&Place { anchor: Anchor::Text((7, 12, 4)), x: 0.0, y: 1.0 }), Some((90.0, 104.0)), "line 12 is the third row");
        assert_eq!(g.resolve(&Place { anchor: Anchor::Text((7, 3, 0)), x: 0.0, y: 0.0 }), None, "scrolled off");
        assert_eq!(g.resolve(&Place { anchor: Anchor::Pane(99), x: 0.0, y: 0.0 }), None);
        assert_eq!(g.resolve(&Place { anchor: Anchor::Caret, x: 0.0, y: 1.0 }), Some((90.0, 124.0)));
        let layout = g.layout();
        assert_eq!((layout.columns, layout.rows, layout.modeline, layout.caret), (80, 30, 29, Some((9, 5))));
        assert_eq!((layout.panes[0].id, layout.panes[0].y, layout.panes[0].height, layout.panes[0].focused), (7, 2, 27, true));
    }

    #[test]
    fn an_item_given_again_keeps_its_glide_and_a_changed_one_starts_over() {
        let mut layers = Layers::default();
        layers.layers.insert(("x".into(), 1), Layer { level: LayerLevel::AboveText, items: Vec::new() });
        let t0 = Instant::now();
        let mut walking = item(1, Place { anchor: Anchor::Modeline, x: 0.0, y: 0.0 });
        walking.glide = Some(glide(10.0, 1000));
        layers.set("x", 1, vec![walking.clone()], t0);
        layers.set("x", 1, vec![walking.clone()], t0 + Duration::from_millis(400));
        assert_eq!(layers.layers[&("x".into(), 1)].items[0].since, t0, "the same item keeps going");
        assert!(layers.moving(t0 + Duration::from_millis(400), true), "gliding: every frame");
        assert!(!layers.moving(t0 + Duration::from_millis(400), false));
        assert!(layers.arrivals(t0 + Duration::from_millis(500), true).is_empty());
        assert_eq!(layers.arrivals(t0 + Duration::from_millis(1000), true), [("x".to_string(), 1, 1)]);
        assert!(layers.arrivals(t0 + Duration::from_millis(1200), true).is_empty(), "told once");
        // Still, it cycles its two frames at 4 a second, waking when the
        // next is due (not a frame's length from now); with motion off,
        // nothing wakes.
        let wake = layers.next_wake(t0 + Duration::from_millis(1200), true).unwrap();
        assert_eq!(wake, t0 + Duration::from_millis(1250));
        assert_eq!(layers.next_wake(t0, false), None);
        // Turned round, it keeps its glide and its frames' clock; a new
        // glide starts from now, and new frames from now.
        walking.flip = true;
        layers.set("x", 1, vec![walking.clone()], t0 + Duration::from_millis(1300));
        let live = &layers.layers[&("x".into(), 1)].items[0];
        assert_eq!((live.since, live.frames_since), (t0, t0), "the same glide and frames");
        walking.glide = Some(glide(20.0, 1000));
        walking.frames = vec![2, 3];
        layers.set("x", 1, vec![walking], t0 + Duration::from_millis(1400));
        let live = &layers.layers[&("x".into(), 1)].items[0];
        assert_eq!((live.since, live.frames_since, live.arrived), (t0 + Duration::from_millis(1400), t0 + Duration::from_millis(1400), false));
        layers.forget("x");
        assert!(layers.layers.is_empty());
    }

    #[test]
    fn glides_ease_and_arc() {
        assert_eq!((ease(Ease::Linear, 0.25), ease(Ease::Smooth, 0.5), ease(Ease::Out, 1.0), ease(Ease::In, 0.0)), (0.25, 0.5, 1.0, 0.0));
        assert!(ease(Ease::Smooth, 0.1) < 0.1 && ease(Ease::Out, 0.1) > 0.1 && ease(Ease::In, 0.5) < 0.5);
        assert!((0..=10).any(|i| ease(Ease::Spring, i as f32 / 10.0) > 1.0), "a spring overshoots");
        assert!((ease(Ease::Spring, 1.0) - 1.0).abs() < 1e-6, "and settles");
        let t0 = Instant::now();
        let mut hop = item(1, Place { anchor: Anchor::Modeline, x: 0.0, y: 0.0 });
        hop.glide = Some(fenix_wasm::host::Glide { ease: Ease::Smooth, arc: 2.0, ..glide(10.0, 1000) });
        let live = Live::new(hop, t0);
        let g = geometry();
        let at = |ms| foot(&live, &g, t0 + Duration::from_millis(ms), true).unwrap();
        assert_eq!(at(0), (0.0, 580.0));
        // Halfway: halfway along, and two lines (of 20 pixels) up.
        assert_eq!(at(500), (50.0, 540.0));
        assert_eq!(at(1000), (100.0, 580.0));
        assert_eq!(foot(&live, &g, t0, false).unwrap(), (100.0, 580.0), "motion off: there at once");
    }

    #[test]
    fn a_model_blends_into_a_new_clip_and_turns_the_short_way() {
        let model = stick();
        let t0 = Instant::now();
        let mut acting = Acting::new(&pose("bend"), t0);
        let playing = acting.playing(&model, t0 + Duration::from_millis(1250));
        assert_eq!(playing.len(), 1);
        assert!((playing[0].seconds - 0.25).abs() < 1e-3, "round again: {playing:?}");
        assert!(acting.moving(&model, 0.0, t0 + Duration::from_secs(5)), "going round");
        // A new clip: the old one blends out over `BLEND`.
        let t1 = t0 + Duration::from_millis(1500);
        acting.update(&Pose { clip: "nothing".into(), ..pose("bend") }, t1);
        let blend = acting.playing(&model, t1 + Duration::from_millis(100));
        assert_eq!(blend.len(), 2);
        assert_eq!((blend[0].clip, blend[1].clip), (Playing::REST, 0));
        assert!((blend[0].weight - 0.5).abs() < 1e-3);
        assert_eq!(acting.playing(&model, t1 + Duration::from_millis(300)).len(), 1, "blended");
        assert!(!acting.moving(&model, 0.0, t1 + Duration::from_millis(300)), "at rest, still");
        // Once: held at its end.
        acting.update(&Pose { once: true, ..pose("bend") }, t1);
        assert!(!acting.moving(&model, 0.0, t1 + Duration::from_secs(2)));
        // From 350 to 10 degrees is 20 the short way.
        acting.yaw = 350.0;
        acting.yaw_at = t1;
        acting.turn(10.0, t1 + Duration::from_millis(10), true);
        assert!((acting.yaw - 357.2).abs() < 1e-3, "{}", acting.yaw);
        assert!(acting.moving(&model, 10.0, t1));
        acting.turn(10.0, t1 + Duration::from_millis(100), true);
        assert_eq!(acting.yaw, 10.0);
    }

    #[test]
    fn a_model_drawn_a_few_times_a_second_wakes_at_its_rate() {
        let mut layers = Layers::default();
        layers.layers.insert(("x".into(), 1), Layer { level: LayerLevel::AboveText, items: Vec::new() });
        layers.models.insert(("x".into(), 3), stick());
        let t0 = Instant::now();
        let mut pet = item(1, Place { anchor: Anchor::Modeline, x: 0.0, y: 0.0 });
        pet.pose = Some(pose("bend"));
        pet.fps = 10;
        layers.set("x", 1, vec![pet.clone()], t0);
        assert!(!layers.moving(t0, true), "not every frame");
        assert_eq!(layers.next_wake(t0 + Duration::from_millis(130), true), Some(t0 + Duration::from_millis(200)));
        let live = &layers.layers[&("x".into(), 1)].items[0];
        assert_eq!(sampled(live, t0 + Duration::from_millis(130), true), t0 + Duration::from_millis(100));
        pet.fps = 0;
        layers.set("x", 1, vec![pet], t0);
        assert!(layers.moving(t0, true), "every frame");
        assert_eq!(layers.next_wake(t0, true), None);
        layers.forget("x");
        assert!(layers.models.is_empty());
    }

    #[test]
    fn a_click_finds_the_topmost_clickable_item() {
        let layers = Layers {
            hits: vec![("a".into(), 1, 1, fenix_window::Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 }), ("b".into(), 2, 5, fenix_window::Rect { x: 10.0, y: 10.0, w: 20.0, h: 20.0 })],
            ..Layers::default()
        };
        assert_eq!(layers.hit((15.0, 15.0)), Some(("b".into(), 2, 5)));
        assert_eq!(layers.hit((5.0, 5.0)), Some(("a".into(), 1, 1)));
        assert_eq!(layers.hit((50.0, 50.0)), None);
    }
}
