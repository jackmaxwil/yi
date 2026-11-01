//! The loopback CONNECT proxy every contained spawn's network leaves through (#601): a token per
//! spawn names the holder, a domain is decided once per holder, and the address dialed is checked.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use yi_types::egress::Domain;

use crate::permission::PermissionBroker;
use crate::wall::Wall;

/// Past each the proxy answers with a row naming the cap, and a client retries or reports it.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub connections: usize,
    pub per_holder: usize,
    pub pending_asks: usize,
    pub decisions: usize,
    pub rows: usize,
    /// How long a peer may take over its head, and a connection may wait for a person.
    pub patience: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: 256,
            per_holder: 64,
            pending_asks: 4,
            decisions: 1024,
            rows: 8,
            patience: Duration::from_secs(10),
        }
    }
}

const HEAD_MAX_BYTES: usize = 8 * 1024;
const RELAY_STACK_BYTES: usize = 64 * 1024;

/// Every key a package manager or HTTP client reads for its proxy, lowercase forms included.
const PROXY_KEYS: [&str; 16] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "WS_PROXY",
    "WSS_PROXY",
    "YARN_HTTP_PROXY",
    "YARN_HTTPS_PROXY",
    "NPM_CONFIG_HTTP_PROXY",
    "NPM_CONFIG_HTTPS_PROXY",
    "NPM_CONFIG_PROXY",
    "BUNDLE_HTTP_PROXY",
    "BUNDLE_HTTPS_PROXY",
    "PIP_PROXY",
];

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("no entropy for a proxy token: {0}")]
    Entropy(io::Error),
    #[error("the egress proxy cannot listen: {0}")]
    Bind(io::Error),
    #[error("the egress proxy's holder table is poisoned")]
    Poisoned,
}

/// A spawn's credential: its only copy past [`Lease::env`] is a digest in the table.
struct Token([u8; 16]);

impl Token {
    fn mint(source: &Path) -> Result<Self, Error> {
        let mut bytes = [0u8; 16];
        std::fs::File::open(source)
            .and_then(|mut random| random.read_exact(&mut bytes))
            .map_err(Error::Entropy)?;
        Ok(Self(bytes))
    }

    fn hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Token(redacted)")
    }
}

/// The `Basic` credential a client sends for this token, which is what the table is keyed by.
fn credential(token: &Token) -> String {
    let url_safe = yi_types::encoding::base64url_nopad(format!("yi:{}", token.hex()).as_bytes());
    let standard = url_safe.replace('-', "+").replace('_', "/");
    let pad = "=".repeat((4 - standard.len() % 4) % 4);
    format!("{standard}{pad}")
}

fn digest(credential: &str) -> [u8; 32] {
    Sha256::digest(credential.as_bytes()).into()
}

/// What a client sent before the blank line, parsed with no IO so a hostile one costs a value.
#[derive(Debug, PartialEq, Eq)]
pub struct Connect {
    pub domain: Domain,
    pub port: u16,
    pub credential: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BadHead {
    Incomplete,
    NotConnect,
    Authority,
}

/// # Errors
/// [`BadHead`] for anything but one complete `CONNECT name:port` head with a name [`Domain`] takes.
pub fn connect_head(head: &[u8]) -> Result<Connect, BadHead> {
    let end = (head.windows(4))
        .position(|window| window == b"\r\n\r\n")
        .ok_or(BadHead::Incomplete)?;
    let text = head
        .get(..end)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or(BadHead::NotConnect)?;
    let mut lines = text.split("\r\n");
    let mut request = lines.next().unwrap_or_default().split(' ');
    if request.next() != Some("CONNECT") {
        return Err(BadHead::NotConnect);
    }
    let (host, port) = (request.next())
        .and_then(|authority| authority.rsplit_once(':'))
        .ok_or(BadHead::Authority)?;
    let port = port.parse().map_err(|_| BadHead::Authority)?;
    let domain = Domain::parse(host).map_err(|_| BadHead::Authority)?;
    let credential = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        (name.eq_ignore_ascii_case("proxy-authorization"))
            .then(|| value.strip_prefix("Basic ").map(str::to_owned))
            .flatten()
    });
    Ok(Connect {
        domain,
        port,
        credential,
    })
}

