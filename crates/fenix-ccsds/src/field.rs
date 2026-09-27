//! A decoded field: what every decoder returns, as a tree.

/// Whether a field checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Ok(String),
    Warn(String),
    Bad(String),
}

impl Check {
    pub fn is_bad(&self) -> bool {
        matches!(self, Check::Bad(_))
    }
}

/// Where a field leads: a definition in the MIB, or the standard that
/// defines it (and the heading to find in its PDF).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A TM packet, by SPID.
    Spid(String),
    /// A TM or TC parameter, by name.
    Parameter(String),
    /// A telecommand, by name.
    Telecommand(String),
    /// A standard's id (`CCSDS 133.0-B`) and a heading in it.
    Standard(&'static str, &'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Field {
    pub name: String,
    /// Bit offset from the start of the bytes the whole tree decodes.
    pub bit: usize,
    pub bits: usize,
    /// The raw value, as it's best read: a number, or hex.
    pub raw: String,
    /// What it means.
    pub value: String,
    pub check: Option<Check>,
    pub link: Option<Link>,
    pub children: Vec<Field>,
}

impl Field {
    pub fn new(name: impl Into<String>, bit: usize, bits: usize, raw: impl Into<String>, value: impl Into<String>) -> Field {
        Field { name: name.into(), bit, bits, raw: raw.into(), value: value.into(), ..Default::default() }
    }

    /// A field grouping `children`, spanning them.
    pub fn group(name: impl Into<String>, children: Vec<Field>) -> Field {
        let bit = children.iter().map(|c| c.bit).min().unwrap_or(0);
        let end = children.iter().map(|c| c.bit + c.bits).max().unwrap_or(bit);
        Field { name: name.into(), bit, bits: end - bit, children, ..Default::default() }
    }

    pub fn checked(mut self, check: Check) -> Field {
        self.check = Some(check);
        self
    }

    pub fn linked(mut self, link: Link) -> Field {
        self.link = Some(link);
        self
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Field {
        self.value = value.into();
        self
    }

    /// Moves the whole tree `by` bits later -- a packet decoded on its own,
    /// placed where it sits in a frame.
    pub fn shift(&mut self, by: usize) {
        self.bit += by;
        for c in &mut self.children {
            c.shift(by);
        }
    }

    /// Every field, depth first, with its depth.
    pub fn walk(&self) -> Vec<(usize, &Field)> {
        let mut out = Vec::new();
        fn go<'a>(f: &'a Field, depth: usize, out: &mut Vec<(usize, &'a Field)>) {
            out.push((depth, f));
            for c in &f.children {
                go(c, depth + 1, out);
            }
        }
        go(self, 0, &mut out);
        out
    }

    /// The first field named `name`, anywhere below.
    pub fn find(&self, name: &str) -> Option<&Field> {
        self.walk().into_iter().map(|(_, f)| f).find(|f| f.name == name)
    }

    /// Whether anything in the tree is bad.
    pub fn any_bad(&self) -> bool {
        self.walk().iter().any(|(_, f)| f.check.as_ref().is_some_and(Check::is_bad))
    }
}

/// Bytes as spaced hex: `0B F2 C1`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Bytes as unspaced hex: `0BF2C1`.
pub fn hex_tight(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_spans_its_children_and_shifts_with_them() {
        let mut g = Field::group("g", vec![Field::new("a", 8, 4, "1", ""), Field::new("b", 12, 12, "2", "").checked(Check::Bad("x".into()))]);
        assert_eq!((g.bit, g.bits), (8, 16));
        assert!(g.any_bad());
        g.shift(16);
        assert_eq!(g.find("b").unwrap().bit, 28);
        assert_eq!(g.walk().len(), 3);
        assert_eq!(hex(&[0x0b, 0xf2]), "0B F2");
    }
}
