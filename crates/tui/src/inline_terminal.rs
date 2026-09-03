use std::io::{self, Stdout, Write};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::{TerminalOptions, Viewport};

pub(crate) type InlineTerminal = Terminal<CrosstermBackend<Stdout>>;

struct InlineTerminalGuard {
    terminal: InlineTerminal,
    closed: bool,
}

impl InlineTerminalGuard {
    fn open(height: u16) -> io::Result<Self> {
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;
        Ok(Self {
            terminal,
            closed: false,
        })
    }

    fn close(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;

        let area = self.terminal.get_frame().area();
        let last_row = area.bottom().saturating_sub(1);
        let cursor_result = self.terminal.set_cursor_position((0, last_row));
        let output_result = write_viewport_terminator(&mut io::stdout());
        cursor_result?;
        output_result
    }
}

impl Drop for InlineTerminalGuard {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub(crate) fn with_inline_terminal<T, E>(
    height: u16,
    run: impl FnOnce(&mut InlineTerminal) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<io::Error>,
{
    let mut guard = InlineTerminalGuard::open(height).map_err(E::from)?;
    let result = run(&mut guard.terminal);
    let close_result = guard.close();
    match result {
        Ok(value) => {
            close_result.map_err(E::from)?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

fn write_viewport_terminator(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\r\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_terminator_moves_following_output_to_a_fresh_line() {
        let mut output = Vec::new();

        write_viewport_terminator(&mut output).unwrap();

        assert_eq!(output, b"\r\n");
    }

    #[test]
    fn inline_viewport_construction_is_owned_by_this_module() {
        fn assert_shared_owner(path: &std::path::Path) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    assert_shared_owner(&path);
                } else if path.extension().and_then(|value| value.to_str()) == Some("rs")
                    && path.file_name().and_then(|value| value.to_str())
                        != Some("inline_terminal.rs")
                {
                    let source = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        !source.contains("Viewport::Inline"),
                        "{} constructs an inline viewport outside its shared owner",
                        path.display()
                    );
                }
            }
        }

        assert_shared_owner(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    }
}
