use std::error::Error;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use proptest::prelude::*;
use tokio::sync::broadcast;
use yi_runtime::egress::{BadHead, Dialer, Lease, Limits, Proxy, connect_head, public_address};
use yi_runtime::{AskOutcome, Asker, PermissionBroker, PermissionMode, Wall};
use yi_types::egress::Domain;
use yi_types::permission::{Answerer, PermissionRecord};

type TestResult = Result<(), Box<dyn Error>>;

const SEEDED: &str = "crates.io";
const TOKEN_HEX: usize = 32;

/// Echoes bytes until its peer half-closes, then says `bye` and closes: a stand-in for the host
/// a real dial would reach, since a test may not leave this machine.
fn stand_in() -> Result<Dialer, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut bytes = [0u8; 256];
                while let Ok(read) = stream.read(&mut bytes) {
                    if read == 0 {
                        break;
                    }
                    let _ = stream.write_all(bytes.get(..read).unwrap_or_default());
                }
                let _ = stream.write_all(b"bye");
            });
        }
    });
    Ok(Arc::new(move |_: &Domain, _: u16| {
        TcpStream::connect(address)
    }))
}

fn broker(
    mode: PermissionMode,
    asker: Option<Asker>,
) -> Result<Arc<PermissionBroker>, Box<dyn Error>> {
    let (events, _) = broadcast::channel(16);
    let cwd = std::env::temp_dir();
    Ok(Arc::new(PermissionBroker::new(
        mode,
        cwd,
        Vec::new(),
        asker,
        events,
    )))
}

fn headless() -> Result<Arc<PermissionBroker>, Box<dyn Error>> {
    broker(PermissionMode::Auto, None)
}

fn limits() -> Limits {
    Limits {
        patience: Duration::from_millis(300),
        ..Limits::default()
    }
}

fn wall() -> Wall {
    Wall::default()
}

/// The URL a child is handed, split the way a client splits it.
fn proxy_of(lease: &Lease) -> Result<(String, u16), Box<dyn Error>> {
    let env = lease.env();
    let url = (env.iter())
        .find(|(key, _)| key == "HTTPS_PROXY")
        .map(|(_, url)| url.as_str())
        .ok_or("no HTTPS_PROXY")?;
    let rest = url.strip_prefix("http://yi:").ok_or("no credentials")?;
    let (token, port) = rest.split_once("@127.0.0.1:").ok_or("not loopback")?;
    assert_eq!(token.len(), TOKEN_HEX);
    Ok((token.to_owned(), port.parse()?))
}

/// RFC 4648 by hand: what a client sends must not come from the code that reads it.
fn basic(user_pass: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in user_pass.as_bytes().chunks(3) {
        let at = |index: usize| chunk.get(index).copied().map_or(0u32, u32::from);
        let bits = (at(0) << 16) | (at(1) << 8) | at(2);
        for place in 0..4usize {
            let index = (bits >> (18 - 6 * place)) & 63;
            let wanted = place <= chunk.len();
            let symbol = TABLE.get(index as usize).copied().unwrap_or(b'=');
            out.push(if wanted { char::from(symbol) } else { '=' });
        }
    }
    out
}

fn head(domain: &str, token: &str) -> String {
    let credential = basic(&format!("yi:{token}"));
    format!(
        "CONNECT {domain}:443 HTTP/1.1\r\nHost: {domain}:443\r\nProxy-Authorization: Basic {credential}\r\n\r\n"
    )
}

/// One CONNECT; its status line, and the stream to go on with.
fn connect(port: u16, request: &str) -> Result<(String, TcpStream), Box<dyn Error>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(request.as_bytes())?;
    let mut status = Vec::new();
    let mut byte = [0u8; 1];
    while !status.ends_with(b"\r\n\r\n") && stream.read(&mut byte)? == 1 {
        status.extend(byte);
    }
    let line = String::from_utf8_lossy(&status);
    let first = line.lines().next().unwrap_or_default().to_owned();
    Ok((first, stream))
}

fn through(lease: &Lease, domain: &str) -> Result<(String, TcpStream), Box<dyn Error>> {
    let (token, port) = proxy_of(lease)?;
    connect(port, &head(domain, &token))
}

/// Counts asks, and answers each as told.
fn asker(answer: AskOutcome, asked: &Arc<AtomicUsize>) -> Asker {
    let asked = Arc::clone(asked);
    Arc::new(move |_ask| {
        asked.fetch_add(1, Ordering::SeqCst);
        answer
    })
}

fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    (0..200).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        done()
    })
}

