#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    fn flush(&mut self) -> Option<SseEvent> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        let event = SseEvent {
            event: self.event.take(),
            data: self.data.join("\n"),
        };
        self.data.clear();
        Some(event)
    }

    fn decode_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.flush();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.find(':') {
            Some(index) => {
                let value = &line[index.saturating_add(1)..];
                (&line[..index], value.strip_prefix(' ').unwrap_or(value))
            }
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
        None
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        loop {
            let Some(break_index) = self.buffer.find(['\r', '\n']) else {
                break;
            };
            let mut next_index = break_index.saturating_add(1);
            if self.buffer.as_bytes().get(break_index) == Some(&b'\r')
                && self.buffer.as_bytes().get(next_index) == Some(&b'\n')
            {
                next_index = next_index.saturating_add(1);
            }
            let line: String = self.buffer.drain(..next_index).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(event) = self.decode_line(line) {
                events.push(event);
            }
        }
        events
    }

    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.decode_line(line.trim_end_matches(['\r', '\n'])) {
                events.push(event);
            }
        }
        if let Some(event) = self.flush() {
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_on_blank_lines() {
        let mut decoder = SseDecoder::default();
        let mut events =
            decoder.feed("event: message_start\ndata: {\"a\":1}\n\nevent: ping\ndata: {}\n\n");
        events.extend(decoder.finish());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"a\":1}");
    }

    #[test]
    fn joins_multiline_data_and_handles_crlf() {
        let mut decoder = SseDecoder::default();
        let mut events = decoder.feed("data: one\r\ndata: two\r\n\r\n");
        events.extend(decoder.finish());
        assert_eq!(events[0].data, "one\ntwo");
    }

    #[test]
    fn trailing_event_without_blank_line_flushes_on_finish() {
        let mut decoder = SseDecoder::default();
        let mut events = decoder.feed("data: tail");
        events.extend(decoder.finish());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "tail");
    }
}
