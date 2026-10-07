use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use ureq::rustls::ClientConnection;
use ureq::rustls::pki_types::ServerName;
use ureq::rustls::pki_types::pem::PemObject;

use super::ToolFn;
use super::proto;

pub async fn run(
    base: &str,
    token: &str,
    body: Vec<u8>,
    tool: &ToolFn,
    tools: &[super::proto::ToolWire],
    workspace: &str,
    out: &mut super::Out<'_>,
) -> Result<(), String> {
    let host = host_of(base)?;
    let io = connect(&host).await?;
    let (mut client, connection) = h2::client::handshake(io)
        .await
        .map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request_id = yi_oauth::pkce::random_bytes(16)
        .map(|bytes| yi_oauth::pkce::hex(&bytes))
        .unwrap_or_else(|_| "cursor".to_owned());
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}{}", proto::RUN_PATH))
        .header("content-type", "application/connect+proto")
        .header("connect-protocol-version", "1")
        .header("t\u{0065}", "trailers")
        .header("authorization", format!("Bearer {token}"))
        .header("x-ghost-mode", "true")
        .header("x-cursor-client-type", "cli")
        .header("x-cursor-client-version", proto::CLIENT_VERSION)
        .header("x-request-id", request_id)
        .body(())
        .map_err(|error| error.to_string())?;
    let (response, mut send) = client
        .send_request(request, false)
        .map_err(|error| error.to_string())?;
    send.send_data(bytes::Bytes::from(body), false)
        .map_err(|error| error.to_string())?;
    let mut response = std::pin::pin!(response);
    let mut beat = tokio::time::interval(std::time::Duration::from_secs(15));
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let _ = beat.tick().await;
    let response = loop {
        tokio::select! {
            result = response.as_mut() => break result,
            _ = beat.tick() => {
                send.send_data(bytes::Bytes::from(proto::frame(&proto::heartbeat())), false)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    .map_err(|error| error.to_string())?;
    if response.status() != http::StatusCode::OK {
        return Err(format!(
            "cursor run failed: status code {}",
            response.status()
        ));
    }
    let mut recv = response.into_body();
    let mut pending = Vec::new();
    let mut blobs = super::Blobs::new();
    'read: loop {
        let chunk = tokio::select! {
            biased;
            _ = wait_stop(out.stop) => break,
            chunk = recv.data() => chunk,
            _ = beat.tick() => {
                send.send_data(bytes::Bytes::from(proto::frame(&proto::heartbeat())), false)
                    .map_err(|error| error.to_string())?;
                continue;
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|error| error.to_string())?;
        let _ = recv.flow_control().release_capacity(chunk.len());
        pending.extend_from_slice(&chunk);
        let (ready, rest) = proto::split_ready(&pending);
        pending = rest;
        for frame in ready {
            if frame.end {
                let text = String::from_utf8_lossy(&frame.message);
                return Err(format!("cursor stream ended: {text}"));
            }
            let inbound = proto::decode(&frame.message);
            let ended = matches!(inbound, Ok(proto::Inbound::TurnEnded));
            let applied = super::apply_message(inbound, tool, tools, workspace, &mut blobs).await;
            for reply in applied.replies {
                send.send_data(bytes::Bytes::from(proto::frame(&reply)), false)
                    .map_err(|error| error.to_string())?;
            }
            for event in applied.events {
                if !out.blocks.push(out.sink, event).await {
                    break 'read;
                }
            }
            if ended || (out.stop)() {
                break 'read;
            }
        }
    }
    let _ = send.send_data(bytes::Bytes::new(), true);
    Ok(())
}

async fn wait_stop(stop: &super::StopFn) {
    while !stop() {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

fn host_of(base: &str) -> Result<String, String> {
    let host = crate::request::host_of(base);
    if host.is_empty() {
        return Err("cursor base url has no host".to_owned());
    }
    Ok(host.to_owned())
}

async fn connect(host: &str) -> Result<Tls, String> {
    let addr = tokio::net::lookup_host(format!("{host}:443"))
        .await
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| format!("no address for {host}"))?;
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|error| error.to_string())?;
    tcp.set_nodelay(true).map_err(|error| error.to_string())?;
    let name = ServerName::try_from(host.to_owned()).map_err(|error| error.to_string())?;
    let conn =
        ClientConnection::new(Arc::new(tls_config()?), name).map_err(|error| error.to_string())?;
    Ok(Tls {
        conn,
        sock: tcp,
        held: 0,
        out: Vec::new(),
        pending_in: Vec::new(),
    })
}

fn tls_config() -> Result<ureq::rustls::ClientConfig, String> {
    let mut roots = ureq::rustls::RootCertStore::empty();
    let pem = system_roots()?;
    let ders = ureq::rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let (valid, _) = roots.add_parsable_certificates(ders);
    if valid == 0 {
        return Err("no system root certificates".to_owned());
    }
    let mut config = ureq::rustls::ClientConfig::builder_with_provider(
        ureq::rustls::crypto::ring::default_provider().into(),
    )
    .with_protocol_versions(&[&ureq::rustls::version::TLS13, &ureq::rustls::version::TLS12])
    .map_err(|error| error.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

fn system_roots() -> Result<Vec<u8>, String> {
    if cfg!(target_os = "macos") {
        #[expect(
            clippy::disallowed_methods,
            reason = "the system trust store is what the cursor TLS handshake verifies against"
        )]
        let output = std::process::Command::new("security")
            .args([
                "find-certificate",
                "-a",
                "-p",
                "/System/Library/Keychains/SystemRootCertificates.keychain",
            ])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        return Ok(output.stdout);
    }
    std::fs::read("/etc/ssl/certs/ca-certificates.crt")
        .or_else(|_| std::fs::read("/etc/ssl/cert.pem"))
        .map_err(|error| error.to_string())
}

struct Tls {
    conn: ClientConnection,
    sock: tokio::net::TcpStream,
    held: usize,
    out: Vec<u8>,
    pending_in: Vec<u8>,
}

impl AsyncRead for Tls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        loop {
            match this.conn.reader().read(buf.initialize_unfilled()) {
                Ok(0) => return Poll::Ready(Ok(())),
                Ok(n) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Poll::Ready(Err(error)),
            }
            match ingest(this, cx) {
                Poll::Ready(Ok(_)) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {
                    if this.conn.wants_write()
                        && let Poll::Ready(result) = flush_tls(this, cx)
                    {
                        result?;
                        continue;
                    }
                    return Poll::Pending;
                }
            }
            if this.conn.wants_write() {
                std::task::ready!(flush_tls(this, cx))?;
            }
        }
    }
}

