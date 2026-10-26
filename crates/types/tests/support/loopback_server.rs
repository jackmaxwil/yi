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
