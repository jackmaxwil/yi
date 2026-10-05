//! The slash verbs the solo TUI and the ACP worker both route here.

use crate::{AgentSession, PermissionMode};

/// The verbs a host advertises; `/pr` and `/base` still answer, as pointers to `/land`.
pub const SESSION_VERBS: [&str; 10] = [
    "advisor",
    "plan",
    "goal",
    "permissions",
    "compact",
    "lanes",
    "land",
    "discard",
    "heartbeat",
    "todo",
];

pub fn split(line: &str) -> (&str, &str) {
    line.split_once(char::is_whitespace)
        .map_or((line, ""), |(head, rest)| (head, rest.trim()))
}

pub fn undo(session: &AgentSession, cwd: &std::path::Path) -> String {
    if session.status() == crate::Status::Running {
        return "/undo: the current turn is still running; stop it first".to_owned();
    }
    let Some(store) = session.store() else {
        return "/undo: this session has no store to read checkpoints from".to_owned();
    };
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    match crate::undo(&store, cwd, &home) {
        crate::UndoOutcome::Restored { changes, scoped } => {
            format!("/undo: {}", crate::describe_undo(&changes, scoped))
        }
        // Scoped to this session on purpose: undoing a turn the reader never saw is not what
        // the word means. An earlier session's turns stay reachable, just not from here.
        crate::UndoOutcome::NoCheckpoint => {
            "/undo: this session has taken no turn yet — `yi undo` restores an earlier session"
                .to_owned()
        }
        crate::UndoOutcome::Failed(error) => format!("/undo failed: {error}"),
    }
}

