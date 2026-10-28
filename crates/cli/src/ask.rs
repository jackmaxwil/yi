use std::time::{Duration, Instant};

use yi_runtime::Status;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::telemetry::ErrorClass;

use super::{
    Args, Resume, attach_store, build_session, exit_refused, release_lane, render_text,
    session_target, tty,
};

const HOLD_POLL: Duration = Duration::from_millis(500);
const EXIT_DENIED: i32 = 4;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

pub(super) fn run(args: &Args) -> i32 {
    use std::io::IsTerminal;
    let deadline = args
        .deadline
        .and_then(|secs| Instant::now().checked_add(Duration::from_secs(secs)));
    let interactive = std::io::stdin().is_terminal();
    let config = ErrorClass::RefusalConfig;
    // Incident: a review round's prompt passed Linux's 128 KiB cap on one argument (E2BIG).
    let prompt = match args.prompt.as_str() {
        "-" => match std::io::read_to_string(std::io::stdin()) {
            Ok(text) if !text.is_empty() => text,
            Ok(_) => return refuse(args.json, 2, &config, "no prompt on stdin"),
            Err(error) => {
                let message = format!("reading the prompt from stdin: {error}");
                return refuse(args.json, 2, &config, &message);
            }
        },
        given => given.to_owned(),
    };
    let runtime = match crate::current_thread() {
        Ok(runtime) => runtime,
        Err(code) => {
            let class = ErrorClass::Harness("runtime".to_owned());
            return early_final(args.json, code, &class, "the async runtime did not start");
        }
    };
    // Session wiring spawns runtime tasks, so the runtime context must exist before it.
    let target = session_target(args);
    let (session, host) = {
        let _guard = runtime.enter();
        let asker: Option<yi_runtime::Asker> =
            interactive.then(|| std::sync::Arc::new(tty::tty_ask) as yi_runtime::Asker);
        // Every spelling of a headless ask (`ask`, bare prompt words) lands here: no tty, no asker.
        match build_session(args, asker, Some(&target.id), false, [!interactive, true]) {
            Ok(built) => built,
            Err(refused) => {
                let (class, reason) = (refused.class.clone(), refused.reason.clone());
                return early_final(args.json, exit_refused(refused), &class, &reason);
            }
        }
    };
    let attaching = yi_types::trace::span("ask.attach_store");
    let attached = attach_store(args, &session, &target);
    drop(attaching);
    match attached {
        Ok(_id) => {}
        // A requested resume that cannot be honoured is an error; an
        // unavailable store for a fresh turn only costs the recording.
        Err(error) if args.resume == Resume::Fresh => {
            eprintln!("warning: session store unavailable: {error}");
        }
        Err(error) => {
            let store = ErrorClass::Harness("store".into());
            return refuse(args.json, 1, &store, &error.to_string());
        }
    }
    let schema = match crate::schema(args) {
        Some(Ok(schema)) => Some(schema),
        Some(Err(error)) => return refuse(args.json, 2, &config, &error),
        None => None,
    };
    let lane = session.lane();
    let reserve = args.deadline.map_or(SHUTDOWN_GRACE, |secs| {
        SHUTDOWN_GRACE.min(Duration::from_secs(secs) / 16)
    });
    let ends = deadline.and_then(|at| at.checked_sub(reserve));
    let modes = Modes {
        json: args.json,
        eval: args.eval,
        no_deltas: args.no_deltas,
        headless: !interactive,
    };
    let json = args.json;
    let code = runtime.block_on(async move {
        if session
            .prompt_message(yi_runtime::session::user_input(&prompt))
            .is_err()
        {
            return refuse(json, 1, &ErrorClass::Harness("busy".into()), "session busy");
        }
        stream(&session, &host, modes, schema.as_ref(), (ends, deadline)).await
    });
    release_lane(lane.as_deref());
    runtime.shutdown_timeout(left(deadline));
    yi_runtime::spend::drain_settled(deadline);
    code
}

