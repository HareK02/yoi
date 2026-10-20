use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// Per-test warning capture; never installs a global subscriber or writes fixture secrets.
#[derive(Clone, Default)]
pub struct CapturedWarnings(Arc<Mutex<Vec<u8>>>);

impl CapturedWarnings {
    pub fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let output = self.clone();
        tracing_subscriber::fmt()
            .with_writer(move || output.clone())
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish()
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for CapturedWarnings {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