#[test]
fn a_seeded_host_tunnels_both_ways_and_a_half_close_carries() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (status, mut stream) = through(&lease, SEEDED)?;
    assert!(status.contains("200"), "{status}");
    stream.write_all(b"ping")?;
    let mut echoed = [0u8; 4];
    stream.read_exact(&mut echoed)?;
    assert_eq!(&echoed, b"ping");
    // Incident shape: a client that sends its last request and half-closes still reads the answer.
    stream.shutdown(Shutdown::Write)?;
    let mut rest = String::new();
    stream.read_to_string(&mut rest)?;
    assert_eq!(rest, "bye");
    Ok(())
}

#[test]
fn bytes_sent_with_the_head_reach_the_host() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (token, port) = proxy_of(&lease)?;
    let (status, mut stream) = connect(port, &format!("{}early", head(SEEDED, &token)))?;
    assert!(status.contains("200"), "{status}");
    let mut echoed = [0u8; 5];
    stream.read_exact(&mut echoed)?;
    assert_eq!(&echoed, b"early");
    Ok(())
}

#[test]
fn no_token_another_token_and_a_token_past_its_spawn_get_407() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (token, port) = proxy_of(&lease)?;
    let bare = format!("CONNECT {SEEDED}:443 HTTP/1.1\r\n\r\n");
    assert!(connect(port, &bare)?.0.contains("407"));
    // A client like git offers its credentials only when the 407 names a scheme.
    let mut raw = TcpStream::connect(("127.0.0.1", port))?;
    raw.write_all(bare.as_bytes())?;
    let mut answer = String::new();
    raw.read_to_string(&mut answer)?;
    assert!(answer.contains("Proxy-Authenticate: Basic"), "{answer}");
    assert!(
        connect(port, &head(SEEDED, &"0".repeat(TOKEN_HEX)))?
            .0
            .contains("407")
    );
    assert!(connect(port, &head(SEEDED, &token))?.0.contains("200"));
    let _no_refusals = lease.revoke();
    assert!(connect(port, &head(SEEDED, &token))?.0.contains("407"));
    Ok(())
}

#[test]
fn a_spawns_end_closes_its_tunnels() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (status, mut stream) = through(&lease, SEEDED)?;
    assert!(status.contains("200"), "{status}");
    let _no_refusals = lease.revoke();
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest)?;
    assert!(rest.is_empty(), "a revoked tunnel still carried {rest:?}");
    Ok(())
}

#[test]
fn an_unknown_host_headless_is_refused_with_a_row_naming_it() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (status, _stream) = through(&lease, "example.com")?;
    assert!(
        status.contains("403") && status.contains("example.com"),
        "{status}"
    );
    let rows = lease.take_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = rows.first().ok_or("no row")?;
    assert!(row.starts_with("[egress: refused example.com: "), "{row}");
    Ok(())
}

#[test]
fn a_person_is_asked_once_and_their_refusal_stands_for_the_holder() -> TestResult {
    let asked = Arc::new(AtomicUsize::new(0));
    let proxy = Proxy::start(stand_in()?, limits())?;
    let broker = broker(
        PermissionMode::Auto,
        Some(asker(AskOutcome::Reject, &asked)),
    )?;
    let lease = proxy.lease(broker, wall())?;
    for _ in 0..3 {
        assert!(through(&lease, "example.com")?.0.contains("403"));
    }
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn an_allow_once_lets_this_holder_and_asks_the_next() -> TestResult {
    let asked = Arc::new(AtomicUsize::new(0));
    let proxy = Proxy::start(stand_in()?, limits())?;
    let broker = broker(
        PermissionMode::Auto,
        Some(asker(AskOutcome::AllowOnce, &asked)),
    )?;
    let first = proxy.lease(Arc::clone(&broker), wall())?;
    assert!(through(&first, "example.com")?.0.contains("200"));
    assert!(through(&first, "example.com")?.0.contains("200"));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    let second = proxy.lease(broker, wall())?;
    assert!(through(&second, "example.com")?.0.contains("200"));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn an_always_reaches_later_holders_unasked() -> TestResult {
    let asked = Arc::new(AtomicUsize::new(0));
    let proxy = Proxy::start(stand_in()?, limits())?;
    let broker = broker(
        PermissionMode::Auto,
        Some(asker(AskOutcome::AllowAlways(0), &asked)),
    )?;
    let first = proxy.lease(Arc::clone(&broker), wall())?;
    assert!(through(&first, "example.com")?.0.contains("200"));
    let second = proxy.lease(broker, wall())?;
    assert!(through(&second, "example.com")?.0.contains("200"));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn a_host_in_sandbox_egress_needs_no_ask_and_yolo_needs_none_either() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let (events, _) = broadcast::channel(16);
    let named = PermissionBroker::new(
        PermissionMode::Auto,
        std::env::temp_dir(),
        Vec::new(),
        None,
        events,
    )
    .with_egress(vec![Domain::parse("corp.example")?]);
    let lease = proxy.lease(Arc::new(named), wall())?;
    assert!(through(&lease, "corp.example")?.0.contains("200"));
    assert!(through(&lease, "other.example")?.0.contains("403"));
    let yolo = proxy.lease(broker(PermissionMode::Yolo, None)?, wall())?;
    assert!(through(&yolo, "other.example")?.0.contains("200"));
    Ok(())
}

#[test]
fn a_walled_url_outranks_the_seed() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let walled = Wall {
        deny_url: vec![format!("https://{SEEDED}/api")],
        ..Wall::default()
    };
    let lease = proxy.lease(headless()?, walled)?;
    assert!(through(&lease, SEEDED)?.0.contains("403"));
    assert!(through(&lease, "pypi.org")?.0.contains("200"));
    Ok(())
}

/// A person spells a deny entry with the port they see in a URL; the tunnel's host has no port,
/// so an entry that keeps one closes nothing unless it is cut back to the host it names.
#[test]
fn a_wall_entry_that_spells_a_port_closes_its_host() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let walled = Wall {
        deny_url: vec![format!("https://{SEEDED}:443/")],
        ..Wall::default()
    };
    let lease = proxy.lease(headless()?, walled)?;
    assert!(through(&lease, SEEDED)?.0.contains("403"));
    Ok(())
}

