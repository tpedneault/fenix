//! The editor's side of `crate::motion`: what's animating right now, how
//! a keypress starts an animation (a jump's beacon, an undo's pulse, the
//! caret's glide), and what each frame reads back to draw it. Nothing
//! here waits: every animation starts from the state a key already
//! produced, and drawing only interpolates towards it.

use super::*;
use crate::motion::{self, lerp, lerp_rect, lerp_rgba, Anim, Level, Prefs};
use crate::sprite::{Sprite, SpriteRenderer};

/// The GPU pieces the animations and polish draw with, per window.
pub(super) struct MotionGpu {
    /// Rects drawn after text in the base pass: dimming, fold reveals,
    /// sticky-scroll backgrounds, the overview ruler, squiggles.
    pub overlay: RectRenderer,
    /// Images drawn before popups in the overlay pass: the modeline's
    /// spinner.
    pub sprites: SpriteRenderer,
    /// Images drawn over everything: the old frame a theme change fades
    /// out of.
    pub top: SpriteRenderer,
    /// The spinner's three blades, at the pixel size they were drawn at.
    pub blades: Option<(u32, [Sprite; 3])>,
    /// The frame copied just before a theme change, and the texture it
    /// lives in.
    pub snapshot: Option<(wgpu::Texture, Sprite)>,
}

impl MotionGpu {
    pub fn new(gpu: &GpuState) -> Self {
        let sprites = SpriteRenderer::new(Arc::clone(&gpu.device), Arc::clone(&gpu.queue), gpu.config.format);
        let top = SpriteRenderer::new(Arc::clone(&gpu.device), Arc::clone(&gpu.queue), gpu.config.format);
        MotionGpu { overlay: RectRenderer::new(gpu), sprites, top, blades: None, snapshot: None }
    }

    /// The spinner's blades at `size` pixels, drawn the first time and
    /// again only when the size changes.
    pub fn blades(&mut self, size: u32) -> &[Sprite; 3] {
        if self.blades.as_ref().is_none_or(|(s, _)| *s != size) {
            let upload = |i| {
                let image = fenix_brand::blade(i, size);
                self.sprites.upload(image.width, image.height, &image.rgba)
            };
            let blades = [upload(0), upload(1), upload(2)];
            self.blades = Some((size, blades));
        }
        &self.blades.as_ref().expect("just drawn").1
    }

    /// Queues the spinner at `(x, y)`, `size` pixels square, blade `lit`
    /// bright and the others faint.
    pub fn push_spinner(&mut self, x: f32, y: f32, size: f32, lit: usize) {
        self.blades(size as u32);
        let Some((_, blades)) = &self.blades else { return };
        for (i, blade) in blades.iter().enumerate() {
            let alpha = if i == lit { 1.0 } else { 0.28 };
            self.sprites.push(blade, x, y, size, size, [1.0, 1.0, 1.0, alpha]);
        }
    }

    /// A texture the size of the surface to copy a frame into.
    pub fn snapshot_target(&mut self, gpu: &GpuState) -> &wgpu::Texture {
        let (w, h) = (gpu.config.width, gpu.config.height);
        if self.snapshot.as_ref().is_none_or(|(t, _)| t.width() != w || t.height() != h) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("theme-fade-snapshot"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: gpu.config.format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let sprite = self.top.sprite_of_view(&view, w, h);
            self.snapshot = Some((texture, sprite));
        }
        &self.snapshot.as_ref().expect("just made").0
    }
}

/// What a pane draws besides its text and today's highlights: the
/// animations' marks and the polish, worked out while its buffer is at
/// hand and drawn later with everything else.
#[derive(Default)]
pub(super) struct PaneExtras {
    /// The buffer an ordinary code pane shows.
    pub buffer: Option<BufferId>,
    /// The document display row at the pane's top, and the first visual
    /// column shown -- what a caret glide converts through.
    pub base_line: usize,
    pub scroll_col: usize,
    /// Whole rows flashing: the jump beacon.
    pub row_flashes: Vec<(usize, [f32; 4])>,
    /// Cells flashing: what a change added.
    pub cell_flashes: Vec<(usize, usize, usize, [f32; 4])>,
    /// Where a change removed text: row, column, whether whole lines.
    pub seams: Vec<(usize, usize, bool, [f32; 4])>,
    /// Rows still opening under an unfolded row: the first row, how
    /// many, and how far open (0 to 1).
    pub reveal: Option<(usize, usize, f32)>,
    /// Problem underlines: row, columns, colour.
    pub squiggles: Vec<(usize, usize, usize, [f32; 4])>,
    pub ruler: Option<Ruler>,
    /// Sticky scroll's rows, one span list each, outermost first.
    pub sticky: Vec<RowSpans>,
}

/// The overview ruler down a code pane's right edge.
#[derive(Debug, Clone, Default)]
pub(super) struct Ruler {
    /// Display lines in the document.
    pub lines: usize,
    /// The first display line shown, and how many fit.
    pub first: f32,
    pub visible: f32,
    pub cursor: usize,
    pub marks: Vec<(usize, RulerMark)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RulerMark {
    Search,
    Error,
    Warning,
    Added,
    Modified,
    Deleted,
}

/// After a jump: the line the cursor landed on, flashing once.
#[derive(Debug, Clone, Copy)]
pub(super) struct Beacon {
    pub buffer: BufferId,
    pub line: usize,
    pub anim: Anim,
}

/// What an undo, redo, repeat or substitution changed.
#[derive(Debug, Clone)]
pub(super) enum Change {
    /// Text that's there now, by char range.
    Added(std::ops::Range<usize>),
    /// Where text went: a char index, and whether whole lines went.
    Removed { at: usize, lines: bool },
}

#[derive(Debug, Clone)]
pub(super) struct ChangeMark {
    pub buffer: BufferId,
    pub change: Change,
    pub anim: Anim,
}

/// A popup's life: when it appeared and, while it fades out, what it
/// last showed.
#[derive(Debug, Clone)]
pub(super) struct PopupLife {
    pub shown: Instant,
    pub rect: fenix_window::Rect,
    pub spans: RowSpans,
    pub selected: Option<usize>,
    pub closing: Option<Instant>,
}

/// One popup as a frame draws it.
pub(super) struct PopupDraw {
    pub id: popup::PopupId,
    pub rect: fenix_window::Rect,
    pub spans: RowSpans,
    pub selected: Option<usize>,
    pub alpha: f32,
}

/// Something easing from one value to another.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tween<T> {
    pub from: T,
    pub to: T,
    pub anim: Option<Anim>,
}

