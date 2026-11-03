use std::error::Error;
use std::ffi::OsString;
use std::sync::Arc;

use crate::scratch::Scratch;
use yi_runtime::schedule::clock::wait_schedule;
use yi_runtime::schedule::{HeartbeatService, JobStore, Zone, parse_local_ms};
use yi_types::plan::doc::{BlockedOn, TodoLabel};
use yi_types::schedule::ScheduleKind;

fn instant(iso: &str) -> Result<u64, String> {
    parse_local_ms(iso, &Zone::default()).ok_or_else(|| format!("bad instant {iso}"))
}

/// The process zone for one test, put back on drop: the suite runs one test at a time
/// (`--test-threads=1`), and nextest gives a test its own process.
struct Pinned(Option<OsString>);

impl Pinned {
    fn new(zone: &str) -> Self {
        let before = std::env::var_os("TZ");
        // SAFETY: this test owns the process for its whole run; the drop below restores `TZ`.
        unsafe { std::env::set_var("TZ", zone) };
        Self(before)
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        // SAFETY: as in `Pinned::new`.
        unsafe {
            match self.0.take() {
                Some(before) => std::env::set_var("TZ", before),
                None => std::env::remove_var("TZ"),
            }
        }
    }
}

/// Dies with `wait_schedule` reading the zone from anywhere but the process zone: a reminder on a
/// naive `clock://at` time arms at the wrong instant in any zone but UTC, and its job with it.
#[test]
fn a_naive_clock_wait_arms_at_its_instant_in_the_process_zone() -> Result<(), Box<dyn Error>> {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/zoneinfo/America_Los_Angeles"
    );
    let _zone = Pinned::new(fixture);
    let now = instant("2027-01-03T03:48:00Z")?;

    let (schedule, next) = wait_schedule("clock://at 2027-01-04T09:00:00", now)?;
    assert_eq!(schedule.kind, ScheduleKind::Once);
    assert_eq!(next, instant("2027-01-04T17:00:00Z")?, "09:00 PST");
    let (_, named) = wait_schedule("clock://at 2027-01-04T09:00:00Z", now)?;
    assert_eq!(
        named,
        instant("2027-01-04T09:00:00Z")?,
        "a named offset is not local"
    );

    let dir = Scratch::new("yi-reminder-zone")?;
    let store = Arc::new(JobStore::open(dir.join("scheduled-jobs.json")));
    let service = HeartbeatService::new(Arc::clone(&store), "/tmp");
    service.bind_session("test".to_owned());
    let label = TodoLabel::new("renew the cert")?;
    let on = BlockedOn::Channel {
        address: "clock://at 2027-01-04T09:00:00".to_owned(),
        filter: None,
    };
    service.arm(&[(None, label.clone(), on)]);
    let armed = store
        .snapshot()
        .jobs
        .iter()
        .filter(|job| job.unblocks.as_ref() == Some(&label))
        .map(|job| job.next_run_at)
        .collect::<Vec<_>>();
    assert_eq!(
        armed,
        vec![Some(instant("2027-01-04T17:00:00Z")?)],
        "the reminder's job armed at the wrong instant"
    );
    Ok(())
}
