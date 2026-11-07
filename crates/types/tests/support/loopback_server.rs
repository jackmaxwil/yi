//! Scripted replies for loopback HTTP fakes, using the sibling module's one request reader.
use std::io::{BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::loopback::read_request;

/// Answers one request per reply and returns its port, request count and captured requests.
pub fn serve<I, S>(
    replies: I,
) -> std::io::Result<(u16, Arc<AtomicUsize>, std::thread::JoinHandle<Vec<String>>)>
where
    I: IntoIterator<Item = S> + Send + 'static,
    S: AsRef<str>,
{
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for reply in replies {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(stream);
            let request = read_request(&mut reader);
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = reader.get_mut().write_all(reply.as_ref().as_bytes());
            requests.push(request);
        }
        requests
    });
    Ok((port, seen, handle))
}
#[derive(Clone)]
/// One scripted reply: `head` after `stall`, then each event `gap` after the write before it.
pub struct Script {
    stall: std::time::Duration,
    gap: std::time::Duration,
    head: String,
    events: Vec<String>,
}

impl Script {
    pub fn new(head: impl Into<String>) -> Self {
        Self {
            stall: std::time::Duration::ZERO,
            gap: std::time::Duration::ZERO,
            head: head.into(),
            events: Vec::new(),
        }
    }

    pub fn stall(mut self, stall: std::time::Duration) -> Self {
        self.stall = stall;
        self
    }

    pub fn then(
        mut self,
        gap: std::time::Duration,
        events: impl IntoIterator<Item = String>,
    ) -> Self {
        self.gap = gap;
        self.events = events.into_iter().collect();
        self
    }
}

/// [`serve`] with a script per reply: a stall before the first byte and a gap between chunks.
pub fn serve_scripted<I>(
    scripts: I,
) -> std::io::Result<(u16, Arc<AtomicUsize>, std::thread::JoinHandle<Vec<String>>)>
where
    I: IntoIterator<Item = Script> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for script in scripts {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(stream);
            let request = read_request(&mut reader);
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(script.stall);
            if reader.get_mut().write_all(script.head.as_bytes()).is_err() {
                requests.push(request);
                continue;
            }
            for event in script.events {
                std::thread::sleep(script.gap);
                if reader.get_mut().write_all(event.as_bytes()).is_err() {
                    break;
                }
            }
            requests.push(request);
        }
        requests
    });
    Ok((port, seen, handle))
}
