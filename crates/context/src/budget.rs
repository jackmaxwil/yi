#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Bytes(pub usize);

/// Per-source byte budgets enforced at assembly (design §4.4). Zero disables a
/// source entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceBudgets {
    pub project_instructions: Bytes,
    pub skills_meta: Bytes,
    pub ledger: Bytes,
    pub memory: Bytes,
}

impl Default for SourceBudgets {
    fn default() -> Self {
        Self {
            project_instructions: Bytes(32_768),
            skills_meta: Bytes(16_384),
            ledger: Bytes(16_384),
            memory: Bytes(32_768),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    pub text: String,
    pub truncated: bool,
}

/// Head-truncates a source to its byte budget with an explicit marker — a
/// silently clipped source reads as complete to the model.
pub fn fit(source: &str, budget: Bytes) -> Truncated {
    if source.len() <= budget.0 {
        return Truncated {
            text: source.to_owned(),
            truncated: false,
        };
    }
    let mut end = budget.0;
    while end > 0 && !source.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let dropped = source.len().saturating_sub(end);
    Truncated {
        text: format!(
            "{}\n[... truncated: {dropped} bytes over budget ...]",
            &source[..end]
        ),
        truncated: true,
    }
}
