use std::fmt;

const HOOK_CAP: usize = 240;
const BODY_CAP: usize = 8 * 1024;
const NAME_CAP: usize = 80;
const NAME_WORDS: usize = 5;
pub(super) const PLACEHOLDER: &str = "the situation, then the rule, one line";
const KNOWN: [&str; 4] = ["name", "description", "type", "scope"];
const SUGGEST_KEY_CAP: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryName(String);

impl MemoryName {
    pub fn from_stem(stem: &str) -> Option<Self> {
        let safe = !stem.is_empty()
            && !stem.starts_with('.')
            && !stem.eq_ignore_ascii_case("memory")
            && stem
                .chars()
                .all(|c| !c.is_control() && !matches!(c, '/' | '\\'));
        safe.then(|| Self(stem.to_owned()))
    }

    pub fn slug(raw: &str) -> Result<Self, DocError> {
        let mut out = String::new();
        for c in raw.trim().chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            } else if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        }
        let mut slug: String = out.trim_matches('-').chars().take(NAME_CAP).collect();
        if out.trim_matches('-').len() > NAME_CAP
            && let Some(cut) = slug.rfind('-').filter(|cut| *cut > 0)
        {
            slug = slug.get(..cut).unwrap_or_default().to_owned();
        }
        Self::from_stem(&slug).ok_or_else(|| DocError::BadName(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn file(&self) -> String {
        format!("{}.md", self.0)
    }
}

impl fmt::Display for MemoryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

impl MemoryType {
    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Repo,
    Global,
}

impl Scope {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "repo" => Some(Self::Repo),
            "global" => Some(Self::Global),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Text(String),
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    pub line: usize,
    pub reason: String,
}

impl fmt::Display for Trouble {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "frontmatter line {}: {}", self.line, self.reason)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DocError {
    #[error("{0}")]
    Frontmatter(Trouble),
    #[error("no description: add `description:` or start the body with one line")]
    NoDescription,
    #[error("the description is the template's placeholder; write the situation and the rule")]
    Placeholder,
    #[error("description is {len} characters, the cap is {max}")]
    HookTooLong { len: usize, max: usize },
    #[error("body is {len} bytes, the cap is {max}")]
    BodyTooLong { len: usize, max: usize },
    #[error("no type: add `type:` (user, feedback, project, or reference)")]
    NoType,
    #[error("type is {0:?}; it must be user, feedback, project, or reference")]
    BadType(String),
    #[error("scope is {0:?}; it must be repo or global")]
    BadScope(String),
    #[error("name {0:?} is reserved or makes no file name; pick another kebab-case name")]
    BadName(String),
    #[error("the note carries a memory or yard fence; save the fact, not a recalled block")]
    LoopGuard,
    #[error("the note looks like it holds {0}; save where to find it, not the value")]
    Secret(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub name: MemoryName,
    pub hook: String,
    pub kind: Option<MemoryType>,
    pub body: String,
    pub front: Vec<(String, Field)>,
    pub trouble: Option<Trouble>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    pub name: MemoryName,
    pub hook: String,
    pub kind: MemoryType,
    pub body: String,
    pub extra: Vec<(String, Field)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub memory: Memory,
    pub scope: Scope,
    pub warnings: Vec<String>,
}

struct Split<'a> {
    block: Option<&'a str>,
    body: &'a str,
}

fn split(text: &str) -> Result<Split<'_>, Trouble> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok(Split {
            block: None,
            body: text,
        });
    };
    if first.trim() != "---" {
        return Ok(Split {
            block: None,
            body: text,
        });
    }
    let start = first.len();
    let mut offset = start;
    for line in lines {
        if line.trim_end() == "---" {
            return Ok(Split {
                block: text.get(start..offset),
                body: text.get(offset.saturating_add(line.len())..).unwrap_or(""),
            });
        }
        offset = offset.saturating_add(line.len());
    }
    Err(Trouble {
        line: 1,
        reason: "the block opened here never closes".to_owned(),
    })
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn unquote(raw: &str, line: usize) -> Result<String, Trouble> {
    let unclosed = || Trouble {
        line,
        reason: "unclosed quote".to_owned(),
    };
    if let Some(rest) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Ok(out),
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => return Err(unclosed()),
                },
                other => out.push(other),
            }
        }
        return Err(unclosed());
    }
    if let Some(rest) = raw.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\'' {
                out.push(c);
            } else if chars.peek() == Some(&'\'') {
                out.push('\'');
                chars.next();
            } else {
                return Ok(out);
            }
        }
        return Err(unclosed());
    }
    Ok(raw.trim().to_owned())
}