/// Which pane of which workspace of which window: a pane id alone is
/// only unique within one window tree.
pub(super) type PaneKey = (usize, usize, fenix_window::WindowId);

/// The caret's place in a document -- display row and visual column --
/// so a glide stays put while the view scrolls under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CaretSpot {
    pub pane: PaneKey,
    pub buffer: BufferId,
    pub row: f32,
    pub col: f32,
}

/// Rows opening under an unfolded row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum RevealTarget {
    Sidebar,
    Explorer,
    Buffer(BufferId),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Reveal {
    pub target: RevealTarget,
    /// The first revealed row: an entry index for the explorers, a
    /// document line for a buffer.
    pub first: usize,
    pub count: usize,
    pub anim: Anim,
}

/// The state of the keyboard a key found, so what it changed can be
/// told afterwards.
pub(super) struct KeySnapshot {
    mode: Mode,
    jumps: (usize, usize, Option<(BufferId, usize)>),
    pane: fenix_window::WindowId,
    frame: usize,
    buffer: BufferId,
    line: usize,
    pulse: Option<Instant>,
}

/// Everything animating, and what the last frame drew to animate from.
#[derive(Default)]
pub(super) struct MotionState {
    pub beacon: Option<Beacon>,
    pub changes: Vec<ChangeMark>,
    pub popups: HashMap<popup::PopupId, PopupLife>,
    pub mode: Option<Tween<[f32; 4]>>,
    pub error_flash: Option<Anim>,
    pub last_error: Option<Instant>,
    pub tab_accents: HashMap<PaneKey, Tween<(f32, f32)>>,
    pub scroll_cols: HashMap<PaneKey, Tween<f32>>,
    pub glide: Option<(CaretSpot, Tween<(f32, f32)>)>,
    pub last_caret: Option<CaretSpot>,
    /// Set by a key that may glide the caret; the next frame takes it.
    pub glide_armed: bool,
    /// The key being handled is the keyboard repeating it.
    pub key_repeat: bool,
    /// A trackpad's scrolling not yet worth a whole line.
    pub wheel_remainder: f32,
    /// The which-key menu: when its sequence started waiting, whether
    /// it's open yet, the page it's on and how many there are, the keys
    /// it's for, and the drawer last drawn (kept to fade it out).
    pub which_key_since: Option<Instant>,
    pub which_key_shown: bool,
    pub which_key_page: usize,
    pub which_key_pages: usize,
    pub which_key_path: Vec<KeyPress>,
    pub which_key_drawer: Option<super::which_key::Drawer>,
    pub layout: HashMap<fenix_window::WindowId, Tween<fenix_window::Rect>>,
    pub layout_last: Option<((usize, usize), fenix_window::Rect, Vec<(fenix_window::WindowId, fenix_window::Rect)>)>,
    pub reveals: Vec<Reveal>,
    /// The explorers' lengths last frame, to notice a folder opening.
    pub explorer_len: HashMap<bool, (PathBuf, usize)>,
    /// The theme to fade from, set by a theme change until the next
    /// frame has copied it.
    pub theme_from: Option<&'static Theme>,
    pub theme_fade: Option<Anim>,
    /// This frame is the old theme's, drawn only to be copied.
    pub capture_frame: bool,
    /// A `git fetch` running in the background.
    pub git_fetching: bool,
    /// Whole-buffer search matches for the overview ruler, by buffer:
    /// the buffer's edit count and the pattern they're for, and the
    /// lines they're on.
    pub ruler_search: HashMap<BufferId, (u64, String, Vec<usize>)>,
    /// Bracket depths for rainbow brackets, by buffer: the edit count
    /// they're for, and each bracket's char index and depth.
    pub brackets: HashMap<BufferId, (u64, Vec<(usize, usize)>)>,
    /// When the focused buffer was last typed into, for holding new
    /// diagnostics back while typing.
    pub last_typed: Option<Instant>,
    /// The diagnostics drawn, by path, until typing pauses.
    pub shown_diagnostics: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
}

impl MotionState {
    /// The popups this frame draws: the ones open now, fading in, and the
    /// ones just closed, fading out.
    pub(super) fn popup_draws(&mut self, prefs: Prefs, open: Vec<(popup::PopupId, fenix_window::Rect, RowSpans, Option<usize>)>, now: Instant) -> Vec<PopupDraw> {
        let mut draws = Vec::new();
        let open_ids: Vec<popup::PopupId> = open.iter().map(|(id, ..)| *id).collect();
        for (id, rect, spans, selected) in open {
            let life = self.popups.entry(id).or_insert_with(|| PopupLife { shown: now, rect, spans: Vec::new(), selected: None, closing: None });
            if life.closing.is_some() {
                // Reopened while fading out: carry on from here.
                *life = PopupLife { shown: now, rect, spans: Vec::new(), selected: None, closing: None };
            }
            life.rect = rect;
            life.spans = spans.clone();
            life.selected = selected;
            let drawer = id == popup::PopupId::WhichKey;
            let (alpha, rise) = if prefs.popups {
                // The which-key drawer slides up from behind the modeline,
                // opaque; every other popup fades in rising a few pixels.
                let length = if drawer { prefs.popup_in_d.mul_f32(1.6) } else { prefs.popup_in_d };
                let t = motion::ease_out_cubic(now.saturating_duration_since(life.shown).as_secs_f32() / length.as_secs_f32().max(1e-3));
                if drawer { (1.0, (1.0 - t) * rect.h) } else { (t, (1.0 - t) * 4.0) }
            } else {
                (1.0, 0.0)
            };
            let rect = fenix_window::Rect { y: (rect.y + rise).round(), ..rect };
            draws.push(PopupDraw { id, rect, spans, selected, alpha });
        }
        for (id, life) in self.popups.iter_mut() {
            if open_ids.contains(id) {
                continue;
            }
            if !prefs.popups {
                life.closing = Some(now - Duration::from_secs(60));
                continue;
            }
            let closing = *life.closing.get_or_insert(now);
            let drawer = *id == popup::PopupId::WhichKey;
            let length = if drawer { prefs.popup_out_d.mul_f32(1.6) } else { prefs.popup_out_d };
            let t = now.saturating_duration_since(closing).as_secs_f32() / length.as_secs_f32().max(1e-3);
            if t < 1.0 {
                let eased = motion::ease_out_cubic(t);
                let (rect, alpha) = if drawer {
                    (fenix_window::Rect { y: life.rect.y + eased * life.rect.h, ..life.rect }, 1.0)
                } else {
                    (life.rect, 1.0 - eased)
                };
                draws.push(PopupDraw { id: *id, rect, spans: life.spans.clone(), selected: life.selected, alpha });
            }
        }
        self.popups.retain(|id, life| open_ids.contains(id) || life.closing.is_some_and(|c| now.saturating_duration_since(c) < prefs.popup_out_d.mul_f32(1.6)));
        draws
    }

