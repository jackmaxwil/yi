use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use ureq::rustls::ClientConnection;
use ureq::rustls::pki_types::ServerName;
use ureq::rustls::pki_types::pem::PemObject;

use super::proto;
use super::ToolFn;
use yi_types::event::AssistantMessageEvent;

pub async fn run(
    base: &str,
    token: &str,
    body: Vec<u8>,
    tool: &ToolFn,
) -> Result<Vec<AssistantMessageEvent>, String> {
    let host = host_of(base)?;
    let io = connect(&host).await?;
    let (mut client, connection) = h2::client::handshake(io).await.map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}{}", proto::RUN_PATH))
        .header("content-type", "application/connect+proto")
        .header("connect-protocol-version", "1")
        .header("te", "trailers")
        .header("authorization", format!("Bearer {token}"))
        .header("x-ghost-mode", "true")
        .header("x-cursor-client-type", "cli")
        .header("x-cursor-client-version", proto::CLIENT_VERSION)
        .body(())
        .map_err(|error| error.to_string())?;
    let (response, mut send) = client
        .send_request(request, false)
        .map_err(|error| error.to_string())?;
    send.send_data(bytes::Bytes::from(body), false)
        .map_err(|error| error.to_string())?;
    let response = response.await.map_err(|error| error.to_string())?;
    if response.status() != http::StatusCode::OK {
        return Err(format!("cursor run failed: status code {}", response.status()));
    }
    let mut recv = response.into_body();
    let mut pending = Vec::new();
    let mut events = Vec::new();
    while let Some(chunk) = recv.data().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        let _ = recv.flow_control().release_capacity(chunk.len());
        pending.extend_from_slice(&chunk);
        let (ready, rest) = split_ready(&pending)?;
        pending = rest;
        for frame in ready {
            if frame.end {
                let text = String::from_utf8_lossy(&frame.message);
                return Err(format!("cursor stream ended: {text}"));
            }
            let applied = apply_live(&frame.message, tool).await;
            for reply in applied.replies {
                send.send_data(bytes::Bytes::from(proto::frame(&reply)), false)
                    .map_err(|error| error.to_string())?;
            }
            events.extend(applied.events);
        }
    }
    let _ = send.send_data(bytes::Bytes::new(), true);
    Ok(events)
}

async fn apply_live(message: &[u8], tool: &ToolFn) -> super::Applied {
    match proto::decode(message) {
        Ok(proto::Inbound::Tool {
            id,
            name,
            call_id,
            args,
        }) => {
            let output = tool(call_id.clone(), name.clone(), args.clone()).await;
            super::Applied {
                events: vec![super::tool_event(&call_id, &name, args)],
                replies: vec![proto::tool_reply(id, &output)],
            }
        }
        _ => {
            let mut sink = |_name: &str, _args: &serde_json::Map<String, serde_json::Value>| String::new();
            super::apply_message(message, &mut sink)
        }
    }
}

fn split_ready(bytes: &[u8]) -> Result<(Vec<proto::Frame>, Vec<u8>), String> {
    let mut frames = Vec::new();
    let mut i = 0;
    while bytes.len() - i >= 5 {
        let len = u32::from_be_bytes([bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]]) as usize;
        if bytes.len() - i < 5 + len {
            break;
        }
        frames.push(proto::Frame {
            end: bytes[i] & 0x02 != 0,
            message: bytes[i + 5..i + 5 + len].to_vec(),
        });
        i += 5 + len;
    }
    Ok((frames, bytes[i..].to_vec()))
}

fn host_of(base: &str) -> Result<String, String> {
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))
        .unwrap_or(base);
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.split(':').next().unwrap_or(host);
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
    let std_tcp = tcp.into_std().map_err(|error| error.to_string())?;
    std_tcp.set_nonblocking(true).map_err(|error| error.to_string())?;
    let name = ServerName::try_from(host.to_owned()).map_err(|error| error.to_string())?;
    let conn = ClientConnection::new(Arc::new(tls_config()?), name).map_err(|error| error.to_string())?;
    let sock = AsyncFd::new(std_tcp).map_err(|error| error.to_string())?;
    Ok(Tls { conn, sock })
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
    sock: AsyncFd<std::net::TcpStream>,
}

impl AsyncRead for Tls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        loop {
            if flush_tls(this, cx)?.is_pending() && this.conn.wants_write() {
                return Poll::Pending;
            }
            match this.conn.reader().read(buf.initialize_unfilled()) {
                Ok(0) => return Poll::Ready(Ok(())),
                Ok(n) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Poll::Ready(Err(error)),
            }
            let mut ready = std::task::ready!(this.sock.poll_read_ready(cx))?;
            let mut buf = [0u8; 8192];
            let pulled = ready.try_io(|fd| fd.get_ref().read(&mut buf));
            let n = match pulled {
                Ok(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "tls closed",
                    )));
                }
                Ok(Ok(n)) => n,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => continue,
            };
            let read = this
                .conn
                .read_tls(&mut &buf[..n])
                .map_err(std::io::Error::other)?;
            if read == 0 {
                continue;
            }
            this.conn.process_new_packets().map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
            })?;
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
        let written = this.conn.writer().write(buf)?;
        std::task::ready!(flush_tls(this, cx))?;
        Poll::Ready(Ok(written))
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

fn flush_tls(tls: &mut Tls, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
    while tls.conn.wants_write() {
        let mut encrypted = Vec::new();
        let n = tls
            .conn
            .write_tls(&mut encrypted)
            .map_err(std::io::Error::other)?;
        if n == 0 {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "tls write wrote nothing",
            )));
        }
        let mut ready = std::task::ready!(tls.sock.poll_write_ready(cx))?;
        let mut offset = 0;
        while offset < encrypted.len() {
            match ready.try_io(|fd| fd.get_ref().write(&encrypted[offset..])) {
                Ok(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "tls write wrote nothing",
                    )));
                }
                Ok(Ok(wrote)) => offset += wrote,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    ready = std::task::ready!(tls.sock.poll_write_ready(cx))?;
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => ready = std::task::ready!(tls.sock.poll_write_ready(cx))?,
            }
        }
    }
    Poll::Ready(Ok(()))
}
