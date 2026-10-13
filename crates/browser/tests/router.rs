use serde_json::Value;
use yi_browser::{Router, Update};
use yi_types::cdp::CdpMessage;

const FIRST: &str = "data:text/html,<body style='margin:0;background:%23c33'><a href=x>first</a>";
const SECOND: &str = "data:text/html,<body style='margin:0;background:%233c3'>second</body>";

fn fixture(name: &str) -> Result<Vec<u8>, std::io::Error> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../types/tests/fixtures/");
    std::fs::read(format!("{dir}{name}"))
}

fn sent(router: &mut Router) -> Result<Vec<Value>, serde_json::Error> {
    router.take().iter().map(serde_json::to_value).collect()
}

/// The transcript is Chrome's own pipe output, recorded while a client sent the commands in
/// the sent file; replayed through the router, the router must send exactly those commands.
#[test]
fn a_recorded_pipe_replays_into_the_commands_chrome_answered()
-> Result<(), Box<dyn std::error::Error>> {
    let transcript = fixture("cdp-pipe-v1.bin")?;
    let expected: Vec<Value> = serde_json::from_slice(&fixture("cdp-pipe-v1-sent.json")?)?;
    let mut router = Router::new(FIRST, (64, 48));
    let mut commands = sent(&mut router)?;
    let mut updates = Vec::new();
    for frame in transcript
        .split(|byte| *byte == 0)
        .filter(|frame| !frame.is_empty())
    {
        let message: CdpMessage = serde_json::from_slice(frame)?;
        let update = router.feed(message);
        let first_frame = matches!(update, Some(Update::Frame(_)))
            && !updates.iter().any(|u| matches!(u, Update::Frame(_)));
        updates.extend(update);
        if first_frame {
            commands.extend(sent(&mut router)?);
            router.navigate(SECOND);
        }
        commands.extend(sent(&mut router)?);
    }
    assert_eq!(commands, expected);
    let shapes: Vec<&str> = updates
        .iter()
        .map(|update| match update {
            Update::Frame(png) if png.starts_with("iVBORw0KGgo") => "png",
            Update::Frame(_) => "frame that is not a PNG",
            Update::Url(url) if url == SECOND => "second",
            Update::Url(_) => "another url",
            Update::Failed(_) => "failed",
        })
        .collect();
    assert_eq!(shapes, ["png", "second", "png"]);
    Ok(())
}

#[test]
fn an_error_answer_names_its_method_and_a_stray_id_is_ignored()
-> Result<(), Box<dyn std::error::Error>> {
    let mut router = Router::new("about:blank", (10, 10));
    router.take();
    let stray: CdpMessage = serde_json::from_str(r#"{"id": 99, "result": {}}"#)?;
    assert_eq!(router.feed(stray), None);
    let refused: CdpMessage =
        serde_json::from_str(r#"{"id": 1, "error": {"code": -32000, "message": "no target"}}"#)?;
    assert_eq!(
        router.feed(refused),
        Some(Update::Failed("Target.createTarget: no target".to_owned()))
    );
    Ok(())
}
