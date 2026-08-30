use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::model::Model;

use crate::mailbox::{ParentLink, register_child_messaging};
use crate::session::AgentSession;
use crate::subagent::{
    ChildBuild, ChildFactory, DEFAULT_MAX_CHILDREN, SubagentHost, SubagentHostOptions,
};

/// A child is a fresh session: it runs its own extensions against its own cwd
/// and shares the universal cached prefix with its parent.
fn child_factory(wiring: RuntimeWiring) -> Arc<ChildFactory> {
    Arc::new(move |build: ChildBuild<'_>| {
        let mut child = AgentSession::new(
            crate::session::SessionConfig {
                system_prompt: wiring.system_prompt.clone(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: wiring.tool_execution,
            },
            Arc::clone(&wiring.provider),
        );
        let child_cwd = build
            .cwd
            .map_or_else(|| wiring.cwd.clone(), Path::to_path_buf);
        child.install_extensions(crate::ext::install(crate::ext::ExtOptions {
            cwd: child_cwd.clone(),
            home: wiring.home.clone(),
            mode: wiring
                .broker
                .as_ref()
                .map_or(yi_permission::PermissionMode::Auto, |broker| broker.mode()),
            user_system: String::new(),
            schema_instruction: None,
        }));
        attach_runtime(
            &mut child,
            RuntimeWiring {
                depth: wiring.depth.saturating_add(1),
                rlm_dir: build.session_dir.to_path_buf(),
                cwd: child_cwd,
                parent_link: Some(build.link),
                wall: build.wall,
                kernel_prewarm: false,
                ..wiring.clone()
            },
        );
        Ok(child)
    })
}

/// Carried again by every child one level deeper.
#[derive(Clone)]
pub struct RuntimeWiring {
    pub provider: Arc<crate::provider::ProviderStream>,
    pub system_prompt: String,
    pub tool_execution: yi_loop::ExecutionMode,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub broker: Option<Arc<crate::permission::PermissionBroker>>,
    pub tools: Arc<dyn Fn() -> Vec<Arc<dyn yi_tools::Tool>> + Send + Sync>,
    pub depth: u8,
    pub max_depth: u8,
    pub rlm_dir: PathBuf,
    /// §12 roles resolved to models; `None` keeps the session's own model.
    pub summarizer: Option<Model>,
    /// Naming `models.advisor` in config enables the LLM reviewer (D28).
    pub advisor: Option<Model>,
    /// Model consulted on a Write or Exec permission ask; `None` keeps admission fully
    /// deterministic, so no permission decision costs a model call (M7, D81).
    pub auto_review: Option<Model>,
    /// `plan.staleReminderTurns` config; None keeps the default.
    pub plan_stale_turns: Option<u64>,
    /// Set for a child: its B6 route back into the family that spawned it.
    pub parent_link: Option<ParentLink>,
    /// B1 reduction: paths this session may not touch (plan §3.4 wall).
    pub wall: crate::wall::Wall,
    /// D13 `bash.autoBackgroundMs`; None keeps every command in the turn.
    pub auto_background: Option<std::time::Duration>,
    /// `kernel.prewarm` (default true): boot the kernel in the background at
    /// session open. Children never prewarm — they spawn to run a cell now.
    pub kernel_prewarm: bool,
}

/// Every spawned child wires itself the same way at depth+1; the depth check in
/// [`SubagentHost::spawn`] is what terminates the recursion.
fn wire_schedule(
    session: &AgentSession,
    wiring: &RuntimeWiring,
    registry: &mut crate::kernel::HostRegistry,
) {
    let shared = crate::schedule::shared::intern(wiring.rlm_dir.join("scheduled-jobs.json"));
    let heartbeats_cwd = wiring.cwd.to_string_lossy().into_owned();
    let hook = session.heartbeat_hook();
    let busy = session.activity_handle();
    let deliver: Arc<crate::schedule::DeliverFn> = Arc::new(move |job| {
        let activity = crate::schedule::SessionActivity {
            is_streaming: busy(),
            ..Default::default()
        };
        if crate::schedule::should_defer(job, &activity) {
            return crate::schedule::RunOutcome::Skipped;
        }
        let mode = job
            .delivery_mode
            .unwrap_or(crate::schedule::DEFAULT_HEARTBEAT_DELIVERY_MODE);
        hook(
            crate::schedule::heartbeat_message(job, yi_session::now_ms()),
            mode,
        );
        crate::schedule::RunOutcome::Ran
    });
    let heartbeats = Arc::new(
        crate::schedule::HeartbeatService::new(Arc::clone(&shared.store), heartbeats_cwd)
            .with_lane(Arc::clone(&shared.hub), Arc::clone(&deliver)),
    );
    heartbeats.register(registry);
    session.set_schedule(Arc::clone(&heartbeats));
}

