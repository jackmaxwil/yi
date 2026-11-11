#[test]
fn version_path_does_not_spawn_assist() {
    let src = include_str!("../src/main.rs");
    let mut lines = src
        .lines()
        .skip_while(|line| !line.starts_with("fn run_version"));
    let mut body = String::new();
    if let Some(head) = lines.next() {
        body.push_str(head);
    }
    for line in lines {
        if line.starts_with("fn ") {
            break;
        }
        body.push('\n');
        body.push_str(line);
    }
    assert!(!body.contains("Command"), "{body}");
    assert!(!body.contains("spell"), "{body}");
    assert!(!body.contains("predict"), "{body}");
}
