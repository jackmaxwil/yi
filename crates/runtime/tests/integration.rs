//! Every integration test of the crate but the two gate harnesses (behavior, request_budget),
//! in one binary. As 62 binaries each linked its own copy of the stack, so an edit to yi-types
//! relinked all of them: 71 of the 173 CPU-seconds that rebuild cost. Cargo.toml sets
//! `autotests = false`, so a new file here runs only once it has a line below.
#[path = "support/own.rs"]
mod own;
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
#[path = "support/family.rs"]
mod support;

mod advisor_compaction;
mod advisor_e2e;
mod affordance;
mod auto_review;
mod branch_diff;
mod breakpoints;
mod cache_miss;
mod channel_e2e;
mod child_transcript;
mod claims;
mod classifier_e2e;
mod compaction_faux;
mod documents;
mod effort_session;
mod environment;
mod ext_e2e;
mod family;
mod fetch_session;
mod gate;
mod gate_battery;
mod goal_e2e;
mod host_facts;
mod injection_canary;
mod ipython_faux;
mod judge;
mod kernel_across_sessions;
mod kernel_data_surface;
mod kernel_lane;
mod kernel_sandbox;
mod lanes;
mod levers;
mod node;
mod permission_scope;
mod plan_declare;
mod plan_e2e;
mod plan_fuzz;
mod plan_import;
mod plan_journal;
mod plan_ledger;
mod plan_ops;
mod plan_program;
mod plan_recovery;
mod plan_timer;
mod plan_trace;
mod plan_verify;
mod plan_walkthrough;
mod prompt_drift;
mod prompts;
mod provider_keys;
mod reader_e2e;
mod reap_kernel;
mod recursion_e2e;
mod requests_e2e;
mod rewind_branch;
mod route_record;
mod rules_e2e;
mod sandbox_seam;
mod schedule_e2e;
mod schedule_unit;
mod schema;
mod session_faux;
mod skills_e2e;
mod snapshot_e2e;
mod subagent;
mod subagent_fuzz;
mod tape;
mod telemetry;
mod title;
mod todo_coupling;
mod todo_e2e;
mod todo_mirror;
mod undo_to;
mod wall_e2e;
