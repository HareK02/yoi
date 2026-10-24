//! Live public Worker protocol assertions. No Submit/Resume/model request is sent.
use std::net::TcpStream;
use std::time::{Duration, Instant};

use protocol::{Event, SessionSnapshot, WorkerState};
use tungstenite::{Message, WebSocket, client::IntoClientRequest};

use crate::{HarnessError, Result, WorkerE2eEnvironment};

const WS_WAIT: Duration = Duration::from_secs(10);

pub struct WorkerSessionSubscription {
    socket: WebSocket<TcpStream>,
}

impl WorkerSessionSubscription {
    pub fn connect(
        environment: &WorkerE2eEnvironment,
        worker_id: &str,
        expected: &SessionSnapshot,
    ) -> Result<Self> {
        let authorized = environment.worker_protocol_request(worker_id)?;
        let mut url = authorized.url().clone();
        url.set_scheme("ws")
            .map_err(|_| error("invalid protocol URL"))?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| error("build protocol handshake"))?;
        request.headers_mut().extend(authorized.headers().clone());
        // Only this environment's loopback endpoint is permitted, never a proxy.
        let address = url
            .socket_addrs(|| None)
            .map_err(|_| error("resolve protocol address"))?
            .into_iter()
            .find(|address| address.ip().is_loopback())
            .ok_or_else(|| error("protocol address is not loopback"))?;
        let stream = TcpStream::connect_timeout(&address, WS_WAIT)?;
        stream.set_read_timeout(Some(WS_WAIT))?;
        stream.set_write_timeout(Some(WS_WAIT))?;
        let (socket, _) =
            tungstenite::client(request, stream).map_err(|_| error("protocol handshake failed"))?;
        let mut subscription = Self { socket };
        let deadline = Instant::now() + WS_WAIT;
        loop {
            match subscription.read(deadline)? {
                Message::Text(text) => {
                    let raw: serde_json::Value = serde_json::from_str(&text)?;
                    validate_full_snapshot(&raw, expected)?;
                    // Only validated public content is retained, never request headers.
                    std::fs::write(
                        environment.artifacts_dir.join("restored-session.json"),
                        serde_json::to_vec_pretty(&raw)?,
                    )?;
                    return Ok(subscription);
                }
                Message::Ping(_) => subscription
                    .socket
                    .flush()
                    .map_err(|_| error("flush protocol Pong"))?,
                Message::Pong(_) => {}
                _ => return Err(error("protocol ended before its initial full Snapshot")),
            }
        }
    }

    /// Prove an already-open subscription remains usable after the other
    /// environment is stopped and cleaned up. This does not invoke the model.
    pub fn assert_alive(&mut self) -> Result<()> {
        let payload = b"restore-e2e-isolation".to_vec();
        self.socket
            .send(Message::Ping(payload.clone().into()))
            .map_err(|_| error("send protocol Ping"))?;
        let deadline = Instant::now() + WS_WAIT;
        loop {
            match self.read(deadline)? {
                Message::Pong(bytes) if bytes.as_ref() == payload.as_slice() => return Ok(()),
                Message::Ping(_) => self
                    .socket
                    .flush()
                    .map_err(|_| error("flush protocol Pong"))?,
                Message::Pong(_) => {}
                _ => {
                    return Err(error(
                        "idle subscription changed or ended during isolation check",
                    ));
                }
            }
        }
    }

    /// A public stop must terminate the connected execution, not merely update
    /// a retained catalog row. Require the shutdown event or websocket close.
    pub fn expect_stopped(mut self) -> Result<()> {
        let deadline = Instant::now() + WS_WAIT;
        loop {
            match self.read(deadline)? {
                Message::Close(_) => return Ok(()),
                Message::Text(text) => match serde_json::from_str::<Event>(&text)? {
                    Event::Shutdown => return Ok(()),
                    Event::WorkerState { .. } => {}
                    _ => return Err(error("unexpected protocol event after Worker stop")),
                },
                Message::Ping(_) => self
                    .socket
                    .flush()
                    .map_err(|_| error("flush protocol Pong"))?,
                Message::Pong(_) => {}
                _ => return Err(error("unexpected protocol frame after Worker stop")),
            }
        }
    }

    fn read(&mut self, deadline: Instant) -> Result<Message> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| error("protocol deadline exceeded"))?;
        self.socket.get_mut().set_read_timeout(Some(remaining))?;
        self.socket
            .read()
            .map_err(|_| error("protocol read failed or timed out"))
    }
}

