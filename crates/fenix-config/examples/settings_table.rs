//! Prints the settings table docs/CONFIGURATION.md carries, from the
//! schema: `cargo run -p fenix-config --example settings_table`, then
//! paste it over the old one (a test in fenix-gui checks they agree).

fn main() {
    print!("{}", fenix_config::schema::markdown_table());
}
