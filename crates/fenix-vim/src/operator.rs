use fenix_keymap::KeyPress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
    /// `gU`: to upper case.
    Upper,
    /// `gu`: to lower case.
    Lower,
    /// `g~`: case toggled.
    ToggleCase,
    /// `g?`: rot13.
    Rot13,
    /// `gq`: lines wrapped at the text width, the cursor after them.
    Format,
    /// `gw`: `gq`, the cursor staying where it was in the text.
    FormatKeep,
    /// `>`: lines indented a level.
    Indent,
    /// `<`: lines dedented a level.
    Dedent,
    /// `!`: lines filtered through a command -- the command line opens
    /// with their range (`:.,.+2!`) for it.
    Filter,
}

impl Operator {
    /// The keys that start this operator (`d`, `gU`).
    pub fn keys(self) -> &'static [char] {
        match self {
            Operator::Delete => &['d'],
            Operator::Change => &['c'],
            Operator::Yank => &['y'],
            Operator::Upper => &['g', 'U'],
            Operator::Lower => &['g', 'u'],
            Operator::ToggleCase => &['g', '~'],
            Operator::Rot13 => &['g', '?'],
            Operator::Format => &['g', 'q'],
            Operator::FormatKeep => &['g', 'w'],
            Operator::Indent => &['>'],
            Operator::Dedent => &['<'],
            Operator::Filter => &['!'],
        }
    }

    /// The key that, typed again after the operator, means "the current
    /// line" (`dd`, `gUU`, `g??`, `>>`) -- the operator's last key. A
    /// `g` operator also takes its two keys again (`gUgU`).
    pub fn doubled_key(self) -> KeyPress {
        KeyPress::char(*self.keys().last().expect("every operator has a key"))
    }

    /// The key that triggers this operator, also used to detect the
    /// doubled form (`dd`, `cc`, `yy`) meaning "linewise, current line".
    pub fn trigger_key(self) -> KeyPress {
        self.doubled_key()
    }

    /// Whether it acts on whole lines whatever the motion (`gq`, `>`).
    pub fn always_linewise(self) -> bool {
        matches!(self, Operator::Format | Operator::FormatKeep | Operator::Indent | Operator::Dedent | Operator::Filter)
    }

    /// Whether it changes the text (everything but `y`).
    pub fn edits(self) -> bool {
        self != Operator::Yank
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_keys_are_distinct() {
        assert_ne!(Operator::Delete.trigger_key(), Operator::Change.trigger_key());
        assert_ne!(Operator::Change.trigger_key(), Operator::Yank.trigger_key());
    }

    #[test]
    fn a_g_operator_doubles_on_its_last_key() {
        assert_eq!(Operator::Upper.doubled_key(), KeyPress::char('U'));
        assert_eq!(Operator::Rot13.doubled_key(), KeyPress::char('?'));
    }
}
