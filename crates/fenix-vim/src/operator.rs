use fenix_keymap::KeyPress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

impl Operator {
    /// The key that triggers this operator, also used to detect the
    /// doubled form (`dd`, `cc`, `yy`) meaning "linewise, current line".
    pub fn trigger_key(self) -> KeyPress {
        match self {
            Operator::Delete => KeyPress::char('d'),
            Operator::Change => KeyPress::char('c'),
            Operator::Yank => KeyPress::char('y'),
        }
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
}
