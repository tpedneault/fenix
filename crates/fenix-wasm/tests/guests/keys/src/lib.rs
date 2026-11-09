//! API 1.5's key scheme, as an extension uses it: what `key-for` and
//! `editor-keys` answer, said in the log, and `key-scheme-changed` heard.

use fenix_extension_api::{export, info, key_for, key_scheme, Extension, KeyScheme};

#[derive(Default)]
struct Keys;

fn name(scheme: KeyScheme) -> &'static str {
    match scheme {
        KeyScheme::Vim => "vim",
        KeyScheme::Standard => "standard",
    }
}

impl Extension for Keys {
    fn activate(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn run_command(&mut self, id: &str) -> Result<(), String> {
        match id {
            "keys.say" => {
                info!("scheme {} save {} nothing {}", name(key_scheme()), key_for("file.save").unwrap_or_default(), key_for("nope.nothing").is_none());
                Ok(())
            }
            _ => Err(format!("no command {id}")),
        }
    }

    fn key_scheme_changed(&mut self, scheme: KeyScheme) {
        info!("changed to {}", name(scheme));
    }
}

export!(Keys);
