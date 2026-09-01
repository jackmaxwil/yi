//! Invariant: this is a strict subset codec, never a YAML implementation —
//! everything outside block maps, block sequences, flow collections, plain and
//! double-quoted scalars is a typed refusal, so a caller may not assume more.

use serde_json::{Map, Number, Value};

/// Invariant: a plan file is user-editable input from outside the process, so
/// both parsers bound their own recursion instead of trusting the writer; 32 is
/// far past any real frontmatter and far short of the stack.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    Anchor,
    Alias,
    Tag,
    BlockScalar,
    SingleQuoted,
    Tab,
    DocumentDelimiter,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum YamlError {
    #[error("{kind:?} at line {line}: {text}")]
    Unsupported {
        kind: Unsupported,
        line: usize,
        text: String,
    },
    #[error("unterminated double quote at line {line}: {text}")]
    UnterminatedQuote { line: usize, text: String },
    #[error("escape outside \\\" \\\\ \\n at line {line}: {text}")]
    Escape { line: usize, text: String },
    #[error("malformed flow collection at line {line}: {text}")]
    Flow { line: usize, text: String },
    #[error("not a usable mapping key at line {line}: {text}")]
    Key { line: usize, text: String },
    #[error("duplicate key at line {line}: {text}")]
    DuplicateKey { line: usize, text: String },
    #[error("indent at line {line} is neither the block's nor the block's plus two: {text}")]
    Indent { line: usize, text: String },
    #[error("unparsed text at line {line}: {text}")]
    Trailing { line: usize, text: String },
    #[error("nesting past {limit} at line {line}")]
    TooDeep { line: usize, limit: usize },
    #[error("no opening --- delimiter: {head}")]
    NoFrontmatter { head: String },
    #[error("frontmatter opened at line 1 is unclosed after {lines} lines")]
    OpenFrontmatter { lines: usize },
    #[error("not an integer: {value}")]
    NotAnInteger { value: f64 },
    #[error("no subset escape encodes this scalar: {text}")]
    Unencodable { text: String },
    #[error("nesting past {limit} while emitting")]
    EmitTooDeep { limit: usize },
}

pub fn split_frontmatter(document: &str) -> Result<(&str, &str), YamlError> {
    let mut offset = 0usize;
    let mut opened = false;
    let mut start = 0usize;
    let mut count = 0usize;
    for raw in document.split_inclusive('\n') {
        count = count.saturating_add(1);
        let closing = raw.trim_end() == "---";
        if !opened {
            if !closing {
                return Err(YamlError::NoFrontmatter {
                    head: raw.trim_end().to_string(),
                });
            }
            opened = true;
            start = offset.saturating_add(raw.len());
        } else if closing {
            let body = offset.saturating_add(raw.len());
            return Ok((
                document.get(start..offset).unwrap_or(""),
                document.get(body..).unwrap_or(""),
            ));
        }
        offset = offset.saturating_add(raw.len());
    }
    if opened {
        Err(YamlError::OpenFrontmatter { lines: count })
    } else {
        Err(YamlError::NoFrontmatter {
            head: String::new(),
        })
    }
}

pub fn from_yaml(text: &str) -> Result<Value, YamlError> {
    let mut lines = scan(text)?;
    let Some(first) = lines.first().copied() else {
        return Ok(Value::Null);
    };
    if first.indent != 0 {
        return Err(YamlError::Indent {
            line: first.no,
            text: first.text.to_string(),
        });
    }
    let mut at = 0usize;
    let value = parse_block(&mut lines, &mut at, 0, 0)?;
    match lines.get(at) {
        Some(line) => Err(YamlError::Trailing {
            line: line.no,
            text: line.text.to_string(),
        }),
        None => Ok(value),
    }
}

pub fn to_yaml(value: &Value) -> Result<String, YamlError> {
    let mut out = String::new();
    emit(value, 0, false, &mut out, 0)?;
    Ok(out)
}

