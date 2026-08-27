use std::collections::{BTreeMap, BTreeSet};

/// Strict `{{name}}` interpolation: an unused supplied value is an error, so a
/// renamed placeholder cannot silently drop content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("template placeholder at byte {start} is empty")]
    EmptyPlaceholder { start: usize },
    #[error("template placeholder starting at byte {start} contains a nested `{{`")]
    NestedPlaceholder { start: usize },
    #[error("template contains an unmatched `}}` at byte {start}")]
    UnmatchedClosingDelimiter { start: usize },
    #[error("template placeholder starting at byte {start} is missing `}}`")]
    UnterminatedPlaceholder { start: usize },
    #[error("template value `{name}` was provided more than once")]
    DuplicateValue { name: String },
    #[error("template value `{name}` is not used by this template")]
    ExtraValue { name: String },
    #[error("template placeholder `{name}` is missing a value")]
    MissingValue { name: String },
}

#[derive(Debug, Clone)]
enum Segment {
    Literal(String),
    Placeholder(String),
}

#[derive(Debug, Clone)]
pub struct Template {
    placeholders: BTreeSet<String>,
    segments: Vec<Segment>,
}

impl Template {
    pub fn parse(source: &str) -> Result<Self, TemplateError> {
        let mut placeholders = BTreeSet::new();
        let mut segments = Vec::new();
        let mut literal_start = 0_usize;
        let mut cursor = 0_usize;
        while cursor < source.len() {
            let rest = &source[cursor..];
            if rest.starts_with("{{{{") {
                push_literal(
                    &mut segments,
                    source.get(literal_start..cursor).unwrap_or(""),
                );
                push_literal(&mut segments, "{{");
                cursor = cursor.saturating_add(4);
                literal_start = cursor;
                continue;
            }
            if rest.starts_with("}}}}") {
                push_literal(
                    &mut segments,
                    source.get(literal_start..cursor).unwrap_or(""),
                );
                push_literal(&mut segments, "}}");
                cursor = cursor.saturating_add(4);
                literal_start = cursor;
                continue;
            }
            if rest.starts_with("{{") {
                push_literal(
                    &mut segments,
                    source.get(literal_start..cursor).unwrap_or(""),
                );
                let (placeholder, next_cursor) = parse_placeholder(source, cursor)?;
                placeholders.insert(placeholder.clone());
                segments.push(Segment::Placeholder(placeholder));
                cursor = next_cursor;
                literal_start = cursor;
                continue;
            }
            if rest.starts_with("}}") {
                return Err(TemplateError::UnmatchedClosingDelimiter { start: cursor });
            }
            let Some(ch) = rest.chars().next() else {
                break;
            };
            cursor = cursor.saturating_add(ch.len_utf8());
        }
        push_literal(&mut segments, source.get(literal_start..).unwrap_or(""));
        Ok(Self {
            placeholders,
            segments,
        })
    }

    pub fn render<'a, I>(&self, variables: I) -> Result<String, TemplateError>
    where
        I: IntoIterator<Item = (&'a str, String)>,
    {
        let mut map = BTreeMap::new();
        for (name, value) in variables {
            if map.insert(name.to_owned(), value).is_some() {
                return Err(TemplateError::DuplicateValue {
                    name: name.to_owned(),
                });
            }
        }
        for placeholder in &self.placeholders {
            if !map.contains_key(placeholder.as_str()) {
                return Err(TemplateError::MissingValue {
                    name: placeholder.clone(),
                });
            }
        }
        for name in map.keys() {
            if !self.placeholders.contains(name.as_str()) {
                return Err(TemplateError::ExtraValue { name: name.clone() });
            }
        }
        let mut rendered = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Literal(literal) => rendered.push_str(literal),
                Segment::Placeholder(name) => match map.get(name.as_str()) {
                    Some(value) => rendered.push_str(value),
                    None => {
                        return Err(TemplateError::MissingValue { name: name.clone() });
                    }
                },
            }
        }
        Ok(rendered)
    }
}

pub fn render<'a, I>(template: &str, variables: I) -> Result<String, TemplateError>
where
    I: IntoIterator<Item = (&'a str, String)>,
{
    Template::parse(template)?.render(variables)
}

fn push_literal(segments: &mut Vec<Segment>, literal: &str) {
    if literal.is_empty() {
        return;
    }
    if let Some(Segment::Literal(existing)) = segments.last_mut() {
        existing.push_str(literal);
    } else {
        segments.push(Segment::Literal(literal.to_owned()));
    }
}

fn parse_placeholder(source: &str, start: usize) -> Result<(String, usize), TemplateError> {
    let placeholder_start = start.saturating_add(2);
    let mut cursor = placeholder_start;
    while cursor < source.len() {
        let rest = &source[cursor..];
        if rest.starts_with("{{") {
            return Err(TemplateError::NestedPlaceholder { start });
        }
        if rest.starts_with("}}") {
            let placeholder = source
                .get(placeholder_start..cursor)
                .unwrap_or("")
                .trim()
                .to_owned();
            if placeholder.is_empty() {
                return Err(TemplateError::EmptyPlaceholder { start });
            }
            return Ok((placeholder, cursor.saturating_add(2)));
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        cursor = cursor.saturating_add(ch.len_utf8());
    }
    Err(TemplateError::UnterminatedPlaceholder { start })
}
