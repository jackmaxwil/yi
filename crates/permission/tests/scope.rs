use std::error::Error;
use std::path::PathBuf;

use yi_permission::{
    CatastrophicContext, Decision, PermissionMode, SessionRules, ToolCall, decide, write_grant,
    write_grant_dir, write_scope,
};
use yi_types::permission::{RuleDecision, RuleKind};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const MODES: [PermissionMode; 3] = [
    PermissionMode::Ask,
    PermissionMode::Auto,
    PermissionMode::Yolo,
];

struct Tree {
    _scratch: Scratch,
    root: PathBuf,
    context: CatastrophicContext,
    rules: SessionRules,
}

fn tree(scopes: &[&str]) -> Result<Tree, Box<dyn Error>> {
    let scratch = Scratch::new("yi-scope")?;
    let root = scratch.join("project");
    std::fs::create_dir_all(root.join("crates/runtime/src"))?;
    std::fs::create_dir_all(root.join("evals"))?;
    let root = root.canonicalize()?;
    let context = CatastrophicContext {
        home_dir: Some(scratch.join("home")),
        working_dir: Some(root.clone()),
        workspace_git: vec![root.join(".git")],
        host_owned: Vec::new(),
    };
    let mut rules = SessionRules::new();
    for scope in scopes {
        let kept = write_scope(scope)?;
        rules.insert(kept.kind, &kept.canonical, &kept.label, RuleDecision::Allow)?;
    }
    Ok(Tree {
        _scratch: scratch,
        root,
        context,
        rules,
    })
}

impl Tree {
    /// One call to `tool`: a read, or a write when `writes` (`grep` is a read until it applies).
    fn judge(
        &self,
        mode: PermissionMode,
        (tool, writes): (&str, bool),
        (paths, command): (&[&str], Option<&str>),
    ) -> Decision {
        let targets: Vec<PathBuf> = paths.iter().map(|path| self.root.join(path)).collect();
        let call = ToolCall {
            tool_name: tool,
            reads_only: matches!(tool, "grep" | "read"),
            irreversible: writes,
            in_workspace: true,
            rule_kind: RuleKind::StructuredTool,
            canonical: "canonical",
            display: command.unwrap_or(tool),
            targets: &targets,
            command,
        };
        decide(&call, mode, &[], &self.rules, &[], &self.context)
    }

    fn write(&self, mode: PermissionMode, paths: &[&str]) -> Decision {
        self.judge(mode, ("write", true), (paths, None))
    }

    fn bash(&self, command: &str) -> Decision {
        let yolo = PermissionMode::Yolo;
        self.judge(yolo, ("bash", true), (&[], Some(command)))
    }

    fn grep(&self, paths: &[&str], applies: bool) -> Decision {
        self.judge(PermissionMode::Yolo, ("grep", applies), (paths, None))
    }
}

fn refusal(decision: &Decision) -> Option<&str> {
    match decision {
        Decision::Deny { reason } => Some(reason),
        _ => None,
    }
}

/// Dies with a fix pass editing `crates/runtime` under a brief for `evals/` (#1630, #1638): the
/// refusal must hold in yolo, where those runs ran, and must name the scope and the path.
#[test]
fn a_write_outside_the_scope_is_refused_in_every_mode_and_one_inside_is_not() -> TestResult {
    let (scoped, open) = (tree(&["evals/**"])?, tree(&[])?);
    for mode in MODES {
        let reason = refusal(&scoped.write(mode, &["crates/runtime/src/lock.rs"]))
            .ok_or("an out-of-scope write was not refused")?
            .to_owned();
        assert!(reason.contains("crates/runtime/src/lock.rs"), "{reason}");
        assert!(reason.contains("evals/**"), "{reason}");
        let inside = scoped.write(mode, &["evals/sweep/probe.py"]);
        assert!(refusal(&inside).is_none(), "{mode:?}: {inside:?}");
        let control = open.write(mode, &["crates/runtime/src/lock.rs"]);
        assert!(refusal(&control).is_none(), "{mode:?}: {control:?}");
    }
    Ok(())
}

/// Dies with one allowed path letting a second out-of-scope path of the same `edit` through, as
/// a patch with a `MV` destination or several sections names more than one.
#[test]
fn every_path_a_call_names_must_be_inside() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    let paths = ["evals/a.py", "crates/b.rs", "evals/c.py"];
    let decision = scoped.write(PermissionMode::Yolo, &paths);
    let reason = refusal(&decision).ok_or("a mixed call was allowed")?;
    assert!(
        reason.contains("crates/b.rs") && !reason.contains("a.py"),
        "{reason}"
    );
    Ok(())
}