fn flow_list(raw: &str, line: usize) -> Result<Vec<String>, Trouble> {
    let Some(inner) = raw
        .trim_end()
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Err(Trouble {
            line,
            reason: "unclosed [".to_owned(),
        });
    };
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                current.push(c);
            }
            (Some(open), _) if c == open => {
                quote = None;
                current.push(c);
            }
            (None, ',') => {
                items.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if quote.is_some() {
        return Err(Trouble {
            line,
            reason: "unclosed quote".to_owned(),
        });
    }
    items.push(current);
    items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| unquote(item, line))
        .collect()
}

fn indented_run<'a>(lines: &[&'a str], from: usize) -> Vec<&'a str> {
    lines
        .iter()
        .skip(from)
        .take_while(|line| line.trim().is_empty() || line.starts_with(char::is_whitespace))
        .copied()
        .collect()
}

fn nested(run: &[&str], first_line: usize) -> Result<Option<Field>, Trouble> {
    let items: Vec<(usize, &str)> = run
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(at, line)| (first_line.saturating_add(at), line.trim()))
        .collect();
    let Some((_, head)) = items.first() else {
        return Ok(None);
    };
    if head.starts_with("- ") || *head == "-" {
        let mut list = Vec::new();
        for (line, item) in items {
            if let Some(value) = item.strip_prefix('-') {
                list.push(unquote(value.trim(), line)?);
            }
        }
        return Ok(Some(Field::List(list)));
    }
    let mut map = Vec::new();
    for (line, item) in items {
        if let Some((key, value)) = item.split_once(':')
            && valid_key(key.trim())
        {
            map.push((key.trim().to_owned(), unquote(value.trim(), line)?));
        }
    }
    Ok((!map.is_empty()).then_some(Field::Map(map)))
}

fn parse_yaml(block: &str) -> Result<Vec<(String, Field)>, Trouble> {
    let lines: Vec<&str> = block.lines().collect();
    let mut entries: Vec<(String, Field)> = Vec::new();
    let mut at = 0usize;
    while let Some(line) = lines.get(at) {
        let number = at.saturating_add(2);
        at = at.saturating_add(1);
        if line.trim().is_empty()
            || line.trim_start().starts_with('#')
            || line.starts_with(char::is_whitespace)
        {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !valid_key(key) {
            continue;
        }
        let rest = rest.trim();
        let field = match rest {
            "" => {
                let run = indented_run(&lines, at);
                at = at.saturating_add(run.len());
                nested(&run, number.saturating_add(1))?
            }
            ">" | "|" | ">-" | "|-" => {
                let run = indented_run(&lines, at);
                at = at.saturating_add(run.len());
                let parts: Vec<&str> = run.iter().map(|line| line.trim()).collect();
                let joiner = if rest.starts_with('>') { " " } else { "\n" };
                Some(Field::Text(parts.join(joiner).trim().to_owned()))
            }
            _ if rest.starts_with('[') => Some(Field::List(flow_list(rest, number)?)),
            _ => Some(Field::Text(unquote(rest, number)?)),
        };
        if let Some(field) = field {
            entries.retain(|(existing, _)| existing != key);
            entries.push((key.to_owned(), field));
        }
    }
    Ok(entries)
}

fn json_field(value: &serde_json::Value) -> Option<Field> {
    let text = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Array(items) => Some(Field::List(items.iter().map(text).collect())),
        serde_json::Value::Object(map) => Some(Field::Map(
            map.iter()
                .map(|(key, value)| (key.clone(), text(value)))
                .collect(),
        )),
        other => Some(Field::Text(text(other))),
    }
}

fn parse_json(block: &str) -> Result<Vec<(String, Field)>, Trouble> {
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(block).map_err(|error| Trouble {
            line: error.line().saturating_add(1),
            reason: error.to_string(),
        })?;
    Ok(map
        .iter()
        .filter_map(|(key, value)| json_field(value).map(|field| (key.clone(), field)))
        .collect())
}

