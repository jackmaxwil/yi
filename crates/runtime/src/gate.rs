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
    pub sandboxed: bool,
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
        Self::reason_of(&self.decision)
    }

    pub fn reason_of(decision: &Decision) -> String {
        match decision {
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
            "sandboxed": self.sandboxed,
            "unparsed": self.unparsed,
            "segments": self.segments.iter().map(|segment| json!({
                "argv": segment.argv,
                "class": class_label(segment.class),
            })).collect::<Vec<Value>>(),
        })
    }
}

/// What a model can do with no one to approve a call; `--yolo` and allow rules are a person's.
pub(crate) const MODEL_REMEDIES: &str = "Split the command, keep the writes inside the working tree, or ask the user to approve this call.";

/// The model got here by asking the user, so it is not told to ask again.
pub(crate) fn unaskable(request: impl std::fmt::Display, description: &str) -> String {
    format!(
        "Request {request} cannot be put to anyone: no interactive surface is available. {description} Split the command, keep the writes inside the working tree, or report in your final answer that this call needs the user's approval."
    )
}

/// Holds compile down (plan section 7.6): a child with no interactive surface cannot ask, so
/// its `Ask` is a deny naming the hazard; an attached one's stands on its family's broker.
pub fn compile_ask(decision: Decision, attached: bool) -> Decision {
    match decision {
        Decision::Ask { description, .. } if !attached => Decision::Deny {
            reason: format!(
                "Permission required but no interactive surface is available. {description} {MODEL_REMEDIES}"
            ),
        },
        other => other,
    }
}

/// A refused call's retry with no one to ask: what runs it, which is never an allow rule.
pub(crate) fn headless_refusal(
    sandbox: Option<&yi_tools::Sandbox>,
    refusal: &yi_tools::SandboxRefusal,
) -> String {
    const HEAD: &str = "Permission required but no interactive surface is available: the sandbox refused this command's last contained run";
    match refusal {
        yi_tools::SandboxRefusal::Path(path) => {
            let dir = path.parent().unwrap_or(path);
            let (widen, remedies) = match crate::permission::protected(sandbox, dir) {
                true => (
                    format!("`{}` is protected, so no approval widens it", dir.display()),
                    "Split the command or keep the writes inside the working tree.",
                ),
                false => (
                    format!("an interactive run asks to widen it by `{}`", dir.display()),
                    MODEL_REMEDIES,
                ),
            };
            format!(
                "{HEAD} writing `{}`, and {widen}. {remedies} For a build, set CARGO_TARGET_DIR under the working tree.",
                path.display()
            )
        }
        yi_tools::SandboxRefusal::Scopes(_) => format!(
            "{HEAD}, naming no path (a nested sandbox, or the network past loopback), and only an interactive approval runs it outside the sandbox. {MODEL_REMEDIES}"
        ),
    }
}

/// The one framing every walled session's refusal of a bash call shares.
fn wall_denial(body: &str) -> String {
    format!(
        "Denied by the reviewer wall: {body}. Read only what the wall leaves open, and report what you could not check."
    )
}

/// A walled session's retry near a store or `~/.yi`: run outside, it would read what its wall hides.
pub(crate) fn walled_refusal(path: &std::path::Path) -> String {
    wall_denial(&format!(
        "the sandbox refused this command's last contained run at `{}`, and a walled session never runs it outside the sandbox",
        path.display()
    ))
}

/// A walled session's call that would leave a sandbox that exists (#1001).
pub(crate) fn walled_host_refusal(why: &str) -> String {
    wall_denial(&format!(
        "this command would run outside the sandbox ({why}), and a walled session's commands never leave it"
    ))
}

/// A walled session's command that runs outside the sandbox, naming a walled root (#1001).
pub(crate) fn outside_sandbox_refusal(root: &std::path::Path) -> String {
    wall_denial(&format!(
        "this command runs outside the sandbox and names `{}` or a directory above it, which hold other sessions' transcripts and output",
        root.display()
    ))
}

