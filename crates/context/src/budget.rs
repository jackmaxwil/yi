use yi_tools::cut_row;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Bytes(pub usize);

/// Per-source byte budgets enforced at assembly (design §4.4). Zero disables a
/// source entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceBudgets {
    pub project_instructions: Bytes,
    pub skills_meta: Bytes,
    pub memory: Bytes,
}

impl Default for SourceBudgets {
    fn default() -> Self {
        Self {
            project_instructions: Bytes(49_152),
            skills_meta: Bytes(16_384),
            memory: Bytes(32_768),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    pub text: String,
    pub truncated: bool,
}

/// Head-truncates a source to its byte budget and ends it in a [`cut_row`] row: a silently clipped
/// source reads as complete to the model. `next` is the call that gets the rest of this source.
pub fn fit(source: &str, budget: Bytes, next: &str) -> Truncated {
    if source.len() <= budget.0 {
        return Truncated {
            text: source.to_owned(),
            truncated: false,
        };
    }
    let kept = source.floor_char_boundary(budget.0);
    let row = cut_row(
        kept,
        source.len(),
        "bytes kept",
        "source budget",
        budget.0,
        next,
    );
    Truncated {
        text: format!("{}\n{row}", source.get(..kept).unwrap_or_default()),
        truncated: true,
    }
}