fn is_json(block: &str) -> bool {
    block.trim_start().starts_with("{")
}

fn parse_block(block: &str) -> Result<Vec<(String, Field)>, Trouble> {
    if is_json(block) {
        parse_json(block)
    } else {
        parse_yaml(block)
    }
}

fn text_of<'a>(front: &'a [(String, Field)], key: &str) -> Option<&'a str> {
    front.iter().find_map(|(name, field)| match field {
        Field::Text(text) if name == key => Some(text.as_str()),
        _ => None,
    })
}

fn kind_of(front: &[(String, Field)]) -> Option<&str> {
    text_of(front, "type").or_else(|| {
        front.iter().find_map(|(name, field)| match field {
            Field::Map(map) if name == "metadata" => map
                .iter()
                .find(|(key, _)| key == "type")
                .map(|(_, value)| value.as_str()),
            _ => None,
        })
    })
}

fn first_line(body: &str) -> String {
    body.lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

pub fn read_note(name: MemoryName, text: &str) -> Note {
    let (front, body, trouble) = match split(text) {
        Ok(Split { block: None, body }) => (Vec::new(), body, None),
        Ok(Split {
            block: Some(block),
            body,
        }) => match parse_block(block) {
            Ok(front) => (front, body, None),
            Err(trouble) => (Vec::new(), body, Some(trouble)),
        },
        Err(trouble) => {
            let rest = text.split_once('\n').map_or("", |(_, rest)| rest);
            (Vec::new(), rest, Some(trouble))
        }
    };
    let body = body.trim().to_owned();
    let hook = text_of(&front, "description")
        .map(str::to_owned)
        .unwrap_or_else(|| first_line(&body));
    let hook: String = hook.chars().take(HOOK_CAP).collect();
    let kind = kind_of(&front).and_then(MemoryType::parse);
    Note {
        name,
        hook,
        kind,
        body,
        front,
        trouble,
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row.first().copied().unwrap_or(0);
        if let Some(first) = row.first_mut() {
            *first = i.saturating_add(1);
        }
        for (j, cb) in b.iter().enumerate() {
            let above = row.get(j.saturating_add(1)).copied().unwrap_or(usize::MAX);
            let left = row.get(j).copied().unwrap_or(usize::MAX);
            let cost = usize::from(ca != *cb);
            let next = diagonal
                .saturating_add(cost)
                .min(above.saturating_add(1))
                .min(left.saturating_add(1));
            diagonal = above;
            if let Some(cell) = row.get_mut(j.saturating_add(1)) {
                *cell = next;
            }
        }
    }
    row.last().copied().unwrap_or(0)
}

fn suggestion(key: &str) -> Option<&'static str> {
    if key.chars().count() > SUGGEST_KEY_CAP {
        return None;
    }
    KNOWN
        .iter()
        .map(|known| (edit_distance(key, known), *known))
        .filter(|(distance, _)| *distance <= 2)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, known)| known)
}

fn key_run(token: &str, prefix: &str, min: usize) -> bool {
    token.find(prefix).is_some_and(|at| {
        token
            .get(at.saturating_add(prefix.len())..)
            .unwrap_or("")
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
            .count()
            >= min
    })
}

fn secret_in(text: &str) -> Option<&'static str> {
    if text.contains("PRIVATE KEY-----") {
        return Some("a private key");
    }
    text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '='))
        .find_map(|token| {
            if key_run(token, "sk-", 16) {
                Some("an API key")
            } else if key_run(token, "ghp_", 20) || key_run(token, "github_pat_", 20) {
                Some("a GitHub token")
            } else if key_run(token, "AKIA", 16) {
                Some("an AWS key")
            } else {
                None
            }
        })
}

fn loops(text: &str) -> bool {
    text.contains("<<<yi-external") || text.to_ascii_lowercase().contains("<memory")
}