fn wire_goal(
    session: &AgentSession,
    registry: &mut crate::kernel::HostRegistry,
    plan_stale_turns: Option<u64>,
) {
    let service = crate::goal::attach_goal(session);
    service.register(registry);
    session.set_goal_service(service);
    let plan = crate::plan::attach_plan(session, plan_stale_turns);
    plan.register(registry);
    session.set_plan_service(plan);
}

fn wire_advisor(session: &AgentSession, wiring: &RuntimeWiring) {
    let hold_sink: Option<crate::advisor::HoldSink> = wiring.broker.as_ref().map(|broker| {
        let broker = Arc::clone(broker);
        Arc::new(move |advice: &yi_types::advisor::Advice| {
            if !broker.can_ask() {
                return false;
            }
            broker.insert_hold(yi_permission::Hold {
                pattern: advice.target.clone().unwrap_or_default(),
                reason: advice.text.clone(),
                source: yi_permission::HoldSource::Advisor,
                expires_at_ms: Some(yi_session::now_ms().saturating_add(3_600_000)),
            });
            true
        }) as crate::advisor::HoldSink
    });
    // V10: ADVISOR.md attention text, project-local, best-effort.
    let attention = std::fs::read_to_string(wiring.cwd.join("ADVISOR.md")).ok();
    let llm = wiring.advisor.clone().map(|model| {
        Arc::new(crate::advisor::review::LlmReviewer::new(
            Arc::clone(&wiring.provider),
            model,
            attention.clone(),
        ))
    });
    let advisor = crate::advisor::attach_advisor(
        session,
        crate::advisor::AdvisorConfig {
            attention,
            reviewer: llm.is_some(),
            rules_dir: Some(wiring.cwd.join(".yi/rules")),
            ..crate::advisor::AdvisorConfig::default()
        },
        crate::advisor::AdvisorDeps { hold_sink, llm },
    );
    session.set_advisor(advisor);
}

/// A job finishing between turns reports through the R3 follow-up queue, so the
/// model hears about it without a turn being interrupted.
fn wire_job_completions(session: &AgentSession) {
    let follow_up = session.follow_up_hook();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            for report in yi_tools::jobs::registry().take_finished() {
                follow_up(&format!(
                    "<async_result job=\"{}\" exit=\"{}\">{}\n{}</async_result>",
                    report.id,
                    report.exit_code.unwrap_or(-1),
                    report.command,
                    report.output
                ));
            }
        }
    });
}