fn error(message: &str) -> HarnessError {
    HarnessError::Protocol(message.into())
}

pub fn validate_full_snapshot(raw: &serde_json::Value, expected: &SessionSnapshot) -> Result<()> {
    let event: Event = serde_json::from_value(raw.clone())?;
    let Event::Snapshot {
        session,
        state,
        in_flight,
        internal_workers,
        ..
    } = event
    else {
        return Err(error("first protocol event is not a full Snapshot"));
    };
    if state.state != WorkerState::Idle || !in_flight.is_empty() || !internal_workers.is_empty() {
        return Err(error(
            "restored Worker is not an idle, completed standalone Session",
        ));
    }
    // Typed equality covers entry order/ID/timestamp/provenance/content; raw
    // equality also refuses unknown fields that could leak private data.
    if session != *expected
        || raw.pointer("/data/session") != Some(&serde_json::to_value(expected)?)
    {
        return Err(error(
            "full Session snapshot differs from supplied expected rows",
        ));
    }
    if !session.pending_submissions.submissions.is_empty() {
        return Err(error("fixture has pending submissions"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{
        Greeting, InFlightSnapshot, Segment, SessionEntryProvenance, SessionSnapshotEntry,
        SessionSnapshotEntryData, WorkerStateSnapshot, WorkerStatus,
    };

    // Protocol-validation samples only, not a substitute saved Worker fixture.
    fn sample() -> (serde_json::Value, SessionSnapshot) {
        let session = SessionSnapshot {
            pending_submissions: Default::default(),
            entries: vec![SessionSnapshotEntry {
                entry_id: "E-unit-1".into(),
                timestamp: 123,
                provenance: SessionEntryProvenance::HumanInput,
                derived_from: vec![],
                use_policy: None,
                data: SessionSnapshotEntryData::UserInput {
                    segments: vec![Segment::text("unit input")],
                },
            }],
        };
        let event = Event::Snapshot {
            session: session.clone(),
            state: WorkerStateSnapshot::from(WorkerStatus::Idle),
            in_flight: InFlightSnapshot::default(),
            internal_workers: vec![],
            greeting: Greeting {
                worker_name: "unit".into(),
                cwd: ".".into(),
                provider: "unit".into(),
                model: "unit".into(),
                reasoning: None,
                scope_summary: "none".into(),
                tools: vec![],
                context_window: 0,
                context_tokens: 0,
                context_usage: None,
            },
        };
        (serde_json::to_value(event).unwrap(), session)
    }

    #[test]
    fn full_snapshot_requires_exact_ids_timestamps_contents_and_no_unknown_fields() {
        let (raw, expected) = sample();
        validate_full_snapshot(&raw, &expected).unwrap();
        for (key, value) in [
            ("entry_id", serde_json::json!("different")),
            ("timestamp", serde_json::json!(124)),
            (
                "hidden_reasoning",
                serde_json::json!("must never be exposed"),
            ),
            ("segments", serde_json::json!([])),
        ] {
            let mut changed = raw.clone();
            changed["data"]["session"]["entries"][0][key] = value;
            assert!(
                validate_full_snapshot(&changed, &expected).is_err(),
                "{key}"
            );
        }
        let mut missing = raw.clone();
        missing["data"]["session"]["entries"] = serde_json::json!([]);
        assert!(validate_full_snapshot(&missing, &expected).is_err());
        let mut extra = raw;
        let entry = extra["data"]["session"]["entries"][0].clone();
        extra["data"]["session"]["entries"]
            .as_array_mut()
            .unwrap()
            .push(entry);
        assert!(validate_full_snapshot(&extra, &expected).is_err());
    }

    #[test]
    fn non_snapshot_event_cannot_stand_in_for_live_snapshot() {
        let (_, expected) = sample();
        assert!(
            validate_full_snapshot(&serde_json::to_value(Event::Shutdown).unwrap(), &expected)
                .is_err()
        );
    }
}
