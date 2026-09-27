//! The launch splash: the Forged mark assembling blade by blade, the
//! wordmark, and a hairline that fills as startup gets further along.
//! It runs on a thread of its own, drawing into the window's surface
//! while the main thread loads the session, the fonts and everything
//! else -- so the window is up and visibly alive from the first moment,
//! however long that takes. When the editor is ready it hands the
//! surface back (`SplashHandle::finish`) and the editor's first frames
//! fade the splash out on top of themselves (`SplashScene::draw` with
//! `cover`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use winit::window::Window;

use crate::motion::{ease_out_cubic, lerp};
use crate::sprite::{Sprite, SpriteRenderer};

/// How long the editor takes to fade the splash out once it's ready.
pub const FADE_OUT: Duration = Duration::from_millis(220);

/// When the blades start breathing, after assembling.
const BREATHE_AFTER: f32 = 0.8;
const BREATHE_PERIOD: f32 = 1.2;

/// The splash's images, and how they're laid out and moved.
pub struct SplashScene {
    blades: [Sprite; 3],
    word: Sprite,
    white: Sprite,
    /// The mark's box, in pixels.
    mark: f32,
    bg: [f32; 4],
    fg: [f32; 4],
}

impl SplashScene {
    /// Draws the images for a window `height` pixels tall, on `bg`, with
    /// the wordmark and hairline in `fg`.
    pub fn new(sprites: &SpriteRenderer, height: u32, bg: [f32; 4], fg: [f32; 4]) -> Self {
        let mark = (height as f32 * 0.16).clamp(64.0, 176.0).round();
        let size = mark as u32;
        let blade = |i| {
            let image = fenix_brand::blade(i, size);
            sprites.upload(image.width, image.height, &image.rgba)
        };
        let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        let word = fenix_brand::wordmark((mark * 0.2).round().max(6.0) as u32, [to_u8(fg[0]), to_u8(fg[1]), to_u8(fg[2])]);
        SplashScene {
            blades: [blade(0), blade(1), blade(2)],
            word: sprites.upload(word.width, word.height, &word.rgba),
            white: sprites.white(),
            mark,
            bg,
            fg,
        }
    }

    /// Queues the splash as it looks `t` seconds into launch, with
    /// startup `progress` (0 to 1) along, everything at `alpha`. With
    /// `cover`, it first covers the window in its background: that's
    /// how the editor fades it out over its own first frames.
    pub fn draw(&self, sprites: &mut SpriteRenderer, width: u32, height: u32, t: f32, progress: f32, alpha: f32, cover: bool) {
        let (w, h) = (width as f32, height as f32);
        if cover {
            let [r, g, b, _] = self.bg;
            sprites.push(&self.white, 0.0, 0.0, w, h, [r, g, b, alpha]);
        }
        let s = self.mark;
        let x = ((w - s) / 2.0).round();
        let y = ((h - s) / 2.0 - s * 0.35).round();
        let breathe = ((t - BREATHE_AFTER) / 0.3).clamp(0.0, 1.0);
        for (i, sprite) in self.blades.iter().enumerate() {
            let appear = ease_out_cubic((t - 0.1 * i as f32) / 0.2);
            let travel = (1.0 - appear) * s * 0.14;
            let (dx, dy) = if i == 0 { (0.0, travel) } else { (-travel, 0.0) };
            // After assembling, a slow wave passes through the blades in
            // turn: alive, never distracting.
            let phase = (t - BREATHE_AFTER - i as f32 * 0.2) / BREATHE_PERIOD * std::f32::consts::TAU;
            let wave = 0.5 + 0.5 * phase.cos();
            let level = lerp(1.0, 0.45 + 0.55 * wave, breathe);
            sprites.push(sprite, x + dx, y + dy, s, s, [1.0, 1.0, 1.0, alpha * appear * level]);
        }
        let word_alpha = ((t - 0.3) / 0.2).clamp(0.0, 1.0);
        let (ww, wh) = (self.word.width as f32, self.word.height as f32);
        let word_y = (y + s + s * 0.16).round();
        sprites.push(&self.word, ((w - ww) / 2.0).round(), word_y, ww, wh, [1.0, 1.0, 1.0, alpha * word_alpha]);
        // The hairline: a faint track, filled in ember as startup moves.
        let bar_w = (s * 1.3).round();
        let bar_x = ((w - bar_w) / 2.0).round();
        let bar_y = (word_y + wh + s * 0.2).round();
        let [r, g, b, _] = self.fg;
        sprites.push(&self.white, bar_x, bar_y, bar_w, 2.0, [r, g, b, alpha * word_alpha * 0.14]);
        let [er, eg, eb] = fenix_brand::EMBER.map(|c| c as f32 / 255.0);
        let fill = (bar_w * progress.clamp(0.0, 1.0)).round();
        sprites.push(&self.white, bar_x, bar_y, fill, 2.0, [er, eg, eb, alpha * word_alpha]);
    }
}

