use super::{
    DeliveryMode, HeartbeatService, JobSpec, JobStatus, new_job, next_run_at_for_schedule,
    normalize_heartbeat_schedule, parse_schedule,
};
use yi_types::schedule::{CatchUp, Job, Overlap};

fn policy(
    job: &mut Job,
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let word = |key: &str| payload.get(key).and_then(serde_json::Value::as_str);
    let overlap = match word("overlap") {
        None => None,
        Some("skip") => Some(Overlap::Skip),
        Some("buffer_one") => Some(Overlap::BufferOne),
        Some("allow") => Some(Overlap::Allow),
        Some(other) => {
            return Err(format!(
                "overlap {other:?} is not skip, buffer_one or allow"
            ));
        }
    };
    let catch_up = match word("catchUp") {
        None => None,
        Some("once") => Some(CatchUp::Once),
        Some("skip") => Some(CatchUp::Skip),
        Some("all") => Some(CatchUp::All),
        Some(other) => return Err(format!("catchUp {other:?} is not once, skip or all")),
    };
    if let Some(value) = payload.get("intent") {
        job.intent = serde_json::from_value(value.clone())
            .map_err(|error| format!("intent must be a list of user://<n> addresses: {error}"))?;
    }
    job.overlap = overlap;
    job.catch_up = catch_up;
    Ok(())
}

impl HeartbeatService {
    /// Registers the kernel-side vocabulary (design §15.2): list, create,
    /// update (pause/resume), delete.
    pub fn register(self: &std::sync::Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let list = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.list", move |_payload| {
            let state = list.store.snapshot();
            let jobs: Vec<_> = state
                .jobs
                .iter()
                .filter(|job| {
                    job.source == Some(yi_types::schedule::JobSource::RlmHeartbeat)
                        && list.owns(job)
                })
                .collect();
            let reply = serde_json::json!({"jobs": jobs})
                .as_object()
                .cloned()
                .unwrap_or_default();
            Box::pin(async move { Ok(reply) })
        });
        let create = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.create", move |payload| {
            let result = (|| {
                let schedule_text = payload
                    .get("schedule")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.create requires a schedule")?;
                let prompt = payload
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.create requires a prompt")?;
                let label = payload
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                let delivery = match payload
                    .get("deliveryMode")
                    .and_then(serde_json::Value::as_str)
                {
                    None => None,
                    Some("steer") => Some(DeliveryMode::Steer),
                    Some("follow_up") => Some(DeliveryMode::FollowUp),
                    Some(_) => {
                        return Err(
                            "Heartbeat delivery mode must be \"steer\" or \"follow_up\"".to_owned()
                        );
                    }
                };
                let now = yi_session::now_ms();
                let (parsed, next_run_at) =
                    parse_schedule(&normalize_heartbeat_schedule(Some(schedule_text)), now)?;
                let mut job = new_job(JobSpec {
                    id: format!("rhb-{}", crate::subagent::random_suffix()?),
                    session_id: create.bound_session_id()?,
                    cwd: create.cwd.clone(),
                    source: yi_types::schedule::JobSource::RlmHeartbeat,
                    delivery_mode: delivery,
                    label,
                    prompt: prompt.to_owned(),
                    schedule: parsed,
                    next_run_at,
                    now_ms: now,
                });
                policy(&mut job, &payload)?;
                if job.intent.is_empty() {
                    job.intent = create.latest_words();
                }
                create.store.mutate(|state| state.jobs.push(job.clone()));
                serde_json::json!({"job": job})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "serialization failed".to_owned())
            })();
            Box::pin(async move { result })
        });
        let update = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.update", move |payload| {
            let result = (|| {
                let id = payload
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.update requires an id")?
                    .to_owned();
                let target = match payload.get("status").and_then(serde_json::Value::as_str) {
                    Some("pause") => JobStatus::Paused,
                    Some("resume") => JobStatus::Active,
                    _ => {
                        return Err(
                            "rlm_heartbeat.update status must be \"pause\" or \"resume\""
                                .to_owned(),
                        );
                    }
                };
                let now = yi_session::now_ms();
                let owner = update.bound_session_id()?;
                let updated = update.store.mutate(|state| {
                    let found = state
                        .jobs
                        .iter_mut()
                        .find(|job| job.id == id && job.session_id == owner);
                    found.map(|job| {
                        job.status = target;
                        if target == JobStatus::Active && job.next_run_at.is_none() {
                            job.next_run_at =
                                next_run_at_for_schedule(&job.schedule, now).ok().flatten();
                        }
                        job.updated_at = now;
                        job.clone()
                    })
                });
                let job = updated.ok_or(format!("unknown heartbeat job: {id}"))?;
                serde_json::json!({"job": job})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "serialization failed".to_owned())
            })();
            Box::pin(async move { result })
        });
        let delete = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.delete", move |payload| {
            let result = (|| {
                let id = payload
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rlm_heartbeat.delete requires an id")?
                    .to_owned();
                let now = yi_session::now_ms();
                let owner = delete.bound_session_id()?;
                let found = delete.store.mutate(|state| {
                    let target = state
                        .jobs
                        .iter_mut()
                        .find(|job| job.id == id && job.session_id == owner);
                    target.map(|job| {
                        job.status = JobStatus::Cancelled;
                        job.next_run_at = None;
                        job.updated_at = now;
                    })
                });
                if found.is_none() {
                    return Err(format!("unknown heartbeat job: {id}"));
                }
                serde_json::json!({"deleted": true})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "serialization failed".to_owned())
            })();
            Box::pin(async move { result })
        });
    }
}
