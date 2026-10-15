//! One control namespace for live Worker sources and their verified archives.
//! It outlives aggregate removal and is also used by publishers and readers.

use session_store::retention::SessionRetentionStore;
use std::path::Path;

pub fn runtime_session_retention(runtime_root: &Path) -> SessionRetentionStore {
    SessionRetentionStore::new(runtime_root.join("retention/session-sources"))
}
