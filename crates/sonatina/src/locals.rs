//! Which source bindings are visible around one IR instruction, as lowering
//! records them and as the byte map carries them.
//!
//! A binding is named by its [`hull::BinderId`]. The record rides in the
//! post-optimization provenance of the instruction (see `compile.rs`), so it
//! reaches the byte map without any change to the bytes.

use std::fmt;

/// What the instruction does to the call stack, which decides what its `JUMP`
/// opcodes may say about the bindings that hold after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// Any other instruction.
    Plain,
    /// An internal call: the instruction after the call runs in another frame
    /// until the call returns.
    Call,
    /// A return: the instruction after it runs in the caller's frame.
    Return,
    /// A `break` or `continue`: control leaves the scopes of the statement, so
    /// the bindings that hold at the jump are not those that hold at its target.
    Jump,
}

impl Frame {
    fn letter(self) -> char {
        match self {
            Self::Plain => 'p',
            Self::Call => 'c',
            Self::Return => 'r',
            Self::Jump => 'j',
        }
    }

    fn from_letter(letter: &str) -> Option<Self> {
        Some(match letter {
            "p" => Self::Plain,
            "c" => Self::Call,
            "r" => Self::Return,
            "j" => Self::Jump,
            _ => return None,
        })
    }
}

/// The visible bindings before and after one IR instruction, in the order they
/// were bound (outermost first), and the instruction's frame effect.
///
/// `before` holds while the instruction's own opcodes run; `after` holds once
/// the instruction has completed, which is later than `before` for the
/// instruction that completes a `let`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalsTag {
    /// The IR instruction, unique within its function. Opcodes of one run
    /// share it.
    pub inst: u32,
    pub before: Vec<u32>,
    pub after: Vec<u32>,
    pub frame: Frame,
}

impl LocalsTag {
    /// `<inst>:<before ids>:<after ids>:<frame>`, with the ids comma-separated.
    pub(crate) fn encode(&self) -> String {
        let ids = |ids: &[u32]| ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        format!(
            "{}:{}:{}:{}",
            self.inst,
            ids(&self.before),
            ids(&self.after),
            self.frame.letter()
        )
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut parts = text.split(':');
        let mut next = |what: &str| {
            parts
                .next()
                .ok_or_else(|| format!("locals tag `{text}` has no {what}"))
        };
        let inst = next("instruction")?
            .parse()
            .map_err(|_| format!("locals tag `{text}` has an unreadable instruction"))?;
        let before = parse_ids(text, next("before list")?)?;
        let after = parse_ids(text, next("after list")?)?;
        let frame = Frame::from_letter(next("frame")?)
            .ok_or_else(|| format!("locals tag `{text}` has an unknown frame"))?;
        if parts.next().is_some() {
            return Err(format!("locals tag `{text}` has trailing fields"));
        }
        Ok(Self {
            inst,
            before,
            after,
            frame,
        })
    }
}

fn parse_ids(text: &str, ids: &str) -> Result<Vec<u32>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    ids.split(',')
        .map(|id| {
            id.parse()
                .map_err(|_| format!("locals tag `{text}` has an unreadable binder id"))
        })
        .collect()
}

impl fmt::Display for LocalsTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}