impl AsyncWrite for Tls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.held > 0 {
            std::task::ready!(flush_tls(this, cx))?;
            let written = this.held;
            this.held = 0;
            return Poll::Ready(Ok(written));
        }
        loop {
            std::task::ready!(flush_tls(this, cx))?;
            match this.conn.writer().write(buf) {
                Ok(0) if !buf.is_empty() => {
                    if std::task::ready!(ingest(this, cx))? == 0 {
                        continue;
                    }
                }
                Ok(n) => match flush_tls(this, cx) {
                    Poll::Ready(Ok(())) => return Poll::Ready(Ok(n)),
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => {
                        this.held = n;
                        return Poll::Pending;
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        flush_tls(self.get_mut(), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.conn.send_close_notify();
        flush_tls(this, cx)
    }
}

fn take_tls(tls: &mut Tls, bytes: &[u8]) -> std::io::Result<usize> {
    let mut rest = bytes;
    let mut taken = 0;
    while !rest.is_empty() {
        let n = match tls.conn.read_tls(&mut rest) {
            Ok(n) => n,
            Err(error) if error.to_string() == "received plaintext buffer full" => {
                tls.pending_in.extend_from_slice(rest);
                return Ok(taken);
            }
            Err(error) => return Err(std::io::Error::other(error)),
        };
        if n == 0 {
            break;
        }
        taken += n;
        tls.conn.process_new_packets().map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        if !tls.conn.wants_read() {
            tls.pending_in.extend_from_slice(rest);
            return Ok(taken);
        }
    }
    if !rest.is_empty() {
        tls.pending_in.extend_from_slice(rest);
    }
    Ok(taken)
}

fn ingest(tls: &mut Tls, cx: &mut Context<'_>) -> Poll<std::io::Result<usize>> {
    if !tls.pending_in.is_empty() {
        let buffered = std::mem::take(&mut tls.pending_in);
        return Poll::Ready(take_tls(tls, &buffered));
    }
    let mut buf = [0u8; 16384];
    let mut read = ReadBuf::new(&mut buf);
    std::task::ready!(Pin::new(&mut tls.sock).poll_read(cx, &mut read))?;
    let filled = read.filled().len();
    if filled == 0 {
        return Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "tls closed",
        )));
    }
    Poll::Ready(take_tls(tls, &buf[..filled]))
}

fn flush_tls(tls: &mut Tls, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
    loop {
        if tls.out.is_empty() {
            if !tls.conn.wants_write() {
                return Poll::Ready(Ok(()));
            }
            let n = tls
                .conn
                .write_tls(&mut tls.out)
                .map_err(std::io::Error::other)?;
            if n == 0 {
                return Poll::Ready(Ok(()));
            }
        }
        match Pin::new(&mut tls.sock).poll_write(cx, &tls.out) {
            Poll::Pending => {
                if tls.conn.is_handshaking() {
                    let _ = std::task::ready!(ingest(tls, cx))?;
                    continue;
                }
                return Poll::Pending;
            }
            Poll::Ready(Ok(0)) => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "tls write wrote nothing",
                )));
            }
            Poll::Ready(Ok(wrote)) => {
                tls.out.drain(..wrote);
            }
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::wait_stop;
    use crate::cursor::StopFn;

    #[tokio::test]
    async fn a_raised_stop_ends_the_wait() {
        let flag = Arc::new(AtomicBool::new(false));
        let stop: StopFn = Arc::new({
            let flag = Arc::clone(&flag);
            move || flag.load(Ordering::SeqCst)
        });
        let waiting = tokio::spawn(async move { wait_stop(&stop).await });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!waiting.is_finished());
        flag.store(true, Ordering::SeqCst);
        let joined = tokio::time::timeout(std::time::Duration::from_millis(500), waiting).await;
        assert!(joined.is_ok());
    }
}