    /// The active tab accent's `(x, width)` this frame, sliding from where
    /// it was to `target`.
    pub(super) fn tab_accent(&mut self, prefs: Prefs, key: PaneKey, target: (f32, f32), now: Instant) -> (f32, f32) {
        let lerp2 = |a: (f32, f32), b: (f32, f32), t: f32| (lerp(a.0, b.0, t), lerp(a.1, b.1, t));
        let tween = self.tab_accents.entry(key).or_insert(Tween { from: target, to: target, anim: None });
        if tween.to != target {
            let from = match tween.anim {
                Some(anim) if anim.running(now) => lerp2(tween.from, tween.to, anim.eased(now)),
                _ => tween.to,
            };
            // Small moves (the strip re-measured by a pixel) snap.
            let far = (from.0 - target.0).abs() > 2.0 || (from.1 - target.1).abs() > 2.0;
            *tween = Tween { from, to: target, anim: (prefs.tabs && far).then(|| Anim::new(now, prefs.tab_d)) };
        }
        match tween.anim {
            Some(anim) if anim.running(now) => lerp2(tween.from, tween.to, anim.eased(now)),
            _ => target,
        }
    }

    /// Where to draw the focused caret, in document space (display row,
    /// visual column): gliding from the last place after a motion key.
    pub(super) fn caret_spot(&mut self, prefs: Prefs, spot: CaretSpot, visible_rows: usize, now: Instant) -> (f32, f32) {
        let armed = std::mem::take(&mut self.glide_armed);
        let current = |glide: &Option<(CaretSpot, Tween<(f32, f32)>)>, last: CaretSpot| match glide {
            Some((key, tween)) if key.pane == last.pane && key.buffer == last.buffer => match tween.anim {
                Some(anim) if anim.running(now) => (lerp(tween.from.0, tween.to.0, anim.eased(now)), lerp(tween.from.1, tween.to.1, anim.eased(now))),
                _ => (last.row, last.col),
            },
            _ => (last.row, last.col),
        };
        if let Some(last) = self.last_caret {
            let moved = (last.row, last.col) != (spot.row, spot.col);
            let same_place = last.pane == spot.pane && last.buffer == spot.buffer;
            if moved {
                let near = (last.row - spot.row).abs() <= visible_rows as f32;
                if armed && prefs.caret_glide && same_place && near {
                    let from = current(&self.glide, last);
                    self.glide = Some((spot, Tween { from, to: (spot.row, spot.col), anim: Some(Anim::new(now, prefs.glide_d)) }));
                } else {
                    self.glide = None;
                }
            }
        }
        self.last_caret = Some(spot);
        match &self.glide {
            Some((key, tween)) if key.pane == spot.pane && key.buffer == spot.buffer => match tween.anim {
                Some(anim) if anim.running(now) => (lerp(tween.from.0, tween.to.0, anim.eased(now)), lerp(tween.from.1, tween.to.1, anim.eased(now))),
                _ => (spot.row, spot.col),
            },
            _ => (spot.row, spot.col),
        }
    }

}

impl App {
    /// Pixels of rounding on floating boxes: the setting, else the
    /// theme's (TempleOS stays square).
    pub(super) fn corner_radius(&self) -> f32 {
        match self.config.polish.corner_radius {
            Some(r) => r as f32,
            None if self.theme.name == "TempleOS" => 0.0,
            None => 6.0,
        }
    }

    /// The animation settings as a frame reads them.
    pub(super) fn motion(&self) -> Prefs {
        Prefs::from_config(&self.config)
    }

