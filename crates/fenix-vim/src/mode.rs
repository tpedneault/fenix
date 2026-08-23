#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    Replace,
    Command,
    /// Typing a `/`/`?` search query -- see `VimState::search_forward`
    /// for which direction. Shown only fleetingly in practice, since the
    /// host UI overrides the modeline suffix while actually typing a
    /// query, the same way it already does for `Command`.
    Search,
}

impl Mode {
    /// Short, uppercase name for the modeline (`NORMAL`, `INSERT`, ...).
    /// While `Visual`, prefer `VisualKind::label()` instead -- this generic
    /// one doesn't distinguish charwise/line/block.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::Replace => "REPLACE",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
        }
    }
}

/// Which kind of selection Visual mode is making. A field on `VimState`
/// rather than a variant of `Mode` itself, since `Mode::Visual` already
/// gates all the mode-dispatch logic correctly -- only selection
/// rendering and operator application need to know which kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualKind {
    Char,
    Line,
    Block,
}

impl VisualKind {
    pub fn label(self) -> &'static str {
        match self {
            VisualKind::Char => "VISUAL",
            VisualKind::Line => "V-LINE",
            VisualKind::Block => "V-BLOCK",
        }
    }
}