/// Dies with a refused read, or a refused write to `/tmp`: a scope is the brief's files, and the
/// sandbox and the in-tree ask already bound a path outside the working tree.
#[test]
fn reads_and_paths_outside_the_tree_are_not_the_scopes_to_judge() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    let read = scoped.grep(&["crates/runtime/src/lib.rs"], false);
    assert!(refusal(&read).is_none(), "{read:?}");
    let elsewhere = scoped.root.with_file_name("elsewhere/x.txt");
    let abroad = scoped.write(PermissionMode::Yolo, &[&elsewhere.to_string_lossy()]);
    assert!(refusal(&abroad).is_none(), "{abroad:?}");
    Ok(())
}

/// Dies with a link inside `evals/` carrying a write out to `crates/`: the scope reads where the
/// kernel's open lands, not the spelling, and `link/..` is the link target's parent.
#[test]
fn a_link_inside_the_scope_does_not_carry_a_write_out() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    let (crates, evals) = (scoped.root.join("crates"), scoped.root.join("evals"));
    std::os::unix::fs::symlink(&crates, evals.join("out"))?;
    std::os::unix::fs::symlink(crates.join("runtime"), evals.join("up"))?;
    for (path, lands) in [
        ("evals/out/runtime/src/new.rs", "crates/runtime/src/new.rs"),
        ("evals/up/../lock.rs", "crates/lock.rs"),
    ] {
        let decision = scoped.write(PermissionMode::Yolo, &[path]);
        let reason = refusal(&decision).ok_or("a write through a link was allowed")?;
        assert!(reason.contains(lands), "{path}: {reason}");
    }
    Ok(())
}

/// Dies with `grep ... apply` rewriting the whole tree: it names no path of its own, so a call
/// with none walks from the working directory, and a root inside the scope is a write inside it.
#[test]
fn a_grep_that_rewrites_is_judged_by_its_root_and_a_search_is_not() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    let walked = scoped.grep(&[], true);
    let reason = refusal(&walked).ok_or("a rewrite of the whole tree was allowed")?;
    assert!(reason.contains("the working directory"), "{reason}");
    assert!(refusal(&scoped.grep(&["crates"], true)).is_some());
    assert!(refusal(&scoped.grep(&["evals"], true)).is_none());
    assert!(refusal(&scoped.grep(&[], false)).is_none());
    Ok(())
}

/// Dies with `evals` refusing its own files, `*` walking into subdirectories as `**` does, or a
/// second scope replacing the first instead of widening it.
#[test]
fn a_scope_names_a_directory_and_a_star_stops_at_a_slash() -> TestResult {
    let allowed =
        |scoped: &Tree, path: &str| refusal(&scoped.write(PermissionMode::Yolo, &[path])).is_none();
    let dir = tree(&["evals"])?;
    assert!(allowed(&dir, "evals/sweep/probe.py"));
    let flat = tree(&["evals/*.py", "docs/*.md"])?;
    assert!(allowed(&flat, "evals/probe.py"));
    assert!(allowed(&flat, "docs/YI_DESIGN.md"));
    assert!(!allowed(&flat, "evals/sweep/probe.py"));
    assert!(!allowed(&flat, "evals/probe.rs"));
    Ok(())
}

/// Dies with the grammar of a scope silently matching nothing: an absolute glob, an empty one or
/// a broken one is refused when the run starts, not at its first write.
#[test]
fn a_scope_that_could_never_match_is_refused_up_front() {
    for bad in ["", "/work/evals/**", "evals/[", "{a,"] {
        assert!(write_scope(bad).is_err(), "{bad:?}");
    }
    assert!(write_scope("evals/**").is_ok());
}

/// Dies with a scope read as a writable directory (it would widen the sandbox), or `--allow-write`
/// read as a scope (it would confine a run that asked for no brief).
#[test]
fn a_scope_and_a_write_grant_do_not_pass_for_each_other() -> TestResult {
    let (scope, grant) = (
        write_scope("evals/**")?,
        write_grant("/work/evals".as_ref()),
    );
    let mut both = SessionRules::new();
    for kept in [&grant, &scope] {
        both.insert(kept.kind, &kept.canonical, &kept.label, RuleDecision::Allow)?;
    }
    let dirs: Vec<PathBuf> = (both.state().rules.iter())
        .filter_map(write_grant_dir)
        .collect();
    assert_eq!(dirs, [PathBuf::from("/work/evals")]);
    let mut granted_only = tree(&[])?;
    granted_only.rules.insert(
        grant.kind,
        &grant.canonical,
        &grant.label,
        RuleDecision::Allow,
    )?;
    let decision = granted_only.write(PermissionMode::Yolo, &["crates/runtime/src/lock.rs"]);
    assert!(refusal(&decision).is_none(), "{decision:?}");
    Ok(())
}

