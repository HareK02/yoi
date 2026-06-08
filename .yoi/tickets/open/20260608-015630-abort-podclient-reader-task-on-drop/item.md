---
id: '20260608-015630-abort-podclient-reader-task-on-drop'
slug: 'abort-podclient-reader-task-on-drop'
title: 'Abort PodClient reader task on drop'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['client', 'pod', 'tui', 'fd-leak', 'bug']
workflow_state: 'inprogress'
created_at: '2026-06-08T01:56:30Z'
updated_at: '2026-06-08T02:42:04Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T02:40:58Z'
---

## Background

When `yoi panel` is left open, file descriptor count can increase steadily even while the user does nothing. Observed behavior: FD count rises by roughly 10 per poll cycle, and eventually panel diagnostics show unrelated-looking errors such as:

```text
Panel local role registry unavailable: local role session registry I/O error: Too many open files (os error 24)
Ticket config is unusable: failed to read Ticket config .../.yoi/ticket.config.toml: Too many open files (os error 24)
```

The likely root cause is `client::PodClient::connect()`: it splits the Unix socket and spawns a background reader task, but `PodClient` does not keep the `JoinHandle` or abort the task on drop.

Current shape:

```rust
pub struct PodClient {
    writer: JsonLineWriter<tokio::io::WriteHalf<UnixStream>>,
    event_rx: mpsc::Receiver<Event>,
}

pub async fn connect(path: &Path) -> Result<Self, io::Error> {
    let stream = UnixStream::connect(path).await?;
    let (reader, writer) = tokio::io::split(stream);
    ...
    tokio::spawn(async move {
        let mut reader = JsonLineReader::new(reader);
        while let Ok(Some(event)) = reader.next::<Event>().await {
            if event_tx.send(event).await.is_err() {
                break;
            }
        }
    });
    Ok(Self { writer, event_rx })
}
```

Panel live Pod probing repeatedly creates short-lived `PodClient`s. Dropping the `PodClient` drops the writer and receiver, but the spawned reader task can remain blocked on socket read while holding the read half/FD. If there are ~10 live probe targets, each panel poll can leak ~10 FDs.

## Goal

Make `PodClient` own and clean up its background reader task so short-lived clients do not leak socket file descriptors.

## Requirements

- Store the reader task `JoinHandle` inside `PodClient` or otherwise provide owned cancellation.
- Implement `Drop` for `PodClient` to abort/cancel the reader task.
- Ensure aborting the reader task drops the read half of the Unix socket promptly.
- Preserve normal long-lived TUI client behavior: events should continue to be received while `PodClient` is alive.
- Ensure `PodClient` remains usable for existing one-shot clients and communication tools.
- Avoid relying on remote socket close to terminate the reader task.
- Add regression coverage that repeatedly creates/drops `PodClient` connections and verifies server-side/client-side tasks/connections are closed or at least that the reader task is aborted.
- Add/adjust tests for status probing if needed.

## Acceptance criteria

- Dropping a `PodClient` aborts its background reader task.
- Repeated live Pod probing no longer increases open FD count monotonically.
- Panel can remain open through multiple poll cycles without leaking one Unix socket per live Pod per cycle.
- Existing Pod client send/receive behavior still works.
- Focused tests cover reader task cleanup.
- `cargo test -p client ... --lib` and relevant TUI/pod tests pass.
- `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
