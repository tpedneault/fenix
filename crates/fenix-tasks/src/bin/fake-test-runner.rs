//! A made-up test runner for the test explorer's tests: `fenix_tasks::fake_runner`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    fenix_tasks::fake_runner::main(&args, &|| std::process::Command::new(std::env::current_exe().expect("its own path")))
}
