//! Where the human lines go. No markup, no colours: one line per call.

use std::io::Write;
use std::sync::{Arc, Mutex};

/// A line-oriented output with a flag telling whether a person is watching.
pub struct Console {
    sink: Box<dyn Write + Send>,
    terminal: bool,
}

impl Console {
    #[must_use]
    pub fn to(sink: Box<dyn Write + Send>, terminal: bool) -> Self {
        Self { sink, terminal }
    }

    #[must_use]
    pub fn stdout(terminal: bool) -> Self {
        Self::to(Box::new(std::io::stdout()), terminal)
    }

    #[must_use]
    pub fn stderr(terminal: bool) -> Self {
        Self::to(Box::new(std::io::stderr()), terminal)
    }

    /// A console that records what is printed (for tests and for capturing).
    #[must_use]
    pub fn recording(terminal: bool) -> (Self, SharedBuffer) {
        let buffer = SharedBuffer::default();
        (Self::to(Box::new(buffer.clone()), terminal), buffer)
    }

    /// Print one line. A closed pipe never aborts the work, so errors are ignored.
    pub fn print(&mut self, line: &str) {
        let _ = writeln!(self.sink, "{line}");
        let _ = self.sink.flush();
    }

    /// Print `text` without a line end (a prompt) and flush.
    pub fn prompt(&mut self, text: &str) {
        let _ = write!(self.sink, "{text}");
        let _ = self.sink.flush();
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }
}

/// A clonable in-memory writer.
#[derive(Clone, Default)]
pub struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    /// Everything written so far, as text.
    ///
    /// # Panics
    ///
    /// Panics if another thread panicked while writing.
    #[must_use]
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("buffer lock")).into_owned()
    }

    /// The written lines.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.contents().lines().map(str::to_owned).collect()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("buffer lock"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_console_keeps_the_lines_and_the_terminal_flag() {
        let (mut console, buffer) = Console::recording(true);
        console.print("a");
        console.print("b");
        assert_eq!(buffer.lines(), ["a", "b"]);
        assert!(console.is_terminal());
    }
}
