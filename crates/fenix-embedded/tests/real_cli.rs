//! Against a real `arduino-cli` with the `arduino:avr` package installed.
//! Ignored by default (CI has neither); run with
//! `cargo test -p fenix-embedded -- --ignored`.

use fenix_embedded::{Debugging, ToolOverrides, Tools};

fn run(command: &fenix_embedded::Command) -> (bool, String) {
    let out = std::process::Command::new(&command.program).args(&command.args).output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
#[ignore]
fn a_new_sketch_compiles_and_the_cli_answers_every_query() {
    let tools = Tools::discover(&ToolOverrides::default());
    assert!(tools.arduino_cli.is_some(), "arduino-cli must be installed for this test");
    let parent = std::env::temp_dir().join(format!("fenix-embedded-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).unwrap();

    let ino = fenix_embedded::arduino::create_sketch(&parent, "RealBlink", "arduino:avr:uno").unwrap();
    let root = ino.parent().unwrap();
    assert_eq!(fenix_embedded::project_root_of(&ino).as_deref(), Some(root));
    let platform = fenix_embedded::detect(root, &tools).expect("a new sketch is detected");

    let (ok, output) = run(&platform.build().unwrap());
    assert!(ok, "compile failed:\n{output}");
    assert!(output.contains("Sketch uses"), "{output}");

    std::fs::write(&ino, "void setup() {\n  pinMode(13, OUTPUT)\n}\nvoid loop() {}\n").unwrap();
    let (ok, output) = run(&platform.build().unwrap());
    assert!(!ok);
    let error_line = output.lines().find(|l| l.contains("error:")).expect("a gcc-style error line");
    assert!(error_line.contains("RealBlink.ino:3:1: error:"), "errors point at the .ino itself: {error_line}");

    let boards = platform.boards().unwrap();
    assert!(boards.iter().any(|b| b.id == "arduino:avr:uno" && b.name == "Arduino UNO"));
    let options = platform.board_options("arduino:avr:nano").unwrap();
    assert!(options.iter().any(|o| o.id == "cpu" && o.values.iter().any(|v| v.value == "atmega328old")));
    let options = platform.board_options("arduino:avr:nano:cpu=atmega328old").unwrap();
    let cpu = options.iter().find(|o| o.id == "cpu").unwrap();
    assert!(cpu.values.iter().find(|v| v.value == "atmega328old").unwrap().selected, "board details reflects the chosen option");

    platform.ports().unwrap();
    let packages = platform.search_packages("avr").unwrap();
    assert!(packages.iter().any(|p| p.id == "arduino:avr" && p.installed.is_some()));
    let libraries = platform.search_libraries("Servo").unwrap();
    assert!(libraries.iter().any(|l| l.name == "Servo" && !l.latest.is_empty()));

    match platform.debugging(None).unwrap() {
        Debugging::Unsupported(reason) => assert!(reason.contains("can't be debugged"), "{reason}"),
        Debugging::Supported { .. } => panic!("the UNO has no debugger support"),
    }
    let _ = std::fs::remove_dir_all(&parent);
}