/// Invariant: a `--json` run that ends before the loop still closes with a `final` line.
fn early_final(json: bool, code: i32, class: &ErrorClass, message: &str) -> i32 {
    if json {
        println!("{}", error_final(class, message));
    }
    code
}

fn refuse(json: bool, code: i32, class: &ErrorClass, message: &str) -> i32 {
    eprintln!("error: {message}");
    early_final(json, code, class, message)
}

fn error_final(class: &ErrorClass, message: &str) -> serde_json::Value {
    serde_json::json!({"type": "final", "kind": "error", "class": class.to_string(), "message": message})
}

/// D10: `--schema` answers are JSON or a non-zero exit, never prose. The answer is the newest
/// assistant message that holds a matching value: a reader narrates before tool calls and wraps up after.
fn emit_structured(schema: &yi_runtime::schema::Schema, said: &[String], json: bool) -> i32 {
    let mut newest_error = None;
    for text in said.iter().rev() {
        let checked = yi_runtime::schema::extract(text).and_then(|value| {
            let valid = schema.validate(&value);
            valid
                .map(|()| value)
                .map_err(|error| format!("answer does not match --schema: {error}"))
        });
        match checked {
            Ok(value) => {
                if !json {
                    crate::print_json(&value);
                }
                return 0;
            }
            Err(error) => {
                newest_error.get_or_insert(error);
            }
        }
    }
    let error = newest_error.unwrap_or_else(|| "answer contains no JSON value".to_owned());
    eprintln!("error: {error}");
    // Incident: a void review round's only evidence was this line; the forge could not keep the session.
    if let Some(text) = said
        .iter()
        .rev()
        .map(|text| text.trim())
        .find(|text| !text.is_empty())
    {
        let (head, total) = (
            text.chars().take(160).collect::<String>(),
            text.chars().count(),
        );
        let kept = total.min(160);
        eprintln!(
            "the newest message began: {head:?} ({kept} of {total} characters; the session file holds the rest)"
        );
    }
    3
}

#[derive(Clone, Copy)]
struct Modes {
    json: bool,
    eval: bool,
    no_deltas: bool,
    headless: bool,
}

fn left(deadline: Option<Instant>) -> Duration {
    deadline.map_or(SHUTDOWN_GRACE, |at| {
        SHUTDOWN_GRACE.min(at.saturating_duration_since(Instant::now()) / 2)
    })
}

fn holds(working: bool, ends: Option<Instant>) -> bool {
    working && ends.is_none_or(|at| Instant::now() < at)
}

fn patience(holding: bool, ends: Option<Instant>) -> Option<Duration> {
    let left = ends.map(|at| at.saturating_duration_since(Instant::now()));
    match (holding, left) {
        (true, left) => Some(left.map_or(HOLD_POLL, |left| left.min(HOLD_POLL))),
        (false, left) => left,
    }
}

async fn stream(
    session: &yi_runtime::AgentSession,
    host: &yi_runtime::SubagentHost,
    modes: Modes,
    schema: Option<&yi_runtime::schema::Schema>,
    (ends, deadline): (Option<Instant>, Option<Instant>),
) -> i32 {
    let held = || host.holds_owner();
    let working = || host.holds_owner() || session.status() != Status::Idle;
    let (code, last) = follow(session.subscribe(), [&held, &working], modes, schema, ends).await;
    // Incident: nothing drove the runtime after the deadline, so a looping cell outlived it.
    if session.status() != Status::Idle {
        session.abort();
        let _still_busy_is_left_to_the_shutdown =
            tokio::time::timeout(left(deadline), session.wait_idle()).await;
    }
    let json = modes.json;
    let status = host.status();
    let members = status.get("members").and_then(serde_json::Value::as_array);
    let services: Vec<String> = members
        .into_iter()
        .flatten()
        .filter(|member| member["service"] == true)
        .filter_map(|member| member["name"].as_str().map(str::to_owned))
        .collect();
    let family = Family {
        members: &host.states(),
        services: &services,
        requests: &host.open_requests(),
    };
    if let Some((said, line)) = leftovers(&family, session.pending_count()) {
        eprintln!("warning: {said}");
        if json {
            println!("{line}");
        }
    }
    if json {
        println!("{last}");
    }
    code
}