/// Invariant: a walled kernel boots only under a profile. With no Seatbelt (Linux), it and its
/// `bash()` jobs would meet no wall, and only bash has a container path (#1001).
pub(crate) const WALLED_KERNEL_UNCONFINED: &str = "ipython is unavailable to a walled session here: with no Seatbelt sandbox (macOS only), its kernel would run unconfined and no wall would hold in a cell. Use read, grep and bash, which the wall checks at each call.";

/// An allowed compound, part of which must leave the sandbox, with no one to ask.
pub(crate) fn headless_split(why: &str) -> String {
    format!(
        "Permission required but no interactive surface is available: part of this command must run outside the sandbox ({why}) and would take the rest with it. {MODEL_REMEDIES}"
    )
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
        Class::Egress => "egress",
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
    let context = CatastrophicContext::detect(cwd);
    let decision = match decide(&call, mode, &[], &SessionRules::new(), &[], &context) {
        Decision::Contain { reason } if !yi_tools::Sandbox::available() => Decision::Ask {
            title: format!("{} requires permission", call.tool_name),
            description: format!("{reason}: {command}"),
            reviewable: true,
        },
        other => other,
    };
    let sandboxed = match decision {
        Decision::Contain { .. } => true,
        Decision::Allow { .. } => {
            yi_tools::Sandbox::available()
                && mode != PermissionMode::Yolo
                && crate::permission::leaves_sandbox(command, &context).is_none()
        }
        Decision::Ask { .. } | Decision::Deny { .. } => false,
    };
    Report {
        command: command.to_owned(),
        mode,
        decision,
        segments,
        unparsed,
        sandboxed,
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

#[cfg(test)]
mod tests {
    use super::{Report, compile_ask, headless_refusal, headless_split, unaskable};
    use yi_permission::Decision;

    /// A model told to "run with --yolo, or add an allow rule" reaches for a person's remedy. Each
    /// refusal names the three it has: split the command, keep writes in the tree, ask the user.
    #[test]
    fn a_headless_refusal_names_the_model_remedies_and_never_a_persons_flag() {
        let refused = [
            Report::reason_of(&compile_ask(
                Decision::Ask {
                    title: "bash requires permission".to_owned(),
                    description: "writes outside the workspace".to_owned(),
                    reviewable: true,
                },
                false,
            )),
            headless_split("it writes /etc/hosts"),
            headless_refusal(
                None,
                &yi_tools::SandboxRefusal::Scopes(vec!["curl".to_owned()]),
            ),
            headless_refusal(
                None,
                &yi_tools::SandboxRefusal::Path(std::path::PathBuf::from("/etc/hosts")),
            ),
        ];
        for text in refused {
            let lowered = text.to_lowercase();
            for remedy in ["split", "working tree", "ask the user"] {
                assert!(lowered.contains(remedy), "no `{remedy}` in: {text}");
            }
            assert!(!lowered.contains("--yolo"), "{text}");
        }
    }

    #[test]
    fn a_protected_headless_path_refusal_never_offers_an_approval()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = std::env::var_os("HOME").ok_or("HOME is unset")?;
        let text = headless_refusal(
            None,
            &yi_tools::SandboxRefusal::Path(std::path::PathBuf::from(home).join("notes")),
        );
        assert!(text.contains("no approval widens it"), "{text}");
        assert!(text.contains("Split the command"), "{text}");
        assert!(text.contains("working tree"), "{text}");
        assert!(!text.contains("ask the user"), "{text}");
        Ok(())
    }

    /// `ask_user` must not answer a model that already asked the user with "ask the user".
    #[test]
    fn ask_user_does_not_loop() {
        let answer = unaskable(7, "writes outside the workspace").to_lowercase();
        assert!(!answer.contains("ask the user"), "{answer}");
        assert!(answer.contains("final answer"), "{answer}");
    }
}
