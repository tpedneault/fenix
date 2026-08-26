fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        winresource::WindowsResource::new()
            .set_icon("../../fenix.ico")
            .compile()
            .expect("failed to embed fenix.ico into the Windows exe resources");
    }
}
