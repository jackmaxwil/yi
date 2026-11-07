use std::path::Path;
use std::sync::Arc;

use crate::session::AgentSession;
use crate::subagent::{ChildBuild, ChildFactory};
use crate::wiring::{RuntimeWiring, attach_runtime};
/// A child is a fresh session with its own extensions and cwd, on its family's stream (D314).
pub(crate) fn child_factory(
    wiring: RuntimeWiring,
    screen: crate::classifier::Screeners,
) -> Arc<ChildFactory> {
    Arc::new(move |build: ChildBuild<'_>| {
        let provider = Arc::new(wiring.provider.for_child());
        let child_cwd = build
            .cwd
            .map_or_else(|| wiring.cwd.clone(), Path::to_path_buf);
        let below = RuntimeWiring {
            broker: child_broker(&wiring, &build.wall, &child_cwd),
            depth: wiring.depth.saturating_add(1),
            rlm_dir: build.session_dir.to_path_buf(),
            family_dir: Some(wiring.family_dir()),
            cwd: child_cwd.clone(),
            parent_link: Some(build.link.clone()),
            wall: build.wall.clone(),
            deadline: build.deadline,
            kernel_prewarm: false,
            provider: Arc::clone(&provider),
            ..wiring.clone()
        };
        if let Some(reader) = build.reader.clone() {
            let rules = crate::rules::discover_armed(&child_cwd, &wiring.home).rules;
            let rules = Some(Arc::new(crate::rules::RuleEngine::new(rules)));
            let kernel = |child: &AgentSession| Some(crate::wiring::worker_ipython(child, &below));
            let child = crate::subagent::reader::session(
                provider,
                build,
                &reader,
                (|| (wiring.tools)(), kernel),
                (child_cwd, &wiring.home),
                below.broker.clone(),
                rules,
            );
            screen.hand_down(&child.screeners(), crate::classifier::journal(&child));
            return Ok(child);
        }
        let mut child = AgentSession::new(
            crate::session::SessionConfig {
                system_prompt: wiring.system_prompt.clone(),
                model: build.model,
                thinking_level: build.thinking,
                tool_execution: wiring.tool_execution,
            },
            Arc::clone(&provider),
        );
        child.install_extensions(crate::ext::install(crate::ext::ExtOptions {
            cwd: child_cwd.clone(),
            home: wiring.home.clone(),
            mode: wiring
                .broker
                .as_ref()
                .map_or(yi_permission::PermissionMode::Auto, |broker| broker.mode()),
            user_system: String::new(),
            schema_instruction: None,
            context_window: child.model().context_window,
            global_skills: Vec::new(),
            can_delegate: wiring.depth.saturating_add(1) < wiring.max_depth,
        }));
        screen.hand_down(&child.screeners(), crate::classifier::journal(&child));
        let host = attach_runtime(&mut child, below);
        host.set_grant(child.wall(), build.tokens);
        Ok(child)
    })
}

/// Rules flow down only: a child's broker starts from a copy of its parent's (#600).
fn child_broker(
    wiring: &RuntimeWiring,
    wall: &crate::wall::Wall,
    cwd: &Path,
) -> Option<Arc<crate::permission::PermissionBroker>> {
    (wiring.broker.as_ref()).map(|broker| Arc::new(broker.for_child(wall, cwd)))
}
