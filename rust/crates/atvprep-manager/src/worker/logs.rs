//! The log of one job item: the manager's step lines and the tools' output, bounded together.

/// The most a stored item log may hold, in bytes. It stays within what the API returns for a log.
pub const ITEM_LOG_LIMIT: usize = 64 * 1024;

/// The log of one job item, as stored in `job_items.log_tail`.
///
/// Two kinds of text go in, in the order they happen:
/// - [`Self::step`]: a line of the manager, stamped `[HH:MM:SS]` (UTC). Step lines are never cut.
/// - [`Self::tool`]: raw output of a tool. Output that does not fit loses its middle, marked with
///   `… n bytes omitted …`, so its beginning and its end stay.
///
/// [`Self::text`] never exceeds the limit. Should the step lines alone exceed it (they do not in
/// practice), whole lines in the middle give way, under the same hint.
pub struct ItemLog {
    limit: usize,
    entries: Vec<Entry>,
}

enum Entry {
    Step(String),
    Tool(Output),
}

/// Output of one uninterrupted run of tool chunks: its first and its last `limit` bytes.
struct Output {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl Output {
    fn push(&mut self, chunk: &[u8], limit: usize) {
        self.total += chunk.len();
        let room = limit.saturating_sub(self.head.len()).min(chunk.len());
        self.head.extend_from_slice(&chunk[..room]);
        self.tail.extend_from_slice(&chunk[room..]);
        if self.tail.len() > limit {
            self.tail.drain(..self.tail.len() - limit);
        }
    }

    /// The bytes between head and tail that were never kept.
    fn dropped(&self) -> usize {
        self.total - self.head.len() - self.tail.len()
    }

    /// The output in at most `room` bytes; `room` is the budget including the hint.
    fn render(&self, room: usize) -> String {
        let head = clean(&self.head);
        let tail = clean(&self.tail);
        if self.dropped() == 0 {
            return cut_middle(&format!("{head}{tail}"), room);
        }
        let hint_room = hint(self.total).len();
        let Some(available) = room.checked_sub(hint_room) else {
            return String::new();
        };
        let front = prefix(&head, available / 2);
        let back = suffix(&tail, available - front.len());
        let omitted = head.len() - front.len() + self.dropped() + tail.len() - back.len();
        format!("{front}{}{back}", hint(omitted))
    }
}

/// The hint where text was cut.
fn hint(omitted: usize) -> String {
    format!("\n… {omitted} bytes omitted …\n")
}

fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\0', "")
}

/// At most `limit` bytes from the start, without splitting a character.
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// At most `limit` bytes from the end, without splitting a character.
fn suffix(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// `text` in at most `room` bytes: its middle replaced by the hint when it does not fit.
fn cut_middle(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_owned();
    }
    let Some(available) = room.checked_sub(hint(text.len()).len()) else {
        return String::new();
    };
    let front = prefix(text, available / 2);
    let back = suffix(text, available - front.len());
    format!(
        "{front}{}{back}",
        hint(text.len() - front.len() - back.len())
    )
}