/// A public address: anything that reaches this machine or a private network is refused, here on
/// the address dialed, since a name may resolve to one.
pub fn public_address(address: IpAddr) -> bool {
    let v4 = |ip: Ipv4Addr| {
        let [a, b, ..] = ip.octets();
        !(ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_documentation()
            || (a == 100 && (64..128).contains(&b)))
    };
    match address {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => v4(mapped),
            None => {
                let first = ip.segments().first().copied().unwrap_or_default();
                !(ip.is_loopback()
                    || ip.is_unspecified()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

pub type Dialer = Arc<dyn Fn(&Domain, u16) -> io::Result<TcpStream> + Send + Sync>;

/// Resolves, keeps the public addresses and connects to one of those, so a name cannot rebind.
pub fn dial_public(domain: &Domain, port: u16) -> io::Result<TcpStream> {
    const DIAL: Duration = Duration::from_secs(10);
    let mut last = io::Error::other("no public address");
    for address in (domain.as_str(), port).to_socket_addrs()? {
        if !public_address(address.ip()) {
            continue;
        }
        match TcpStream::connect_timeout(&address, DIAL) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Pending,
    Allowed,
    Refused,
}

/// Who a token speaks for, and what has been decided for them.
struct Holder {
    broker: Arc<PermissionBroker>,
    wall: Wall,
    decisions: Mutex<HashMap<Domain, Decision>>,
    decided: Condvar,
    rows: Mutex<Vec<String>>,
    /// `None` once its lease is gone, so a tunnel opening in that gap closes at once. Each entry
    /// leaves when its tunnel ends, or a kernel's long lease would hold every socket it ever used.
    tunnels: Mutex<Option<HashMap<usize, TcpStream>>>,
    connections: AtomicUsize,
    limits: Limits,
}

impl Holder {
    fn row(&self, row: String) {
        let Ok(mut rows) = self.rows.lock() else {
            return;
        };
        let max = self.limits.rows;
        match rows.len() {
            count if count < max => rows.push(row),
            count if count == max => rows.push(format!(
                "[egress: {max} rows shown, later ones dropped (egress.rows {max}); the ledger holds each domain's first decision]"
            )),
            _ => {}
        }
    }

    fn denied_by_wall(&self, domain: &Domain) -> bool {
        self.wall.deny_url.iter().any(|url| {
            let host = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
            let host = host.split(['/', ':']).next().unwrap_or_default();
            host.eq_ignore_ascii_case(domain.as_str())
        })
    }

    /// One ask per domain, on its own thread since a person may never answer; a connection waits
    /// [`Limits::patience`] for it, then gets a 403 naming the open ask.
    fn allows(self: &Arc<Self>, domain: &Domain) -> Result<(), String> {
        let refused = |why: String| format!("[egress: refused {domain}: {why}]");
        if self.denied_by_wall(domain) {
            return Err(refused("this session's wall denies it".to_owned()));
        }
        if self.broker.egress_allowed(domain) {
            return Ok(());
        }
        let mut decisions = self
            .decisions
            .lock()
            .map_err(|_| refused("poisoned".into()))?;
        let pending = decisions
            .values()
            .filter(|d| **d == Decision::Pending)
            .count();
        match decisions.get(domain).copied() {
            Some(Decision::Allowed) => return Ok(()),
            Some(Decision::Refused) => {
                return Err(refused(
                    "refused earlier this session; sandbox.egress in config.json lets it"
                        .to_owned(),
                ));
            }
            Some(Decision::Pending) => {}
            None if pending >= self.limits.pending_asks => {
                let cap = self.limits.pending_asks;
                return Err(refused(format!(
                    "{pending} asks already open (egress.pending_asks {cap}); retry after they are answered"
                )));
            }
            None if decisions.len() >= self.limits.decisions => {
                let cap = self.limits.decisions;
                return Err(refused(format!(
                    "{cap} domains already decided (egress.decisions {cap}); sandbox.egress in config.json lets it"
                )));
            }
            None => {
                decisions.insert(domain.clone(), Decision::Pending);
                let (holder, asked) = (Arc::clone(self), domain.clone());
                let spawned = std::thread::Builder::new()
                    .name("yi-egress-ask".to_owned())
                    .spawn(move || holder.ask(&asked));
                if spawned.is_err() {
                    decisions.insert(domain.clone(), Decision::Refused);
                    return Err(refused("no thread to ask on".to_owned()));
                }
            }
        }
        let (decisions, _) = self
            .decided
            .wait_timeout_while(decisions, self.limits.patience, |decisions| {
                decisions.get(domain) == Some(&Decision::Pending)
            })
            .map_err(|_| refused("poisoned".to_owned()))?;
        match decisions.get(domain) {
            Some(Decision::Allowed) => Ok(()),
            Some(Decision::Pending) => Err(refused(
                "not seeded or in sandbox.egress; asked you, retry after your answer".to_owned(),
            )),
            _ => Err(refused(
                "not seeded or in sandbox.egress, and the ask was declined; sandbox.egress in config.json lets it".to_owned(),
            )),
        }
    }

    fn ask(&self, domain: &Domain) {
        let allowed = self.broker.egress_ask(domain);
        if let Ok(mut decisions) = self.decisions.lock() {
            let decision = if allowed {
                Decision::Allowed
            } else {
                Decision::Refused
            };
            decisions.insert(domain.clone(), decision);
        }
        self.decided.notify_all();
    }
}

struct Inner {
    holders: Mutex<HashMap<[u8; 32], Arc<Holder>>>,
    connections: AtomicUsize,
    dial: Dialer,
    limits: Limits,
    tunnel_ids: AtomicUsize,
}

/// Counts a connection against both caps for as long as it lives.
struct Counted {
    inner: Arc<Inner>,
    holder: Option<Arc<Holder>>,
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.inner.connections.fetch_sub(1, Ordering::Relaxed);
        if let Some(holder) = &self.holder {
            holder.connections.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// One listener on loopback. Process lifetime is intended: nothing joins it.
pub struct Proxy {
    port: u16,
    inner: Arc<Inner>,
    entropy: PathBuf,
}

/// A failed bind must not stick, and `OnceLock` has no stable fallible init.
static SHARED: Mutex<Option<Arc<Proxy>>> = Mutex::new(None);

impl Proxy {
    /// # Errors
    /// [`Error::Bind`] when it cannot listen, which refuses the spawn; the next one retries.
    pub fn shared() -> Result<Arc<Self>, Error> {
        let mut shared = SHARED.lock().map_err(|_| Error::Poisoned)?;
        if let Some(proxy) = &*shared {
            return Ok(Arc::clone(proxy));
        }
        let proxy = Arc::new(Self::start(Arc::new(dial_public), Limits::default())?);
        *shared = Some(Arc::clone(&proxy));
        Ok(proxy)
    }

    /// # Errors
    /// [`Error::Bind`] when it cannot listen or start its accept thread.
    pub fn start(dial: Dialer, limits: Limits) -> Result<Self, Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(Error::Bind)?;
        let port = listener.local_addr().map_err(Error::Bind)?.port();
        let inner = Arc::new(Inner {
            holders: Mutex::new(HashMap::new()),
            connections: AtomicUsize::new(0),
            dial,
            limits,
            tunnel_ids: AtomicUsize::new(0),
        });
        let accepting = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("yi-egress".to_owned())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    accept(&accepting, stream);
                }
            })
            .map_err(Error::Bind)?;
        Ok(Self {
            port,
            inner,
            entropy: PathBuf::from("/dev/urandom"),
        })
    }

    /// Where tokens come from; a source that fails refuses every spawn.
    #[must_use]
    pub fn with_entropy(mut self, source: PathBuf) -> Self {
        self.entropy = source;
        self
    }

    /// # Errors
    /// [`Error::Entropy`] or a poisoned table, either of which refuses the spawn.
    pub fn lease(&self, broker: Arc<PermissionBroker>, wall: Wall) -> Result<Lease, Error> {
        let token = Token::mint(&self.entropy)?;
        let key = digest(&credential(&token));
        let holder = Arc::new(Holder {
            broker,
            wall,
            decisions: Mutex::new(HashMap::new()),
            decided: Condvar::new(),
            rows: Mutex::new(Vec::new()),
            tunnels: Mutex::new(Some(HashMap::new())),
            connections: AtomicUsize::new(0),
            limits: self.inner.limits,
        });
        let mut holders = self.inner.holders.lock().map_err(|_| Error::Poisoned)?;
        holders.insert(key, Arc::clone(&holder));
        Ok(Lease {
            url: format!("http://yi:{}@127.0.0.1:{}", token.hex(), self.port),
            key,
            holder,
            inner: Arc::clone(&self.inner),
        })
    }
}

/// One spawn's standing with the proxy. Revoked by [`Lease::revoke`], `Drop` being the backstop.
#[must_use = "a dropped lease revokes the token the child was just given"]
pub struct Lease {
    url: String,
    key: [u8; 32],
    holder: Arc<Holder>,
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Lease(redacted)")
    }
}

impl Lease {
    /// The child's environment. Through `Command::env`, never `argv`, which `ps` reads.
    pub fn env(&self) -> Vec<(String, String)> {
        let no_proxy = "localhost,127.0.0.1,::1".to_owned();
        (PROXY_KEYS.iter())
            .map(|key| ((*key).to_owned(), self.url.clone()))
            .chain([
                ("NO_PROXY".to_owned(), no_proxy.clone()),
                ("no_proxy".to_owned(), no_proxy),
                ("NODE_USE_ENV_PROXY".to_owned(), "1".to_owned()),
            ])
            .collect()
    }

    pub fn take_rows(&self) -> Vec<String> {
        self.holder
            .rows
            .lock()
            .map(|mut rows| std::mem::take(&mut *rows))
            .unwrap_or_default()
    }

    /// The next CONNECT with this token gets a 407 and its tunnels close, so a spawn's end frees
    /// the threads it held.
    pub fn revoke(self) -> Vec<String> {
        self.take_rows()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut holders) = self.inner.holders.lock() {
            holders.remove(&self.key);
        }
        let open = (self.holder.tunnels.lock().ok()).and_then(|mut tunnels| tunnels.take());
        for tunnel in open.into_iter().flat_map(HashMap::into_values) {
            let _gone_already_is_fine = tunnel.shutdown(Shutdown::Both);
        }
    }
}

fn answer(stream: &mut TcpStream, status: &str) {
    let _the_client_may_be_gone =
        stream.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n").as_bytes());
}

/// Incident: `git` holds the credentials and offers them only once a 407 names a scheme, so a bare
/// 407 failed every contained `git fetch` where `curl`, which sends them first, passed.
fn unauthorized(stream: &mut TcpStream) {
    let _the_client_may_be_gone = stream.write_all(
        b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"yi\"\r\nConnection: close\r\n\r\n",
    );
}

fn accept(inner: &Arc<Inner>, mut stream: TcpStream) {
    let cap = inner.limits.connections;
    if inner.connections.fetch_add(1, Ordering::Relaxed) >= cap {
        inner.connections.fetch_sub(1, Ordering::Relaxed);
        answer(
            &mut stream,
            &format!("503 egress.connections {cap} reached"),
        );
        return;
    }
    let refusal = stream.try_clone();
    let counted = Counted {
        inner: Arc::clone(inner),
        holder: None,
    };
    let spawned = std::thread::Builder::new()
        .name("yi-egress-conn".to_owned())
        .stack_size(RELAY_STACK_BYTES)
        .spawn(move || serve(counted, stream));
    if let (Err(_), Ok(mut refusal)) = (spawned, refusal) {
        answer(&mut refusal, "503 egress has no thread for this connection");
    }
}

/// The bytes through the blank line and whatever arrived with them, under one deadline, so an
/// unauthenticated peer holds a thread for [`Limits::patience`] and never buffers past the limit.
fn read_head(stream: &mut TcpStream, patience: Duration) -> io::Result<Vec<u8>> {
    let until = Instant::now()
        .checked_add(patience)
        .unwrap_or_else(Instant::now);
    let mut head = Vec::new();
    let mut chunk = [0u8; 512];
    while head_end(&head).is_none() {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || head.len() >= HEAD_MAX_BYTES {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend(chunk.iter().take(read));
    }
    if head_end(&head).is_some_and(|end| end > HEAD_MAX_BYTES) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(head)
}

fn head_end(head: &[u8]) -> Option<usize> {
    let at = head.windows(4).position(|window| window == b"\r\n\r\n")?;
    at.checked_add(4)
}

fn serve(mut counted: Counted, mut client: TcpStream) {
    let patience = counted.inner.limits.patience;
    let Ok(head) = read_head(&mut client, patience) else {
        answer(&mut client, "400 Bad Request");
        return;
    };
    let Ok(connect) = connect_head(&head) else {
        answer(&mut client, "400 Bad Request");
        return;
    };
    let key = connect.credential.as_deref().map(digest);
    let holder = key.and_then(|key| {
        let holders = counted.inner.holders.lock().ok()?;
        holders.get(&key).cloned()
    });
    let Some(holder) = holder else {
        unauthorized(&mut client);
        return;
    };
    let cap = holder.limits.per_holder;
    if holder.connections.fetch_add(1, Ordering::Relaxed) >= cap {
        holder.connections.fetch_sub(1, Ordering::Relaxed);
        holder.row(format!(
            "[egress: {} not opened: {cap} connections held (egress.per_holder {cap}); close some and retry]",
            connect.domain
        ));
        answer(&mut client, &format!("503 egress.per_holder {cap} reached"));
        return;
    }
    counted.holder = Some(Arc::clone(&holder));
    if let Err(row) = holder.allows(&connect.domain) {
        holder.row(row);
        answer(&mut client, &format!("403 {} refused", connect.domain));
        return;
    }
    let mut upstream = match (counted.inner.dial)(&connect.domain, connect.port) {
        Ok(upstream) => upstream,
        Err(error) => {
            holder.row(format!(
                "[egress: {}:{} did not answer: {}]",
                connect.domain,
                connect.port,
                error.kind()
            ));
            answer(&mut client, "502 Bad Gateway");
            return;
        }
    };
    let early = head_end(&head).and_then(|end| head.get(end..));
    if upstream.write_all(early.unwrap_or_default()).is_err() {
        answer(&mut client, "502 Bad Gateway");
        return;
    }
    let id = counted.inner.tunnel_ids.fetch_add(1, Ordering::Relaxed);
    relay(&holder, id, client, upstream);
    if let Ok(mut tunnels) = holder.tunnels.lock()
        && let Some(open) = tunnels.as_mut()
    {
        open.remove(&id);
    }
}

/// Two copies, one each way, each passing EOF on as a half-close so a protocol that shuts one side
/// keeps the other. A spawn's end closes the tunnel through the clone the holder keeps.
fn relay(holder: &Holder, id: usize, mut client: TcpStream, upstream: TcpStream) {
    {
        let Ok(mut tunnels) = holder.tunnels.lock() else {
            return;
        };
        let (Some(open), Ok(kept)) = (tunnels.as_mut(), client.try_clone()) else {
            unauthorized(&mut client);
            return;
        };
        open.insert(id, kept);
    }
    let (Ok(mut client_up), Ok(mut upstream_up)) = (client.try_clone(), upstream.try_clone())
    else {
        answer(&mut client, "503 egress has no descriptor for this tunnel");
        return;
    };
    let up = std::thread::Builder::new()
        .name("yi-egress-up".to_owned())
        .stack_size(RELAY_STACK_BYTES)
        .spawn(move || {
            let _either_side_may_end_first = io::copy(&mut client_up, &mut upstream_up);
            let _ = upstream_up.shutdown(Shutdown::Write);
        });
    let Ok(up) = up else {
        answer(&mut client, "503 egress has no thread for this tunnel");
        return;
    };
    let answered = client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .and_then(|()| client.set_read_timeout(None));
    if answered.is_ok() {
        let (mut client_down, mut upstream_down) = (client, upstream);
        let _either_side_may_end_first = io::copy(&mut upstream_down, &mut client_down);
        let _ = client_down.shutdown(Shutdown::Write);
    }
    let _ = up.join();
}
