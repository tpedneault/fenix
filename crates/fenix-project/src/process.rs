use std::process::Command;

/// A `Command` for a helper program (`git`, `rg`, `fd`) that never opens
/// a console window. Fenix is a windowed app, so without
/// `CREATE_NO_WINDOW` every one of these flashes a console on Windows --
/// and can take the keyboard with it.
pub(crate) fn quiet(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
