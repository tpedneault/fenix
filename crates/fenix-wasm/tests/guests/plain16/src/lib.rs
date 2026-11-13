//! An extension rebuilt on the 1.6 API crate that uses nothing 1.6 added:
//! no test provider, `api = "1.5"` in its manifest. The crate exports the
//! five `tests-*` functions all the same, with their defaults; a Fenix 1.5
//! must still start it (tests/f15.rs), and this one treats it as a 1.5
//! extension.

use fenix_extension_api::{export, info, Extension};

#[derive(Default)]
struct Plain {
    said: u32,
}

impl Extension for Plain {
    fn activate(&mut self) -> Result<(), String> {
        info!("plain16 activated");
        Ok(())
    }

    fn run_command(&mut self, id: &str) -> Result<(), String> {
        match id {
            "plain.say" => {
                self.said += 1;
                info!("plain16 said {}", self.said);
                Ok(())
            }
            _ => Err(format!("no command {id}")),
        }
    }
}

export!(Plain);
