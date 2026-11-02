use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use ratatui::text::Line;

use super::{OPTIONS, render_stream};
use crate::colors::Theme;

/// Rows of a source that only grows, rendered past its last settled point alone: blocks
/// before the last one can no longer change, and an open fence settles line by line.
#[derive(Default)]
pub struct Settled {
    at: usize,
    reopen: Option<String>,
    lang: Option<crate::highlight::Lang>,
    shown: bool,
    opaque: bool,
}

impl Settled {
    /// The rows newly settled and the rows of the rest, which extend every earlier call's
    /// settled rows to `render(source)`. `None` once a link definition makes rows global.
    pub fn advance(
        &mut self,
        source: &str,
        width: usize,
        theme: &Theme,
    ) -> Option<(Vec<Line<'static>>, Vec<Line<'static>>)> {
        let rest = source.get(self.at..)?;
        self.opaque |= rest.lines().any(|line| {
            let body = line.trim_start_matches(' ');
            line.len() - body.len() <= 3 && body.starts_with('[') && body.contains("]:")
        });
        if self.opaque {
            return None;
        }
        let complete = rest.rfind('\n').map_or(0, |at| at + 1);
        let mut settled = Vec::new();
        if let Some((cut, reopen)) = self.settle_point(rest.get(..complete)?) {
            let mut lang = self.lang.take();
            settled = self.segment(rest.get(..cut)?, width, theme, &mut lang);
            (self.at, self.reopen, self.lang) = (self.at + cut, reopen, lang);
            self.shown |= !settled.is_empty();
        }
        let tail = source.get(self.at..)?;
        Some((
            settled,
            self.segment(tail, width, theme, &mut self.lang.clone()),
        ))
    }

    /// The last top-level block's start, or past the last complete line of its open fence.
    fn settle_point(&self, text: &str) -> Option<(usize, Option<String>)> {
        let open = self.reopen.as_ref().map(|line| format!("{line}\n"));
        let doc = format!("{}{text}", open.as_deref().unwrap_or_default());
        let (mut depth, mut last, mut code) = (0_usize, None, 0_usize);
        for (event, range) in Parser::new_ext(&doc, OPTIONS).into_offset_iter() {
            match &event {
                Event::Text(body) if depth == 1 => code += body.len(),
                Event::End(_) => depth = depth.saturating_sub(1),
                _ if depth > 0 => {}
                _ => {
                    let line = doc.get(..range.start)?.rfind('\n').map_or(0, |at| at + 1);
                    let fenced = matches!(
                        event,
                        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_)))
                    );
                    (last, code) = (Some((line, fenced)), 0);
                }
            }
            if let Event::Start(_) = event {
                depth += 1;
            }
        }
        let (start, fenced) = last?;
        let opener = doc.get(start..)?.split_inclusive('\n').next()?;
        // A fence whose code runs to the end is still open, and each complete line is final.
        let open_fence = fenced && start + opener.len() + code == doc.len();
        let (cut, reopen) = match open_fence && opener.ends_with('\n') {
            true => (doc.len(), Some(opener.trim_end_matches('\n').to_owned())),
            false => (start, None),
        };
        let cut = cut.checked_sub(open.map_or(0, |line| line.len()))?;
        (cut > 0).then_some((cut, reopen))
    }

    /// A continued fence joins the rows above; any other block gets the blank row between.
    fn segment(
        &self,
        text: &str,
        width: usize,
        theme: &Theme,
        lang: &mut Option<crate::highlight::Lang>,
    ) -> Vec<Line<'static>> {
        let (mut text, mut reopen) = (text, self.reopen.as_deref());
        // A fence closing before any code leaves no row to set the next block off from.
        if let Some(open) = reopen {
            let doc = format!("{open}\n{text}");
            let mut events = Parser::new_ext(&doc, OPTIONS).into_offset_iter();
            if let (Some((Event::Start(_), block)), Some((Event::End(_), _))) =
                (events.next(), events.next())
            {
                let after = block.end.saturating_sub(open.len() + 1);
                (text, reopen) = (text.get(after..).unwrap_or_default(), None);
            }
        }
        let source = match reopen {
            Some(open) if !text.is_empty() => format!("{open}\n{text}"),
            _ => text.to_owned(),
        };
        let mut rows = render_stream(&source, width, theme, reopen.is_some(), lang);
        if self.shown && reopen.is_none() && !rows.is_empty() {
            rows.insert(0, Line::default());
        }
        rows
    }
}