struct Family<'a> {
    members: &'a [yi_runtime::family::MemberView],
    services: &'a [String],
    requests: &'a [(String, String, String)],
}

fn leftovers(family: &Family<'_>, undelivered: usize) -> Option<(String, serde_json::Value)> {
    use yi_runtime::family::MemberState;
    let live: Vec<_> = family
        .members
        .iter()
        .filter(|view| !matches!(view.state, MemberState::Finished | MemberState::Failed))
        .map(|view| (view.name.as_str(), view.state.as_str()))
        .collect();
    let (services, requests) = (family.services, family.requests);
    if live.is_empty() && undelivered == 0 && services.is_empty() && requests.is_empty() {
        return None;
    }
    let named: Vec<String> = live
        .iter()
        .map(|(name, state)| format!("{name} ({state})"))
        .collect();
    let asked: Vec<String> = requests
        .iter()
        .map(|(id, asker, respondent)| format!("{id} {asker} → {respondent}"))
        .collect();
    let said = format!(
        "the run ends with {} live child(ren) [{}], {} service(s) [{}], {} open request(s) [{}] and {undelivered} undelivered message(s); they end with it",
        live.len(),
        named.join(", "),
        services.len(),
        services.join(", "),
        requests.len(),
        asked.join(", ")
    );
    let children: Vec<_> = live
        .iter()
        .map(|(name, state)| serde_json::json!({"name": name, "state": state}))
        .collect();
    let requests: Vec<_> = requests
        .iter()
        .map(|(id, asker, respondent)| serde_json::json!({"id": id, "from": asker, "to": respondent}))
        .collect();
    let line = serde_json::json!({"type": "leftovers", "children": children, "services": services,
        "requests": requests, "undelivered": undelivered});
    Some((said, line))
}

const EXIT_QUOTA: i32 = 4;

/// The reset instant is the first `YYYY-MM-DDTHH:MM:SSZ` in the provider's message.
fn reset_instant(text: &str) -> Option<&str> {
    const SHAPE: &[u8; 20] = b"dddd-dd-ddTdd:dd:ddZ";
    text.char_indices().find_map(|(at, _)| {
        let window = text.get(at..at.checked_add(SHAPE.len())?)?;
        let fits = window.bytes().zip(SHAPE).all(|(byte, want)| match want {
            b'd' => byte.is_ascii_digit(),
            want => byte == *want,
        });
        fits.then_some(window)
    })
}

/// Best effort: the first window the text names, whole words only (`15h` and `17d` name nothing).
fn quota_window(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let unit_next = |at: usize, units: [&str; 2]| {
        words
            .get(at.saturating_add(1))
            .is_some_and(|next| units.contains(next))
    };
    words.iter().enumerate().find_map(|(at, word)| match *word {
        "5h" => Some("5h"),
        "7d" | "week" | "weekly" => Some("7d"),
        "5" if unit_next(at, ["hour", "hours"]) => Some("5h"),
        "7" if unit_next(at, ["day", "days"]) => Some("7d"),
        _ => None,
    })
}

/// The last event of a json run: why it stopped, in one closed vocabulary a driver can branch on.
fn final_event(deadline: bool, answered: bool, error: Option<&str>) -> serde_json::Value {
    let class = error.map(ErrorClass::from_provider_text);
    let kind = match (&class, deadline, answered) {
        (Some(ErrorClass::Provider(what)), ..) if what == "quota" => "quota",
        (Some(ErrorClass::TransportTimeout), ..) => "timeout",
        (Some(_), ..) => "error",
        (None, true, _) => "deadline",
        (None, false, true) => "stop",
        (None, false, false) => "no_answer",
    };
    let mut line = serde_json::json!({"type": "final", "kind": kind});
    if let (Some(class), Some(text)) = (class, error) {
        line["class"] = class.to_string().into();
        line["message"] = text.into();
    }
    if kind == "quota"
        && let Some(text) = error
    {
        line["window"] = quota_window(text).into();
        line["reset"] = reset_instant(text).into();
    }
    line
}