#[derive(Clone, Copy)]
struct Line<'a> {
    no: usize,
    indent: usize,
    text: &'a str,
}

fn scan(text: &str) -> Result<Vec<Line<'_>>, YamlError> {
    let mut lines = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let no = index.saturating_add(1);
        let rest = raw.trim_start_matches(' ');
        if rest.starts_with('\t') {
            return Err(YamlError::Unsupported {
                kind: Unsupported::Tab,
                line: no,
                text: raw.to_string(),
            });
        }
        let content = strip_comment(rest, no)?.trim_end();
        if content.is_empty() {
            continue;
        }
        if content == "---" || content == "..." {
            return Err(YamlError::Unsupported {
                kind: Unsupported::DocumentDelimiter,
                line: no,
                text: content.to_string(),
            });
        }
        lines.push(Line {
            no,
            indent: raw.len().saturating_sub(rest.len()),
            text: content,
        });
    }
    Ok(lines)
}

fn strip_comment(text: &str, line: usize) -> Result<&str, YamlError> {
    let bytes = text.as_bytes();
    let mut quoted = false;
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' if quoted => index = index.saturating_add(1),
            b'"' => quoted = !quoted,
            b'#' if !quoted
                && (index == 0 || bytes.get(index.saturating_sub(1)) == Some(&b' ')) =>
            {
                return Ok(text.get(..index).unwrap_or(text));
            }
            _ => {}
        }
        index = index.saturating_add(1);
    }
    if quoted {
        return Err(YamlError::UnterminatedQuote {
            line,
            text: text.to_string(),
        });
    }
    Ok(text)
}

fn is_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

fn split_key(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut quoted = false;
    let mut nest = 0usize;
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' if quoted => index = index.saturating_add(1),
            b'"' => quoted = !quoted,
            b'[' | b'{' if !quoted => nest = nest.saturating_add(1),
            b']' | b'}' if !quoted => nest = nest.saturating_sub(1),
            b':' if !quoted && nest == 0 => {
                let after = index.saturating_add(1);
                match bytes.get(after) {
                    None => return Some((text.get(..index)?, "")),
                    Some(b' ') => {
                        return Some((text.get(..index)?, text.get(after..)?.trim_start()));
                    }
                    Some(_) => {}
                }
            }
            _ => {}
        }
        index = index.saturating_add(1);
    }
    None
}

