use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use yi_kernel::tarball::{agent_with, download};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn serve(chunks: usize, gap: Duration) -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}/pin.zip", listener.local_addr()?);
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _request = stream.read(&mut [0u8; 1024]);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            chunks.max(1)
        );
        let _ = stream.write_all(head.as_bytes());
        for _ in 0..chunks {
            std::thread::sleep(gap);
            if stream
                .write_all(b"x")
                .and_then(|()| stream.flush())
                .is_err()
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    });
    Ok(url)
}

/// Incident: a total deadline replaced the read timeout and cut a 124 MB download that was still arriving; only a read that
/// stalls is a timeout, and the error then says what to do.
#[test]
fn a_body_that_keeps_arriving_outlives_the_stall_timeout() -> TestResult {
    let url = serve(8, Duration::from_millis(150))?;
    let agent = agent_with(Duration::from_millis(500)).build();
    assert_eq!(download(&agent, &url)?.len(), 8);
    Ok(())
}

#[test]
fn a_stalled_body_names_the_stall_and_the_remedy() -> TestResult {
    let url = serve(0, Duration::from_secs(5))?;
    let agent = agent_with(Duration::from_millis(300)).build();
    let Err(error) = download(&agent, &url) else {
        return Err("a stalled body was accepted".into());
    };
    assert!(
        error.contains("no bytes arrived") && error.contains("faster link"),
        "{error}"
    );
    Ok(())
}