pub fn attach_runtime(session: &mut AgentSession, wiring: RuntimeWiring) -> Arc<SubagentHost> {
    if session.compactor().is_none() {
        session.enable_compaction_with_summarizer(
            yi_context::Settings::default(),
            wiring.summarizer.clone(),
        );
    }
    crate::checkpoint::wire_turn_checkpoints(session, &wiring.home, &wiring.cwd);
    let mut registry = crate::kernel::HostRegistry::default();
    registry.register_mcp_stubs();
    if let Some(compactor) = session.compactor() {
        // compact.run only schedules and returns — running inline would abort
        // the turn whose cell awaits the reply (design §6).
        registry.register("compact.run", move |payload| {
            let instructions = payload
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
            compactor.schedule_with_instructions(instructions);
            Box::pin(async {
                let mut reply = Map::new();
                reply.insert("scheduled".to_owned(), Value::Bool(true));
                Ok(reply)
            })
        });
    }
    if let Some(status) = session.compact_status_handle() {
        registry.register("compact.status", move |_payload| {
            let status = status();
            Box::pin(async move {
                let mut reply = Map::new();
                reply.insert("tokens".to_owned(), Value::from(status.tokens));
                reply.insert(
                    "context_window".to_owned(),
                    Value::from(status.context_window),
                );
                reply.insert("percent".to_owned(), Value::from(status.percent));
                reply.insert("scheduled".to_owned(), Value::Bool(status.scheduled));
                Ok(reply)
            })
        });
    }
    let factory = child_factory(wiring.clone());
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: wiring.depth,
        max_depth: wiring.max_depth,
        max_children: DEFAULT_MAX_CHILDREN,
        parent_session_dir: wiring.rlm_dir.clone(),
        defaults: session.settings_handle(),
        factory,
        notice: session.notice_hook(),
        events: session.events_sender(),
        parent_messages: session.history_handle(),
        cwd: wiring.cwd.clone(),
        report: {
            let deliver = session.heartbeat_hook();
            Arc::new(move |message| {
                deliver(message, yi_types::schedule::DeliveryMode::Steer);
            })
        },
        attribute: session.attribution_handle(),
        store: session.store_handle(),
    }));
    host.register(&mut registry);
    if let Some(link) = wiring.parent_link.clone() {
        register_child_messaging(link, &host, &mut registry);
    }
    wire_schedule(session, &wiring, &mut registry);
    wire_goal(session, &mut registry, wiring.plan_stale_turns);
    let restore_notice = session.notice_hook();
    let service = Arc::new(crate::kernel::KernelService::new(
        crate::kernel::KernelServiceOptions {
            cwd: wiring.cwd.clone(),
            home: wiring.home.clone(),
            session_dir: Some(wiring.rlm_dir.clone()),
            host: Arc::new(registry),
            on_restore: Some(Arc::new(move |restore| {
                restore_notice(&crate::kernel::restore_notice_text(restore));
            })),
        },
    ));
    wire_advisor(session, &wiring);
    {
        let service = Arc::clone(&service);
        let notice = session.notice_hook();
        let store = session.store_handle();
        let advisor = session.advisor();
        session.set_on_compacted(Arc::new(move || {
            let service = Arc::clone(&service);
            let notice = Arc::clone(&notice);
            let handle = store();
            let file = handle
                .as_ref()
                .and_then(|store| yi_session::lock_session(store).file_path().cloned());
            notice(&crate::affordance::compacted(file.as_deref()));
            crate::advisor::note_last_compaction(advisor.as_deref(), handle.as_ref());
            tokio::spawn(async move {
                if let Some(text) = service.sync_after_compaction().await {
                    notice(&text);
                }
            });
        }));
    }
    if wiring.kernel_prewarm {
        let warm = Arc::clone(&service);
        tokio::spawn(async move { warm.prewarm().await });
    }
    let mut tools = (wiring.tools)();
    tools.push(crate::kernel::ipython_tool(service));
    crate::auto_review::wire(session, &wiring, &mut tools);
    if let (Some(plan), Some(advisor)) = (session.plan_service(), session.advisor()) {
        plan.set_on_change(Arc::new(move |plan| {
            advisor.request_review(Some(crate::plan::summary_line(plan)));
        }));
    }
    let rule_set = crate::rules::discover(&wiring.cwd, &wiring.home);
    if !rule_set.warnings.is_empty() {
        let notice = session.notice_hook();
        for warning in &rule_set.warnings {
            notice(warning);
        }
    }
    // Attached even with zero rules: the adapters capture this Arc when tools
    // are installed, so a rule promoted mid-session (V11) arms immediately.
    let engine = Arc::new(crate::rules::RuleEngine::new(rule_set.rules));
    crate::rules::attach_rules(session, Arc::clone(&engine));
    session.set_rules_engine(engine);
    session.set_wall(wiring.wall.clone());
    session.use_tools_with_background(
        tools,
        wiring.cwd.clone(),
        wiring.broker.clone(),
        wiring.auto_background,
    );
    wire_job_completions(session);
    host
}