    /// `SPC t a`: off, subtle, full, and round again.
    pub(crate) fn cycle_motion_level(&mut self) {
        let next = motion::level(&self.config).next();
        self.config.animations = None;
        self.config.motion.level = Some(next.name().to_string());
        let label = match next {
            Level::Off => "Motion off",
            Level::Subtle => "Motion: subtle",
            Level::Full => "Motion: full",
        };
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {err}"));
        } else {
            self.set_message(label);
        }
    }

    pub(super) fn pane_key(&self, pane: fenix_window::WindowId) -> PaneKey {
        (self.active_frame, self.workspaces.active, pane)
    }

    // -- Keys -------------------------------------------------------------

    pub(super) fn motion_before_key(&mut self) -> KeySnapshot {
        let buffer = self.focused_buffer_id();
        // Only this key's own edits should pulse.
        if let Some(ob) = self.buffers.get_mut(buffer) {
            let _ = ob.buffer.take_changes();
        }
        let cursor = self.cursor();
        let line = self.open().buffer.line_col(&cursor).0;
        KeySnapshot {
            mode: self.vim.mode(),
            jumps: self.jump_signature(),
            pane: self.focused_pane_id(),
            frame: self.focused_frame,
            buffer,
            line,
            pulse: self.pulse.as_ref().map(|p| p.started),
        }
    }

    fn jump_signature(&self) -> (usize, usize, Option<(BufferId, usize)>) {
        (self.jump_back_stack.len(), self.jump_forward_stack.len(), self.jump_back_stack.last().map(|j| (j.buffer, j.char_idx)))
    }

    pub(super) fn motion_after_key(&mut self, before: KeySnapshot, key: KeyPress) {
        let prefs = self.motion();
        let now = Instant::now();
        let mode = self.vim.mode();
        if mode == Mode::Insert || before.mode == Mode::Insert {
            self.motion.last_typed = Some(now);
        }
        self.motion.glide_armed =
            prefs.caret_glide && !self.motion.key_repeat && before.mode != Mode::Insert && mode != Mode::Insert && before.mode != Mode::Command;

        let plain = key.mods == Mods::default();
        let changed_text = matches!(before.mode, Mode::Normal | Mode::Visual)
            && (matches!(key.code, KeyCode::Char('u' | 'U' | '.')) && plain
                || key.code == KeyCode::Char('r') && key.mods.ctrl)
            || before.mode == Mode::Command && key.code == KeyCode::Named(FenixNamedKey::Enter);
        let pulsed = self.pulse.as_ref().map(|p| p.started) != before.pulse;
        if changed_text && prefs.change_pulse && !pulsed && self.focused_buffer_id() == before.buffer {
            self.pulse_changes(before.buffer, now, prefs.pulse_d);
        }

        if prefs.beacon && self.focused_frame == before.frame {
            let pane = self.focused_pane_id();
            let buffer = self.focused_buffer_id();
            let line = self.open().buffer.line_col(&self.cursor()).0;
            let searched = before.mode == Mode::Normal && plain_or_shift(key) && matches!(key.code, KeyCode::Char('n' | 'N' | '*' | '#'));
            let jumped = self.jump_signature() != before.jumps || searched && line != before.line;
            let switched = pane != before.pane && prefs.beacon_on_focus;
            if jumped || switched || (buffer != before.buffer && pane == before.pane && line != 0) {
                self.start_beacon(buffer, line, now, prefs.beacon_d);
            }
        }
    }

    /// Flashes `line` of `buffer` once.
    pub(super) fn start_beacon(&mut self, buffer: BufferId, line: usize, now: Instant, duration: Duration) {
        self.motion.beacon = Some(Beacon { buffer, line, anim: Anim::new(now, duration) });
    }

    /// Flashes the line the cursor just landed on, if the beacon is on --
    /// for a jump that doesn't come from a key (a debugger stop).
    pub(super) fn beacon_here(&mut self) {
        let prefs = self.motion();
        if prefs.beacon {
            let buffer = self.focused_buffer_id();
            let line = self.open().buffer.line_col(&self.cursor()).0;
            self.start_beacon(buffer, line, Instant::now(), prefs.beacon_d);
        }
    }

    /// Turns `buffer`'s logged edits into pulses: added text flashes,
    /// and where text went, a seam does.
    pub(super) fn pulse_changes(&mut self, buffer: BufferId, now: Instant, duration: Duration) {
        let Some(ob) = self.buffers.get_mut(buffer) else { return };
        let Some(deltas) = ob.buffer.take_changes() else { return };
        let anim = Anim::new(now, duration);
        let changes = settle_changes(&deltas);
        self.motion.changes.retain(|c| c.buffer != buffer);
        self.motion.changes.extend(changes.into_iter().map(|change| ChangeMark { buffer, change, anim }));
    }

    // -- What a frame draws ------------------------------------------------

    /// The beacon's row and colour in a pane showing `buffer`, if it's
    /// flashing there.
    pub(super) fn beacon_row(&self, buffer: BufferId, visible_document_lines: &[usize], visible: usize, now: Instant) -> Option<(usize, [f32; 4])> {
        let beacon = self.motion.beacon.filter(|b| b.buffer == buffer && b.anim.running(now))?;
        let row = visible_document_lines.iter().position(|&l| l == beacon.line).filter(|&r| r <= visible)?;
        let [r, g, b, _] = self.theme.caret;
        Some((row, [r, g, b, 0.45 * (1.0 - beacon.anim.eased(now))]))
    }

    /// The change pulses in the focused pane: added text as segments,
    /// removals as seams `(row, col, whole_lines)`, each with its alpha.
    pub(super) fn change_overlays(&self, buffer: BufferId, visible: usize, now: Instant) -> (Vec<(usize, usize, usize, f32)>, Vec<(usize, usize, bool, f32)>) {
        let mut added = Vec::new();
        let mut seams = Vec::new();
        let Some(ob) = self.buffers.get(buffer) else { return (added, seams) };
        let len = ob.buffer.len_chars();
        for mark in self.motion.changes.iter().filter(|m| m.buffer == buffer && m.anim.running(now)) {
            let alpha = 1.0 - mark.anim.eased(now);
            match &mark.change {
                Change::Added(range) => {
                    let range = range.start.min(len)..range.end.min(len);
                    for (row, s, e) in self.range_to_segments(range, visible) {
                        added.push((row, s, e, alpha));
                    }
                }
                Change::Removed { at, lines } => {
                    let at = (*at).min(len);
                    let (line, col) = ob.buffer.line_col(&Cursor { char_idx: at, sticky_col: 0 });
                    let Some(row) = line.checked_sub(self.render_base_line()).filter(|&r| r <= visible) else { continue };
                    seams.push((row, if *lines { 0 } else { col }, *lines, alpha));
                }
            }
        }
        (added, seams)
    }

    /// The mode rail's colour this frame: easing to `target` after a mode
    /// change, and flashing the error colour once after an error.
    pub(super) fn rail_color(&mut self, target: [f32; 4], now: Instant) -> [f32; 4] {
        let prefs = self.motion();
        let color = match &mut self.motion.mode {
            Some(tween) if tween.to == target => match tween.anim {
                Some(anim) if anim.running(now) => lerp_rgba(tween.from, tween.to, anim.eased(now)),
                _ => target,
            },
            slot => {
                let from = match slot {
                    Some(Tween { from, to, anim: Some(anim) }) if anim.running(now) => lerp_rgba(*from, *to, anim.eased(now)),
                    Some(tween) => tween.to,
                    None => target,
                };
                let anim = (prefs.mode_fade && from != target).then(|| Anim::new(now, prefs.mode_d));
                *slot = Some(Tween { from, to: target, anim });
                from
            }
        };
        let error_at = self.status_message.as_ref().filter(|m| m.is_error).map(|m| m.set_at);
        if error_at.is_some() && error_at != self.motion.last_error {
            self.motion.last_error = error_at;
            self.motion.error_flash = prefs.error_flash.then(|| Anim::new(now, prefs.error_d));
        }
        match self.motion.error_flash.filter(|a| a.running(now)) {
            Some(anim) => lerp_rgba(glyphon_to_rgba(self.theme.git_conflicted), color, anim.eased(now)),
            None => color,
        }
    }

    /// How opaque the modeline's message is: fading in when set and out
    /// at the end of its time.
    pub(super) fn message_alpha(&self, now: Instant) -> f32 {
        let prefs = self.motion();
        let Some(message) = &self.status_message else { return 1.0 };
        if !prefs.messages {
            return 1.0;
        }
        let age = now.saturating_duration_since(message.set_at);
        let fade_in = (age.as_secs_f32() / prefs.message_in_d.as_secs_f32().max(1e-3)).clamp(0.0, 1.0);
        let left = MESSAGE_DURATION.saturating_sub(age);
        let fade_out = (left.as_secs_f32() / prefs.message_out_d.as_secs_f32().max(1e-3)).clamp(0.0, 1.0);
        fade_in.min(fade_out)
    }

    /// Work going on in the background, newest first: what the
    /// modeline's progress segment names.
    pub(super) fn background_jobs(&self) -> Vec<String> {
        let mut jobs = Vec::new();
        if self.motion.git_fetching {
            jobs.push("fetching".to_string());
        }
        if self.task_session.as_ref().is_some_and(|s| s.runner.is_some()) {
            jobs.push("running task".to_string());
        }
        if self.agenda_sync.pulling {
            jobs.push("syncing Jira".to_string());
        }
        for (key, session) in &self.lsp_sessions {
            if session.capabilities.is_none() {
                jobs.push(format!("starting {:?} server", key.language).to_lowercase());
            }
        }
        jobs
    }

    /// The first visual column to draw in a pane this frame, easing to
    /// `target` a column at a time.
    pub(super) fn scroll_col(&mut self, pane: fenix_window::WindowId, target: usize, now: Instant) -> usize {
        let prefs = self.motion();
        let key = self.pane_key(pane);
        let target_f = target as f32;
        let tween = self.motion.scroll_cols.entry(key).or_insert(Tween { from: target_f, to: target_f, anim: None });
        if tween.to != target_f {
            let from = match tween.anim {
                Some(anim) if anim.running(now) => lerp(tween.from, tween.to, anim.eased(now)),
                _ => tween.to,
            };
            *tween = Tween { from, to: target_f, anim: prefs.smooth_scroll.then(|| Anim::new(now, prefs.scroll_d)) };
        }
        match tween.anim {
            Some(anim) if anim.running(now) => lerp(tween.from, tween.to, anim.eased(now)).round().max(0.0) as usize,
            _ => target,
        }
    }

    /// The panes' rects this frame: easing to `layout` after a split, a
    /// close or a resize by key. A window resize or a workspace switch
    /// snaps.
    pub(super) fn motion_layout(&mut self, layout: Vec<(fenix_window::WindowId, fenix_window::Rect)>, area: fenix_window::Rect, now: Instant) -> Vec<(fenix_window::WindowId, fenix_window::Rect)> {
        let prefs = self.motion();
        let key = (self.active_frame, self.workspaces.active);
        let previous = self.motion.layout_last.take();
        let same_place = previous.as_ref().is_some_and(|(k, a, _)| *k == key && rect_eq(*a, area));
        let mut out = Vec::with_capacity(layout.len());
        match previous {
            Some((_, _, old)) if same_place && prefs.layout && old != layout => {
                // Where each pane was drawn last: mid-tween, its current rect.
                let drawn = |pane: fenix_window::WindowId| -> Option<fenix_window::Rect> {
                    let target = old.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r)?;
                    Some(match self.motion.layout.get(&pane) {
                        Some(Tween { from, to, anim: Some(anim) }) if anim.running(now) => lerp_rect(*from, *to, anim.eased(now)),
                        _ => target,
                    })
                };
                let mut tweens = HashMap::new();
                for &(pane, rect) in &layout {
                    let from = drawn(pane).unwrap_or_else(|| collapsed_from(rect, area));
                    if !rect_eq(from, rect) {
                        tweens.insert(pane, Tween { from, to: rect, anim: Some(Anim::new(now, prefs.layout_d)) });
                    }
                }
                self.motion.layout = tweens;
            }
            Some(_) if same_place => {
                self.motion.layout.retain(|pane, t| layout.iter().any(|(p, r)| p == pane && rect_eq(*r, t.to)));
            }
            _ => self.motion.layout.clear(),
        }
        for &(pane, rect) in &layout {
            let shown = match self.motion.layout.get(&pane) {
                Some(Tween { from, to, anim: Some(anim) }) if anim.running(now) && rect_eq(*to, rect) => lerp_rect(*from, *to, anim.eased(now)),
                _ => rect,
            };
            out.push((pane, shown));
        }
        self.motion.layout_last = Some((key, area, layout));
        out
    }

    /// Notices an explorer growing under its selected row -- a folder
    /// opening -- and reveals the new rows.
    pub(super) fn notice_explorer_growth(&mut self, sidebar: bool, now: Instant) {
        let prefs = self.motion();
        let explorer = if sidebar { self.sidebar.as_ref() } else { self.explorer.as_ref() };
        let Some(explorer) = explorer else {
            self.motion.explorer_len.remove(&sidebar);
            return;
        };
        let root = explorer.cwd.clone();
        let len = explorer.entries.len();
        let selected = explorer.selected;
        let depth = explorer.entries.get(selected).map(|e| e.depth);
        let grew_under = explorer.entries.get(selected + 1).zip(depth).is_some_and(|(next, depth)| next.depth > depth);
        if let Some((old_root, old_len)) = self.motion.explorer_len.get(&sidebar) {
            if *old_root == root && len > *old_len && grew_under && prefs.folds {
                let target = if sidebar { RevealTarget::Sidebar } else { RevealTarget::Explorer };
                self.motion.reveals.retain(|r| r.target != target);
                self.motion.reveals.push(Reveal { target, first: selected + 1, count: len - old_len, anim: Anim::new(now, prefs.fold_d) });
            }
        }
        self.motion.explorer_len.insert(sidebar, (root, len));
    }

    /// Reveals `count` rows of `buffer` from display line `first` -- a
    /// code fold opening.
    pub(super) fn reveal_lines(&mut self, buffer: BufferId, first: usize, count: usize) {
        let prefs = self.motion();
        if prefs.folds && count > 0 {
            self.motion.reveals.retain(|r| r.target != RevealTarget::Buffer(buffer));
            self.motion.reveals.push(Reveal { target: RevealTarget::Buffer(buffer), first, count, anim: Anim::new(Instant::now(), prefs.fold_d) });
        }
    }

    /// How much of a reveal is still covered: the rows from
    /// `first + count * shown` down are hidden.
    pub(super) fn reveal_cover(&self, target: RevealTarget, now: Instant) -> Option<(usize, usize, f32)> {
        let reveal = self.motion.reveals.iter().find(|r| r.target == target && r.anim.running(now))?;
        Some((reveal.first, reveal.count, reveal.anim.eased(now)))
    }

    /// Starts a theme change's fade from `old` -- the next frame copies
    /// the old theme's picture first.
    pub(super) fn fade_theme_from(&mut self, old: &'static Theme) {
        if self.motion().theme_fade && !std::ptr::eq(old, self.theme) && old.name != self.theme.name {
            self.motion.theme_from.get_or_insert(old);
        }
    }

    /// Whether anything is moving -- what keeps frames coming at
    /// animation speed.
    pub(super) fn motion_animating(&self, now: Instant) -> bool {
        let m = &self.motion;
        m.beacon.is_some_and(|b| b.anim.running(now))
            || m.changes.iter().any(|c| c.anim.running(now))
            || m.popups.values().any(|p| p.closing.is_some() || now.saturating_duration_since(p.shown) < Duration::from_millis(400))
            || m.mode.as_ref().and_then(|t| t.anim).is_some_and(|a| a.running(now))
            || m.error_flash.is_some_and(|a| a.running(now))
            || m.tab_accents.values().any(|t| t.anim.is_some_and(|a| a.running(now)))
            || m.scroll_cols.values().any(|t| t.anim.is_some_and(|a| a.running(now)))
            || m.glide.as_ref().is_some_and(|(_, t)| t.anim.is_some_and(|a| a.running(now)))
            || m.layout.values().any(|t| t.anim.is_some_and(|a| a.running(now)))
            || m.reveals.iter().any(|r| r.anim.running(now))
            || m.theme_from.is_some()
            || m.theme_fade.is_some_and(|a| a.running(now))
            || self.splash_fade.is_some()
            || self.message_fading(now)
    }

    fn message_fading(&self, now: Instant) -> bool {
        let prefs = self.motion();
        let Some(message) = &self.status_message else { return false };
        let age = now.saturating_duration_since(message.set_at);
        prefs.messages && (age < prefs.message_in_d || (age < MESSAGE_DURATION && MESSAGE_DURATION - age < prefs.message_out_d))
    }

    /// The next time something needs a frame without anything moving
    /// now: a message about to fade out, or the spinner's next step.
    pub(super) fn motion_next_wake(&self, now: Instant) -> Option<Instant> {
        let prefs = self.motion();
        let mut wake: Option<Instant> = None;
        let mut at = |t: Instant| wake = Some(wake.map_or(t, |w| w.min(t)));
        if let Some(message) = &self.status_message {
            let fade_at = message.set_at + MESSAGE_DURATION.saturating_sub(prefs.message_out_d);
            if prefs.messages && fade_at > now {
                at(fade_at);
            }
            let gone = message.set_at + MESSAGE_DURATION;
            if gone > now {
                at(gone);
            }
        }
        if !self.background_jobs().is_empty() {
            at(now + SPINNER_STEP);
        }
        // A key menu due to open.
        if let Some(due) = self.which_key_wake().filter(|&due| due > now) {
            at(due);
        }
        // Problems held back while typing, due once typing pauses.
        if let Some(due) = self.diagnostics_wake().filter(|&due| due > now) {
            at(due);
        }
        wake
    }

    /// Drops what's finished, so the lists don't grow.
    pub(super) fn prune_motion(&mut self, now: Instant) {
        let m = &mut self.motion;
        if m.beacon.is_some_and(|b| !b.anim.running(now)) {
            m.beacon = None;
        }
        m.changes.retain(|c| c.anim.running(now));
        m.reveals.retain(|r| r.anim.running(now));
        if m.theme_fade.is_some_and(|a| !a.running(now)) {
            m.theme_fade = None;
        }
        if self.splash_fade.as_ref().is_some_and(|f| f.done(now)) {
            self.splash_fade = None;
        }
    }
}