/// Incident: a plan child's finish wakes the owner, so it waits, but only on what wakes it (#489).
async fn follow(
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    [held, working]: [&dyn Fn() -> bool; 2],
    Modes {
        json,
        eval,
        no_deltas,
        headless,
    }: Modes,
    schema: Option<&yi_runtime::schema::Schema>,
    ends: Option<Instant>,
) -> (i32, serde_json::Value) {
    let (mut answer, mut said, mut exit) = (String::new(), Vec::new(), 0);
    let (mut holding, mut ended) = (false, false);
    let (mut last_stop, mut said_anything, mut due) = (None, false, false);
    let (mut last_error, mut finished, mut schema_failed) = (None, false, false);
    let (mut calls, mut denied) = (std::collections::HashMap::new(), None);
    loop {
        let next = match patience(holding, ends) {
            None => yi_runtime::next_event(&mut events).await,
            Some(wait) if wait.is_zero() => {
                ended = true;
                break;
            }
            Some(wait) => {
                match tokio::time::timeout(wait, yi_runtime::next_event(&mut events)).await {
                    Ok(next) => next,
                    Err(_) if holds(holding && working(), ends) => continue,
                    Err(_) => {
                        ended = true;
                        break;
                    }
                }
            }
        };
        let event = match next {
            Some(Ok(event)) => event,
            // A slow reader is not the end of the run: breaking here exited 0 mid-turn.
            Some(Err(gap)) => {
                match serde_json::to_string(&gap) {
                    Ok(line) if json => println!("{line}"),
                    _ => eprintln!(
                        "warning: {} events dropped behind a slow reader",
                        gap.dropped
                    ),
                }
                continue;
            }
            None => break,
        };
        let delta = matches!(event, AgentEvent::MessageUpdate { .. });
        if json
            && !(no_deltas && delta)
            && let Ok(line) = serde_json::to_string(&event)
        {
            println!("{line}");
        }
        if let Some(chunk) = render_text(&event) {
            if schema.is_some() {
                answer.push_str(&chunk);
            } else if !json {
                let apart = if std::mem::take(&mut due) { "\n\n" } else { "" };
                print!("{apart}{chunk}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
        if let AgentEvent::MessageEnd {
            message:
                AgentMessage::Assistant {
                    stop_reason,
                    content,
                    error_message,
                    ..
                },
        } = &event
        {
            last_error = (*stop_reason == StopReason::Error).then(|| {
                error_message
                    .clone()
                    .unwrap_or_else(|| "provider error".to_owned())
            });
            let text = yi_types::message::has_text(content);
            said_anything |= text;
            due |= text;
            last_stop = Some((*stop_reason, text));
            said.push(std::mem::take(&mut answer));
        }
        match &event {
            AgentEvent::ToolExecutionStart {
                tool_call_id, args, ..
            } => {
                calls.insert(tool_call_id.clone(), args.clone());
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let args = calls.remove(tool_call_id);
                let refused = *is_error
                    && yi_types::event::ToolErrorKind::from_details(&result.details)
                        == Some(yi_types::event::ToolErrorKind::Denied.as_str());
                denied = refused.then(|| {
                    let reason = yi_types::message::join_text(&result.content, "\n");
                    (tool_name.clone(), args.unwrap_or_default(), reason)
                });
            }
            AgentEvent::MessageEnd {
                message:
                    AgentMessage::Assistant {
                        stop_reason: StopReason::Error,
                        error_message,
                        ..
                    },
            } if !json => {
                eprintln!(
                    "error: {}",
                    error_message.as_deref().unwrap_or("provider error")
                );
            }
            AgentEvent::Wait { wait: Some(wait) } => {
                if let Some(warning) = wait.warning() {
                    eprintln!("{warning}");
                }
            }
            AgentEvent::AgentStart if holding => {
                holding = false;
                said.clear();
            }
            AgentEvent::AgentEnd { .. } if held() => holding = true,
            AgentEvent::AgentEnd { .. } => {
                (ended, finished) = (true, true);
                break;
            }
            _ => {}
        }
    }
    if ended && let Some(schema) = schema {
        exit = emit_structured(schema, &said, json);
        schema_failed = exit != 0;
    } else if ended && !json {
        println!();
    }
    if ended
        && headless
        && exit == 0
        && let Some((tool, args, reason)) = denied
    {
        eprintln!("error: the run ended on a denied {tool} call {args}: {reason}");
        if json {
            println!(
                "{}",
                serde_json::json!({"type": "denied", "tool": tool, "args": args, "reason": reason})
            );
        }
        exit = EXIT_DENIED;
    }
    let final_answer = matches!(last_stop, Some((StopReason::Stop, true)));
    if ended && eval && !final_answer {
        let stop = last_stop.map(|(stop, _)| stop);
        eprintln!("warning: the run ended without a final answer");
        if json {
            println!(
                "{}",
                serde_json::json!({"type": "no_answer", "lastStop": stop, "saidAnything": said_anything})
            );
        }
    } else if ended && !said_anything && exit == 0 {
        eprintln!("error: the run ended with no assistant text at all");
        exit = 1;
    } else if !json && exit == 0 && matches!(last_stop, Some((StopReason::Error, _))) {
        exit = 1;
    }
    // Incident: a hold that expired with no `--deadline` set closed as kind `deadline`.
    let deadline = !finished && ends.is_some_and(|at| Instant::now() >= at);
    let mut line = final_event(deadline, final_answer, last_error.as_deref());
    if exit == EXIT_DENIED && matches!(line["kind"].as_str(), Some("stop" | "no_answer")) {
        let class = ErrorClass::Harness("denied".to_owned());
        line = error_final(&class, "the run ended on a denied tool call");
    }
    // The final line describes the run's exit, so a clean stop whose answer broke the schema is an error.
    if schema_failed && matches!(line["kind"].as_str(), Some("stop" | "no_answer")) {
        let class = ErrorClass::Harness("schema".to_owned());
        line = error_final(&class, "answer does not match --schema");
    }
    if line["kind"] == "quota" {
        exit = EXIT_QUOTA;
    }
    (exit, line)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use yi_types::event::AgentEvent;

    /// Dies with no leftovers report: `mbx-detach` exited 0 with its child mid-work, silently.
    /// Dies too with an idle service and an open request left unnamed (`mbx-service`).
    #[test]
    fn a_run_that_ends_with_a_live_child_names_it() -> Result<(), Box<dyn std::error::Error>> {
        use yi_runtime::family::{MemberState, MemberView};
        let view = |name: &str, state| MemberView {
            name: name.to_owned(),
            state,
            note: None,
            tools: 0,
            tokens: 0,
            cost: 0.0,
            idle_s: 0,
            worktree: None,
        };
        let members = [
            view("slow", MemberState::Running),
            view("done", MemberState::Finished),
        ];
        let family = |members| super::Family {
            members,
            services: &[],
            requests: &[],
        };
        let (said, line) = super::leftovers(&family(&members), 1).ok_or("nothing reported")?;
        assert!(
            said.contains("slow (running)") && !said.contains("done"),
            "{said}"
        );
        assert_eq!(line["children"][0]["name"], "slow");
        assert_eq!(line["undelivered"], 1);
        assert!(
            super::leftovers(&family(&members[1..]), 0).is_none(),
            "a clean end says nothing"
        );
        let open = [(
            "parent-1".to_owned(),
            "parent".to_owned(),
            "tally".to_owned(),
        )];
        let serving = super::Family {
            members: &members[1..],
            services: &["tally".to_owned()],
            requests: &open,
        };
        let (said, line) = super::leftovers(&serving, 0).ok_or("a service and a request")?;
        assert!(
            said.contains("1 service(s) [tally]") && said.contains("parent-1 parent → tally"),
            "{said}"
        );
        assert_eq!(line["requests"][0]["to"], "tally");
        Ok(())
    }

    /// Dies with the deadline unread: a finish that never settles holds `yi ask` open forever.
    #[test]
    fn a_hold_ends_at_the_deadline() {
        assert!(super::holds(true, None));
        assert!(!super::holds(true, Some(Instant::now())));
        assert!(!super::holds(false, None));
    }

    /// Dies with the deadline read only while holding: a Steer's owner turn ran unbounded.
    /// A run the deadline ends with no assistant text at all exits nonzero and says so.
    #[test]
    fn a_steered_turn_past_the_deadline_ends_the_run() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (events, receiver) = tokio::sync::broadcast::channel(8);
        events.send(AgentEvent::AgentEnd {
            messages: Vec::new(),
        })?;
        events.send(AgentEvent::AgentStart)?;
        let always = || true;
        let ends = Instant::now().checked_add(Duration::from_millis(50));
        let modes = super::Modes {
            json: true,
            eval: false,
            no_deltas: false,
            headless: false,
        };
        let run = super::follow(receiver, [&always, &always], modes, None, ends);
        let ran =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), run).await });
        assert_eq!(
            ran.ok().map(|run| (run.0, run.1["kind"].clone())),
            Some((1, "deadline".into())),
            "the steered turn outlived the deadline"
        );
        drop(events);
        Ok(())
    }

    /// Dies with a final answer demanded of every run: a length stop with text exited 1, and
    /// an eval run with none exited 1, which harbor reads as the agent crashing.
    #[test]
    fn only_a_run_with_no_text_at_all_exits_nonzero_and_never_under_eval()
    -> Result<(), Box<dyn std::error::Error>> {
        use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
        use yi_types::message::StopReason;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let never = || false;
        let cut = faux_assistant_message(vec![faux_text("half an answer")], StopReason::Length);
        let call = faux_tool_call("c1", "bash", serde_json::Map::new());
        let call = faux_assistant_message(vec![call], StopReason::ToolUse);
        let cases = [(cut, false, 0), (call.clone(), false, 1), (call, true, 0)];
        for (message, eval, code) in cases {
            let (events, receiver) = tokio::sync::broadcast::channel(8);
            events.send(AgentEvent::MessageEnd { message })?;
            events.send(AgentEvent::AgentEnd {
                messages: Vec::new(),
            })?;
            let modes = super::Modes {
                json: true,
                eval,
                no_deltas: false,
                headless: false,
            };
            let run = super::follow(receiver, [&never, &never], modes, None, None);
            assert_eq!(runtime.block_on(run).0, code, "eval {eval}");
        }
        Ok(())
    }

    /// Dies with any provider error setting the exit: a stream error the loop retried and
    /// answered past still exited 1 in plain mode, which a script reads as a failed run.
    #[test]
    fn a_provider_error_the_run_recovered_from_exits_zero() -> Result<(), Box<dyn std::error::Error>>
    {
        use yi_runtime::faux::{faux_assistant_message, faux_text};
        use yi_types::message::StopReason;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let never = || false;
        let error = faux_assistant_message(Vec::new(), StopReason::Error);
        let answer = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
        let cases = [
            (vec![error.clone(), answer.clone()], 0),
            (vec![answer, error], 1),
        ];
        for (messages, code) in cases {
            let (events, receiver) = tokio::sync::broadcast::channel(8);
            for message in messages {
                events.send(AgentEvent::MessageEnd { message })?;
            }
            events.send(AgentEvent::AgentEnd {
                messages: Vec::new(),
            })?;
            let modes = super::Modes {
                json: false,
                eval: false,
                no_deltas: false,
                headless: false,
            };
            let run = super::follow(receiver, [&never, &never], modes, None, None);
            assert_eq!(runtime.block_on(run).0, code);
        }
        Ok(())
    }

    /// Dies with a quota 429 read as plain transport: the run exited 1 or 0 with no reset time,
    /// and a driver could not tell the 5-hour window from a crash.
    #[test]
    fn a_quota_refusal_ends_the_run_with_its_window_and_exit_code()
    -> Result<(), Box<dyn std::error::Error>> {
        use yi_runtime::faux::faux_assistant_message;
        use yi_types::message::StopReason;
        let text = "HTTP 429 quota exhausted: {\"error\":{\"message\":\"You have reached the 5 hour usage limit, resets at 2026-10-27T18:00:00Z\",\"type\":\"exceeded_current_quota_error\"}}";
        let line = super::final_event(false, false, Some(text));
        assert_eq!(line["kind"], "quota");
        assert_eq!(line["window"], "5h");
        assert_eq!(line["reset"], "2026-10-27T18:00:00Z");
        let both = "quota exhausted: the 5 hour limit is spent; the weekly limit is fine";
        assert_eq!(super::quota_window(both), Some("5h"));
        assert_eq!(
            super::quota_window("quota exhausted, retry in 15h or 17d"),
            None
        );
        let timeout = super::final_event(
            false,
            false,
            Some("Network Error: timed out reading response"),
        );
        assert_eq!(timeout["kind"], "timeout");
        assert_eq!(super::final_event(true, true, None)["kind"], "deadline");
        assert_eq!(super::final_event(false, true, None)["kind"], "stop");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let never = || false;
        let mut message = faux_assistant_message(Vec::new(), StopReason::Error);
        if let yi_types::message::AgentMessage::Assistant { error_message, .. } = &mut message {
            *error_message = Some(text.to_owned());
        }
        for json in [true, false] {
            let (events, receiver) = tokio::sync::broadcast::channel(8);
            events.send(AgentEvent::MessageEnd {
                message: message.clone(),
            })?;
            events.send(AgentEvent::AgentEnd {
                messages: Vec::new(),
            })?;
            let modes = super::Modes {
                json,
                eval: false,
                no_deltas: false,
                headless: true,
            };
            let run = super::follow(receiver, [&never, &never], modes, None, None);
            assert_eq!(runtime.block_on(run).0, super::EXIT_QUOTA, "json {json}");
        }
        Ok(())
    }

    /// Dies with an interactive user's own "no" read as a refusal nobody could answer, or a
    /// denial the model got past held against the run: only a headless run that ends on one exits 4.
    #[test]
    fn only_a_headless_run_that_ends_on_a_refused_call_exits_4()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;
        use yi_runtime::faux::{faux_assistant_message, faux_text};
        use yi_types::message::StopReason;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let never = || false;
        let answer = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
        let cases = [
            (true, vec![true], 4),
            (false, vec![true], 0),
            (true, vec![true, false], 0),
        ];
        for (headless, calls, code) in cases {
            let (events, receiver) = tokio::sync::broadcast::channel(16);
            for (n, refused) in calls.into_iter().enumerate() {
                let (id, kind) = (format!("c{n}"), refused.then_some("denied"));
                let start = json!({"type": "tool_execution_start", "toolCallId": id,
                    "toolName": "bash", "args": {"command": format!("run {n}")}});
                let end = json!({"type": "tool_execution_end", "toolCallId": id, "toolName": "bash",
                    "result": {"content": [], "details": {"errorKind": kind}}, "isError": refused});
                for wire in [start, end] {
                    events.send(serde_json::from_value(wire)?)?;
                }
            }
            events.send(AgentEvent::MessageEnd {
                message: answer.clone(),
            })?;
            events.send(AgentEvent::AgentEnd {
                messages: Vec::new(),
            })?;
            let modes = super::Modes {
                json: true,
                eval: false,
                no_deltas: false,
                headless,
            };
            let run = super::follow(receiver, [&never, &never], modes, None, None);
            assert_eq!(runtime.block_on(run).0, code, "headless {headless}");
        }
        Ok(())
    }

    /// Dies with `final` built from the loop alone: a clean stop whose answer broke `--schema`
    /// exited 1 yet closed as `stop`, which a driver branching on `kind` read as success.
    #[test]
    fn a_schema_failure_closes_the_run_as_an_error() -> Result<(), Box<dyn std::error::Error>> {
        use yi_runtime::faux::{faux_assistant_message, faux_text};
        use yi_types::message::StopReason;
        let path =
            std::env::temp_dir().join(format!("yi-final-schema-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"type":"object","required":["n"]}"#)?;
        let schema = yi_runtime::schema::Schema::load(&path.to_string_lossy())?;
        std::fs::remove_file(&path)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let never = || false;
        let answer = faux_assistant_message(vec![faux_text("{\"m\":1}")], StopReason::Stop);
        let (events, receiver) = tokio::sync::broadcast::channel(8);
        events.send(AgentEvent::MessageEnd { message: answer })?;
        events.send(AgentEvent::AgentEnd {
            messages: Vec::new(),
        })?;
        let modes = super::Modes {
            json: true,
            eval: false,
            no_deltas: false,
            headless: true,
        };
        let run = super::follow(receiver, [&never, &never], modes, Some(&schema), None);
        let (code, line) = runtime.block_on(run);
        assert_ne!(code, 0);
        assert_eq!(
            (&line["kind"], &line["class"]),
            (&"error".into(), &"harness:schema".into())
        );
        Ok(())
    }

    /// Dies with a later provider error replacing the distinct refusal exit a script depends on.
    #[test]
    fn a_refusal_followed_by_a_provider_error_still_exits_4()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;
        use yi_runtime::faux::faux_assistant_message;
        use yi_types::message::StopReason;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (events, receiver) = tokio::sync::broadcast::channel(8);
        for wire in [
            json!({"type": "tool_execution_end", "toolCallId": "c0", "toolName": "bash",
                "result": {"content": [], "details": {"errorKind": "denied"}}, "isError": true}),
            serde_json::to_value(AgentEvent::MessageEnd {
                message: faux_assistant_message(Vec::new(), StopReason::Error),
            })?,
            serde_json::to_value(AgentEvent::AgentEnd {
                messages: Vec::new(),
            })?,
        ] {
            events.send(serde_json::from_value(wire)?)?;
        }
        let never = || false;
        let modes = super::Modes {
            json: false,
            eval: false,
            no_deltas: false,
            headless: true,
        };
        assert_eq!(
            runtime
                .block_on(super::follow(receiver, [&never, &never], modes, None, None))
                .0,
            4
        );
        Ok(())
    }

    /// Dies with every run that was not closed by `AgentEnd` called a deadline: the owner's
    /// hold expired once its children finished, no `--deadline` was set, and `final` said so.
    #[test]
    fn a_hold_that_expires_closes_the_run_as_a_stop_not_a_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        use yi_runtime::faux::{faux_assistant_message, faux_text};
        use yi_types::message::StopReason;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let answer = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
        let (events, receiver) = tokio::sync::broadcast::channel(8);
        events.send(AgentEvent::MessageEnd { message: answer })?;
        events.send(AgentEvent::AgentEnd {
            messages: Vec::new(),
        })?;
        let (held, never) = (std::cell::Cell::new(true), || false);
        let holds_once = || held.replace(false);
        let modes = super::Modes {
            json: true,
            eval: false,
            no_deltas: false,
            headless: true,
        };
        let run = super::follow(receiver, [&holds_once, &never], modes, None, None);
        let (code, line) = runtime.block_on(run);
        assert_eq!((code, &line["kind"]), (0, &"stop".into()));
        Ok(())
    }

    /// Dies with half of `--deadline`'s remainder: a blocking request held an early exit.
    #[test]
    fn the_runtime_shutdown_never_waits_past_its_grace() {
        let far = Instant::now().checked_add(Duration::from_secs(600));
        assert!(super::left(far) <= super::SHUTDOWN_GRACE);
        let near = Instant::now().checked_add(Duration::from_secs(4));
        assert!(super::left(near) <= Duration::from_secs(2));
    }
}