/// A wall is a prefix list: the form a fetch honours has to close the tunnel too.
#[test]
fn a_bare_scheme_in_the_wall_closes_every_host() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let walled = Wall {
        deny_url: vec!["https://".to_owned()],
        ..Wall::default()
    };
    let lease = proxy.lease(headless()?, walled)?;
    assert!(through(&lease, SEEDED)?.0.contains("403"));
    Ok(())
}

#[test]
fn the_first_decision_per_host_reaches_the_ledger_once() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let broker = headless()?;
    let records: Arc<Mutex<Vec<PermissionRecord>>> = Arc::default();
    let kept = Arc::clone(&records);
    broker.set_journal(Arc::new(move |record| {
        if let Ok(mut records) = kept.lock() {
            records.push(record);
        }
    }));
    let lease = proxy.lease(broker, wall())?;
    for _ in 0..3 {
        assert!(through(&lease, SEEDED)?.0.contains("200"));
    }
    let records = records.lock().map_err(|_| "poisoned")?;
    let seeded: Vec<&PermissionRecord> = (records.iter())
        .filter(|record| record.title == "egress")
        .collect();
    assert_eq!(seeded.len(), 1, "{seeded:?}");
    assert!(
        seeded
            .iter()
            .all(|record| record.allowed && record.by == Answerer::Other("seed".to_owned()))
    );
    Ok(())
}

#[test]
fn the_token_is_never_in_a_row_a_debug_or_a_record() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let broker = headless()?;
    let records: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = Arc::clone(&records);
    broker.set_journal(Arc::new(move |record| {
        if let Ok(mut records) = kept.lock() {
            records.push(format!("{record:?}"));
        }
    }));
    let lease = proxy.lease(broker, wall())?;
    let (token, _) = proxy_of(&lease)?;
    assert!(through(&lease, "example.com")?.0.contains("403"));
    assert!(through(&lease, SEEDED)?.0.contains("200"));
    let debug = format!("{lease:?}");
    assert!(!debug.contains(&token), "{debug}");
    assert!(!lease.take_rows().concat().contains(&token));
    assert!(
        !records
            .lock()
            .map_err(|_| "poisoned")?
            .concat()
            .contains(&token)
    );
    Ok(())
}

#[test]
fn a_failed_entropy_source_refuses_the_lease() -> TestResult {
    let absent = std::env::temp_dir().join("yi-egress-no-such-entropy");
    let proxy = Proxy::start(stand_in()?, limits())?.with_entropy(absent);
    assert!(proxy.lease(headless()?, wall()).is_err());
    Ok(())
}

#[test]
fn a_slow_head_is_cut_at_the_deadline() -> TestResult {
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (_, port) = proxy_of(&lease)?;
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(b"CONNECT crates.io:443 HTTP/1.1\r\n")?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer)?;
    assert!(answer.starts_with("HTTP/1.1 400"), "{answer:?}");
    Ok(())
}

