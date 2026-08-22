use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use fenix_core::{Buffer, Cursor};

use crate::commands::CommandRegistry;
use crate::gpu::GpuState;
use crate::rect::RectRenderer;
use crate::text::{self, TextPipeline};
use crate::theme::{self, Theme};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// Smallest adjustment to `scroll_line` that brings `cursor_line` into the
/// `[scroll_line, scroll_line + visible_lines)` window.
fn scroll_to_include(scroll_line: usize, cursor_line: usize, visible_lines: usize) -> usize {
    if cursor_line < scroll_line {
        cursor_line
    } else if cursor_line >= scroll_line + visible_lines {
        cursor_line + 1 - visible_lines
    } else {
        scroll_line
    }
}

pub struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    text: Option<TextPipeline>,
    rect: Option<RectRenderer>,

    buffer: Buffer,
    cursor: Cursor,
    /// Index of the topmost buffer line currently on screen.
    scroll_line: usize,

    theme: &'static Theme,

    modifiers: ModifiersState,
    blink_on: bool,
    next_blink: Instant,
}

impl App {
    pub fn new() -> Self {
        Self::with_file(env::args().nth(1))
    }

    fn with_file(file_arg: Option<String>) -> Self {
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
            scroll_line: 0,
            theme: &theme::ORBIT_DARK,
            modifiers: ModifiersState::empty(),
            blink_on: true,
            next_blink: Instant::now() + BLINK_INTERVAL,
        }
    }

    pub(crate) fn save(&mut self) {
        if self.buffer.path().is_none() {
            eprintln!("fenix: no file path to save to yet; pass a file path as the first argument");
            return;
        }
        match self.buffer.save() {
            Ok(()) => println!("fenix: saved {:?}", self.buffer.path().unwrap()),
            Err(err) => eprintln!("fenix: save failed: {err}"),
        }
        self.wake_caret();
    }

    pub(crate) fn undo(&mut self) {
        self.buffer.undo(&mut self.cursor);
        self.wake_caret();
    }

    pub(crate) fn redo(&mut self) {
        self.buffer.redo(&mut self.cursor);
        self.wake_caret();
    }

    /// Resets the caret blink timer so an edit or navigation always leaves
    /// the caret visible instead of possibly mid-blink.
    fn wake_caret(&mut self) {
        self.blink_on = true;
        self.next_blink = Instant::now() + BLINK_INTERVAL;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() {
            if let Key::Character(s) = &event.logical_key {
                let id = if s.eq_ignore_ascii_case("s") {
                    Some("file.save")
                } else if s.eq_ignore_ascii_case("z") && self.modifiers.shift_key() {
                    Some("edit.redo")
                } else if s.eq_ignore_ascii_case("z") {
                    Some("edit.undo")
                } else if s.eq_ignore_ascii_case("y") {
                    Some("edit.redo")
                } else if s.eq_ignore_ascii_case("q") {
                    Some("app.quit")
                } else {
                    None
                };
                if let Some(id) = id {
                    // Built fresh per chord rather than stored on App: it's
                    // just a few fn-pointer entries, and this sidesteps
                    // borrowing `self` both as receiver and argument.
                    CommandRegistry::with_builtins().run(self, event_loop, id);
                }
            }
            return;
        }

        let page_size = self
            .gpu
            .as_ref()
            .map(|gpu| text::visible_line_count(gpu.size.height as f32))
            .unwrap_or(20);

        match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => self.buffer.move_left(&mut self.cursor),
            Key::Named(NamedKey::ArrowRight) => self.buffer.move_right(&mut self.cursor),
            Key::Named(NamedKey::ArrowUp) => self.buffer.move_up(&mut self.cursor),
            Key::Named(NamedKey::ArrowDown) => self.buffer.move_down(&mut self.cursor),
            Key::Named(NamedKey::Home) => self.buffer.move_home(&mut self.cursor),
            Key::Named(NamedKey::End) => self.buffer.move_end(&mut self.cursor),
            Key::Named(NamedKey::PageUp) => self.buffer.move_page(&mut self.cursor, page_size, false),
            Key::Named(NamedKey::PageDown) => self.buffer.move_page(&mut self.cursor, page_size, true),
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

        self.wake_caret();
    }

    /// Keeps the cursor's line within the visible window, scrolling as
    /// needed. Must be called with the same `visible_lines` used to render.
    fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        let (line, _) = self.buffer.line_col(&self.cursor);
        self.scroll_line = scroll_to_include(self.scroll_line, line, visible_lines);
    }

    fn modeline_text(&self) -> String {
        let filename = self
            .buffer
            .path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[No Name]".to_string());
        let modified = if self.buffer.is_dirty() { " [+]" } else { "" };
        let (line, col) = self.buffer.line_col(&self.cursor);
        format!(" {filename}{modified}   TEXT   Ln {}, Col {} ", line + 1, col + 1)
    }

    fn redraw(&mut self) {
        let Some(window_height) = self.gpu.as_ref().map(|gpu| gpu.size.height as f32) else {
            return;
        };
        let visible_lines = text::visible_line_count(window_height);
        self.ensure_cursor_visible(visible_lines);

        let content_text = self.buffer.visible_text(self.scroll_line, visible_lines);
        let modeline_text = self.modeline_text();
        let (line, col) = self.buffer.line_col(&self.cursor);
        let caret_row_in_view = line - self.scroll_line;
        let blink_on = self.blink_on;
        let theme = self.theme;

        let (Some(window), Some(gpu), Some(text), Some(rect)) =
            (&self.window, &mut self.gpu, &mut self.text, &mut self.rect)
        else {
            return;
        };

        text.set_text(&content_text);
        text.set_modeline_text(&modeline_text);

        rect.clear();
        rect.push_rect(
            gpu,
            0.0,
            gpu.size.height as f32 - text::MODELINE_HEIGHT,
            gpu.size.width as f32,
            text::MODELINE_HEIGHT,
            theme.bg_modeline,
        );
        if blink_on {
            let caret_x = text::PAD_LEFT + col as f32 * text::CHAR_WIDTH;
            let caret_y = text::PAD_TOP + caret_row_in_view as f32 * text::LINE_HEIGHT;
            rect.push_rect(gpu, caret_x, caret_y, 2.0, text::LINE_HEIGHT, theme.caret);
        }
        rect.flush(gpu);

        text.prepare(gpu, theme);

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
            let [r, g, b, a] = theme.bg;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: a as f64,
                        }),
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
        text.set_text(&self.buffer.visible_text(0, text::visible_line_count(gpu.size.height as f32)));
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
                self.handle_key(&event, event_loop);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_stays_put_when_cursor_already_visible() {
        assert_eq!(scroll_to_include(5, 7, 10), 5);
    }

    #[test]
    fn scroll_jumps_up_when_cursor_is_above_the_window() {
        assert_eq!(scroll_to_include(10, 3, 10), 3);
    }

    #[test]
    fn scroll_advances_when_cursor_is_below_the_window() {
        // window [10, 20); cursor at line 20 is the first line below it
        assert_eq!(scroll_to_include(10, 20, 10), 11);
    }

    #[test]
    fn single_visible_line_tracks_cursor_exactly() {
        assert_eq!(scroll_to_include(0, 42, 1), 42);
    }

    #[test]
    fn app_auto_scrolls_to_keep_cursor_in_view() {
        let mut app = App::with_file(None);
        for _ in 0..30 {
            app.buffer.insert_char(&mut app.cursor, '\n');
        }
        // cursor is now on line 30; a 10-line viewport starting at 0 doesn't include it
        app.ensure_cursor_visible(10);
        assert_eq!(app.scroll_line, 21);
    }

    #[test]
    fn modeline_reflects_filename_dirty_state_and_position() {
        let mut app = App::with_file(None);
        assert_eq!(app.modeline_text(), " [No Name]   TEXT   Ln 1, Col 1 ");

        app.buffer.insert_char(&mut app.cursor, 'a');
        app.buffer.insert_char(&mut app.cursor, 'b');
        assert_eq!(app.modeline_text(), " [No Name] [+]   TEXT   Ln 1, Col 3 ");
    }
}
