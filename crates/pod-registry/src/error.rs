//! Error type for mutating pod-registry operations.

use std::io;
use std::path::PathBuf;

use manifest::ScopeRule;
use session_store::SessionId;

/// Errors raised by the mutating pod-registry operations.
#[derive(Debug, thiserror::Error)]
pub enum ScopeLockError {
    #[error("I/O error on pods.json: {0}")]
    Io(#[from] io::Error),
    #[error("pod name `{0}` is already registered")]
    DuplicatePodName(String),
    #[error("requested scope `{}` conflicts with pod `{competitor}`", .rule.target.display())]
    WriteConflict { competitor: String, rule: ScopeRule },
    #[error(
        "requested scope `{}` is not within spawner `{spawner}`'s effective scope",
        .rule.target.display()
    )]
    NotSubset { spawner: String, rule: ScopeRule },
    #[error("pod `{0}` is not registered")]
    UnknownPod(String),
    #[error(
        "session {session_id} is already held by pod `{pod_name}` at {}",
        .socket.display()
    )]
    SessionConflict {
        session_id: SessionId,
        pod_name: String,
        socket: PathBuf,
    },
}
