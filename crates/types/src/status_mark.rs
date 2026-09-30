//! The one status vocabulary every surface draws: sidebar, rail, child card, roster, tool
//! cells, plan tree, timeline and gate jobs. Owner: "? = needs you, ✕ = failed" (D336).

/// What a status mark means; `glyph` is the only table of what each one looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMark {
    NeedsYou,
    Working,
    Stuck,
    DoneUnseen,
    Idle,
    Failed,
    Unknown,
}

impl StatusMark {
    /// One terminal cell each, so no mark moves the column after it.
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::NeedsYou => "?",
            Self::Working => "◐",
            Self::Stuck => "!",
            Self::DoneUnseen => "●",
            Self::Idle => "○",
            Self::Failed => "✕",
            Self::Unknown => "·",
        }
    }

    /// The glyph for the surfaces that draw into a `char` grid.
    pub fn symbol(self) -> char {
        self.glyph().chars().next().unwrap_or(' ')
    }
}