pub fn draft(markdown: &str, overlay: &[(String, String)]) -> Result<Draft, DocError> {
    if loops(markdown) || overlay.iter().any(|(_, value)| loops(value)) {
        return Err(DocError::LoopGuard);
    }
    if let Some(kind) = secret_in(markdown) {
        return Err(DocError::Secret(kind));
    }
    let parts = split(markdown).map_err(DocError::Frontmatter)?;
    let mut warnings = Vec::new();
    let mut front = match parts.block {
        Some(block) => {
            if is_json(block) {
                warnings.push("frontmatter was JSON; written as YAML".to_owned());
            }
            parse_block(block).map_err(DocError::Frontmatter)?
        }
        None => Vec::new(),
    };
    for (key, value) in overlay {
        front.retain(|(existing, _)| existing != key);
        front.push((key.clone(), Field::Text(value.clone())));
    }
    let body = parts.body.trim().to_owned();
    if body.len() > BODY_CAP {
        return Err(DocError::BodyTooLong {
            len: body.len(),
            max: BODY_CAP,
        });
    }
    let hook = text_of(&front, "description")
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|| first_line(&body));
    if hook.is_empty() {
        return Err(DocError::NoDescription);
    }
    if hook == PLACEHOLDER {
        return Err(DocError::Placeholder);
    }
    let len = hook.chars().count();
    if len > HOOK_CAP {
        return Err(DocError::HookTooLong { len, max: HOOK_CAP });
    }
    if hook.contains('\n') {
        return Err(DocError::NoDescription);
    }
    let kind = match text_of(&front, "type") {
        Some(text) => MemoryType::parse(text).ok_or_else(|| DocError::BadType(text.to_owned()))?,
        None => return Err(DocError::NoType),
    };
    let scope = match text_of(&front, "scope") {
        Some(text) => Scope::parse(text).ok_or_else(|| DocError::BadScope(text.to_owned()))?,
        None => Scope::Repo,
    };
    let name = match text_of(&front, "name") {
        Some(text) => {
            let name = MemoryName::slug(text)?;
            if name.as_str() != text.trim() {
                warnings.push(format!("name `{}` saved as `{name}`", text.trim()));
            }
            name
        }
        None => {
            let words: Vec<&str> = hook.split_whitespace().take(NAME_WORDS).collect();
            let name = MemoryName::slug(&words.join(" "))?;
            warnings.push(format!("no name given; saved as `{name}`"));
            name
        }
    };
    let extra: Vec<(String, Field)> = front
        .into_iter()
        .filter(|(key, _)| !KNOWN.contains(&key.as_str()))
        .collect();
    for (key, _) in &extra {
        if let Some(known) = suggestion(key) {
            warnings.push(format!("ignored key `{key}`; did you mean `{known}`"));
        }
    }
    Ok(Draft {
        memory: Memory {
            name,
            hook,
            kind,
            body,
            extra,
        },
        scope,
        warnings,
    })
}

impl Memory {
    pub fn carry(&mut self, old: &Note) {
        for (key, field) in &old.front {
            if !KNOWN.contains(&key.as_str()) && !self.extra.iter().any(|(own, _)| own == key) {
                self.extra.push((key.clone(), field.clone()));
            }
        }
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "---\nname: {}\ndescription: {}\ntype: {}\n",
            self.name,
            quoted(&self.hook),
            self.kind.as_str()
        );
        for (key, field) in &self.extra {
            match field {
                Field::Text(text) => out.push_str(&format!("{key}: {}\n", scalar(text))),
                Field::List(items) => {
                    let items: Vec<String> = items.iter().map(|item| list_item(item)).collect();
                    out.push_str(&format!("{key}: [{}]\n", items.join(", ")));
                }
                Field::Map(map) => {
                    out.push_str(&format!("{key}:\n"));
                    for (inner, value) in map {
                        out.push_str(&format!("  {inner}: {}\n", scalar(value)));
                    }
                }
            }
        }
        out.push_str("---\n");
        if !self.body.is_empty() {
            out.push('\n');
            out.push_str(&self.body);
            out.push('\n');
        }
        out
    }

    pub fn index_line(&self) -> String {
        format!("- [{}]({}) — {}", self.name, self.name.file(), self.hook)
    }
}

