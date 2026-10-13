use std::path::Path;
use std::time::{Duration, Instant};

use yi_browser::{Browser, Update};
use yi_types::cdp::{MouseButton, MouseInput, MouseKind};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn press(kind: MouseKind, buttons: u8) -> MouseInput {
    MouseInput {
        kind,
        x: 160,
        y: 100,
        button: MouseButton::Left,
        buttons,
        click_count: 1,
        delta_x: 0,
        delta_y: 0,
        modifiers: 0,
    }
}

/// Polls until `done` holds over everything seen since the call, or 20 s pass.
fn until(
    browser: &mut Browser,
    mut done: impl FnMut(&[Update]) -> bool,
) -> Result<Vec<Update>, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(browser.poll());
        if let Some(Update::Failed(why)) = seen.iter().find(|u| matches!(u, Update::Failed(_))) {
            return Err(why.clone());
        }
        if done(&seen) {
            return Ok(seen);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let shown: Vec<String> = seen
        .iter()
        .map(|u| match u {
            Update::Frame(_) => "frame".to_owned(),
            other => format!("{other:?}"),
        })
        .collect();
    Err(format!("timed out after {shown:?}"))
}

fn frames(seen: &[Update]) -> usize {
    seen.iter()
        .filter(|u| matches!(u, Update::Frame(_)))
        .count()
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the journey serves its pages with the real `python -m http.server`"
)]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_click_on_a_served_link_brings_its_page_and_a_new_frame() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-browser-journey-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("index.html"),
        "<body style=margin:0><a href=next.html style='display:block;width:100vw;\
         height:100vh;background:#c33'>next</a></body>",
    )?;
    std::fs::write(
        dir.join("next.html"),
        "<body style='background:#3c3'>arrived</body>",
    )?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let _server = Server(
        std::process::Command::new("python3")
            .args([
                "-m",
                "http.server",
                &port.to_string(),
                "--bind",
                "127.0.0.1",
            ])
            .arg("--directory")
            .arg(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if Instant::now() > deadline {
            return Err("python -m http.server never listened".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let chrome = yi_browser::find(None)
        .map_err(|looked| format!("no Chrome: looked in {}", looked.join(", ")))?;
    let url = format!("http://127.0.0.1:{port}/");
    let mut browser = Browser::launch(
        Path::new(env!("CARGO_BIN_EXE_yi")),
        &chrome,
        &dir.join("profile"),
        &url,
        (320, 200),
    )?;
    until(&mut browser, |seen| frames(seen) > 0)?;
    browser.router.mouse(&press(MouseKind::MousePressed, 1));
    browser.router.mouse(&press(MouseKind::MouseReleased, 0));
    let next = format!("{url}next.html");
    let seen = until(&mut browser, |seen| {
        let at = seen.iter().position(|u| *u == Update::Url(next.clone()));
        at.is_some_and(|at| frames(seen.get(at..).unwrap_or_default()) > 0)
    })?;
    assert!(seen.contains(&Update::Url(next)), "{seen:?}");
    drop(browser);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
