use super::doc::DocError;
use crate::url::Url;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TodoLabel(String);

impl TodoLabel {
    pub fn new(label: impl Into<String>) -> Result<Self, DocError> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(DocError::LabelEmpty);
        }
        let chars = label.chars().count();
        if chars > TODO_LABEL_MAX {
            return Err(DocError::LabelTooLong {
                label,
                max: TODO_LABEL_MAX,
            });
        }
        if label.contains(['\n', '\r']) {
            return Err(DocError::LabelNewline { label });
        }
        Ok(Self(label))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TodoLabel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for TodoLabel {
    type Error = DocError;

    fn try_from(label: String) -> Result<Self, Self::Error> {
        Self::new(label)
    }
}

impl From<TodoLabel> for String {
    fn from(label: TodoLabel) -> Self {
        label.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GoalText(String);

impl GoalText {
    pub fn new(goal: impl Into<String>) -> Result<Self, DocError> {
        let goal = goal.into();
        if goal.trim().is_empty() {
            return Err(DocError::GoalEmpty);
        }
        let chars = goal.chars().count();
        if chars > GOAL_TEXT_MAX {
            return Err(DocError::GoalTooLong {
                goal,
                max: GOAL_TEXT_MAX,
            });
        }
        if goal.contains(['\n', '\r']) {
            return Err(DocError::GoalNewline { goal });
        }
        Ok(Self(goal))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GoalText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for GoalText {
    type Error = DocError;

    fn try_from(goal: String) -> Result<Self, Self::Error> {
        Self::new(goal)
    }
}

impl From<GoalText> for String {
    fn from(goal: GoalText) -> Self {
        goal.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InlineNote(String);

impl InlineNote {
    pub fn new(note: impl Into<String>) -> Result<Self, DocError> {
        let note = note.into();
        if note.trim().is_empty() {
            return Err(DocError::NoteEmpty);
        }
        if note.len() > INLINE_NOTE_MAX_BYTES {
            return Err(DocError::NoteTooLong {
                bytes: note.len(),
                max: INLINE_NOTE_MAX_BYTES,
            });
        }
        Ok(Self(note))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for InlineNote {
    type Error = DocError;

    fn try_from(note: String) -> Result<Self, Self::Error> {
        Self::new(note)
    }
}

impl From<InlineNote> for String {
    fn from(note: InlineNote) -> Self {
        note.0
    }
}

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

impl ProbeCommand {
    pub fn new(probe: impl Into<String>) -> Result<Self, DocError> {
        let probe = probe.into();
        if probe.trim().is_empty() {
            return Err(DocError::ProbeEmpty);
        }
        if probe.contains(['\n', '\r']) {
            return Err(DocError::ProbeNewline { probe });
        }
        Ok(Self(probe))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProbeCommand {
    type Error = DocError;

    fn try_from(probe: String) -> Result<Self, Self::Error> {
        Self::new(probe)
    }
}

impl From<ProbeCommand> for String {
    fn from(probe: ProbeCommand) -> Self {
        probe.0
    }
}

macro_rules! bounded_text {
    ($name:ident, $cap:ident, $empty:ident, $long:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// # Errors
            /// Blank, or over the byte cap.
            pub fn new(text: impl Into<String>) -> Result<Self, DocError> {
                let text = text.into();
                if text.trim().is_empty() {
                    return Err(DocError::$empty);
                }
                if text.len() > $cap {
                    return Err(DocError::$long {
                        bytes: text.len(),
                        max: $cap,
                    });
                }
                Ok(Self(text))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = DocError;

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

bounded_text!(Intent, INTENT_MAX_BYTES, IntentEmpty, IntentTooLong);
bounded_text!(Note, NOTE_MAX_BYTES, NoteEmpty, NoteTooLong);
