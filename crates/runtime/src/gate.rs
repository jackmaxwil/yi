use std::path::Path;

use serde_json::{Value, json};
use yi_permission::{
    CatastrophicContext, Class, Decision, Parsed, PermissionMode, SessionRules, ToolCall,
    canonical_command_identity, classify, decide, parse,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub argv: Vec<String>,
    pub class: Class,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub command: String,
    pub mode: PermissionMode,
    pub decision: Decision,
    pub segments: Vec<Segment>,
    pub unparsed: bool,
}

impl Report {
    pub fn outcome(&self) -> &'static str {
        match self.decision {
            Decision::Allow { .. } => "allow",
            Decision::Contain { .. } => "contain",
            Decision::Deny { .. } => "deny",
            Decision::Ask { .. } => "ask",
        }
    }

    /// Contained counts as running: the sandbox is what stands in for the
    /// question, and `explain` has already downgraded it where none exists.
    pub fn allowed(&self) -> bool {
        matches!(
            self.decision,
            Decision::Allow { .. } | Decision::Contain { .. }
        )
    }

    pub fn reason(&self) -> String {
        match &self.decision {
            Decision::Allow { reason }
            | Decision::Contain { reason }
            | Decision::Deny { reason } => reason.clone(),
            Decision::Ask { description, .. } => description.clone(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "command": self.command,
            "mode": mode_label(self.mode),
            "decision": self.outcome(),
            "reason": self.reason(),
            "sandboxed": matches!(self.decision, Decision::Contain { .. }),
            "unparsed": self.unparsed,
            "segments": self.segments.iter().map(|segment| json!({
                "argv": segment.argv,
                "class": class_label(segment.class),
            })).collect::<Vec<Value>>(),
        })
    }
}

pub fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
    }
}

pub fn class_label(class: Class) -> &'static str {
    match class {
        Class::Safe => "safe",
        Class::Destructive => "destructive",
        Class::Unknown => "unknown",
    }
}

/// What the broker would answer for this command in a fresh session: the same `decide()` the
/// tool seam calls, without session rules or holds, which a dry run cannot have.
pub fn explain(command: &str, mode: PermissionMode, cwd: &Path) -> Report {
    let (segments, unparsed) = match parse(command) {
        Parsed::Segments(segments) => (
            segments
                .into_iter()
                .map(|argv| Segment {
                    class: classify(&argv),
                    argv,
                })
                .collect(),
            false,
        ),
        Parsed::Unparsed => (Vec::new(), true),
    };
    let canonical = canonical_command_identity(command, &cwd.to_string_lossy());
    let call = ToolCall {
        tool_name: "bash",
        reads_only: false,
        irreversible: bash_irreversible(command, cwd),
        in_workspace: false,
        rule_kind: yi_types::permission::RuleKind::Command,
        canonical: &canonical,
        display: command,
        targets: &[],
        command: Some(command),
    };
    let decision = match decide(
        &call,
        mode,
        &[],
        &SessionRules::new(),
        &[],
        &CatastrophicContext::detect(cwd),
    ) {
        Decision::Contain { reason } if !yi_tools::Sandbox::available() => Decision::Ask {
            title: format!("{} requires permission", call.tool_name),
            description: format!("{reason}: {command}"),
            reviewable: true,
        },
        other => other,
    };
    Report {
        command: command.to_owned(),
        mode,
        decision,
        segments,
        unparsed,
    }
}

/// The bash tool screens its own read-only verbs, so the dry run asks the tool
/// rather than guessing what the seam would pass.
fn bash_irreversible(command: &str, cwd: &Path) -> bool {
    let mut input = serde_json::Map::new();
    input.insert("command".to_owned(), Value::String(command.to_owned()));
    crate::builtin_tools()
        .iter()
        .find(|tool| tool.name() == "bash")
        .is_none_or(|tool| {
            let _cwd = cwd;
            tool.irreversible(&input)
        })
}

pub fn render(report: &Report) -> String {
    let mut out = format!(
        "{}: {}\n  {}\n",
        mode_label(report.mode),
        report.outcome(),
        report.reason()
    );
    if report.unparsed {
        out.push_str(&format!("  unparsed     {}\n", report.command));
        return out;
    }
    for segment in &report.segments {
        out.push_str(&format!(
            "  {:<12} {}\n",
            class_label(segment.class),
            segment.argv.join(" ")
        ));
    }
    out
}