fn parse_block(
    lines: &mut [Line<'_>],
    at: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Value, YamlError> {
    let Some(line) = lines.get(*at).copied() else {
        return Ok(Value::Null);
    };
    if depth > MAX_DEPTH {
        return Err(YamlError::TooDeep {
            line: line.no,
            limit: MAX_DEPTH,
        });
    }
    if is_item(line.text) {
        parse_sequence(lines, at, indent, depth)
    } else if split_key(line.text).is_some() {
        parse_mapping(lines, at, indent, depth)
    } else {
        *at = at.saturating_add(1);
        parse_scalar(line.text, line.no, depth)
    }
}

fn parse_mapping(
    lines: &mut [Line<'_>],
    at: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Value, YamlError> {
    let mut map = Map::new();
    while let Some(line) = lines.get(*at).copied() {
        if line.indent != indent || is_item(line.text) {
            break;
        }
        let (raw, rest) = split_key(line.text).ok_or_else(|| YamlError::Key {
            line: line.no,
            text: line.text.to_string(),
        })?;
        let key = parse_key(raw.trim_end(), line.no)?;
        *at = at.saturating_add(1);
        let value = if rest.is_empty() {
            parse_nested(lines, at, indent, depth)?
        } else {
            parse_scalar(rest, line.no, depth)?
        };
        if map.contains_key(&key) {
            return Err(YamlError::DuplicateKey {
                line: line.no,
                text: key,
            });
        }
        map.insert(key, value);
    }
    Ok(Value::Object(map))
}

fn parse_nested(
    lines: &mut [Line<'_>],
    at: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Value, YamlError> {
    let child = indent.saturating_add(2);
    let deeper = depth.saturating_add(1);
    match lines.get(*at).copied() {
        Some(next) if next.indent == child => parse_block(lines, at, child, deeper),
        Some(next) if next.indent == indent && is_item(next.text) => {
            parse_sequence(lines, at, indent, deeper)
        }
        Some(next) if next.indent > indent => Err(YamlError::Indent {
            line: next.no,
            text: next.text.to_string(),
        }),
        _ => Ok(Value::Null),
    }
}

fn parse_sequence(
    lines: &mut [Line<'_>],
    at: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Value, YamlError> {
    let child = indent.saturating_add(2);
    let deeper = depth.saturating_add(1);
    let mut items = Vec::new();
    while let Some(line) = lines.get(*at).copied() {
        if line.indent != indent || !is_item(line.text) {
            break;
        }
        match line.text.get(2..) {
            Some(rest) => {
                if let Some(slot) = lines.get_mut(*at) {
                    slot.indent = child;
                    slot.text = rest;
                }
                items.push(parse_block(lines, at, child, deeper)?);
            }
            None => {
                *at = at.saturating_add(1);
                match lines.get(*at).copied() {
                    Some(next) if next.indent == child => {
                        items.push(parse_block(lines, at, child, deeper)?);
                    }
                    Some(next) if next.indent > indent => {
                        return Err(YamlError::Indent {
                            line: next.no,
                            text: next.text.to_string(),
                        });
                    }
                    _ => items.push(Value::Null),
                }
            }
        }
    }
    Ok(Value::Array(items))
}

fn parse_key(text: &str, line: usize) -> Result<String, YamlError> {
    match parse_scalar(text, line, 0)? {
        Value::String(key) => Ok(key),
        _ => Err(YamlError::Key {
            line,
            text: text.to_string(),
        }),
    }
}

fn refuse(text: &str, line: usize) -> Result<(), YamlError> {
    let kind = match text.as_bytes().first() {
        Some(b'&') => Unsupported::Anchor,
        Some(b'*') => Unsupported::Alias,
        Some(b'!') => Unsupported::Tag,
        Some(b'\'') => Unsupported::SingleQuoted,
        Some(b'|' | b'>') => Unsupported::BlockScalar,
        _ => return Ok(()),
    };
    Err(YamlError::Unsupported {
        kind,
        line,
        text: text.to_string(),
    })
}

fn parse_scalar(text: &str, line: usize, depth: usize) -> Result<Value, YamlError> {
    if text.is_empty() {
        return Ok(Value::Null);
    }
    refuse(text, line)?;
    match text.as_bytes().first() {
        Some(b'"' | b'[' | b'{') => {
            let (value, used) = parse_flow(text, 0, line, depth)?;
            let tail = text.get(used..).unwrap_or("").trim();
            if tail.is_empty() {
                Ok(value)
            } else {
                Err(YamlError::Trailing {
                    line,
                    text: tail.to_string(),
                })
            }
        }
        _ => Ok(plain(text)),
    }
}

fn plain(text: &str) -> Value {
    match text {
        "null" | "~" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => integer(text).map_or_else(|| Value::String(text.to_string()), Value::Number),
    }
}

fn integer(text: &str) -> Option<Number> {
    if let Ok(signed) = text.parse::<i64>()
        && signed.to_string() == text
    {
        return Some(Number::from(signed));
    }
    if let Ok(unsigned) = text.parse::<u64>()
        && unsigned.to_string() == text
    {
        return Some(Number::from(unsigned));
    }
    None
}

fn parse_quoted(text: &str, line: usize) -> Result<(String, usize), YamlError> {
    let mut out = String::new();
    let mut chars = text.char_indices();
    chars.next();
    while let Some((offset, ch)) = chars.next() {
        match ch {
            '"' => return Ok((out, offset.saturating_add(1))),
            '\\' => match chars.next() {
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, 'n')) => out.push('\n'),
                _ => {
                    return Err(YamlError::Escape {
                        line,
                        text: text.to_string(),
                    });
                }
            },
            _ => out.push(ch),
        }
    }
    Err(YamlError::UnterminatedQuote {
        line,
        text: text.to_string(),
    })
}

fn skip_spaces(text: &str, at: usize) -> usize {
    let mut index = at;
    while text.as_bytes().get(index) == Some(&b' ') {
        index = index.saturating_add(1);
    }
    index
}

fn token_end(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b',' | b']' | b'}' => return index,
            b':' => match bytes.get(index.saturating_add(1)) {
                None | Some(b' ' | b',' | b']' | b'}') => return index,
                Some(_) => {}
            },
            _ => {}
        }
        index = index.saturating_add(1);
    }
    index
}

fn parse_flow(
    text: &str,
    at: usize,
    line: usize,
    depth: usize,
) -> Result<(Value, usize), YamlError> {
    if depth > MAX_DEPTH {
        return Err(YamlError::TooDeep {
            line,
            limit: MAX_DEPTH,
        });
    }
    let start = skip_spaces(text, at);
    let deeper = depth.saturating_add(1);
    match text.as_bytes().get(start) {
        Some(b'[') => parse_flow_sequence(text, start, line, deeper),
        Some(b'{') => parse_flow_mapping(text, start, line, deeper),
        Some(b'"') => {
            let rest = text.get(start..).unwrap_or("");
            let (value, used) = parse_quoted(rest, line)?;
            Ok((Value::String(value), start.saturating_add(used)))
        }
        _ => {
            let rest = text.get(start..).unwrap_or("");
            let stop = token_end(rest);
            let token = rest.get(..stop).unwrap_or("").trim_end();
            refuse(token, line)?;
            let value = if token.is_empty() {
                Value::Null
            } else {
                plain(token)
            };
            Ok((value, start.saturating_add(stop)))
        }
    }
}

fn parse_flow_sequence(
    text: &str,
    start: usize,
    line: usize,
    depth: usize,
) -> Result<(Value, usize), YamlError> {
    let mut items = Vec::new();
    let mut index = start.saturating_add(1);
    loop {
        index = skip_spaces(text, index);
        match text.as_bytes().get(index) {
            Some(b']') => return Ok((Value::Array(items), index.saturating_add(1))),
            None => return Err(flow_error(text, line)),
            Some(_) => {}
        }
        let (value, next) = parse_flow(text, index, line, depth)?;
        items.push(value);
        index = skip_spaces(text, next);
        match text.as_bytes().get(index) {
            Some(b',') => index = index.saturating_add(1),
            Some(b']') => return Ok((Value::Array(items), index.saturating_add(1))),
            _ => return Err(flow_error(text, line)),
        }
    }
}

fn parse_flow_mapping(
    text: &str,
    start: usize,
    line: usize,
    depth: usize,
) -> Result<(Value, usize), YamlError> {
    let mut map = Map::new();
    let mut index = start.saturating_add(1);
    loop {
        index = skip_spaces(text, index);
        match text.as_bytes().get(index) {
            Some(b'}') => return Ok((Value::Object(map), index.saturating_add(1))),
            None => return Err(flow_error(text, line)),
            Some(_) => {}
        }
        let (raw, after) = parse_flow(text, index, line, depth)?;
        let Value::String(key) = raw else {
            return Err(YamlError::Key {
                line,
                text: text.to_string(),
            });
        };
        index = skip_spaces(text, after);
        if text.as_bytes().get(index) != Some(&b':') {
            return Err(flow_error(text, line));
        }
        let (value, next) = parse_flow(text, index.saturating_add(1), line, depth)?;
        if map.contains_key(&key) {
            return Err(YamlError::DuplicateKey { line, text: key });
        }
        map.insert(key, value);
        index = skip_spaces(text, next);
        match text.as_bytes().get(index) {
            Some(b',') => index = index.saturating_add(1),
            Some(b'}') => return Ok((Value::Object(map), index.saturating_add(1))),
            _ => return Err(flow_error(text, line)),
        }
    }
}

fn flow_error(text: &str, line: usize) -> YamlError {
    YamlError::Flow {
        line,
        text: text.to_string(),
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn head(out: &mut String, indent: usize, dash: bool) {
    if dash {
        pad(out, indent.saturating_sub(2));
        out.push_str("- ");
    } else {
        pad(out, indent);
    }
}

fn emit(
    value: &Value,
    indent: usize,
    dash: bool,
    out: &mut String,
    depth: usize,
) -> Result<(), YamlError> {
    if depth > MAX_DEPTH {
        return Err(YamlError::EmitTooDeep { limit: MAX_DEPTH });
    }
    let deeper = depth.saturating_add(1);
    let child = indent.saturating_add(2);
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (position, (key, nested)) in map.iter().enumerate() {
                head(out, indent, dash && position == 0);
                out.push_str(&quote_if_needed(key)?);
                out.push(':');
                emit_value(nested, indent, out, deeper)?;
            }
            Ok(())
        }
        Value::Array(items) if !items.is_empty() => {
            for item in items {
                match item {
                    Value::Object(map) if !map.is_empty() => emit(item, child, true, out, deeper)?,
                    Value::Array(inner) if !inner.is_empty() => {
                        pad(out, indent);
                        out.push_str("-\n");
                        emit(item, child, false, out, deeper)?;
                    }
                    _ => {
                        pad(out, indent);
                        out.push_str("- ");
                        out.push_str(&scalar(item)?);
                        out.push('\n');
                    }
                }
            }
            Ok(())
        }
        _ => {
            head(out, indent, dash);
            out.push_str(&scalar(value)?);
            out.push('\n');
            Ok(())
        }
    }
}

fn emit_value(
    value: &Value,
    indent: usize,
    out: &mut String,
    depth: usize,
) -> Result<(), YamlError> {
    let child = indent.saturating_add(2);
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push('\n');
            emit(value, child, false, out, depth)
        }
        Value::Array(items) if !items.is_empty() => {
            out.push('\n');
            emit(value, child, false, out, depth)
        }
        _ => {
            out.push(' ');
            out.push_str(&scalar(value)?);
            out.push('\n');
            Ok(())
        }
    }
}

fn scalar(value: &Value) -> Result<String, YamlError> {
    match value {
        Value::Null => Ok("null".to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Number(number) => number
            .as_i64()
            .map(|signed| signed.to_string())
            .or_else(|| number.as_u64().map(|unsigned| unsigned.to_string()))
            .ok_or_else(|| YamlError::NotAnInteger {
                value: number.as_f64().unwrap_or(f64::NAN),
            }),
        Value::String(text) => quote_if_needed(text),
        Value::Object(_) => Ok("{}".to_string()),
        Value::Array(_) => Ok("[]".to_string()),
    }
}

fn quote_if_needed(text: &str) -> Result<String, YamlError> {
    if plain_is_safe(text) {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ if ch.is_control() => {
                return Err(YamlError::Unencodable {
                    text: text.to_string(),
                });
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    Ok(out)
}

fn plain_is_safe(text: &str) -> bool {
    if text.is_empty() || plain(text) != Value::String(text.to_string()) {
        return false;
    }
    if text.trim() != text
        || text.contains(": ")
        || text.ends_with(':')
        || text.contains(" #")
        || text.contains('"')
        || text.chars().any(|ch| ch.is_control() && ch != '\t')
    {
        return false;
    }
    !matches!(
        text.as_bytes().first(),
        Some(
            b'-' | b'?'
                | b':'
                | b','
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'#'
                | b'&'
                | b'*'
                | b'!'
                | b'|'
                | b'>'
                | b'\''
                | b'%'
                | b'@'
                | b'`'
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type Fallible = Result<(), Box<dyn std::error::Error>>;

    const VERBATIM: &str = concat!(
        "format: 1\n",
        "plan: 7f3a-auth-refactor\n",
        "goal: \"Ship OAuth login end to end\"   # edits are a user-attributed op (D25)\n",
        "version: 3\n",
        "tier: root                            # sub adds: parent: <plan>/<todo label>\n",
        "state: active\n",
        "todos:\n",
        "  - label: \"Freeze the token API seam\"      # <= 80 chars, unique, the address\n",
        "    state: done                       # ready is NEVER stored - derived from\n",
        "    output: kernel://token_api_seam   # `after` + states at read\n",
        "  - label: \"Implement refresh flow\"\n",
        "    state: running               # the child is agent://<plan>/implement-refresh-flow\n",
        "    after: [\"Freeze the token API seam\"]\n",
        "    delegation:\n",
        "      spec:    { role: coder, effort: med, isolation: worktree }\n",
        "      accept:  { command: \"cargo test -p yi-ai refresh\" }   # or { stated: ... }\n",
        "      output:  { schema: local://.yi/schemas/refresh_result.json }\n",
        "      context: [\"plan://7f3a-auth-refactor/seam-notes\", \"local://docs/auth.md\"]\n",
        "    retries: 1\n",
    );

    const CANONICAL: &str = concat!(
        "format: 1\n",
        "plan: 7f3a-auth-refactor\n",
        "goal: Ship OAuth login end to end\n",
        "version: 3\n",
        "tier: root\n",
        "state: active\n",
        "todos:\n",
        "  - label: Freeze the token API seam\n",
        "    state: done\n",
        "    output: kernel://token_api_seam\n",
        "  - label: Implement refresh flow\n",
        "    state: running\n",
        "    after:\n",
        "      - Freeze the token API seam\n",
        "    delegation:\n",
        "      spec:\n",
        "        role: coder\n",
        "        effort: med\n",
        "        isolation: worktree\n",
        "      accept:\n",
        "        command: cargo test -p yi-ai refresh\n",
        "      output:\n",
        "        schema: local://.yi/schemas/refresh_result.json\n",
        "      context:\n",
        "        - plan://7f3a-auth-refactor/seam-notes\n",
        "        - local://docs/auth.md\n",
        "    retries: 1\n",
    );

    #[test]
    fn canonical_frontmatter_round_trips_byte_exact() -> Fallible {
        assert_eq!(to_yaml(&from_yaml(CANONICAL)?)?, CANONICAL);
        Ok(())
    }

    #[test]
    fn comments_flow_and_alignment_reduce_to_the_canonical_form() -> Fallible {
        assert_eq!(from_yaml(VERBATIM)?, from_yaml(CANONICAL)?);
        assert_eq!(to_yaml(&from_yaml(VERBATIM)?)?, CANONICAL);
        Ok(())
    }

    #[test]
    fn frontmatter_splits_body_from_yaml() -> Fallible {
        let document = format!("---\n{CANONICAL}---\n\n## seam-notes\n");
        let (front, body) = split_frontmatter(&document)?;
        assert_eq!(front, CANONICAL);
        assert_eq!(body, "\n## seam-notes\n");
        assert!(matches!(
            split_frontmatter("# not a plan\n"),
            Err(YamlError::NoFrontmatter { .. })
        ));
        assert!(matches!(
            split_frontmatter("---\na: 1\n"),
            Err(YamlError::OpenFrontmatter { lines: 2 })
        ));
        Ok(())
    }

    #[test]
    fn every_refusal_kind_is_typed_and_names_its_line() {
        let cases: [(&str, Unsupported); 7] = [
            ("a: 1\nb:\n\tc: 2\n", Unsupported::Tab),
            ("a: &anchor 1\n", Unsupported::Anchor),
            ("a: *anchor\n", Unsupported::Alias),
            ("a: !!str 1\n", Unsupported::Tag),
            ("a: 'single'\n", Unsupported::SingleQuoted),
            ("a: |\n  block\n", Unsupported::BlockScalar),
            ("a: 1\n---\nb: 2\n", Unsupported::DocumentDelimiter),
        ];
        for (text, want) in cases {
            let refused = matches!(
                from_yaml(text),
                Err(YamlError::Unsupported { kind, line, .. }) if kind == want && line >= 1
            );
            assert!(refused, "{text:?} was not refused as {want:?}");
        }
    }

    #[test]
    fn malformed_input_is_typed_and_never_a_best_effort_parse() {
        assert!(matches!(
            from_yaml("a: \"unclosed\n"),
            Err(YamlError::UnterminatedQuote { line: 1, .. })
        ));
        assert!(matches!(
            from_yaml("a: \"tab\\there\"\n"),
            Err(YamlError::Escape { line: 1, .. })
        ));
        assert!(matches!(
            from_yaml("a: [1, 2\n"),
            Err(YamlError::Flow { line: 1, .. })
        ));
        assert!(matches!(
            from_yaml(": 1\n"),
            Err(YamlError::Key { line: 1, .. })
        ));
        assert!(matches!(
            from_yaml("a: 1\na: 2\n"),
            Err(YamlError::DuplicateKey { line: 2, .. })
        ));
        assert!(matches!(
            from_yaml("a:\n   b: 1\n"),
            Err(YamlError::Indent { line: 2, .. })
        ));
        assert!(matches!(
            from_yaml("a: [1] leftover\n"),
            Err(YamlError::Trailing { line: 1, .. })
        ));
        assert!(matches!(
            to_yaml(&json!({ "a": 1.5 })),
            Err(YamlError::NotAnInteger { .. })
        ));
        assert!(matches!(
            to_yaml(&json!({ "a": "\ttabbed" })),
            Err(YamlError::Unencodable { .. })
        ));
    }

    #[test]
    fn nesting_bombs_error_instead_of_overflowing_the_stack() {
        let flow = format!("a: {}{}", "[".repeat(5_000), "]".repeat(5_000));
        assert!(matches!(
            from_yaml(&flow),
            Err(YamlError::TooDeep { line: 1, .. })
        ));

        let mut block = String::new();
        for level in 0..5_000usize {
            block.push_str(&" ".repeat(level.saturating_mul(2)));
            block.push_str("a:\n");
        }
        assert!(matches!(from_yaml(&block), Err(YamlError::TooDeep { .. })));

        let mut deep = json!(0);
        for _ in 0..100 {
            deep = Value::Array(vec![deep]);
        }
        assert!(matches!(
            to_yaml(&deep),
            Err(YamlError::EmitTooDeep { limit: MAX_DEPTH })
        ));
    }

    #[test]
    fn quoted_scalar_keeps_hash_colon_and_newline() -> Fallible {
        let value = json!({ "note": "cut # here: then\nnext", "url": "kernel://a/b" });
        let text = to_yaml(&value)?;
        assert_eq!(
            text,
            "note: \"cut # here: then\\nnext\"\nurl: kernel://a/b\n"
        );
        assert_eq!(from_yaml(&text)?, value);
        assert_eq!(to_yaml(&from_yaml(&text)?)?, text);
        Ok(())
    }

    #[test]
    fn scalar_edges_round_trip_without_changing_type() -> Fallible {
        let value = json!({
            "empty_map": {},
            "empty_seq": [],
            "empty_text": "",
            "null": null,
            "flags": [true, false],
            "int": -12,
            "looks_null": "null",
            "looks_int": "007",
            "nested_seq": [[1, 2], [3]],
            "key: with colon": "x",
        });
        let text = to_yaml(&value)?;
        assert_eq!(from_yaml(&text)?, value);
        assert_eq!(to_yaml(&from_yaml(&text)?)?, text);
        Ok(())
    }

    #[test]
    fn key_order_is_field_order_both_ways() -> Fallible {
        let text = "zebra: 1\nalpha: 2\nmid: 3\n";
        assert_eq!(to_yaml(&from_yaml(text)?)?, text);
        Ok(())
    }
}