pub fn sessions_listing(listed: &[yi_types::wire::SessionMetadata]) -> String {
    if listed.is_empty() {
        return "no sessions for this directory".to_owned();
    }
    let now = yi_session::now_ms();
    listed
        .iter()
        .map(|metadata| {
            format!(
                "{}  {:>8}  {}",
                metadata.id,
                yi_session::age_label(now.saturating_sub(metadata.created_at)),
                metadata.name.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn run(session: &AgentSession, command: &str, args: &str) -> Option<String> {
    Some(match command {
        "advisor" => advisor(session, args),
        "plan" => plan(session),
        "goal" => goal(session),
        "permissions" => permissions(session, args),
        "compact" => compact(session, args),
        "lanes" => lane_verb(session, "lanes", args),
        "land" => lane_verb(session, "land", args),
        "pr" => lane_verb(session, "pr", args),
        "base" => lane_verb(session, "base", args),
        "discard" => lane_verb(session, "discard", args),
        "heartbeat" => heartbeat(session, args),
        "todo" => todo(session, args),
        _ => return None,
    })
}

fn todo(session: &AgentSession, args: &str) -> String {
    use crate::todo::{Op, Target, TodoError, text};
    let Some(todos) = session.todos() else {
        return "/todo: no todo list is attached to this session".to_owned();
    };
    match args {
        "" => {
            todos.resync();
            text::render(&todos.list())
        }
        "clear" => {
            todos.resync();
            let total = todos.list().progress().total;
            if total == 0 {
                return "no todos to clear".to_owned();
            }
            match todos.apply_as(
                Op::Rm {
                    target: Target::All,
                },
                None,
                "user",
            ) {
                Ok(_) => format!("cleared {total} todos"),
                Err(TodoError::Mirrored { plan }) => format!(
                    "/todo clear: the list is plan {plan}'s view and stays until the plan closes (/plan shows it)"
                ),
                Err(error) => format!("/todo clear: {error}"),
            }
        }
        _ => "/todo [clear]".to_owned(),
    }
}

fn heartbeat(session: &AgentSession, args: &str) -> String {
    let Some(service) = session.heartbeat_service() else {
        return "/heartbeat: no scheduler is attached to this session".to_owned();
    };
    service
        .run(args)
        .unwrap_or_else(|error| format!("/heartbeat: {error}"))
}

/// Invariant: advice promotion has no host request behind it, so this command is
/// the only writer.
fn advisor(session: &AgentSession, args: &str) -> String {
    let Some(advisor) = session.advisor() else {
        return "/advisor: no advisor is attached to this session".to_owned();
    };
    match args.split_once(char::is_whitespace) {
        Some(("promote", id)) => match crate::advisor::promote_advice(session, id.trim()) {
            Ok(path) => format!(
                "/advisor promote {}: armed now and standing in {}",
                id.trim(),
                path.display()
            ),
            Err(error) => format!("/advisor promote: {error}"),
        },
        _ if args == "promote" => {
            "/advisor promote <advice-id> — the id is in the advisory's details".to_owned()
        }
        _ => {
            let promotable = advisor.promotable();
            let listed = if promotable.is_empty() {
                "nothing to promote yet".to_owned()
            } else {
                promotable
                    .iter()
                    .rev()
                    .take(5)
                    .map(|(id, advice)| format!("  {id}  {advice}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("{}\npromotable:\n{listed}", advisor.stats())
        }
    }
}

/// Invariant: the canonical plan file has one writer; this command only renders.
fn plan(session: &AgentSession) -> String {
    use crate::plan::{CanonicalPlanError, frontier_text, summary_line};
    let Some(service) = session.plan_service() else {
        return "/plan: no plan service is attached to this session".to_owned();
    };
    match service.read_plan() {
        Err(CanonicalPlanError::NoPlanOpen { .. }) => "/plan: no plan is open".to_owned(),
        Err(error) => format!("/plan: {error}"),
        Ok(plan) => match frontier_text(&plan) {
            frontier if frontier.is_empty() => summary_line(&plan),
            frontier => format!("{}\n{frontier}", summary_line(&plan)),
        },
    }
}

/// Invariant: a goal is explicit-only (§15.1), so a keystroke may read one and
/// never create one.
fn goal(session: &AgentSession) -> String {
    let Some(service) = session.goal_service() else {
        return "/goal: no goal service is attached to this session".to_owned();
    };
    match service.get() {
        Err(error) => format!("/goal: {error}"),
        Ok(goal) => {
            let field = |key: &str| {
                goal.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let number = |key: &str| goal.get(key).and_then(serde_json::Value::as_u64);
            let budget = match (number("tokensUsed"), number("tokenBudget")) {
                (Some(used), Some(budget)) => format!(" · {used}/{budget} tokens"),
                (Some(used), None) => format!(" · {used} tokens"),
                _ => String::new(),
            };
            let check = goal
                .get("check")
                .and_then(serde_json::Value::as_str)
                .map(|check| format!("\ncheck: {check}"))
                .unwrap_or_default();
            let failure = goal
                .get("checkFailure")
                .and_then(serde_json::Value::as_str)
                .map(|tail| format!("\nlast check failure:\n{tail}"))
                .unwrap_or_default();
            format!(
                "goal [{}]{budget}\n{}{check}{failure}",
                field("status"),
                field("objective")
            )
        }
    }
}

pub fn parse_mode(text: &str) -> Result<PermissionMode, &'static str> {
    match text {
        "ask" => Ok(PermissionMode::Ask),
        "auto" => Ok(PermissionMode::Auto),
        "yolo" => Ok(PermissionMode::Yolo),
        _ => Err("ask, auto, or yolo"),
    }
}

fn permissions(session: &AgentSession, args: &str) -> String {
    let Some(broker) = session.permission_broker() else {
        return "/permissions: no permission broker is attached to this session".to_owned();
    };
    if args.is_empty() {
        let kept: String = (broker.kept_rules().iter())
            .map(|rule| format!("\nkept: {rule}"))
            .collect();
        return format!(
            "permission mode: {}{kept}",
            crate::gate::mode_label(broker.mode())
        );
    }
    match parse_mode(args) {
        Ok(mode) => {
            broker.set_mode_and_fragment(mode, session);
            format!("permission mode: {}", crate::gate::mode_label(mode))
        }
        Err(want) => format!("/permissions [{want}]"),
    }
}

fn compact(session: &AgentSession, args: &str) -> String {
    let Some(compactor) = session.compactor() else {
        return "/compact: compaction is not attached to this session".to_owned();
    };
    if args.is_empty() {
        compactor.schedule();
        "compaction scheduled".to_owned()
    } else {
        compactor.schedule_with_instructions(Some(args.to_owned()));
        format!("compaction scheduled · {args}")
    }
}

/// Two verbs: `/land` ships (and with no title, reports), `/discard` throws away.
/// `/lanes` reads the pool from a lane or the trunk; `/pr` and `/base` point at `/land`.
fn lane_verb(session: &AgentSession, verb: &str, args: &str) -> String {
    let Some(lane) = session.lane() else {
        return format!("/{verb}: not inside a git repository");
    };
    let title = args.trim().trim_matches('"');
    let result = match verb {
        "lanes" => lane.lanes(),
        "land" if title.is_empty() => lane.refresh().map(|landing| landing_line(&landing)),
        "land" => lane.land(title),
        "pr" => return "/pr: `/land` with no title shows the landing".to_owned(),
        "base" => return "/base: `/land` merges main in before it pushes".to_owned(),
        _ => lane.discard(),
    };
    match result {
        Ok(text) => text,
        Err(error) => format!("/{verb}: {error}"),
    }
}

pub fn landing_line(landing: &yi_types::lane::Landing) -> String {
    use yi_types::lane::Landing;
    match landing {
        Landing::Unlanded => "unlanded".to_owned(),
        Landing::Pushed { branch } => format!("pushed {branch}, no pull request yet"),
        Landing::Open { pr, jobs, behind } => {
            let jobs = jobs
                .iter()
                .map(|job| format!("{} {}", job.name, job.state.glyph()))
                .collect::<Vec<_>>()
                .join(" · ");
            let behind = match behind {
                0 => String::new(),
                n => format!(" · main +{n}"),
            };
            format!("PR {pr} open · {jobs}{behind}")
        }
        Landing::Merged { pr } => format!("PR {pr} merged"),
    }
}
