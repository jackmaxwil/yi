use std::time::{Duration, Instant};

pub fn poll_until<T>(
    mut probe: impl FnMut() -> Option<T>,
) -> Result<T, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Some(value) = probe() {
            return Ok(value);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("timed out waiting for the job state under test".into())
}