#[test]
fn a_head_passes_at_its_limit_and_fails_one_past() -> TestResult {
    const LIMIT: usize = 8 * 1024;
    let proxy = Proxy::start(stand_in()?, limits())?;
    let lease = proxy.lease(headless()?, wall())?;
    let (token, port) = proxy_of(&lease)?;
    let base = head(SEEDED, &token);
    let sized = |total: usize| -> Option<String> {
        let (start, end) = base.split_at(base.len().checked_sub(2)?);
        let padding = total
            .checked_sub(base.len())?
            .checked_sub("X-Pad: \r\n".len())?;
        Some(format!("{start}X-Pad: {}\r\n{end}", "x".repeat(padding)))
    };
    let at = sized(LIMIT).ok_or("short")?;
    assert_eq!(at.len(), LIMIT);
    assert!(connect(port, &at)?.0.contains("200"));
    let past = sized(LIMIT + 1).ok_or("short")?;
    assert!(connect(port, &past)?.0.contains("400"));
    Ok(())
}

#[test]
fn the_connection_cap_holds_at_its_limit_and_refuses_one_past() -> TestResult {
    let proxy = Proxy::start(
        stand_in()?,
        Limits {
            connections: 3,
            patience: Duration::from_secs(5),
            ..limits()
        },
    )?;
    let lease = proxy.lease(headless()?, wall())?;
    let (_, port) = proxy_of(&lease)?;
    let held: Vec<TcpStream> = (0..3)
        .map(|_| TcpStream::connect(("127.0.0.1", port)))
        .collect::<Result<_, _>>()?;
    for mut stream in &held {
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut byte = [0u8; 1];
        let waited = stream.read(&mut byte);
        assert!(
            waited.is_err(),
            "the cap refused a connection at its limit: {waited:?}"
        );
    }
    let (status, _) = connect(port, "")?;
    assert!(status.contains("503 egress.connections 3"), "{status}");
    Ok(())
}

#[test]
fn the_per_holder_cap_refuses_one_past_and_leaves_a_sibling_alone() -> TestResult {
    let proxy = Proxy::start(
        stand_in()?,
        Limits {
            per_holder: 2,
            ..limits()
        },
    )?;
    let lease = proxy.lease(headless()?, wall())?;
    let mut tunnels = Vec::new();
    for _ in 0..2 {
        let (status, stream) = through(&lease, SEEDED)?;
        assert!(status.contains("200"), "{status}");
        tunnels.push(stream);
    }
    assert!(
        through(&lease, SEEDED)?
            .0
            .contains("503 egress.per_holder 2")
    );
    let rows = lease.take_rows();
    assert!(rows.concat().contains("egress.per_holder 2"), "{rows:?}");
    let sibling = proxy.lease(headless()?, wall())?;
    assert!(through(&sibling, SEEDED)?.0.contains("200"));
    Ok(())
}

/// A person who has not answered: every ask blocks until the test ends.
fn silent(asked: &Arc<AtomicUsize>) -> Asker {
    let (hold, wait) = std::sync::mpsc::channel::<()>();
    let (asked, wait) = (Arc::clone(asked), Mutex::new(wait));
    std::mem::forget(hold);
    Arc::new(move |_ask| {
        asked.fetch_add(1, Ordering::SeqCst);
        let _blocked = wait.lock().map(|wait| wait.recv());
        AskOutcome::Reject
    })
}

#[test]
fn sixty_five_hosts_from_one_holder_raise_at_most_four_asks() -> TestResult {
    let asked = Arc::new(AtomicUsize::new(0));
    let proxy = Proxy::start(
        stand_in()?,
        Limits {
            patience: Duration::from_millis(20),
            ..limits()
        },
    )?;
    let lease = proxy.lease(broker(PermissionMode::Auto, Some(silent(&asked)))?, wall())?;
    for host in 0..65 {
        let (status, _) = through(&lease, &format!("host{host}.example"))?;
        assert!(status.contains("403"), "{status}");
    }
    assert!(wait_for(|| asked.load(Ordering::SeqCst) >= 1));
    let rows = lease.take_rows().concat();
    assert!(rows.contains("egress.pending_asks 4"), "{rows}");
    Ok(())
}

#[test]
fn the_decision_table_refuses_a_new_host_one_past_its_limit() -> TestResult {
    let asked = Arc::new(AtomicUsize::new(0));
    let proxy = Proxy::start(
        stand_in()?,
        Limits {
            decisions: 2,
            ..limits()
        },
    )?;
    let lease = proxy.lease(
        broker(
            PermissionMode::Auto,
            Some(asker(AskOutcome::AllowOnce, &asked)),
        )?,
        wall(),
    )?;
    assert!(through(&lease, "one.example")?.0.contains("200"));
    assert!(through(&lease, "two.example")?.0.contains("200"));
    assert!(through(&lease, "three.example")?.0.contains("403"));
    assert!(through(&lease, "one.example")?.0.contains("200"));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert!(lease.take_rows().concat().contains("egress.decisions 2"));
    Ok(())
}