/// Dies with `cat > crates/... <<EOF` and `2>crates/err.log` writing where the brief forbids
/// because only the edit tools were judged; each spelling the engine reads is refused.
#[test]
fn a_redirect_into_the_tree_is_judged_like_a_write() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    for command in [
        "echo hi > crates/runtime/src/lock.rs",
        "cargo test >>crates/test.log",
        "make 2>crates/err.log",
        "make &>crates/all.log",
        "echo hi >| crates/x",
        "make >&crates/build.log",
        "echo hi >& crates/lock.rs",
        "cargo test &>> crates/lock.rs",
        "cat <<EOF > crates/x\nbody\nEOF",
        "# don't touch crates\necho hi > crates/x",
        "(cd crates && true) ; echo hi > \"crates/with space\"",
    ] {
        let reason = refusal(&scoped.bash(command))
            .ok_or_else(|| format!("{command:?} was allowed"))?
            .to_owned();
        assert!(
            reason.contains("crates/") && reason.contains("evals/**"),
            "{reason}"
        );
    }
    for command in [
        "echo hi > evals/out.txt",
        "cargo test 2>&1 | tail -5 > /dev/null",
        "echo hi >&2",
        "echo hi >& evals/out.txt",
        "echo hi >&-",
        "cat evals/probe.py",
        "echo \"a > crates/b\" ; python3 -c 'print(2 > 1)'",
        "echo $PATH > $OUT",
        "echo hi > evals/x # a > b, and don't",
    ] {
        let decision = scoped.bash(command);
        assert!(refusal(&decision).is_none(), "{command:?}: {decision:?}");
    }
    Ok(())
}

/// Dies with `cd` hiding a redirect from the scope, or a redirect in `evals` judged against the
/// old directory: static directory changes are resolved, and an unreadable one fails closed.
#[test]
fn a_redirect_after_cd_is_judged_from_the_directory_the_shell_uses() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    for (command, path) in [
        ("cd crates && echo x > lock.rs", "crates/lock.rs"),
        ("cd evals\necho hi > ../crates/lock.rs", "crates/lock.rs"),
        ("ls | cd /tmp; echo hi > crates/lock.rs", "crates/lock.rs"),
        ("cd \"$dir\" && echo hi > crates/lock.rs", "crates/lock.rs"),
    ] {
        let decision = scoped.bash(command);
        let reason = refusal(&decision).ok_or_else(|| format!("{command:?} was allowed"))?;
        assert!(
            reason.contains(path) && reason.contains("evals/**"),
            "{command:?}: {reason}"
        );
    }
    for command in [
        "cd evals && python3 gen.py > probe.py",
        "cd evals && cd nested && echo hi > probe.py",
        "cd evals\necho hi > probe.py",
        "cd /tmp && echo hi > probe.py",
    ] {
        let decision = scoped.bash(command);
        assert!(refusal(&decision).is_none(), "{command:?}: {decision:?}");
    }
    let before = scoped.bash("echo hi > crates/before.rs && cd evals && echo hi > probe.py");
    let reason = refusal(&before).ok_or("the redirect before cd was missed")?;
    assert!(reason.contains("crates/before.rs"), "{reason}");
    Ok(())
}

/// Dies with the arrows and comparisons of a heredoc's code read as redirects into `u8`, `0`
/// and `=`: a python file written to `evals/` was refused for its own type hints. The body is
/// shaped like the heredocs a lane writes; the redirect after its terminator is still read.
#[test]
fn a_heredoc_body_names_no_redirect_but_the_lines_around_it_do() -> TestResult {
    let scoped = tree(&["evals/**"])?;
    let body = "def gate(n: int) -> bool:\n    \"\"\"\n    >>> gate(3)\n    True\n    \"\"\"\n    return n >= 2 and n > 1\n";
    let inside =
        format!("cat > evals/gate.py <<'EOF'\n{body}EOF\npython3 evals/gate.py > /dev/null");
    let decision = scoped.bash(&inside);
    assert!(refusal(&decision).is_none(), "{decision:?}");
    let outside = format!("cat > crates/gate.py <<'EOF'\n{body}EOF");
    assert!(refusal(&scoped.bash(&outside)).is_some());
    let after = format!("cat <<-EOF\n\t{body}\tEOF\necho done > crates/after.txt");
    let decision = scoped.bash(&after);
    let reason = refusal(&decision).ok_or("a redirect after a heredoc was missed")?;
    assert!(reason.contains("crates/after.txt"), "{reason}");
    Ok(())
}
