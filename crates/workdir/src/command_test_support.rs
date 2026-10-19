//! Bounded collection for tests whose subject is command routing/lifecycle,
//! rather than the byte-page protocol itself.

use crate::*;

#[derive(Debug)]
pub(crate) struct CollectedCommand {
    pub content: String,
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

#[async_trait::async_trait]
pub(crate) trait CollectCommand: WorkdirSession {
    async fn collect_command_output(
        &self,
        mut request: CommandOutputRequest,
    ) -> Result<CollectedCommand, WorkdirError> {
        let mut bytes = Vec::new();
        request.wait = true;
        loop {
            let page = self.command_output(request.clone()).await?;
            for chunk in page.chunks {
                bytes.extend(chunk.bytes);
                assert!(
                    bytes.len() <= 16 * 1024 * 1024,
                    "fixture output exceeded bounded test collection"
                );
            }
            request.cursor = page.next_cursor;
            if let Some(terminal) = page.terminal {
                return Ok(CollectedCommand {
                    content: String::from_utf8_lossy(&bytes).into_owned(),
                    status: terminal.status,
                    exit_code: terminal.exit_code,
                    timed_out: terminal.status == CommandStatus::TimedOut,
                });
            }
        }
    }
}
impl<T: WorkdirSession + ?Sized> CollectCommand for T {}
