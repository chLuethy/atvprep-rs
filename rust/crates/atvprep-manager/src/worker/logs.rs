//! Bounded process-output capture for worker logs.

/// Keep only the last `limit` bytes of arbitrary process output.
#[derive(Debug, Clone)]
pub struct LogTail {
    limit: usize,
    bytes: Vec<u8>,
}
impl LogTail {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes: Vec::new(),
        }
    }
    pub fn push(&mut self, chunk: &[u8]) {
        if self.limit == 0 {
            return;
        }
        self.bytes.extend_from_slice(chunk);
        let excess = self.bytes.len().saturating_sub(self.limit);
        if excess > 0 {
            self.bytes.drain(..excess);
        }
    }
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).replace('\0', "")
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tails_and_replaces_invalid_utf8() {
        let mut tail = LogTail::new(5);
        tail.push(b"first");
        tail.push(b"second");
        assert_eq!(tail.text(), "econd");
        tail.push(&[0xff, 0]);
        assert!(tail.text().contains('�'));
        assert!(!tail.text().contains('\0'));
    }
    #[test]
    fn zero_limit_keeps_nothing() {
        let mut tail = LogTail::new(0);
        tail.push(b"secret");
        assert!(tail.is_empty());
    }
}
