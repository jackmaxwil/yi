use serde_json::{Map, Value};
use yi_tools::{BashTool, Tool, ToolKind};

fn input(command: &str) -> Map<String, Value> {
    let mut input = Map::new();
    input.insert("command".to_owned(), Value::String(command.to_owned()));
    input
}

/// Incident: a hand-split verb list called each of these a read, so the advisor and the
/// parallel scheduler treated a delete as harmless; the permission classifier sees the write.
#[test]
fn a_command_that_writes_behind_a_read_verb_is_not_a_read() {
    let bash = BashTool::default();
    for command in [
        "find . -delete",
        "fd -x rm",
        "git branch -D x",
        "env rm -rf x",
        "ls & rm -rf x",
        "find . -delete>/dev/null",
        "find . -name '*.tmp' -delete>/dev/null",
        "git branch -Dx>/dev/null",
        "echo hi>/etc/hosts",
        "rg x src>/etc/hosts",
        "rg -l Tool crates>/tmp/x",
        "cat <(rm${IFS}-rf${IFS}src)",
    ] {
        let input = input(command);
        assert_eq!(bash.kind_for(&input), ToolKind::Exec, "{command:?}");
        assert!(bash.irreversible(&input), "{command:?}");
    }
}

/// Two searches are reads the loop runs side by side, so the classifier must keep calling them
/// reads, and so is a command that prints one named variable.
#[test]
fn read_only_searches_stay_reads() {
    let bash = BashTool::default();
    for command in [
        "rg -n parse crates",
        "rg -l Tool crates/tools 2>/dev/null",
        "ag -n parse crates",
        "printenv PATH",
    ] {
        let input = input(command);
        assert_eq!(bash.kind_for(&input), ToolKind::Read, "{command:?}");
        assert!(!bash.irreversible(&input), "{command:?}");
    }
    // A bare `env` prints every key in the process, so a pipeline over it is not a proven read.
    for command in ["env | grep PATH", "printenv OPENAI_API_KEY"] {
        let input = input(command);
        assert_eq!(bash.kind_for(&input), ToolKind::Exec, "{command:?}");
        assert!(bash.irreversible(&input), "{command:?}");
    }
}
