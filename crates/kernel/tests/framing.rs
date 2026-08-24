use serde_json::{Map, Value, json};
use yi_kernel::framing::{DELIM, build_message, decode, encode};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn map(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn signature_matches_python_hashlib_reference() -> TestResult {
    let message = build_message(
        "execute_request",
        map(json!({"code": "1+1"})),
        "s-1",
        "yi",
        "m-1".to_owned(),
        "2026-08-24T00:00:00Z".to_owned(),
    );
    let frames = encode(&message, "secret-key");
    assert_eq!(frames[0], DELIM);
    assert_eq!(
        String::from_utf8(frames[1].clone())?,
        "ebc03e7a5436037a8ff64606bee2f6a5d246c0d8812f657a8eee9a9eadcc2465",
        "ipykernel drops messages whose hex HMAC-SHA256 over the four frames differs"
    );
    Ok(())
}

#[test]
fn decode_skips_identity_frames_and_round_trips() -> TestResult {
    let message = build_message(
        "kernel_info_request",
        Map::new(),
        "s-2",
        "yi",
        "m-2".to_owned(),
        "2026-08-24T00:00:00Z".to_owned(),
    );
    let mut frames = vec![b"zmq-identity".to_vec()];
    frames.extend(encode(&message, "k"));
    let decoded = decode(&frames).ok_or("decode failed")?;
    assert_eq!(decoded, message);
    Ok(())
}

#[test]
fn decode_rejects_truncated_and_delimiterless_frames() {
    assert!(decode(&[b"no-delim".to_vec()]).is_none());
    assert!(decode(&[DELIM.to_vec(), b"sig".to_vec(), b"{}".to_vec()]).is_none());
    assert!(
        decode(&[
            DELIM.to_vec(),
            b"sig".to_vec(),
            b"not-json".to_vec(),
            b"{}".to_vec(),
            b"{}".to_vec(),
            b"{}".to_vec(),
        ])
        .is_none()
    );
}
