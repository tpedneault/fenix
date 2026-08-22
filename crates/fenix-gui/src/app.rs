use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use fenix_core::{Buffer, Cursor};

use crate::gpu::GpuState;
use crate::rect::RectRenderer;
use crate::text::{self, TextPipeline};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);

pub struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    text: Option<TextPipeline>,
    rect: Option<RectRenderer>,

    buffer: Buffer,
    cursor: Cursor,

    modifiers: ModifiersState,
    blink_on: bool,
    next_blink: Instant,
}

impl App {
    pub fn new() -> Self {
        let file_arg = env::args().nth(1);
        let buffer = match file_arg.as_deref() {
            Some(path) => Buffer::from_path(path).unwrap_or_else(|err| {
                eprintln!("fenix: couldn't open {path} ({err}), starting empty buffer");
                Buffer::empty()
            }),
            None => Buffer::empty(),
        };

        Self {
            window: None,
            gpu: None,
            text: None,
            rect: None,
            buffer,
            cursor: Cursor::at_start(),
            modifiers: ModifiersState::empty(),
            blink_on: true,
            next_blink: Instant::now() + BLINK_INTERVAL,
        }
    }

    fn save(&mut self) {
        if self.buffer.path().is_none() {
            eprintln!("fenix: no file path to save to yet; pass a file path as the first argument");
            return;
        }
        match self.buffer.save() {
            Ok(()) => println!("fenix: saved {:?}", self.buffer.path().unwrap()),
            Err(err) => eprintln!("fenix: save failed: {err}"),
        }
    }

    fn handle_key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() {
            if let Key::Character(s) = &event.logical_key {
                if s.eq_ignore_ascii_case("s") {
                    self.save();
                }
            }
            return;
        }

        match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => self.buffer.move_left(&mut self.cursor),
            Key::Named(NamedKey::ArrowRight) => self.buffer.move_right(&mut self.cursor),
            Key::Named(NamedKey::ArrowUp) => self.buffer.move_up(&mut self.cursor),
            Key::Named(NamedKey::ArrowDown) => self.buffer.move_down(&mut self.cursor),
            Key::Named(NamedKey::Home) => self.buffer.move_home(&mut self.cursor),
            Key::Named(NamedKey::End) => self.buffer.move_end(&mut self.cursor),
            Key::Named(NamedKey::PageUp) => self.buffer.move_page(&mut self.cursor, 20, false),
            Key::Named(NamedKey::PageDown) => self.buffer.move_page(&mut self.cursor, 20, true),
            Key::Named(NamedKey::Backspace) => self.buffer.delete_backward(&mut self.cursor),
            Key::Named(NamedKey::Delete) => self.buffer.delete_forward(&mut self.cursor),
            Key::Named(NamedKey::Enter) => self.buffer.insert_char(&mut self.cursor, '\n'),
            Key::Named(NamedKey::Tab) => self.buffer.insert_char(&mut self.cursor, '\t'),
            _ => {
                if let Some(text) = &event.text {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        self.buffer.insert_char(&mut self.cursor, ch);
                    }
                }
            }
        }

        self.blink_on = true;
        self.next_blink = Instant::now() + BLINK_INTERVAL;
    }

    fn redraw(&mut self) {
        let (Some(window), Some(gpu), Some(text), Some(rect)) =
            (&self.window, &mut self.gpu, &mut self.text, &mut self.rect)
        else {
            return;
        };

        text.set_text(&self.buffer.text());

        if self.blink_on {
            let (line, col) = self.buffer.line_col(&self.cursor);
            let caret_x = text::PAD_LEFT + col as f32 * text::CHAR_WIDTH;
            let caret_y = text::PAD_TOP + line as f32 * text::LINE_HEIGHT;
            rect.set_rect(gpu, caret_x, caret_y, 2.0, text::LINE_HEIGHT, [0.88, 0.69, 0.41, 1.0]);
        } else {
            rect.clear();
        }

        text.prepare(gpu);

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            status @ (wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation) => {
                eprintln!("fenix: surface acquire failed: {status:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.098, g: 0.106, b: 0.149, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            text.render(&mut pass);
            rect.render(&mut pass);
        }
        gpu.queue.submit(Some(encoder.finish()));
        window.pre_present_notify();
        gpu.queue.present(frame);
        text.trim();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes().with_title("Fenix");
        let window =
            Arc::new(event_loop.create_window(attrs).expect("failed to create window"));

        let gpu = pollster::block_on(GpuState::new(window.clone()));
        let mut text = TextPipeline::new(&gpu);
        text.set_text(&self.buffer.text());
        let rect = RectRenderer::new(&gpu);

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.text = Some(text);
        self.rect = Some(rect);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                if let Some(text) = &mut self.text {
                    text.resize(size.width as f32, size.height as f32);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(&event);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_blink {
            self.blink_on = !self.blink_on;
            self.next_blink = now + BLINK_INTERVAL;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(self.next_blink));
    }
}
