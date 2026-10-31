//! {{name}}: an extension for Fenix.

use fenix_extension_api::{export, info, notify, page, Extension, Key, Page};

#[derive(Default)]
struct {{name_pascal}} {
    count: u32,
}

impl Extension for {{name_pascal}} {
    fn activate(&mut self) -> Result<(), String> {
        info!("{{name}} is up");
        Ok(())
    }

    fn run_command(&mut self, id: &str) -> Result<(), String> {
        match id {
            "{{name_kebab}}.hello" => {
                notify("Hello from {{name}}");
                Ok(())
            }
            "{{name_kebab}}.open" => {
                page::open("main", "{{name}}");
                Ok(())
            }
            _ => Err(format!("no command {id}")),
        }
    }

    fn page(&mut self, _page: u64, width: u32, _height: u32) -> Page {
        Page::new()
            .line([page::title("{{name}}"), page::muted(format!("  {width} cells wide"))])
            .rule()
            .line([page::text("Pressed j "), page::accent(self.count.to_string()), page::text(" times")])
            .focus(2)
            .key("j", "count")
    }

    fn page_key(&mut self, page: u64, key: Key) {
        if key == Key::Char('j') {
            self.count += 1;
            page::refresh(page);
        }
    }
}

export!({{name_pascal}});