fn quoted(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn plain_safe(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    text.trim() == text
        && !"-?:,[]{}#&*!|>'\"%@`".contains(first)
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.contains(['\n', '\t', '\\'])
}

fn scalar(text: &str) -> String {
    if plain_safe(text) {
        text.to_owned()
    } else {
        quoted(text)
    }
}

fn list_item(text: &str) -> String {
    if plain_safe(text) && !text.contains([',', ']']) {
        text.to_owned()
    } else {
        quoted(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(stem: &str) -> MemoryName {
        MemoryName::from_stem(stem).unwrap()
    }

    const TEMPLATE: &str = "---\nname: buildhost-tmp-is-ram\ndescription: Buildhost /tmp is RAM; never scratch there\ntype: feedback\n---\nThe host's /tmp is a 48 GB tmpfs.\n\n**Why:** one probe, one dead VM.\n\n**How to apply:** scratch under ~/scratch.\n";

    #[test]
    fn the_template_drafts_and_renders_back_to_itself() {
        let draft = draft(TEMPLATE, &[]).unwrap();
        assert_eq!(draft.scope, Scope::Repo);
        assert!(draft.warnings.is_empty(), "{:?}", draft.warnings);
        let rendered = draft.memory.render();
        let again = read_note(name("buildhost-tmp-is-ram"), &rendered);
        assert_eq!(again.hook, "Buildhost /tmp is RAM; never scratch there");
        assert_eq!(again.kind, Some(MemoryType::Feedback));
        assert_eq!(again.trouble, None);
        assert_eq!(draft.memory.render(), {
            let redraft = super::draft(&rendered, &[]).unwrap();
            redraft.memory.render()
        });
    }

    #[test]
    fn claude_code_metadata_type_is_read() {
        let text = "---\nname: x\ndescription: \"a hook\"\nmetadata: \n  node_type: memory\n  type: feedback\n---\n\nbody\n";
        let note = read_note(name("x"), text);
        assert_eq!(note.kind, Some(MemoryType::Feedback));
        assert_eq!(note.hook, "a hook");
        assert_eq!(note.trouble, None);
    }

    #[test]
    fn a_broken_block_still_yields_a_hook_and_names_the_line() {
        let text = "---\nname: x\ndescription: \"unclosed\ntype: project\n---\n\nFirst line of the body.\n";
        let note = read_note(name("x"), text);
        assert_eq!(note.trouble.as_ref().map(|t| t.line), Some(3));
        assert_eq!(note.hook, "First line of the body.");
    }

    #[test]
    fn no_frontmatter_takes_the_first_line() {
        let note = read_note(name("x"), "The forge CLI is fgj.\n\nmore\n");
        assert_eq!(note.hook, "The forge CLI is fgj.");
        assert_eq!(note.kind, None);
    }

    #[test]
    fn json_frontmatter_is_accepted_and_warned() {
        let text =
            "---\n{\"name\": \"j\", \"description\": \"a hook\", \"type\": \"user\"}\n---\nbody\n";
        let draft = draft(text, &[]).unwrap();
        assert_eq!(draft.memory.kind, MemoryType::User);
        assert_eq!(
            draft.warnings,
            vec!["frontmatter was JSON; written as YAML"]
        );
        assert!(draft.memory.render().starts_with("---\nname: j\n"));
    }

    #[test]
    fn a_near_miss_key_is_kept_and_warned() {
        let text = TEMPLATE.replace("type: feedback\n", "type: feedback\nnames: typo\n");
        let draft = draft(&text, &[]).unwrap();
        assert_eq!(
            draft.warnings,
            vec!["ignored key `names`; did you mean `name`"]
        );
        let nearest = TEMPLATE.replace("type: feedback\n", "type: feedback\nscpe: repo\n");
        assert_eq!(
            super::draft(&nearest, &[]).unwrap().warnings,
            vec!["ignored key `scpe`; did you mean `scope`"]
        );
        assert!(draft.memory.render().contains("names: typo\n"));
    }

    #[test]
    fn the_boundary_refuses_with_the_field_and_the_bound() {
        let long = TEMPLATE.replace(
            "Buildhost /tmp is RAM; never scratch there",
            &"x".repeat(HOOK_CAP.saturating_add(1)),
        );
        assert_eq!(
            draft(&long, &[]),
            Err(DocError::HookTooLong {
                len: HOOK_CAP.saturating_add(1),
                max: HOOK_CAP
            })
        );
        let gotcha = TEMPLATE.replace("type: feedback", "type: gotcha");
        assert_eq!(
            draft(&gotcha, &[]),
            Err(DocError::BadType("gotcha".to_owned()))
        );
        let unedited = TEMPLATE.replace("Buildhost /tmp is RAM; never scratch there", PLACEHOLDER);
        assert_eq!(draft(&unedited, &[]), Err(DocError::Placeholder));
        let untyped = TEMPLATE.replace("type: feedback\n", "");
        assert_eq!(draft(&untyped, &[]), Err(DocError::NoType));
        let unclosed = TEMPLATE.replace("description: Buildhost", "description: \"Buildhost");
        assert_eq!(
            draft(&unclosed, &[]).map(|_| ()),
            Err(DocError::Frontmatter(Trouble {
                line: 3,
                reason: "unclosed quote".to_owned()
            }))
        );
        assert_eq!(
            draft("---\nname: x\n", &[]).map(|_| ()),
            Err(DocError::Frontmatter(Trouble {
                line: 1,
                reason: "the block opened here never closes".to_owned()
            }))
        );
    }

    #[test]
    fn an_overlay_types_a_bare_body() {
        let draft = draft(
            "The forge CLI is fgj.\n",
            &[("type".to_owned(), "user".to_owned())],
        )
        .unwrap();
        assert_eq!(draft.memory.name.as_str(), "the-forge-cli-is-fgj");
        assert_eq!(draft.memory.hook, "The forge CLI is fgj.");
        assert_eq!(
            draft.warnings,
            vec!["no name given; saved as `the-forge-cli-is-fgj`"]
        );
        let long = super::draft(
            "The forge CLI is fgj; pass -R apex/yi from any worktree.",
            &[("type".to_owned(), "reference".to_owned())],
        )
        .unwrap();
        assert_eq!(long.memory.name.as_str(), "the-forge-cli-is-fgj");
        let traversal = TEMPLATE.replace("name: buildhost-tmp-is-ram", "name: ../../etc/passwd");
        assert_eq!(
            super::draft(&traversal, &[]).unwrap().warnings,
            vec!["name `../../etc/passwd` saved as `etc-passwd`"]
        );
    }

    #[test]
    fn fences_and_secrets_are_refused() {
        let fenced = TEMPLATE.replace("48 GB", "<memory name=\"x\">");
        assert_eq!(draft(&fenced, &[]), Err(DocError::LoopGuard));
        let keyed = TEMPLATE.replace("48 GB", "sk-ant-api03-abcdefghijklmnop");
        assert_eq!(draft(&keyed, &[]), Err(DocError::Secret("an API key")));
        let task = TEMPLATE.replace("48 GB", "task-oriented sk-short");
        assert!(draft(&task, &[]).is_ok());
    }

    #[test]
    fn an_update_carries_unknown_keys_from_the_old_file() {
        let old = read_note(
            name("buildhost-tmp-is-ram"),
            "---\nname: buildhost-tmp-is-ram\ndescription: old\nmetadata:\n  node_type: memory\n---\nold\n",
        );
        let mut memory = draft(TEMPLATE, &[]).unwrap().memory;
        memory.carry(&old);
        let rendered = memory.render();
        assert!(
            rendered.contains("metadata:\n  node_type: memory\n"),
            "{rendered}"
        );
        assert!(rendered.contains("description: \"Buildhost /tmp is RAM; never scratch there\""));
    }

    #[test]
    fn names_are_one_path_component() {
        assert_eq!(
            MemoryName::slug("Buildhost /tmp is RAM").unwrap().as_str(),
            "buildhost-tmp-is-ram"
        );
        assert!(MemoryName::slug("MEMORY").is_err());
        assert!(MemoryName::slug("../..").is_err());
        assert!(MemoryName::from_stem("../x").is_none());
        assert!(MemoryName::from_stem(".lock").is_none());
    }

    #[test]
    fn lists_round_trip_through_flow_form() {
        let text = TEMPLATE.replace(
            "type: feedback\n",
            "type: feedback\nsee:\n  - \"a, b\"\n  - c\n",
        );
        let draft = draft(&text, &[]).unwrap();
        let rendered = draft.memory.render();
        assert!(rendered.contains("see: [\"a, b\", c]\n"), "{rendered}");
        let again = read_note(name("buildhost-tmp-is-ram"), &rendered);
        assert!(again.front.contains(&(
            "see".to_owned(),
            Field::List(vec!["a, b".to_owned(), "c".to_owned()])
        )));
    }
}
