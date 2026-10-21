//! Internal adapter diagnostics are independent of public failure classifications.
//!
//! Only closed vocabularies, numeric status and opaque correlation tokens cross this boundary.
//! In particular, a transport error's Display/Debug and server-controlled text are never recorded.
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Adapter {
    Drive,
    WorkspaceConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Request,
    Response,
    Completion,
    Reconcile,
    Attach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    Deadline,
    TaskJoin,
    TransportUnavailable,
    InvalidPath,
    Communication,
    Decode,
    RequestSize,
    ResponseSize,
    ResponseLength,
    Identity,
    InvalidMetadata,
    BackendRejected,
    AccessCeiling,
    AttachConversion,
    BackendUnknown,
    UnrecognizedResponse,
    NotCommittedSnapshot,
    IncompleteReceipt,
    Reconciled,
}

/// One logical call, including its original-request reconciliation. Request identities are hashed
/// rather than logged verbatim: callers and responses need not contain safe log text.
#[derive(Clone)]
pub(super) struct Diagnostic {
    adapter: Adapter,
    operation: &'static str,
    mutation: bool,
    correlation: String,
    dispatch: tracing::Dispatch,
    stage_override: Option<Stage>,
}

impl Diagnostic {
    pub(super) fn new(
        adapter: Adapter,
        operation: &'static str,
        mutation: bool,
        request_id: Option<&str>,
    ) -> Self {
        Self {
            adapter,
            operation,
            mutation,
            correlation: request_id
                .map(|id| {
                    let hash: String = Sha256::digest(id.as_bytes())
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    format!("request:{hash}")
                })
                .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
            dispatch: tracing::dispatcher::get_default(Clone::clone),
            stage_override: None,
        }
    }

    pub(super) fn at_stage(&self, stage: Stage) -> Self {
        let mut diagnostic = self.clone();
        diagnostic.stage_override = Some(stage);
        diagnostic
    }

    pub(super) fn record(&self, stage: Stage, reason: Reason, status: Option<u16>) {
        let stage = self.stage_override.unwrap_or(stage);
        tracing::dispatcher::with_default(&self.dispatch, || {
            tracing::warn!(
                target: "worker::backend_diagnostic",
                adapter = ?self.adapter,
                operation = self.operation,
                mutation = self.mutation,
                correlation = %self.correlation,
                stage = ?stage,
                reason = ?reason,
                status,
                "Workspace adapter diagnostic"
            );
        });
    }

    pub(super) fn transport(&self, error: &crate::worker::WorkspaceClientError) {
        use crate::worker::WorkspaceClientError;
        self.record(
            Stage::Request,
            match error {
                WorkspaceClientError::Unavailable(_) => Reason::TransportUnavailable,
                WorkspaceClientError::InvalidPath(_) => Reason::InvalidPath,
                WorkspaceClientError::Request(_) => Reason::Communication,
            },
            None,
        );
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::{Adapter, Diagnostic, Reason, Stage};
    use std::sync::{Arc, Mutex};
    use tracing::{Event, Metadata, Subscriber, field::Visit, span};

    #[test]
    fn unsafe_request_identity_is_correlated_without_logging_its_value() {
        let capture = Capture::default();
        tracing::dispatcher::with_default(&capture.dispatch(), || {
            for id in [
                "secret /host/path\ncredential",
                "secret /host/path\ncredential",
                "different",
            ] {
                Diagnostic::new(Adapter::Drive, "mutate", true, Some(id)).record(
                    Stage::Request,
                    Reason::Communication,
                    None,
                );
            }
        });
        let text = capture.text();
        for secret in ["secret", "/host/path", "credential", "different"] {
            assert!(!text.contains(secret));
        }
        let correlations: Vec<_> = text
            .lines()
            .map(|line| {
                line.split("correlation=")
                    .nth(1)
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(correlations.len(), 3);
        assert_eq!(correlations[0], correlations[1]);
        assert_ne!(correlations[0], correlations[2]);
    }

    #[derive(Clone, Default)]
    pub(crate) struct Capture(pub(crate) Arc<Mutex<Vec<String>>>);
    impl Capture {
        pub(crate) fn dispatch(&self) -> tracing::Dispatch {
            tracing::Dispatch::new(self.clone())
        }
        pub(crate) fn text(&self) -> String {
            self.0.lock().unwrap().join("\n")
        }
    }
    struct Fields(String);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            write!(&mut self.0, "{}={value:?};", field.name()).unwrap();
        }
    }
    impl Subscriber for Capture {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.target() == "worker::backend_diagnostic"
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
        }
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }
}
