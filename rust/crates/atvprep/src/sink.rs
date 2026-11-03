//! One output stream shared by the console, the stage reporter and the JSON printer, so their
//! lines stay in the order they were written.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SharedSink(Arc<Mutex<Box<dyn Write + Send>>>);

impl SharedSink {
    #[must_use]
    pub fn new(inner: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(inner)))
    }
}

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("sink lock"))?
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("sink lock"))?
            .flush()
    }
}
