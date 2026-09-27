//! The picker's logo masks (`data/logos/*.a8`, 32x32 coverage each) ship as one deflated blob
//! plus the key table in the same sorted order, so index and name never drift apart.
//! The highlighter's grammars ship as bat's set cut down to `GRAMMARS` and whatever those embed.
use std::collections::{BTreeSet, HashMap};
use std::io::Write;

use serde_json::Value;
use syntect::parsing::{SyntaxDefinition, SyntaxSetBuilder};

/// The languages fences and edited files name, by bat's grammar name. The rest of bat's 213 were
/// 590 KB of the binary, Julia, Sass, Less, MATLAB, Lisp and CFML the largest of them.
const GRAMMARS: [&str; 44] = [
    "Plain Text",
    "Rust",
    "Go",
    "Java",
    "Kotlin",
    "Swift",
    "JavaScript",
    "TypeScript",
    "TypeScriptReact",
    "C",
    "C++",
    "C#",
    "Python",
    "Scala",
    "Groovy",
    "Objective-C",
    "Ruby",
    "PHP",
    "Bourne Again Shell (bash)",
    "JSON",
    "YAML",
    "TOML",
    "Markdown",
    "HTML",
    "CSS",
    "SQL",
    "Dockerfile",
    "Makefile",
    "Diff",
    "Lua",
    "Zig",
    "XML",
    "INI",
    "Protocol Buffer",
    "GraphQL",
    "Nix",
    "CMake",
    "Terraform",
    "DotENV",
    "Git Config",
    "Git Ignore",
    "Haskell",
    "Elixir",
    "Dart",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?);
    println!("cargo::rerun-if-changed=data/logos");
    let mut names: Vec<String> = std::fs::read_dir("data/logos")?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".a8").map(str::to_owned)
        })
        .collect();
    names.sort();
    let mut masks = Vec::new();
    for name in &names {
        masks.extend(std::fs::read(format!("data/logos/{name}.a8"))?);
    }
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&masks, 9);
    std::fs::File::create(out.join("logos.zz"))?.write_all(&packed)?;
    let quoted: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();
    let table = format!(
        "pub const KEYS: [&str; {}] = [{}];\n",
        names.len(),
        quoted.join(", ")
    );
    std::fs::File::create(out.join("logos.rs"))?.write_all(table.as_bytes())?;
    std::fs::File::create(out.join("grammars.packdump"))?.write_all(&grammars()?)?;
    Ok(())
}

/// A linked grammar points into its set by position (`ContextId`), so a cut set renumbers every
/// pointer. The pointers are private to syntect but serialize as `syntax_index`, which is how they
/// are found and rewritten here.
fn grammars() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let defs: Vec<SyntaxDefinition> = two_face::syntax::extra_newlines()
        .into_builder()
        .syntaxes()
        .to_vec();
    let mut queue = Vec::new();
    for name in GRAMMARS {
        let index = defs.iter().position(|def| def.name == name);
        queue.push(index.ok_or(format!("bat has no grammar named {name:?}"))?);
    }
    let mut kept: HashMap<usize, Value> = HashMap::new();
    while let Some(index) = queue.pop() {
        if kept.contains_key(&index) {
            continue;
        }
        let def = serde_json::to_value(defs.get(index).ok_or("pointer past the set")?)?;
        refs(&def, &mut |target| queue.push(target));
        kept.insert(index, def);
    }
    let order: BTreeSet<usize> = kept.keys().copied().collect();
    let renumber: HashMap<usize, usize> = order
        .iter()
        .enumerate()
        .map(|(new, old)| (*old, new))
        .collect();
    let mut builder = SyntaxSetBuilder::new();
    for old in &order {
        let mut def = kept.remove(old).ok_or("kept grammar vanished")?;
        rewrite(&mut def, &renumber);
        builder.add(serde_json::from_value(def)?);
    }
    Ok(syntect::dumps::dump_binary(&builder.build()))
}

fn refs(value: &Value, found: &mut impl FnMut(usize)) {
    match value {
        Value::Object(map) => {
            if let (Some(index), true) = (
                map.get("syntax_index").and_then(Value::as_u64),
                map.contains_key("context_index"),
            ) {
                found(usize::try_from(index).unwrap_or(usize::MAX));
            }
            map.values().for_each(|inner| refs(inner, found));
        }
        Value::Array(items) => items.iter().for_each(|inner| refs(inner, found)),
        _ => {}
    }
}

fn rewrite(value: &mut Value, renumber: &HashMap<usize, usize>) {
    match value {
        Value::Object(map) => {
            let old = map
                .get("syntax_index")
                .and_then(Value::as_u64)
                .and_then(|old| usize::try_from(old).ok());
            if let (Some(new), true) = (
                old.and_then(|old| renumber.get(&old)),
                map.contains_key("context_index"),
            ) {
                map.insert("syntax_index".to_owned(), Value::from(*new));
            }
            map.values_mut().for_each(|inner| rewrite(inner, renumber));
        }
        Value::Array(items) => items.iter_mut().for_each(|inner| rewrite(inner, renumber)),
        _ => {}
    }
}
