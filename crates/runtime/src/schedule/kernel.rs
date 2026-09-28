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

fn subscription(
    address: &str,
    payload: &serde_json::Map<String, serde_json::Value>,
    label: Option<String>,
    prompt: &str,
) -> Result<super::channel::Subscribe, String> {
    let ms = |key: &str| payload.get(key).and_then(serde_json::Value::as_u64);
    let retention = payload
        .get("retention")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| format!("retention must be {{count, ageMs}}: {error}"))?;
    Ok(super::channel::Subscribe {
        address: address.to_owned(),
        filter: payload
            .get("filter")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        batch: ms("batch").and_then(|size| u32::try_from(size).ok()),
        cadence_ms: ms("minIntervalMs").max(ms("windowMs")),
        retention,
        label,
        prompt: prompt.to_owned(),
    })
}

impl HeartbeatService {
    /// A clock job from `schedule` or a `clock://` address, or a channel subscription from any
    /// other address, with its policy and the words that set it up.
    fn create_job(
        &self,
        payload: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Job, String> {
        let address = payload.get("address").and_then(serde_json::Value::as_str);
        let schedule_text = address
            .and_then(|address| address.strip_prefix(super::clock::CLOCK_SCHEME))
            .or_else(|| payload.get("schedule").and_then(serde_json::Value::as_str));
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
                return Err("Heartbeat delivery mode must be \"steer\" or \"follow_up\"".to_owned());
            }
        };
        let now = yi_session::now_ms();
        let mut job = match (schedule_text, address) {
            (Some(schedule_text), _) => {
                let (parsed, next_run_at) =
                    parse_schedule(&normalize_heartbeat_schedule(Some(schedule_text)), now)?;
                new_job(JobSpec {
                    id: format!("rhb-{}", crate::subagent::random_suffix()?),
                    session_id: self.bound_session_id()?,
                    cwd: self.cwd.clone(),
                    source: yi_types::schedule::JobSource::RlmHeartbeat,
                    delivery_mode: delivery,
                    label,
                    prompt: prompt.to_owned(),
                    schedule: parsed,
                    next_run_at,
                    now_ms: now,
                })
            }
            (None, Some(address)) => {
                let spec = subscription(address, payload, label, prompt)?;
                let mut job = self.channel_job(&self.bound_session_id()?, spec, None, now)?;
                job.delivery_mode = delivery;
                job
            }
            (None, None) => {
                return Err("rlm_heartbeat.create requires a schedule or an address".to_owned());
            }
        };
        policy(&mut job, payload)?;
        if job.intent.is_empty() {
            job.intent = self.latest_words();
        }
        Ok(job)
    }

    /// Registers the kernel-side vocabulary (design §15.2): list, create,
    /// update (pause/resume), delete.
    pub fn register(self: &std::sync::Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let list = std::sync::Arc::clone(self);
        registry.register("rlm_heartbeat.list", move |_payload| {
            let state = list.store().snapshot();
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
                let job = create.create_job(&payload)?;
                create.store().mutate(|state| state.jobs.push(job.clone()));
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
                let updated = update.store().mutate(|state| {
                    let found = state
                        .jobs
                        .iter_mut()
                        .find(|job| job.id == id && job.session_id == owner);
                    found.map(|job| {
                        job.status = target;
                        if let (JobStatus::Active, Some(sub)) = (target, &job.channel) {
                            super::adapter::revive(std::path::Path::new(&sub.path));
                        }
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
                let found = delete.store().mutate(|state| {
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
