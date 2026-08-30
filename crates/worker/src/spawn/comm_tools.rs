//! Socket communication retained for the legacy top-level Worker callback protocol.
//!
//! Direct Internal SubWorker lifecycle is exposed through `worker.control` and the
//! canonical Worker tools, not a second SubWorker-specific tool family.

use std::path::Path;
use std::time::Duration;

use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{Event, Method};
use tokio::net::UnixStream;

/// Timeout applied to each socket-level operation — connect, write,
/// read. Kept short so a stuck child doesn't block the spawner's turn.
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(5);

/// Connect with a timeout, drain the server's connect-time snapshot,
/// write one `Method` line, flush, and close.
///
/// The Worker socket protocol sends replayed alerts and an initial
/// `Event::Snapshot` before it starts reading client methods. Send-only
/// callers must consume that prefix; otherwise a large snapshot can block
/// the server's writer before it reaches the method-read branch. Any
/// socket error maps to an `io::Error`; the caller decides whether to
/// surface it to the LLM or treat it as "worker stopped".
pub(crate) async fn connect_and_send(socket: &Path, method: &Method) -> std::io::Result<()> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out"))??;
    let (r, w) = stream.into_split();
    let mut reader = JsonLineReader::new(r);
    let mut writer = JsonLineWriter::new(w);

    drain_initial_snapshot(&mut reader).await?;

    tokio::time::timeout(SOCKET_OP_TIMEOUT, writer.write(method))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "write timed out"))??;
    Ok(())
}

async fn drain_initial_snapshot<R>(reader: &mut JsonLineReader<R>) -> std::io::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read timed out"))??;
        match event {
            Some(Event::Snapshot { .. }) => return Ok(()),
            Some(_) => continue,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "worker closed connection before Snapshot event",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use protocol::{Alert, AlertLevel, AlertSource, Greeting, WorkerEvent, WorkerStatus};
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio::task::JoinHandle;

    fn snapshot(entries: Vec<serde_json::Value>) -> Event {
        Event::Snapshot {
            session: protocol::SessionSnapshot {
                entries: entries
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| protocol::SessionSnapshotEntry {
                        entry_id: format!("test-{index}"),
                        timestamp: index as u64,
                        provenance: protocol::SessionEntryProvenance::LegacyUnknown,
                        derived_from: Vec::new(),
                        data: protocol::SessionSnapshotEntryData::RunError {
                            message: value.to_string(),
                        },
                    })
                    .collect(),
            },
            greeting: Greeting {
                worker_name: "server".into(),
                cwd: "/tmp".into(),
                provider: "test".into(),
                model: "test".into(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 200_000,
                context_tokens: 0,
            },
            status: WorkerStatus::Idle,
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        }
    }

    fn serve_initial_events_then_method(
        listener: UnixListener,
        events: Vec<Event>,
    ) -> JoinHandle<Option<Method>> {
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.ok()?;
            let (r, w) = stream.into_split();
            let mut reader = JsonLineReader::new(r);
            let mut writer = JsonLineWriter::new(w);
            for event in events {
                writer.write(&event).await.ok()?;
            }
            reader.next::<Method>().await.ok().flatten()
        })
    }

    #[tokio::test]
    async fn connect_and_send_drains_initial_alert_and_snapshot_before_method() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let received = serve_initial_events_then_method(
            listener,
            vec![
                Event::Alert(Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Worker,
                    message: "replayed alert".into(),
                    timestamp_ms: 0,
                }),
                snapshot(Vec::new()),
            ],
        );

        connect_and_send(&socket, &Method::Shutdown).await.unwrap();

        let method = received.await.unwrap().expect("expected method");
        assert!(matches!(method, Method::Shutdown));
    }

    #[tokio::test]
    async fn connect_and_send_delivers_method_after_large_initial_snapshot() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let large_payload = "x".repeat(2 * 1024 * 1024);
        let received = serve_initial_events_then_method(
            listener,
            vec![snapshot(vec![
                serde_json::json!({ "payload": large_payload }),
            ])],
        );
        let expected = Method::WorkerEvent(WorkerEvent::TurnEnded {
            worker_name: "child".into(),
        });

        connect_and_send(&socket, &expected).await.unwrap();

        let method = received.await.unwrap().expect("expected method");
        match method {
            Method::WorkerEvent(WorkerEvent::TurnEnded { worker_name }) => {
                assert_eq!(worker_name, "child")
            }
            other => panic!("expected TurnEnded WorkerEvent, got {other:?}"),
        }
    }
}