/// How long each blade of the modeline spinner stays lit.
pub(super) const SPINNER_STEP: Duration = Duration::from_millis(350);

/// When the spinner's cycle is counted from.
pub(super) fn spinner_epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Which blade is lit at `now`.
pub(super) fn spinner_blade(now: Instant, since: Instant) -> usize {
    (now.saturating_duration_since(since).as_millis() / SPINNER_STEP.as_millis()) as usize % 3
}

fn plain_or_shift(key: KeyPress) -> bool {
    !key.mods.ctrl && !key.mods.alt && !key.mods.super_
}

fn rect_eq(a: fenix_window::Rect, b: fenix_window::Rect) -> bool {
    (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5 && (a.w - b.w).abs() < 0.5 && (a.h - b.h).abs() < 0.5
}

/// Where a new pane grows from: its own far edge, so the divider slides
/// across towards it.
fn collapsed_from(rect: fenix_window::Rect, area: fenix_window::Rect) -> fenix_window::Rect {
    if rect.x > area.x + 0.5 {
        fenix_window::Rect { x: rect.x + rect.w, w: 0.0, ..rect }
    } else if rect.y > area.y + 0.5 {
        fenix_window::Rect { y: rect.y + rect.h, h: 0.0, ..rect }
    } else if rect.w < area.w - 0.5 {
        fenix_window::Rect { w: 0.0, ..rect }
    } else {
        fenix_window::Rect { h: 0.0, ..rect }
    }
}

/// A key's edits, in order, as what's there now: each later edit moves
/// or swallows what the earlier ones marked.
pub(super) fn settle_changes(deltas: &[fenix_core::EditDelta]) -> Vec<Change> {
    let mut changes: Vec<Change> = Vec::new();
    for d in deltas {
        let removed = d.removed.chars().count();
        let inserted = d.new_end_char.saturating_sub(d.start_char);
        let shift = inserted as isize - removed as isize;
        let map = |p: usize| -> usize {
            if p <= d.start_char {
                p
            } else if p >= d.start_char + removed {
                (p as isize + shift).max(0) as usize
            } else {
                d.start_char
            }
        };
        for change in &mut changes {
            match change {
                Change::Added(range) => *range = map(range.start)..map(range.end).max(map(range.start)),
                Change::Removed { at, .. } => *at = map(*at),
            }
        }
        if inserted > 0 {
            changes.push(Change::Added(d.start_char..d.new_end_char));
        } else if removed > 0 {
            changes.push(Change::Removed { at: d.start_char, lines: d.removed.contains('\n') });
        }
    }
    changes.retain(|c| !matches!(c, Change::Added(r) if r.is_empty()));
    // Neighbouring additions read as one.
    changes.sort_by_key(|c| match c {
        Change::Added(r) => r.start,
        Change::Removed { at, .. } => *at,
    });
    let mut merged: Vec<Change> = Vec::new();
    for change in changes {
        if let (Some(Change::Added(last)), Change::Added(next)) = (merged.last_mut(), &change) {
            if next.start <= last.end {
                last.end = last.end.max(next.end);
                continue;
            }
        }
        merged.push(change);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_core::EditDelta;

    fn delta(start: usize, new_end: usize, removed: &str) -> EditDelta {
        EditDelta { start_char: start, new_end_char: new_end, removed: removed.to_string() }
    }

    #[test]
    fn an_insertion_is_what_it_added() {
        let changes = settle_changes(&[delta(4, 9, "")]);
        assert!(matches!(changes.as_slice(), [Change::Added(r)] if *r == (4..9)));
    }

    #[test]
    fn a_deleted_line_is_a_seam_where_it_was() {
        let changes = settle_changes(&[delta(10, 10, "gone\n")]);
        assert!(matches!(changes.as_slice(), [Change::Removed { at: 10, lines: true }]));
    }

    #[test]
    fn later_edits_move_what_earlier_ones_marked() {
        // Add "abc" at 20, then delete 5 chars at 0: the addition moves to 15.
        let changes = settle_changes(&[delta(20, 23, ""), delta(0, 0, "12345")]);
        assert!(changes.iter().any(|c| matches!(c, Change::Added(r) if *r == (15..18))), "{changes:?}");
        assert!(changes.iter().any(|c| matches!(c, Change::Removed { at: 0, lines: false })));
    }

    #[test]
    fn neighbouring_additions_merge() {
        let changes = settle_changes(&[delta(0, 3, ""), delta(3, 6, "")]);
        assert!(matches!(changes.as_slice(), [Change::Added(r)] if *r == (0..6)));
    }

    fn motion_app() -> App {
        let mut app = App::with_file(None);
        app.config.motion.level = Some("full".into());
        for ch in "one
two
three
four
five
six
".chars() {
            app.test_insert(ch);
        }
        app.test_set_cursor(Cursor::at_start());
        app
    }

    #[test]
    fn a_jump_flashes_the_line_it_lands_on() {
        let mut app = motion_app();
        let before = app.motion_before_key();
        let from = JumpEntry { buffer: app.focused_buffer_id(), char_idx: 0 };
        app.test_vim_key(KeyPress::char('G'));
        app.record_jump(from);
        app.motion_after_key(before, KeyPress::char('G'));
        let beacon = app.motion.beacon.expect("a jump starts the beacon");
        assert_eq!(beacon.line, app.open().buffer.line_col(&app.cursor()).0);
        assert!(beacon.anim.running(beacon.anim.started));
    }

    #[test]
    fn plain_movement_doesnt_flash() {
        let mut app = motion_app();
        let before = app.motion_before_key();
        app.test_vim_key(KeyPress::char('j'));
        app.motion_after_key(before, KeyPress::char('j'));
        assert!(app.motion.beacon.is_none());
    }

    #[test]
    fn the_beacon_is_off_when_turned_off() {
        let mut app = motion_app();
        app.config.motion.beacon = Some(false);
        let before = app.motion_before_key();
        let from = JumpEntry { buffer: app.focused_buffer_id(), char_idx: 0 };
        app.test_vim_key(KeyPress::char('G'));
        app.record_jump(from);
        app.motion_after_key(before, KeyPress::char('G'));
        assert!(app.motion.beacon.is_none());
    }

    #[test]
    fn undo_pulses_what_it_brought_back() {
        let mut app = motion_app();
        app.test_dispatch_key(KeyPress::char('d'));
        app.test_dispatch_key(KeyPress::char('d'));
        let before = app.motion_before_key();
        app.test_dispatch_key(KeyPress::char('u'));
        app.motion_after_key(before, KeyPress::char('u'));
        let added = app.motion.changes.iter().find_map(|c| match &c.change {
            Change::Added(range) => Some(range.clone()),
            _ => None,
        });
        assert_eq!(added, Some(0..4), "the restored \"one\n\" flashes: {:?}", app.motion.changes);
    }

    #[test]
    fn a_popup_fades_in_and_out() {
        let mut state = MotionState::default();
        let mut c = fenix_config::Config::empty("s.toml".into(), "state".into());
        c.motion.level = Some("subtle".into());
        let prefs = Prefs::from_config(&c);
        let rect = fenix_window::Rect { x: 10.0, y: 10.0, w: 100.0, h: 40.0 };
        let now = Instant::now();
        let open = |now| vec![(popup::PopupId::Hover, rect, vec![("x".to_string(), glyphon::Color::rgb(1, 1, 1), false)], None)];
        let first = state.popup_draws(prefs, open(now), now);
        assert!(first[0].alpha < 0.1 && first[0].rect.y > rect.y, "starts faint and low");
        let later = state.popup_draws(prefs, open(now), now + Duration::from_millis(500));
        assert_eq!((later[0].alpha, later[0].rect.y), (1.0, rect.y));
        let closing = state.popup_draws(prefs, Vec::new(), now + Duration::from_millis(510));
        assert_eq!(closing.len(), 1, "still drawn while it fades out");
        let gone = state.popup_draws(prefs, Vec::new(), now + Duration::from_millis(1000));
        assert!(gone.is_empty());
    }

    #[test]
    fn the_which_key_drawer_slides_up_and_back_down() {
        let mut state = MotionState::default();
        let mut c = fenix_config::Config::empty("s.toml".into(), "state".into());
        c.motion.level = Some("subtle".into());
        let prefs = Prefs::from_config(&c);
        let rect = fenix_window::Rect { x: 0.0, y: 600.0, w: 1000.0, h: 120.0 };
        let now = Instant::now();
        let open = || vec![(popup::PopupId::WhichKey, rect, Vec::new(), None)];
        let first = state.popup_draws(prefs, open(), now);
        assert_eq!(first[0].alpha, 1.0, "opaque while it slides");
        assert!((first[0].rect.y - (rect.y + rect.h)).abs() < 0.01, "starts fully below its place");
        let settled = state.popup_draws(prefs, open(), now + Duration::from_secs(1));
        assert_eq!(settled[0].rect.y, rect.y);
        let closed_at = now + Duration::from_secs(1);
        state.popup_draws(prefs, Vec::new(), closed_at);
        let closing = state.popup_draws(prefs, Vec::new(), closed_at + Duration::from_millis(40));
        assert!(closing[0].rect.y > rect.y, "slides back down");
    }

    #[test]
    fn a_split_moves_the_divider_instead_of_jumping() {
        let mut app = App::with_file(None);
        app.config.motion.layout = Some(true);
        let area = fenix_window::Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let a = app.focused_pane_id();
        let now = Instant::now();
        app.motion_layout(vec![(a, area)], area, now);
        let buffer = app.focused_buffer_id();
        let b = app.windows_mut().split(SplitKind::Vertical, buffer);
        let left = fenix_window::Rect { w: 100.0, ..area };
        let right = fenix_window::Rect { x: 100.0, w: 100.0, ..area };
        let shown = app.motion_layout(vec![(a, left), (b, right)], area, now);
        assert_eq!(shown[0].1.w, 200.0, "the old pane starts at its old size");
        assert_eq!(shown[1].1.w, 0.0, "the new one starts closed at its far edge");
        let settled = app.motion_layout(vec![(a, left), (b, right)], area, now + Duration::from_secs(1));
        assert_eq!((settled[0].1.w, settled[1].1.w), (100.0, 100.0));
    }

    #[test]
    fn a_window_resize_doesnt_animate_the_panes() {
        let mut app = App::with_file(None);
        app.config.motion.layout = Some(true);
        let a = app.focused_pane_id();
        let now = Instant::now();
        let small = fenix_window::Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let big = fenix_window::Rect { w: 400.0, ..small };
        app.motion_layout(vec![(a, small)], small, now);
        let shown = app.motion_layout(vec![(a, big)], big, now);
        assert_eq!(shown[0].1.w, 400.0);
    }

    #[test]
    fn the_caret_glides_after_a_motion_and_not_while_typing() {
        let mut state = MotionState::default();
        let mut c = fenix_config::Config::empty("s.toml".into(), "state".into());
        c.motion.level = Some("full".into());
        let prefs = Prefs::from_config(&c);
        let app = App::with_file(None);
        let pane = (0, 0, app.focused_pane_id());
        let buffer = app.focused_buffer_id();
        let spot = |col: f32| CaretSpot { pane, buffer, row: 0.0, col };
        let now = Instant::now();
        state.caret_spot(prefs, spot(0.0), 40, now);
        state.glide_armed = true;
        let (_, col) = state.caret_spot(prefs, spot(10.0), 40, now);
        assert_eq!(col, 0.0, "starts from where it was");
        let (_, col) = state.caret_spot(prefs, spot(10.0), 40, now + Duration::from_secs(1));
        assert_eq!(col, 10.0);
        // Not armed (typing): straight there.
        let (_, col) = state.caret_spot(prefs, spot(20.0), 40, now + Duration::from_secs(1));
        assert_eq!(col, 20.0);
    }

    #[test]
    fn a_new_pane_grows_from_its_far_edge() {
        let area = fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        let right = fenix_window::Rect { x: 50.0, y: 0.0, w: 50.0, h: 50.0 };
        let from = collapsed_from(right, area);
        assert_eq!((from.x, from.w), (100.0, 0.0));
        let below = fenix_window::Rect { x: 0.0, y: 25.0, w: 100.0, h: 25.0 };
        let from = collapsed_from(below, area);
        assert_eq!((from.y, from.h), (50.0, 0.0));
    }
}
