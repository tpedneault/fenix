#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    Replace,
    Command,
}

impl Mode {
    /// Short, uppercase name for the modeline (`NORMAL`, `INSERT`, ...).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::Replace => "REPLACE",
            Mode::Command => "COMMAND",
        }
    }
}
