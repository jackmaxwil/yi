use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

struct Workspace {
    dir: Scratch,
    project: PathBuf,
}

impl Workspace {
    fn new(tag: &str, justfile: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-tool-import-{tag}"))?;
        dir.home()?;
        let project = dir.join("project");
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("justfile"), justfile)?;
        Ok(Self { dir, project })
    }

    fn justfile(&self, text: &str) -> Result<(), Box<dyn Error>> {
        Ok(std::fs::write(self.project.join("justfile"), text)?)
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    fn tools(&self) -> PathBuf {
        self.home().join(".yi/tools")
    }

    fn import(&self, words: &[&str]) -> Result<Output, Box<dyn Error>> {
        self.import_in(&self.project, words)
    }

    fn import_in(&self, project: &Path, words: &[&str]) -> Result<Output, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the spawned binary's argv, exit code and stdout are the contract"
        )]
        let mut command = Command::new(env!("CARGO_BIN_EXE_yi"));
        Ok(command
            .args(["tool", "import"])
            .args(words)
            .env("HOME", self.home())
            .current_dir(project)
            .output()?)
    }

    fn sibling(&self, name: &str, justfile: &str) -> Result<PathBuf, Box<dyn Error>> {
        let project = self.dir.join(name);
        std::fs::create_dir_all(&project)?;
        std::fs::write(project.join("justfile"), justfile)?;
        Ok(project)
    }

    fn tool(&self, name: &str, stdin: &str) -> Result<Output, Box<dyn Error>> {
        run(&self.tools().join(name), &[], stdin)
    }

    fn tool_with_path(
        &self,
        name: &str,
        path: &Path,
        stdin: &str,
    ) -> Result<Output, Box<dyn Error>> {
        let held = std::env::var("PATH").unwrap_or_default();
        let joined = format!("{}:{held}", path.display());
        run_env(&self.tools().join(name), &[], stdin, &[("PATH", &joined)])
    }

    fn schema(&self, name: &str) -> Result<Value, Box<dyn Error>> {
        let output = run(&self.tools().join(name), &["--schema"], "")?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.tools().join(name)
    }
}

fn run(program: &Path, args: &[&str], stdin: &str) -> Result<Output, Box<dyn Error>> {
    run_env(program, args, stdin, &[])
}

fn run_env(
    program: &Path,
    args: &[&str],
    stdin: &str,
    env: &[(&str, &str)],
) -> Result<Output, Box<dyn Error>> {
    use std::io::Write;

    #[expect(
        clippy::disallowed_methods,
        reason = "the wrapper's argv, exit code and stdout are the contract under test"
    )]
    let mut child = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes())?;
    }
    Ok(child.wait_with_output()?)
}

const FIXTURE: &str = "\
# Build the project
build target=\"release\":
    echo build {{target}}

# Run the tests
test filter *args:
    echo test {{filter}} {{args}}

# Build everything
all +targets:
    echo all {{targets}}
";

#[test]
fn the_wrapper_schema_carries_each_recipe_parameter() -> TestResult {
    let space = Workspace::new("schema", FIXTURE)?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");

    let build = space.schema("build")?;
    assert_eq!(build["name"], "build");
    assert_eq!(build["description"], "Build the project");
    assert_eq!(build["kind"], "exec");
    assert_eq!(
        build["input_schema"]["properties"]["target"]["default"],
        "release"
    );
    assert_eq!(build["input_schema"].get("required"), None);

    let test = space.schema("test")?;
    assert_eq!(
        test["input_schema"]["properties"]["filter"]["type"],
        "string"
    );
    assert_eq!(
        test["input_schema"]["properties"]["args"],
        serde_json::json!({"type": "array", "items": {"type": "string"}})
    );
    assert_eq!(
        test["input_schema"]["required"],
        serde_json::json!(["filter"])
    );

    let all = space.schema("all")?;
    assert_eq!(
        all["input_schema"]["required"],
        serde_json::json!(["targets"])
    );
    Ok(())
}

