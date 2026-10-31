//! A test extension written the way an author would, with
//! `fenix-extension-api`: its state is a struct, its page is built.

use fenix_extension_api::{export, host, info, page, surface, Event, Extension, Key, Page, SurfaceInput, SurfaceOptions};

#[derive(Default)]
struct Counter {
    count: u32,
    timers: u32,
    /// Its surface, and the size it was last told.
    surface: Option<(u64, u32, u32)>,
}

impl Counter {
    /// Its surface drawn: as many pixels lit, left to right, as it counted.
    fn draw(&self) {
        let Some((id, width, height)) = self.surface else { return };
        let mut frame = surface::Frame::new(width.min(64), height.min(64));
        frame.fill(0, 0, self.count, 1, (0, 255, 0));
        if let Err(e) = frame.show(id) {
            info!("couldn't draw: {e}");
        }
    }
}

impl Extension for Counter {
    fn activate(&mut self) -> Result<(), String> {
        info!("counter is up");
        Ok(())
    }

    fn run_command(&mut self, id: &str) -> Result<(), String> {
        match id {
            "counter.open" => {
                info!("page {}", page::open("counter", "Counter"));
                Ok(())
            }
            "counter.tick" => {
                host::set_timer(20, false);
                Ok(())
            }
            "counter.surface" => {
                let id = surface::open("counter", "Counter", &SurfaceOptions::new().beside(0.3))?;
                info!("surface {id}");
                Ok(())
            }
            _ => Err(format!("no command {id}")),
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Timer(_) => {
                self.timers += 1;
                info!("ticked {}", self.timers);
            }
            Event::SurfaceResized(size) => {
                self.surface = Some((size.surface, size.width, size.height));
                self.draw();
                info!("drawn {}x{}", size.width.min(64), size.height.min(64));
            }
            _ => {}
        }
    }

    fn surface_input(&mut self, _surface: u64, input: SurfaceInput) {
        if let SurfaceInput::Key(key) = input {
            if key.char() == Some('j') && key.down {
                self.count += 1;
                self.draw();
                info!("counted {} on the surface", self.count);
            }
        }
    }

    fn page(&mut self, _page: u64, width: u32, _height: u32) -> Page {
        Page::new()
            .line([page::title("Counter"), page::muted(format!("  {width} wide"))])
            .rule()
            .line([page::text("count "), page::good(self.count.to_string())])
            .focus(2)
            .key("j", "count")
    }

    fn page_key(&mut self, page: u64, key: Key) {
        if key == Key::Char('j') {
            self.count += 1;
            page::refresh(page);
        }
    }

    fn page_state(&mut self, _page: u64) -> Option<String> {
        Some(self.count.to_string())
    }

    fn page_restore(&mut self, _page: u64, kind: &str, state: &str) -> bool {
        match (kind, state.parse()) {
            ("counter", Ok(n)) => {
                self.count = n;
                true
            }
            _ => false,
        }
    }

    fn serve(&mut self, _call: u64, name: &str, _payload: &str) -> Option<Result<String, String>> {
        match name {
            "counter.count" => Some(Ok(self.count.to_string())),
            _ => Some(Err(format!("no service {name}"))),
        }
    }
}

export!(Counter);
