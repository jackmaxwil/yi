//! Anchors cited from an older snapshot move onto the current file through the diff between
//! the texts; a line maps only when its text is unchanged, so nothing is ever guessed.

use super::types::{Anchor, BlockSpan, Cursor, Edit, ParsedRange, PasteTarget};

pub struct LineMap(Vec<Option<u64>>);

impl LineMap {
    pub fn between(old_text: &str, new_text: &str) -> Self {
        Self(crate::diff::line_map(old_text, new_text))
    }

    pub fn line(&self, line: u64) -> Option<u64> {
        let index = usize::try_from(line.checked_sub(1)?).ok()?;
        self.0.get(index).copied().flatten()
    }

    fn range(&self, range: ParsedRange) -> Option<ParsedRange> {
        let start = self.line(range.start.line)?;
        let mut expected = start;
        for line in range.start.line..=range.end.line {
            if self.line(line)? != expected {
                return None;
            }
            expected = expected.checked_add(1)?;
        }
        let end = self.line(range.end.line)?;
        Some(ParsedRange {
            start: Anchor { line: start },
            end: Anchor { line: end },
        })
    }

    fn anchor(&self, anchor: Anchor) -> Option<Anchor> {
        self.line(anchor.line).map(|line| Anchor { line })
    }

    fn cursor(&self, cursor: Cursor) -> Option<Cursor> {
        Some(match cursor {
            Cursor::Bof => Cursor::Bof,
            Cursor::Eof => Cursor::Eof,
            Cursor::BeforeAnchor { anchor } => Cursor::BeforeAnchor {
                anchor: self.anchor(anchor)?,
            },
            Cursor::AfterAnchor { anchor } => Cursor::AfterAnchor {
                anchor: self.anchor(anchor)?,
            },
        })
    }

    /// The old block's every line must map contiguously: a block op replaces or cuts the
    /// whole construct, so a body that changed since the read is not the body that was cited.
    fn block(&self, anchor: Anchor, old_span: Option<BlockSpan>) -> Option<Anchor> {
        match old_span {
            Some(span) => self
                .range(ParsedRange {
                    start: Anchor { line: span.start },
                    end: Anchor { line: span.end },
                })
                .map(|range| range.start),
            None => self.anchor(anchor),
        }
    }
}

pub fn remap_edits(
    edits: Vec<Edit>,
    map: &LineMap,
    old_block: &dyn Fn(u64) -> Option<BlockSpan>,
) -> Result<Vec<Edit>, Vec<u64>> {
    let mut unmapped: Vec<u64> = Vec::new();
    let mut out: Vec<Edit> = Vec::with_capacity(edits.len());
    for edit in edits {
        let mapped = match edit.clone() {
            Edit::Insert {
                cursor,
                text,
                line_num,
                index,
                replacement,
                block_start,
            } => map.cursor(cursor).map(|cursor| Edit::Insert {
                cursor,
                text,
                line_num,
                index,
                replacement,
                block_start,
            }),
            Edit::Delete {
                anchor,
                line_num,
                index,
                old_assertion,
            } => map.anchor(anchor).map(|anchor| Edit::Delete {
                anchor,
                line_num,
                index,
                old_assertion,
            }),
            Edit::Cut {
                range,
                register,
                line_num,
                index,
            } => map.range(range).map(|range| Edit::Cut {
                range,
                register,
                line_num,
                index,
            }),
            Edit::Paste {
                at,
                register,
                line_num,
                index,
                block_start,
            } => {
                let at = match at {
                    PasteTarget::Gap { cursor } => {
                        map.cursor(cursor).map(|cursor| PasteTarget::Gap { cursor })
                    }
                    PasteTarget::Span { range } => {
                        map.range(range).map(|range| PasteTarget::Span { range })
                    }
                };
                at.map(|at| Edit::Paste {
                    at,
                    register,
                    line_num,
                    index,
                    block_start,
                })
            }
            Edit::Block {
                anchor,
                payloads,
                mode,
                register,
                line_num,
                index,
            } => map
                .block(anchor, old_block(anchor.line))
                .map(|anchor| Edit::Block {
                    anchor,
                    payloads,
                    mode,
                    register,
                    line_num,
                    index,
                }),
        };
        match mapped {
            Some(mapped) => out.push(mapped),
            None => unmapped.extend(cited_lines(&edit)),
        }
    }
    if unmapped.is_empty() {
        Ok(out)
    } else {
        unmapped.sort_unstable();
        unmapped.dedup();
        Err(unmapped)
    }
}

fn cited_lines(edit: &Edit) -> Vec<u64> {
    match edit {
        Edit::Delete { anchor, .. } | Edit::Block { anchor, .. } => vec![anchor.line],
        Edit::Cut { range, .. }
        | Edit::Paste {
            at: PasteTarget::Span { range },
            ..
        } => vec![range.start.line, range.end.line],
        Edit::Paste {
            at: PasteTarget::Gap { cursor },
            ..
        }
        | Edit::Insert { cursor, .. } => match cursor {
            Cursor::BeforeAnchor { anchor } | Cursor::AfterAnchor { anchor } => vec![anchor.line],
            Cursor::Bof | Cursor::Eof => Vec::new(),
        },
    }
}