#[test]
fn a_spaced_argument_reaches_a_real_recipe_whole() -> TestResult {
    let space = Workspace::new(
        "spaced",
        "\
# Print the value
show value:
    printf '%s\\n' \"{{value}}\"
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    let called = space.tool("show", r#"{"value":"my dir"}"#)?;
    assert!(called.status.success(), "{called:?}");
    assert_eq!(String::from_utf8(called.stdout)?, "my dir\n");
    Ok(())
}

/// The wrapper hands just an argv array: a value holding a space and a quote arrives as one
/// element, which a shell-string call would have re-split or refused.
#[test]
fn a_space_and_a_quote_survive_as_one_argv_element() -> TestResult {
    let space = Workspace::new(
        "argv",
        "\
# Print the value
show value:
    echo noop
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    let justfile = marker_of(&std::fs::read_to_string(space.path("show"))?)?;

    let fake = space.dir.join("fake-just");
    std::fs::create_dir_all(&fake)?;
    let recorder = fake.join("just");
    std::fs::write(
        &recorder,
        "#!/bin/sh\nfor arg in \"$@\"; do printf '[%s]\\n' \"$arg\"; done\n",
    )?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o755))?;

    let called = space.tool_with_path("show", &fake, r#"{"value":"my dir/it's here"}"#)?;
    assert!(called.status.success(), "{called:?}");
    assert_eq!(
        String::from_utf8(called.stdout)?,
        format!("[--justfile]\n[{justfile}]\n[show]\n[my dir/it's here]\n")
    );
    Ok(())
}

fn marker_of(script: &str) -> Result<String, Box<dyn Error>> {
    let line = script
        .lines()
        .find(|line| line.starts_with("# yi: {"))
        .ok_or("the wrapper carries no marker")?;
    let value: Value = serde_json::from_str(line.trim_start_matches("# yi: "))?;
    Ok(value["justfile"]
        .as_str()
        .ok_or("the marker names no justfile")?
        .to_owned())
}

#[test]
fn a_recipe_named_like_a_builtin_tool_is_refused_by_name() -> TestResult {
    let space = Workspace::new(
        "collision",
        "\
# Read a file
read path:
    echo {{path}}

build:
    echo built
",
    )?;
    let imported = space.import(&["just"])?;
    assert_eq!(imported.status.code(), Some(2), "{imported:?}");
    let complaint = String::from_utf8(imported.stderr)?;
    assert!(
        complaint.contains("read is a built-in tool"),
        "the refusal must name the collision: {complaint}"
    );
    assert!(!space.path("read").exists(), "nothing was written for it");
    assert!(
        !space.path("build").exists(),
        "a refused import writes no wrapper at all"
    );
    Ok(())
}

#[test]
fn a_second_import_changes_no_wrapper() -> TestResult {
    let space = Workspace::new("idempotent", FIXTURE)?;
    let first = space.import(&["just"])?;
    assert!(first.status.success(), "{first:?}");
    let before = std::fs::read(space.path("build"))?;

    let second = space.import(&["just"])?;
    assert!(second.status.success(), "{second:?}");
    let said = String::from_utf8(second.stdout)?;
    assert!(
        said.contains("unchanged 3: all, build, test"),
        "a rerun reports what it left alone: {said}"
    );
    assert_eq!(std::fs::read(space.path("build"))?, before);
    Ok(())
}

#[test]
fn a_recipe_gone_from_the_justfile_takes_its_wrapper_and_nothing_else() -> TestResult {
    let space = Workspace::new("removal", FIXTURE)?;
    let first = space.import(&["just"])?;
    assert!(first.status.success(), "{first:?}");
    std::fs::write(space.path("notes"), "#!/bin/sh\necho hand-written\n")?;
    space.justfile(
        "\
# Build the project
build target=\"release\":
    echo build {{target}}
",
    )?;

    let second = space.import(&["just"])?;
    assert!(second.status.success(), "{second:?}");
    let said = String::from_utf8(second.stdout)?;
    assert!(said.contains("removed 2: all, test"), "{said}");
    assert!(!space.path("test").exists());
    assert!(space.path("build").exists());
    assert!(
        space.path("notes").exists(),
        "a tool yi did not write is never pruned"
    );
    Ok(())
}

#[test]
fn a_hand_written_tool_of_the_same_name_is_not_replaced() -> TestResult {
    let space = Workspace::new("hand-written", FIXTURE)?;
    std::fs::create_dir_all(space.tools())?;
    std::fs::write(space.path("build"), "#!/bin/sh\necho mine\n")?;

    let refused = space.import(&["just"])?;
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    let complaint = String::from_utf8(refused.stderr)?;
    assert!(complaint.contains("hand-written tool"), "{complaint}");
    assert_eq!(
        std::fs::read_to_string(space.path("build"))?,
        "#!/bin/sh\necho mine\n"
    );
    Ok(())
}

#[test]
fn a_recipe_marked_read_is_a_read_tool() -> TestResult {
    let space = Workspace::new(
        "read",
        "\
# yi: read
show:
    echo show

# yi: skip
internal:
    echo internal

status:
    echo status
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    assert_eq!(space.schema("show")?["kind"], "read");
    assert_eq!(
        space.schema("status")?["kind"],
        "exec",
        "a recipe's name never decides its permission kind"
    );
    assert!(
        !space.path("internal").exists(),
        "a skipped recipe is not imported"
    );
    Ok(())
}

#[test]
fn an_omitted_default_leaves_a_later_value_in_its_own_parameter() -> TestResult {
    let space = Workspace::new(
        "shift",
        "\
# Deploy it
deploy env=\"prod\" region=\"us\":
    printf '%s %s\\n' \"{{env}}\" \"{{region}}\"
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    let called = space.tool("deploy", r#"{"region":"eu"}"#)?;
    assert!(called.status.success(), "{called:?}");
    assert_eq!(
        String::from_utf8(called.stdout)?,
        "prod eu\n",
        "an omitted default keeps the supplied value in its own parameter"
    );
    Ok(())
}

#[test]
fn a_call_that_omits_a_required_parameter_is_refused() -> TestResult {
    let space = Workspace::new(
        "required",
        "\
# Print the value
show value:
    printf '%s\\n' \"{{value}}\"
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    let called = space.tool("show", "{}")?;
    assert!(!called.status.success(), "{called:?}");
    let complaint = String::from_utf8(called.stderr)?;
    assert!(
        complaint.contains("show needs value"),
        "an omitted required parameter is named, not shifted: {complaint}"
    );
    Ok(())
}

#[test]
fn a_variadic_call_reaches_the_recipe_as_separate_arguments() -> TestResult {
    let space = Workspace::new(
        "variadic",
        "\
# Build everything
all +targets:
    printf '<%s>\\n' {{targets}}
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    let called = space.tool("all", r#"{"targets":["a","b c"]}"#)?;
    assert!(called.status.success(), "{called:?}");
    assert_eq!(
        String::from_utf8(called.stdout)?,
        "<a>\n<b>\n<c>\n",
        "a variadic list spreads into one argv element per item"
    );
    Ok(())
}

#[test]
fn a_recipe_with_no_parameters_needs_no_interpreter() -> TestResult {
    let space = Workspace::new(
        "no-args",
        "\
# Build the project
build:
    echo built
",
    )?;
    let imported = space.import(&["just"])?;
    assert!(imported.status.success(), "{imported:?}");
    let body = std::fs::read_to_string(space.path("build"))?;
    assert!(
        !body.contains("python3"),
        "a wrapper with nothing to map execs just itself: {body}"
    );
    let called = space.tool("build", "")?;
    assert!(called.status.success(), "{called:?}");
    assert_eq!(String::from_utf8(called.stdout)?, "built\n");
    Ok(())
}

#[test]
fn another_project_s_recipe_is_left_out_instead_of_written_over() -> TestResult {
    let space = Workspace::new("cross-project", "build:\n    echo first\n")?;
    let first = space.import(&["just"])?;
    assert!(first.status.success(), "{first:?}");
    let mine = std::fs::read(space.path("build"))?;
    let other = space.sibling("other", "build:\n    echo second\npack:\n    echo packed\n")?;

    let second = space.import_in(&other, &["just"])?;
    assert!(second.status.success(), "{second:?}");
    let said = String::from_utf8(second.stdout)?;
    assert!(said.contains("conflict 1: build (from"), "{said}");
    assert!(said.contains("added 1: pack"), "{said}");
    assert_eq!(
        std::fs::read(space.path("build"))?,
        mine,
        "the first project's wrapper of that name is untouched"
    );

    let picked = space.import_in(&other, &["just", "build"])?;
    assert_eq!(picked.status.code(), Some(2), "{picked:?}");
    let complaint = String::from_utf8(picked.stderr)?;
    assert!(
        complaint.contains("another project's wrapper"),
        "naming it must refuse rather than write it: {complaint}"
    );
    Ok(())
}

#[test]
fn a_prune_keeps_the_wrappers_another_project_wrote() -> TestResult {
    let space = Workspace::new(
        "cross-prune",
        "build:\n    echo build\ntest:\n    echo test\n",
    )?;
    let first = space.import(&["just"])?;
    assert!(first.status.success(), "{first:?}");
    let other = space.sibling("other", "pack:\n    echo packed\n")?;
    let second = space.import_in(&other, &["just"])?;
    assert!(second.status.success(), "{second:?}");

    space.justfile("build:\n    echo build\n")?;
    let third = space.import(&["just"])?;
    assert!(third.status.success(), "{third:?}");
    let said = String::from_utf8(third.stdout)?;
    assert!(said.contains("removed 1: test"), "{said}");
    assert!(
        !said.contains("removed 2"),
        "the other project's wrapper is not pruned: {said}"
    );
    assert!(space.path("pack").exists());
    assert!(space.path("build").exists());
    Ok(())
}

#[test]
fn a_tool_that_is_not_text_is_not_replaced() -> TestResult {
    let space = Workspace::new("binary", "build:\n    echo build\n")?;
    std::fs::create_dir_all(space.tools())?;
    let compiled = [0xff_u8, 0xfe, b'\n'];
    std::fs::write(space.path("build"), compiled)?;

    let refused = space.import(&["just"])?;
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    let complaint = String::from_utf8(refused.stderr)?;
    assert!(complaint.contains("not a wrapper yi wrote"), "{complaint}");
    assert_eq!(
        std::fs::read(space.path("build"))?,
        compiled,
        "an unreadable-as-text tool is neither overwritten nor deleted"
    );
    Ok(())
}

#[test]
fn naming_recipes_imports_only_those_and_skips_the_prune() -> TestResult {
    let space = Workspace::new("picked", FIXTURE)?;
    let first = space.import(&["just"])?;
    assert!(first.status.success(), "{first:?}");

    let second = space.import(&["just", "build"])?;
    assert!(second.status.success(), "{second:?}");
    let said = String::from_utf8(second.stdout)?;
    assert!(said.contains("imported 1 just recipes"), "{said}");
    assert!(said.contains("unchanged 1: build"), "{said}");
    assert!(
        space.path("test").exists() && space.path("all").exists(),
        "a named import prunes nothing: {said}"
    );

    let unknown = space.import(&["just", "nope"])?;
    assert_eq!(unknown.status.code(), Some(2), "{unknown:?}");
    let complaint = String::from_utf8(unknown.stderr)?;
    assert!(
        complaint.contains("the justfile has no recipe named nope"),
        "{complaint}"
    );

    let skipped = Workspace::new("picked-skip", "# yi: skip\nhidden:\n    echo hidden\n")?;
    let refused = skipped.import(&["just", "hidden"])?;
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    let complaint = String::from_utf8(refused.stderr)?;
    assert!(complaint.contains("marked `yi: skip`"), "{complaint}");
    Ok(())
}