#[test]
fn rows_stop_at_their_cap_and_say_so_once() -> TestResult {
    let proxy = Proxy::start(
        stand_in()?,
        Limits {
            rows: 2,
            ..limits()
        },
    )?;
    let lease = proxy.lease(headless()?, wall())?;
    for host in ["a.example", "b.example"] {
        assert!(through(&lease, host)?.0.contains("403"));
    }
    let at = lease.take_rows();
    assert!(!at.concat().contains("egress.rows"), "{at:?}");
    for host in ["c.example", "d.example", "e.example", "f.example"] {
        assert!(through(&lease, host)?.0.contains("403"));
    }
    let past = lease.take_rows();
    assert_eq!(past.len(), 3, "{past:?}");
    assert!(
        past.last().is_some_and(|row| row.contains("egress.rows 2")),
        "{past:?}"
    );
    Ok(())
}

/// Dies with the address filter gone: a listener answers on loopback, so only the filter says 502.
#[test]
fn a_name_that_resolves_to_this_machine_is_not_dialed() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let proxy = Proxy::start(Arc::new(yi_runtime::egress::dial_public), limits())?;
    let lease = proxy.lease(broker(PermissionMode::Yolo, None)?, wall())?;
    let (token, proxy_port) = proxy_of(&lease)?;
    let request = head("localhost", &token).replace("localhost:443", &format!("localhost:{port}"));
    let (status, _stream) = connect(proxy_port, &request)?;
    assert!(status.contains("502"), "{status}");
    assert!(lease.take_rows().concat().contains("did not answer"));
    Ok(())
}

#[test]
fn an_address_is_public_only_outside_every_private_range() -> TestResult {
    let private: [[u8; 4]; 7] = [
        [127, 0, 0, 1],
        [10, 0, 0, 1],
        [172, 16, 0, 1],
        [192, 168, 1, 1],
        [169, 254, 169, 254],
        [100, 64, 0, 1],
        [0, 0, 0, 0],
    ];
    for octets in private {
        let v4 = Ipv4Addr::from(octets);
        assert!(!public_address(IpAddr::V4(v4)), "{v4}");
        assert!(
            !public_address(IpAddr::V6(v4.to_ipv6_mapped())),
            "mapped {v4}"
        );
    }
    for address in ["::1", "fe80::1", "fc00::1"] {
        assert!(!public_address(address.parse::<IpAddr>()?), "{address}");
    }
    for address in ["1.1.1.1", "100.128.0.1", "2606:4700:4700::1111"] {
        assert!(public_address(address.parse::<IpAddr>()?), "{address}");
    }
    Ok(())
}

#[test]
fn an_address_is_no_host_name_in_any_spelling() {
    for literal in [
        "127.0.0.1",
        "2130706433",
        "0x7f.1",
        "0x7f000001",
        "[::1]",
        "1.2.3.4.",
        "a b",
        "",
        "..",
        "-a.b",
    ] {
        let request = format!("CONNECT {literal}:443 HTTP/1.1\r\n\r\n");
        assert_eq!(
            connect_head(request.as_bytes()),
            Err(BadHead::Authority),
            "{literal}"
        );
    }
}

#[test]
fn a_head_reads_its_name_port_and_credential_however_the_header_is_cased() -> TestResult {
    let parsed = connect_head(
        b"CONNECT Crates.IO.:8443 HTTP/1.1\r\nproxy-AUTHORIZATION:   Basic abc=\r\n\r\n",
    )
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(parsed.domain.as_str(), "crates.io");
    assert_eq!(parsed.port, 8443);
    assert_eq!(parsed.credential.as_deref(), Some("abc="));
    assert_eq!(
        connect_head(b"GET / HTTP/1.1\r\n\r\n"),
        Err(BadHead::NotConnect)
    );
    assert_eq!(
        connect_head(b"CONNECT a.io:443 HTTP/1.1\r\n"),
        Err(BadHead::Incomplete)
    );
    assert_eq!(connect_head(b"\xff\xfe\r\n\r\n"), Err(BadHead::NotConnect));
    Ok(())
}

proptest! {
    #[test]
    fn no_head_panics_the_parser(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _either_verdict_is_fine = connect_head(&bytes);
    }

    #[test]
    fn no_text_panics_the_domain_parser(text in ".{0,300}") {
        let _either_verdict_is_fine = Domain::parse(&text);
    }
}