/// Like [`cut_middle`], but only whole lines give way.
fn cut_middle_lines(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_owned();
    }
    let Some(available) = room.checked_sub(hint(text.len()).len()) else {
        return String::new();
    };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (mut front, mut back) = (0, lines.len());
    let mut used = 0;
    // Alternate between both ends, so the beginning and the end of the log both stay.
    loop {
        let mut grew = false;
        if front < back && used + lines[front].len() <= available {
            used += lines[front].len();
            front += 1;
            grew = true;
        }
        if front < back && used + lines[back - 1].len() <= available {
            used += lines[back - 1].len();
            back -= 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let omitted: usize = lines[front..back].iter().map(|line| line.len()).sum();
    let hint = hint(omitted);
    format!(
        "{}{}{}",
        lines[..front].concat(),
        hint.trim_start_matches('\n'),
        lines[back..].concat()
    )
}

/// `[HH:MM:SS]` of a UTC time in microseconds since the epoch.
fn stamp(at_micros: i64) -> String {
    let seconds = at_micros.div_euclid(1_000_000).rem_euclid(86_400);
    format!(
        "[{:02}:{:02}:{:02}]",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

impl ItemLog {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            entries: Vec::new(),
        }
    }

    /// Add a step line of the manager, stamped with `at_micros` (microseconds since the epoch).
    pub fn step(&mut self, at_micros: i64, line: &str) {
        let line = line.trim_end_matches('\n').replace('\0', "");
        self.entries
            .push(Entry::Step(format!("{} {line}\n", stamp(at_micros))));
    }

    /// Add raw tool output. Consecutive chunks form one output, cut as a whole.
    pub fn tool(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        if !matches!(self.entries.last(), Some(Entry::Tool(_))) {
            self.entries.push(Entry::Tool(Output {
                head: Vec::new(),
                tail: Vec::new(),
                total: 0,
            }));
        }
        if let Some(Entry::Tool(output)) = self.entries.last_mut() {
            output.push(chunk, self.limit);
        }
    }

    /// The log as stored: at most `limit` bytes of UTF-8 without NUL characters.
    #[must_use]
    pub fn text(&self) -> String {
        let steps: usize = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Step(line) => line.len(),
                Entry::Tool(_) => 0,
            })
            .sum();
        // Each output may need a line break before the next step line.
        let outputs: Vec<&Output> = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(output) => Some(output),
                Entry::Step(_) => None,
            })
            .collect();
        let budget = self.limit.saturating_sub(steps + outputs.len());
        let total: usize = outputs
            .iter()
            .map(|output| output.total)
            .fold(0, usize::saturating_add);
        let mut text = String::new();
        for entry in &self.entries {
            match entry {
                Entry::Step(line) => {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(line);
                }
                Entry::Tool(output) => {
                    let room = if total <= budget {
                        usize::MAX
                    } else {
                        // Shares in proportion to size; u128 keeps the product from overflowing.
                        usize::try_from(budget as u128 * output.total as u128 / total as u128)
                            .unwrap_or(budget)
                    };
                    text.push_str(&output.render(room));
                }
            }
        }
        cut_middle_lines(&text, self.limit)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2023-11-14 22:13:20 UTC.
    const AT: i64 = 1_700_000_000_000_000;

    #[test]
    fn step_lines_carry_their_time_of_day_and_stand_apart_from_tool_output() {
        let mut log = ItemLog::new(ITEM_LOG_LIMIT);
        log.step(AT, "Start: Analysieren");
        log.tool(b"frame=1 fps=0");
        log.step(AT + 61_000_000, "Exit code: 0");
        assert_eq!(
            log.text(),
            "[22:13:20] Start: Analysieren\nframe=1 fps=0\n[22:14:21] Exit code: 0\n"
        );
    }

    #[test]
    fn long_tool_output_loses_its_middle_and_every_step_line_survives() {
        let mut log = ItemLog::new(1000);
        log.step(AT, "first step");
        log.tool(b"HEAD-OF-OUTPUT\n");
        for _ in 0..500 {
            log.tool(b"0123456789");
        }
        log.tool(b"\nEND-OF-OUTPUT\n");
        log.step(AT, "last step");
        let text = log.text();
        assert!(text.len() <= 1000, "{} bytes", text.len());
        assert!(text.starts_with("[22:13:20] first step\nHEAD-OF-OUTPUT\n"));
        assert!(text.ends_with("END-OF-OUTPUT\n[22:13:20] last step\n"));
        let omitted = text
            .lines()
            .find_map(|line| line.strip_prefix("… ")?.strip_suffix(" bytes omitted …"))
            .expect("a hint where the middle was cut");
        let omitted: usize = omitted.parse().unwrap();
        let kept = text.len() - "[22:13:20] first step\n[22:13:20] last step\n".len();
        assert!(omitted > 4000 && omitted + kept > 5030, "{omitted} omitted");
    }

    #[test]
    fn output_within_the_limit_is_kept_whole() {
        let mut log = ItemLog::new(ITEM_LOG_LIMIT);
        log.tool(b"one\n");
        log.step(AT, "between");
        log.tool(b"two\n");
        assert_eq!(log.text(), "one\n[22:13:20] between\ntwo\n");
    }

    #[test]
    fn invalid_utf8_and_nul_bytes_never_reach_the_text() {
        let mut log = ItemLog::new(ITEM_LOG_LIMIT);
        log.tool(&[b'a', 0xff, 0, b'b']);
        let text = log.text();
        assert!(text.contains('\u{fffd}'));
        assert!(!text.contains('\0'));
        assert!(!log.is_empty());
    }

    #[test]
    fn the_limit_holds_even_for_multibyte_output() {
        let mut log = ItemLog::new(300);
        log.step(AT, "start");
        for _ in 0..200 {
            log.tool("é😀".as_bytes());
        }
        log.tool(&[0xff; 200]);
        let text = log.text();
        assert!(text.len() <= 300, "{} bytes", text.len());
        assert!(text.contains("bytes omitted"));
    }

    #[test]
    fn steps_alone_beyond_the_limit_are_still_bounded() {
        let mut log = ItemLog::new(200);
        for index in 0..50 {
            log.step(AT, &format!("step {index}"));
        }
        let text = log.text();
        assert!(text.len() <= 200, "{} bytes", text.len());
        assert!(text.starts_with("[22:13:20] step 0\n"));
        assert!(text.ends_with("[22:13:20] step 49\n"));
    }

    #[test]
    fn an_empty_log_is_empty() {
        assert!(ItemLog::new(ITEM_LOG_LIMIT).is_empty());
        assert_eq!(ItemLog::new(ITEM_LOG_LIMIT).text(), "");
    }
}
