use super::doc::DocError;
use crate::url::Url;
use serde::{Deserialize, Serialize};

/// The impls of a `String` newtype on the wire that only `check` admits; the struct stays
/// declared beside it, where the schema lock reads its shape.
macro_rules! text_newtype {
    ($name:ident, $error:ty, $check:expr $(,)?) => {
        impl $name {
            /// # Errors
            /// Whatever the type's check refuses.
            pub fn new(text: impl Into<String>) -> Result<Self, $error> {
                let check: fn(String) -> Result<String, $error> = $check;
                check(text.into()).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = $error;

            fn try_from(text: String) -> Result<Self, Self::Error> {
                Self::new(text)
            }
        }

        impl From<$name> for String {
            fn from(text: $name) -> Self {
                text.0
            }
        }
    };
}
pub(crate) use text_newtype;

/// One line of at most `max` chars: blank, then too long, then a line break is refused.
pub(crate) fn one_line(
    text: String,
    max: usize,
    empty: DocError,
    long: fn(String, usize) -> DocError,
    newline: fn(String) -> DocError,
) -> Result<String, DocError> {
    if text.trim().is_empty() {
        return Err(empty);
    }
    if text.chars().count() > max {
        return Err(long(text, max));
    }
    if text.contains(['\n', '\r']) {
        return Err(newline(text));
    }
    Ok(text)
}

fn within_bytes(
    text: String,
    max: usize,
    empty: DocError,
    long: fn(usize, usize) -> DocError,
) -> Result<String, DocError> {
    if text.trim().is_empty() {
        return Err(empty);
    }
    if text.len() > max {
        return Err(long(text.len(), max));
    }
    Ok(text)
}

pub const PLAN_FORMAT: u32 = 2;
/// The frontmatter document format the importer still reads (plan section 5.5, two releases).
pub const LEGACY_PLAN_FORMAT: u32 = 1;
pub const PLAN_ID_MAX: usize = 96;
pub const SLUG_MAX: usize = 40;
pub const TODO_LABEL_MAX: usize = 80;
pub const GOAL_TEXT_MAX: usize = 512;
/// Big content travels by URL, never inline; 1 KiB keeps even a plan of forty
/// noted delegations inside the 32 KiB frontmatter budget.
pub const INLINE_NOTE_MAX_BYTES: usize = 1024;
/// One paragraph the program or the owner wrote about the plan.
pub const INTENT_MAX_BYTES: usize = 2048;
/// Prose imported from a format-1 body section; a longer section is an artifact reference.
pub const NOTE_MAX_BYTES: usize = 4096;

/// Slug identity of one plan file: lowercase alphanumerics and `-`, with one
/// optional `.` separating a sub-plan from its parent id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PlanId(String);

impl PlanId {
    pub fn new(id: impl Into<String>) -> Result<Self, DocError> {
        let id = id.into();
        if id.is_empty() {
            return Err(DocError::PlanIdEmpty);
        }
        if id.len() > PLAN_ID_MAX {
            return Err(DocError::PlanIdTooLong {
                id,
                max: PLAN_ID_MAX,
            });
        }
        let mut dots = 0usize;
        for (index, ch) in id.char_indices() {
            match ch {
                'a'..='z' | '0'..='9' | '-' => {}
                '.' => {
                    dots += 1;
                    if dots > 1 {
                        return Err(DocError::PlanIdDepth { id });
                    }
                    if index == 0 || index == id.len() - 1 {
                        return Err(DocError::PlanIdChar { id });
                    }
                }
                _ => return Err(DocError::PlanIdChar { id }),
            }
        }
        Ok(Self(id))
    }

    /// The deterministic identity of a plan for a goal: no randomness, so every fixture and
    /// replay names the same file. Collision suffixing is the store's job, not the slug's.
    pub fn slug(text: &str) -> Result<Self, DocError> {
        let slug = slugify(text);
        if slug.is_empty() {
            return Err(DocError::SlugEmpty {
                text: text.to_owned(),
            });
        }
        Self::new(slug)
    }

    pub fn child(&self, label: &TodoLabel) -> Result<Self, DocError> {
        if !self.is_root() {
            return Err(DocError::ChildOfSub {
                parent: self.0.clone(),
            });
        }
        let todo = slugify(label.as_str());
        if todo.is_empty() {
            return Err(DocError::SlugEmpty {
                text: label.as_str().to_owned(),
            });
        }
        Self::new(format!("{}.{todo}", self.0))
    }

    pub fn is_root(&self) -> bool {
        !self.0.contains('.')
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lowercase, non-alphanumeric runs collapsed to `-`, over-cap cuts landing at a word
/// boundary: hard-cut at 40 bytes, then drop the partial word. Golden fixtures pin this.
pub(super) fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    if slug.len() > SLUG_MAX {
        slug = slug.chars().take(SLUG_MAX).collect();
        if let Some(cut) = slug.rfind('-')
            && cut > 0
        {
            slug = slug.chars().take(cut).collect();
        }
    }
    slug.trim_matches('-').to_owned()
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for PlanId {
    type Error = DocError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl From<PlanId> for String {
    fn from(id: PlanId) -> Self {
        id.0
    }
}

/// The todo's address: verbatim-content identity, unique per plan, immutable once created —
/// rewording is append-new plus abandon-old — which makes it safe as an edge endpoint.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TodoLabel(String);

/// The label as its user wrote it, quoted: every refusal names a todo through this.
impl std::fmt::Debug for TodoLabel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}", self.0)
    }
}

text_newtype!(TodoLabel, DocError, |label| one_line(
    label,
    TODO_LABEL_MAX,
    DocError::LabelEmpty,
    |label, max| DocError::LabelTooLong { label, max },
    |label| DocError::LabelNewline { label },
));

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GoalText(String);

text_newtype!(GoalText, DocError, |goal| one_line(
    goal,
    GOAL_TEXT_MAX,
    DocError::GoalEmpty,
    |goal, max| DocError::GoalTooLong { goal, max },
    |goal| DocError::GoalNewline { goal },
));

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InlineNote(String);

text_newtype!(InlineNote, DocError, |note| within_bytes(
    note,
    INLINE_NOTE_MAX_BYTES,
    DocError::NoteEmpty,
    |bytes, max| DocError::NoteTooLong { bytes, max },
));

/// The session's own agent: a todo it runs inline, not through a child.
pub const OWNER_AGENT: &str = "main";

/// Host-internal id of a live agent; the durable address of a child stays the
/// todo label, so this never appears in a terminal record.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentId(String);

impl AgentId {
    pub fn owner() -> Self {
        Self(OWNER_AGENT.to_owned())
    }

    pub fn new(id: impl Into<String>) -> Result<Self, DocError> {
        let id = id.into();
        if id.is_empty() {
            return Err(DocError::AgentIdEmpty);
        }
        if id.chars().any(char::is_whitespace) {
            return Err(DocError::AgentIdWhitespace { id });
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for AgentId {
    type Error = DocError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl From<AgentId> for String {
    fn from(id: AgentId) -> Self {
        id.0
    }
}

/// Counts every applied op; the staleness reminder and the work-triggered
/// nudge read this, since the version no longer moves on bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct TouchCount(pub u64);

impl TouchCount {
    pub fn bump(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Root-plan delegation fuse in the [`crate::plan::doc::TokenBudget`] unit family: charging
/// is the only way up, and the confirmed `fuse_reset` op the only way down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Spawns(u32);

impl Spawns {
    pub fn charge(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Incident: append crossed with retry and decompose has no named total, so a control-flow
/// bug spawns children forever and the fuse trips it. Placeholder until fitted.
pub const SPAWN_CAP: Spawns = Spawns(64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct RetryCount(pub u8);

impl RetryCount {
    pub fn bump(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

/// The pair naming one todo from outside its plan, serialized `<plan>/<verbatim label>`;
/// [`TodoAddr::to_url`] uses the label's slug instead, because a URL carries no whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TodoAddr {
    pub plan: PlanId,
    pub todo: TodoLabel,
}

impl TodoAddr {
    pub fn to_url(&self) -> Result<Url, DocError> {
        let slug = slugify(self.todo.as_str());
        if slug.is_empty() {
            return Err(DocError::SlugEmpty {
                text: self.todo.as_str().to_owned(),
            });
        }
        let rendered = format!("plan://{}/{slug}", self.plan);
        rendered.parse().map_err(|cause| DocError::AddrUrl {
            url: rendered,
            cause,
        })
    }
}

impl std::fmt::Display for TodoAddr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.plan, self.todo)
    }
}

impl TryFrom<String> for TodoAddr {
    type Error = DocError;

    fn try_from(addr: String) -> Result<Self, Self::Error> {
        let Some((plan, todo)) = addr.split_once('/') else {
            return Err(DocError::AddrSyntax { addr });
        };
        Ok(Self {
            plan: PlanId::new(plan)?,
            todo: TodoLabel::new(todo)?,
        })
    }
}

impl From<TodoAddr> for String {
    fn from(addr: TodoAddr) -> Self {
        addr.to_string()
    }
}

/// Invariant: a probe is always a runnable command, reusing
/// [`crate::plan::doc::Check::Command`], because a stated acceptance cannot run on a cadence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProbeCommand(String);

text_newtype!(ProbeCommand, DocError, |probe| match probe {
    blank if blank.trim().is_empty() => Err(DocError::ProbeEmpty),
    probe if probe.contains(['\n', '\r']) => Err(DocError::ProbeNewline { probe }),
    probe => Ok(probe),
});

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Intent(String);

text_newtype!(Intent, DocError, |intent| within_bytes(
    intent,
    INTENT_MAX_BYTES,
    DocError::IntentEmpty,
    |bytes, max| DocError::IntentTooLong { bytes, max },
));

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Note(String);

text_newtype!(Note, DocError, |note| within_bytes(
    note,
    NOTE_MAX_BYTES,
    DocError::NoteEmpty,
    |bytes, max| DocError::NoteTooLong { bytes, max },
));
