use std::time::{Duration, Instant};

use yi_runtime::Status;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};

use super::{
    Args, Resume, attach_store, build_session, emit_structured, exit_refused, release_lane,
    render_text, session_target, tty,
};

const HOLD_POLL: Duration = Duration::from_millis(500);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

pub(super) fn run(args: &Args) -> i32 {
    use std::io::IsTerminal;
    let deadline = args
        .deadline
        .and_then(|secs| Instant::now().checked_add(Duration::from_secs(secs)));
    let interactive = std::io::stdin().is_terminal();
    // Incident: a review round's prompt passed Linux's 128 KiB cap on one argument (E2BIG).
    let prompt = match args.prompt.as_str() {
        "-" => match std::io::read_to_string(std::io::stdin()) {
            Ok(text) if !text.is_empty() => text,
            Ok(_) => {
                eprintln!("error: no prompt on stdin");
                return 2;
            }
            Err(error) => {
                eprintln!("error: reading the prompt from stdin: {error}");
                return 2;
            }
        },
        given => given.to_owned(),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    // Session wiring spawns runtime tasks, so the runtime context must exist before it.
    let target = session_target(args);
    let (session, host) = {
        let _guard = runtime.enter();
        let asker: Option<yi_runtime::Asker> =
            interactive.then(|| std::sync::Arc::new(tty::tty_ask) as yi_runtime::Asker);
        match build_session(args, asker, Some(&target.id)) {
            Ok(built) => built,
            Err(refused) => return exit_refused(refused),
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
            eprintln!("error: {error}");
            return 1;
        }
    }
    let json = args.json;
    let schema = match args.schema.as_deref().map(yi_runtime::schema::Schema::load) {
        Some(Ok(schema)) => Some(schema),
        Some(Err(error)) => {
            eprintln!("error: {error}");
            return 2;
        }
        None => None,
    };
    let lane = session.lane();
    let reserve = args.deadline.map_or(SHUTDOWN_GRACE, |secs| {
        SHUTDOWN_GRACE.min(Duration::from_secs(secs) / 16)
    });
    let ends = deadline.and_then(|at| at.checked_sub(reserve));
    let eval = args.eval;
    let code = runtime.block_on(async move {
        if session
            .prompt_message(yi_runtime::session::user_input(&prompt))
            .is_err()
        {
            eprintln!("error: session busy");
            return 1;
        }
        let modes = Modes { json, eval };
        stream(&session, &host, modes, schema.as_ref(), (ends, deadline)).await
    });
    release_lane(lane.as_deref());
    runtime.shutdown_timeout(left(deadline));
    code
}

#[derive(Clone, Copy)]
struct Modes {
    json: bool,
    eval: bool,
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
    let code = follow(session.subscribe(), [&held, &working], modes, schema, ends).await;
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

/// Incident: a plan child's finish wakes the owner, so it waits, but only on what wakes it (#489).
async fn follow(
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    [held, working]: [&dyn Fn() -> bool; 2],
    Modes { json, eval }: Modes,
    schema: Option<&yi_runtime::schema::Schema>,
    ends: Option<Instant>,
) -> i32 {
    let (mut answer, mut exit) = (String::new(), 0);
    let (mut holding, mut ended) = (false, false);
    let (mut last_stop, mut said_anything) = (None, false);
    loop {
        let next = match patience(holding, ends) {
            None => events.recv().await,
            Some(wait) if wait.is_zero() => {
                ended = true;
                break;
            }
            Some(wait) => match tokio::time::timeout(wait, events.recv()).await {
                Ok(next) => next,
                Err(_) if holds(holding && working(), ends) => continue,
                Err(_) => {
                    ended = true;
                    break;
                }
            },
        };
        let event = match next {
            Ok(event) => event,
            // A slow reader is not the end of the run: breaking here exited 0 mid-turn.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                eprintln!("warning: {missed} events dropped behind a slow reader");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if json && let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
        if let Some(chunk) = render_text(&event) {
            if schema.is_some() {
                answer.push_str(&chunk);
            } else if !json {
                print!("{chunk}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
        if let AgentEvent::MessageEnd {
            message:
                AgentMessage::Assistant {
                    stop_reason,
                    content,
                    ..
                },
        } = &event
        {
            use yi_types::message::Content;
            let text = content.iter().any(
                |block| matches!(block, Content::Text { text, .. } if !text.trim().is_empty()),
            );
            said_anything |= text;
            last_stop = Some((*stop_reason, text));
        }
        match &event {
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
                answer.clear();
            }
            AgentEvent::AgentEnd { .. } if held() => holding = true,
            AgentEvent::AgentEnd { .. } => {
                ended = true;
                break;
            }
            _ => {}
        }
    }
    if ended && let Some(schema) = schema {
        exit = emit_structured(schema, &answer, json);
    } else if ended && !json {
        println!();
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
    } else if !json && matches!(last_stop, Some((StopReason::Error, _))) {
        exit = 1;
    }
    exit
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
        };
        let run = super::follow(receiver, [&always, &always], modes, None, ends);
        let ran =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), run).await });
        assert_eq!(ran.ok(), Some(1), "the steered turn outlived the deadline");
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
            let modes = super::Modes { json: true, eval };
            let run = super::follow(receiver, [&never, &never], modes, None, None);
            assert_eq!(runtime.block_on(run), code, "eval {eval}");
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
            };
            let run = super::follow(receiver, [&never, &never], modes, None, None);
            assert_eq!(runtime.block_on(run), code);
        }
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
