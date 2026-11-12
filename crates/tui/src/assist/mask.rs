use super::CharSpan;

pub(super) fn prose_mask(chars: &[char]) -> Vec<bool> {
    let mut prose = vec![true; chars.len()];
    let mut fence: Option<char> = None;
    let mut line_start = 0;
    for (index, ch) in chars.iter().enumerate() {
        if *ch != '\n' && index + 1 != chars.len() {
            continue;
        }
        let line_end = if *ch == '\n' { index } else { chars.len() };
        let line = &chars[line_start..line_end];
        if let Some(mark) = fence {
            for slot in prose.iter_mut().take(line_end).skip(line_start) {
                *slot = false;
            }
            if opening_fence(line) == Some(mark) {
                fence = None;
            }
        } else if let Some(mark) = opening_fence(line) {
            for slot in prose.iter_mut().take(line_end).skip(line_start) {
                *slot = false;
            }
            fence = Some(mark);
        } else {
            mask_inline(&mut prose, line_start, line_end, chars);
        }
        line_start = index + 1;
    }
    prose
}

fn opening_fence(line: &[char]) -> Option<char> {
    let mut index = 0;
    while index < line.len() && index < 3 && line[index] == ' ' {
        index += 1;
    }
    let mark = *line.get(index)?;
    if mark != '`' && mark != '~' {
        return None;
    }
    let mut count = 0;
    while line.get(index) == Some(&mark) {
        count += 1;
        index += 1;
    }
    (count >= 3).then_some(mark)
}

fn mask_inline(prose: &mut [bool], start: usize, end: usize, chars: &[char]) {
    let mut index = start;
    while index < end {
        if chars[index] == '`' {
            let mut stop = index + 1;
            while stop < end && chars[stop] != '`' {
                stop += 1;
            }
            if stop < end {
                for slot in prose.iter_mut().take(stop + 1).skip(index) {
                    *slot = false;
                }
                index = stop + 1;
                continue;
            }
        }
        if chars[index] == '<'
            && let Some(stop) = (index + 1..end).find(|&at| chars[at] == '>')
        {
            for slot in prose.iter_mut().take(stop + 1).skip(index) {
                *slot = false;
            }
            let closing = chars.get(index + 1) == Some(&'/');
            if !closing
                && let Some(close) =
                    (stop + 1..end).find(|&at| chars[at] == '<' && chars.get(at + 1) == Some(&'/'))
            {
                for slot in prose.iter_mut().take(close).skip(stop + 1) {
                    *slot = false;
                }
            }
            index = stop + 1;
            continue;
        }
        index += 1;
    }
}

pub(super) fn triple_spans(chars: &[char]) -> Vec<CharSpan> {
    let mut spans = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_alphabetic() {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && chars[index].is_alphabetic() {
            index += 1;
        }
        let mut run = 1;
        let mut hit = false;
        for at in start + 1..index {
            if chars[at] == chars[at - 1] {
                run += 1;
                hit |= run >= 3;
            } else {
                run = 1;
            }
        }
        if hit {
            spans.push(CharSpan { start, end: index });
        }
    }
    spans
}

pub(super) fn keep_span(chars: &[char], mask: &[bool], start: usize, end: usize) -> bool {
    if start >= end || end > chars.len() {
        return false;
    }
    if !(start..end).all(|index| mask.get(index).copied().unwrap_or(false)) {
        return false;
    }
    let (token_start, token_end) = bounds(chars, start);
    if (token_start..token_end).any(|index| {
        let ch = chars[index];
        matches!(ch, '_' | '/' | '.' | '\\') || ch.is_ascii_digit()
    }) {
        return false;
    }
    let mut lower = false;
    for ch in &chars[token_start..token_end] {
        if lower && ch.is_uppercase() {
            return false;
        }
        lower = ch.is_lowercase();
    }
    !line_skipped(chars, start)
}

fn bounds(chars: &[char], index: usize) -> (usize, usize) {
    let mut start = index;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = index;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    (start, end)
}

fn line_skipped(chars: &[char], index: usize) -> bool {
    let mut start = index;
    while start > 0 && chars[start - 1] != '\n' {
        start -= 1;
    }
    let mut end = index;
    while end < chars.len() && chars[end] != '\n' {
        end += 1;
    }
    let line: String = chars[start..end].iter().collect();
    let trimmed = line.trim_start();
    if trimmed.starts_with("->") || trimmed.starts_with("=>") {
        return true;
    }
    let Some(rest) = trimmed.strip_prefix('/') else {
        return false;
    };
    if rest.starts_with('/') {
        return false;
    }
    let head = rest.split_whitespace().next().unwrap_or_default();
    !head.is_empty()
        && !head.contains('/')
        && head
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub(super) fn collapse(word: &str) -> Option<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    let mut changed = false;
    while index < chars.len() {
        let mut end = index + 1;
        while end < chars.len() && chars[end] == chars[index] {
            end += 1;
        }
        let run = end - index;
        if run >= 3 {
            out.push(chars[index]);
            out.push(chars[index]);
            changed = true;
        } else {
            for ch in &chars[index..end] {
                out.push(*ch);
            }
        }
        index = end;
    }
    changed.then_some(out)
}