/// What the splash thread hands back when the editor is ready.
pub struct SplashReturn {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub sprites: SpriteRenderer,
    pub scene: SplashScene,
    pub started: Instant,
    pub progress: f32,
    /// Drawn without motion: animations are off.
    pub still: bool,
}

/// The running splash.
pub struct SplashHandle {
    stop: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    first_frame: Receiver<()>,
    thread: JoinHandle<Option<SplashReturn>>,
}

impl SplashHandle {
    /// Starts drawing into `surface` on a thread of its own.
    pub fn start(
        window: Arc<Window>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        bg: [f32; 4],
        fg: [f32; 4],
        still: bool,
    ) -> std::io::Result<SplashHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0));
        let (first_tx, first_frame) = mpsc::channel();
        let (thread_stop, thread_progress) = (Arc::clone(&stop), Arc::clone(&progress));
        let thread = std::thread::Builder::new().name("fenix-splash".into()).spawn(move || {
            let started = Instant::now();
            let mut sprites = SpriteRenderer::new(Arc::clone(&device), Arc::clone(&queue), config.format);
            let scene = SplashScene::new(&sprites, config.height, bg, fg);
            let mut config = config;
            let mut shown = 0.0f32;
            let mut first_tx = Some(first_tx);
            let (mut reported, mut reported_at) = (0.0f32, Instant::now());
            loop {
                let target = f32::from_bits(thread_progress.load(Ordering::Acquire));
                if target != reported {
                    (reported, reported_at) = (target, Instant::now());
                }
                // A long stage reports nothing until it's done, so the
                // hairline creeps on towards the next stage meanwhile --
                // never reaching it -- and eases rather than jumps.
                let waited = reported_at.elapsed().as_secs_f32();
                let goal = target + ((target + 0.4).min(0.95) - target).max(0.0) * (1.0 - (-waited / 2.0).exp());
                shown = shown.max(shown + (goal - shown) * 0.12);
                if thread_stop.load(Ordering::Acquire) {
                    return Some(SplashReturn { surface, config, sprites, scene, started, progress: shown, still });
                }
                let size = window.inner_size();
                if size.width > 0 && size.height > 0 && (size.width, size.height) != (config.width, config.height) {
                    config.width = size.width;
                    config.height = size.height;
                    surface.configure(&device, &config);
                }
                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        surface.configure(&device, &config);
                        continue;
                    }
                    _ => {
                        std::thread::sleep(Duration::from_millis(16));
                        continue;
                    }
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                sprites.begin(config.width, config.height);
                // Held at the moment the mark has assembled when nothing
                // should move; only the hairline goes on filling.
                let t = if still { BREATHE_AFTER } else { started.elapsed().as_secs_f32() };
                scene.draw(&mut sprites, config.width, config.height, t, shown, 1.0, false);
                sprites.flush();
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("splash") });
                {
                    let [r, g, b, a] = bg;
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("splash-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    sprites.render(&mut pass);
                }
                queue.submit(Some(encoder.finish()));
                window.pre_present_notify();
                queue.present(frame);
                if let Some(tx) = first_tx.take() {
                    let _ = tx.send(());
                }
            }
        })?;
        Ok(SplashHandle { stop, progress, first_frame, thread })
    }

    /// Waits (a little) for the first frame, so the window can be shown
    /// with the splash already in it rather than a blank.
    pub fn wait_first_frame(&self, timeout: Duration) {
        let _ = self.first_frame.recv_timeout(timeout);
    }

    /// How far along startup is, 0 to 1.
    pub fn set_progress(&self, progress: f32) {
        self.progress.store(progress.clamp(0.0, 1.0).to_bits(), Ordering::Release);
    }

    /// Stops drawing and takes the surface back.
    pub fn finish(self) -> Option<SplashReturn> {
        self.stop.store(true, Ordering::Release);
        self.thread.join().ok().flatten()
    }
}

/// The splash still showing over the editor's first frames, fading out.
pub struct SplashFade {
    pub sprites: SpriteRenderer,
    pub scene: SplashScene,
    pub started: Instant,
    pub progress: f32,
    pub fade_from: Instant,
    /// Animations are off: the splash goes at once instead of fading.
    pub still: bool,
}

impl SplashFade {
    /// The splash's opacity at `now`: 1 when the fade starts, 0 at its end.
    pub fn alpha(&self, now: Instant) -> f32 {
        if self.still {
            return 0.0;
        }
        let t = now.saturating_duration_since(self.fade_from).as_secs_f32() / FADE_OUT.as_secs_f32();
        1.0 - ease_out_cubic(t)
    }

    pub fn done(&self, now: Instant) -> bool {
        self.still || now.saturating_duration_since(self.fade_from) >= FADE_OUT
    }
}
