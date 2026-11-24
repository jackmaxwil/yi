//! One worker thread for the app's own file writes, so none of them waits on the UI thread.

use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

/// Writes that may wait at once.
const QUEUE: usize = 64;

type Job = Box<dyn FnOnce() + Send>;

/// The worker, kept in a hub's `panes` under no pane. Until the app starts one there is
/// none, and a write is done where it is asked for: in a test, in a scene.
#[derive(Default)]
pub struct Disk {
    jobs: Option<SyncSender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Disk {
    /// Starts the worker thread. It ends when this value is finished or dropped.
    pub fn start() -> std::io::Result<Self> {
        let (jobs, queue) = sync_channel::<Job>(QUEUE);
        let worker = std::thread::Builder::new()
            .name("yd-disk".to_owned())
            .spawn(move || queue.into_iter().for_each(|job| job()))?;
        Ok(Self {
            jobs: Some(jobs),
            worker: Some(worker),
        })
    }

    /// Runs `job` on the worker after every job asked before it, or here with no worker.
    /// Invariant: writes of one file land in the order asked, so a full queue is waited for.
    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        let job: Job = Box::new(job);
        let left = match &self.jobs {
            None => Some(job),
            Some(jobs) => match jobs.try_send(job) {
                Ok(()) => None,
                Err(TrySendError::Full(job)) => jobs.send(job).err().map(|unsent| unsent.0),
                Err(TrySendError::Disconnected(job)) => Some(job),
            },
        };
        if let Some(job) = left {
            job();
        }
    }

    /// Lets the worker finish what was asked of it, then ends it: what quitting waits for.
    pub fn finish(&mut self) {
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            // A worker that panicked wrote what it could: there is nothing left to wait for.
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::Disk;

    #[test]
    fn jobs_run_in_the_order_asked_and_finishing_waits_for_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let (done, heard) = channel();
        let mut disk = Disk::start()?;
        for n in 0..200 {
            let done = done.clone();
            disk.run(move || {
                let _ = done.send(n);
            });
        }
        disk.finish();
        let ran: Vec<i32> = heard.try_iter().collect();
        assert_eq!(ran, (0..200).collect::<Vec<_>>());
        let none = Disk::default();
        let done = done.clone();
        none.run(move || {
            let _ = done.send(-1);
        });
        assert_eq!(heard.try_recv(), Ok(-1), "with no worker it runs here");
        Ok(())
    }
}
