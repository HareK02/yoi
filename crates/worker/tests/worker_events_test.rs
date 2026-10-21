//! Agent-visible Internal SubWorker event summaries.

use protocol::WorkerEvent;
use worker::ipc::event::render_event;

#[test]
fn render_event_identifies_child_and_failure() {
    let rendered = render_event(&WorkerEvent::Errored {
        worker_name: "legacy-child".into(),
        message: "boom".into(),
    });
    assert!(rendered.contains("legacy-child"));
    assert!(rendered.contains("boom"));
}
