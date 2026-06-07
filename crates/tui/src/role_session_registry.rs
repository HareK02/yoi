use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const REGISTRY_VERSION: u32 = 1;
const REGISTRY_FILE: &str = "role-sessions.json";
const REGISTRY_LOCK_FILE: &str = "role-sessions.lock";
const CLAIMS_DIR: &str = "ticket-claims";

#[derive(Debug, Clone)]
pub(crate) struct PanelRegistryStore {
    root: PathBuf,
    workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RoleSessionRegistry {
    pub version: u32,
    pub workspace_root: String,
    pub sessions: BTreeMap<String, RoleSessionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RoleSessionRecord {
    pub role: String,
    pub pod_name: String,
    pub origin: RoleSessionOrigin,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub related_tickets: Vec<RelatedTicketRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoleSessionOrigin {
    PreTicketIntake,
    TicketClaim,
    RoleLaunch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RelatedTicketRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TicketClaim {
    pub ticket_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_slug: Option<String>,
    pub pod_name: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelRegistrySnapshot {
    pub sessions: Vec<RoleSessionRecord>,
    pub claims: Vec<TicketClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TicketClaimResult {
    Claimed,
    AlreadyOwned(TicketClaim),
}

#[derive(Debug)]
pub(crate) enum PanelRegistryError {
    Io(io::Error),
    Json(serde_json::Error),
    TicketAlreadyClaimed(TicketClaim),
}

impl std::fmt::Display for PanelRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "local role session registry I/O error: {error}"),
            Self::Json(error) => write!(f, "local role session registry JSON error: {error}"),
            Self::TicketAlreadyClaimed(claim) => write!(
                f,
                "Ticket {} is already claimed locally by {} ({})",
                claim.ticket_id, claim.pod_name, claim.role
            ),
        }
    }
}

impl std::error::Error for PanelRegistryError {}

impl From<io::Error> for PanelRegistryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PanelRegistryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl PanelRegistryStore {
    pub(crate) fn default_for_workspace(workspace_root: &Path) -> Result<Self, PanelRegistryError> {
        let data_dir = manifest::paths::data_dir().ok_or_else(|| {
            PanelRegistryError::Io(io::Error::other("failed to resolve yoi data directory"))
        })?;
        Ok(Self::for_data_dir(data_dir, workspace_root))
    }

    pub(crate) fn for_data_dir(data_dir: impl AsRef<Path>, workspace_root: &Path) -> Self {
        let workspace_root = normalized_workspace_key(workspace_root);
        let leaf = workspace_leaf(&workspace_root);
        let digest = fnv1a64_hex(workspace_root.as_bytes());
        Self {
            root: data_dir
                .as_ref()
                .join("panel")
                .join("workspaces")
                .join(format!("{leaf}-{digest}")),
            workspace_root: Some(workspace_root),
        }
    }

    pub(crate) fn from_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            workspace_root: None,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn snapshot(&self) -> Result<PanelRegistrySnapshot, PanelRegistryError> {
        let registry = self.load_registry()?;
        let claims = self.load_claims()?;
        Ok(PanelRegistrySnapshot {
            sessions: registry.sessions.into_values().collect(),
            claims,
        })
    }

    pub(crate) fn load_registry(&self) -> Result<RoleSessionRegistry, PanelRegistryError> {
        match fs::read(self.registry_path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RoleSessionRegistry {
                version: REGISTRY_VERSION,
                workspace_root: self.workspace_root.clone().unwrap_or_default(),
                sessions: BTreeMap::new(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn record_session(
        &self,
        pod_name: impl Into<String>,
        role: impl Into<String>,
        origin: RoleSessionOrigin,
        session_id: Option<String>,
        related_tickets: impl IntoIterator<Item = RelatedTicketRef>,
    ) -> Result<(), PanelRegistryError> {
        let pod_name = pod_name.into();
        let role = role.into();
        let related_tickets: Vec<RelatedTicketRef> = related_tickets.into_iter().collect();
        self.update_registry(|registry| {
            let now = now_timestamp_string();
            let mut tickets: BTreeSet<RelatedTicketRef> = registry
                .sessions
                .get(&pod_name)
                .map(|record| record.related_tickets.iter().cloned().collect())
                .unwrap_or_default();
            tickets.extend(related_tickets);
            let created_at = registry
                .sessions
                .get(&pod_name)
                .map(|record| record.created_at.clone())
                .unwrap_or_else(|| now.clone());
            registry.sessions.insert(
                pod_name.clone(),
                RoleSessionRecord {
                    role,
                    pod_name,
                    origin,
                    created_at,
                    updated_at: now,
                    session_id,
                    related_tickets: tickets.into_iter().collect(),
                },
            );
            Ok(())
        })
    }

    pub(crate) fn claim_ticket(
        &self,
        ticket_id: &str,
        ticket_slug: Option<&str>,
        pod_name: &str,
        role: &str,
    ) -> Result<TicketClaimResult, PanelRegistryError> {
        fs::create_dir_all(self.claims_dir())?;
        let claim_path = self.claim_path(ticket_id);
        let claim = TicketClaim {
            ticket_id: ticket_id.to_string(),
            ticket_slug: ticket_slug.map(ToOwned::to_owned),
            pod_name: pod_name.to_string(),
            role: role.to_string(),
        };
        match self.create_claim_file(&claim_path, &claim) {
            Ok(()) => {
                if let Err(error) = self.record_session(
                    pod_name.to_string(),
                    role.to_string(),
                    RoleSessionOrigin::TicketClaim,
                    None,
                    [RelatedTicketRef {
                        id: ticket_id.to_string(),
                        slug: ticket_slug.map(ToOwned::to_owned),
                    }],
                ) {
                    let _ = fs::remove_file(&claim_path);
                    return Err(error);
                }
                Ok(TicketClaimResult::Claimed)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = self.load_claim(ticket_id)?;
                if existing.pod_name == pod_name && existing.role == role {
                    Ok(TicketClaimResult::AlreadyOwned(existing))
                } else {
                    Err(PanelRegistryError::TicketAlreadyClaimed(existing))
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn load_claim(&self, ticket_id: &str) -> Result<TicketClaim, PanelRegistryError> {
        let bytes = fs::read(self.claim_path(ticket_id))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub(crate) fn claim_for_ticket(
        &self,
        ticket_id: &str,
    ) -> Result<Option<TicketClaim>, PanelRegistryError> {
        match self.load_claim(ticket_id) {
            Ok(claim) => Ok(Some(claim)),
            Err(PanelRegistryError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn update_registry(
        &self,
        update: impl FnOnce(&mut RoleSessionRegistry) -> Result<(), PanelRegistryError>,
    ) -> Result<(), PanelRegistryError> {
        fs::create_dir_all(&self.root)?;
        let _lock = self.acquire_registry_lock()?;
        let mut registry = self.load_registry()?;
        registry.version = REGISTRY_VERSION;
        if let Some(workspace_root) = self.workspace_root.as_ref() {
            registry.workspace_root = workspace_root.clone();
        }
        update(&mut registry)?;
        self.save_registry(&registry)
    }

    fn acquire_registry_lock(&self) -> Result<RegistryLockGuard, PanelRegistryError> {
        let lock_path = self.root.join(REGISTRY_LOCK_FILE);
        for _ in 0..50 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(RegistryLockGuard { path: lock_path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(PanelRegistryError::Io(io::Error::new(
            io::ErrorKind::WouldBlock,
            "timed out acquiring panel role session registry lock",
        )))
    }

    fn save_registry(&self, registry: &RoleSessionRegistry) -> Result<(), PanelRegistryError> {
        let path = self.registry_path();
        let temp_path = path.with_extension(format!("json.{}.tmp", now_timestamp_string()));
        let bytes = serde_json::to_vec_pretty(registry)?;
        fs::write(&temp_path, [&bytes[..], b"\n"].concat())?;
        fs::rename(temp_path, path)?;
        Ok(())
    }

    fn create_claim_file(&self, claim_path: &Path, claim: &TicketClaim) -> io::Result<()> {
        let temp_path = self
            .claims_dir()
            .join(format!(".{}.tmp", now_timestamp_string()));
        let bytes = serde_json::to_vec_pretty(claim).map_err(io::Error::other)?;
        fs::write(&temp_path, [&bytes[..], b"\n"].concat())?;
        let link_result = fs::hard_link(&temp_path, claim_path);
        let remove_result = fs::remove_file(&temp_path);
        match (link_result, remove_result) {
            (Ok(()), Ok(())) | (Ok(()), Err(_)) => Ok(()),
            (Err(error), _) => Err(error),
        }
    }

    fn load_claims(&self) -> Result<Vec<TicketClaim>, PanelRegistryError> {
        let mut claims: Vec<TicketClaim> = Vec::new();
        match fs::read_dir(self.claims_dir()) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if entry.file_type()?.is_file()
                        && entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "json")
                    {
                        let bytes = fs::read(entry.path())?;
                        claims.push(serde_json::from_slice(&bytes)?);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        claims.sort_by(|left, right| left.ticket_id.cmp(&right.ticket_id));
        Ok(claims)
    }

    fn registry_path(&self) -> PathBuf {
        self.root.join(REGISTRY_FILE)
    }

    fn claims_dir(&self) -> PathBuf {
        self.root.join(CLAIMS_DIR)
    }

    fn claim_path(&self, ticket_id: &str) -> PathBuf {
        self.claims_dir()
            .join(format!("{}.json", encode_path_component(ticket_id)))
    }
}

struct RegistryLockGuard {
    path: PathBuf,
}

impl Drop for RegistryLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl PanelRegistrySnapshot {
    pub(crate) fn empty() -> Self {
        Self {
            sessions: Vec::new(),
            claims: Vec::new(),
        }
    }

    pub(crate) fn claim_for_ticket(&self, ticket_id: &str) -> Option<&TicketClaim> {
        self.claims
            .iter()
            .find(|claim| claim.ticket_id == ticket_id)
    }
}

fn normalized_workspace_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn workspace_leaf(workspace_root: &str) -> String {
    let leaf = workspace_root
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("workspace");
    let sanitized = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "workspace".to_string()
    } else {
        sanitized
    }
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn encode_path_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn now_timestamp_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn registry_path_is_workspace_scoped_under_data_dir() {
        let data_dir = TempDir::new().unwrap();
        let store = PanelRegistryStore::for_data_dir(data_dir.path(), Path::new("/repo/yoi"));
        let other = PanelRegistryStore::for_data_dir(data_dir.path(), Path::new("/repo/other"));

        assert!(store.root().starts_with(data_dir.path()));
        let root = store.root().to_string_lossy();
        assert!(root.contains("panel/workspaces/yoi-"));
        assert_ne!(store.root(), other.root());

        store
            .record_session(
                "ticket-intake-preticket",
                "intake",
                RoleSessionOrigin::PreTicketIntake,
                None,
                [],
            )
            .unwrap();
        assert_eq!(store.load_registry().unwrap().workspace_root, "/repo/yoi");
    }

    #[test]
    fn claim_ticket_rejects_second_active_local_pod() {
        let temp = TempDir::new().unwrap();
        let store = PanelRegistryStore::from_root(temp.path().join("registry"));

        assert!(matches!(
            store.claim_ticket("T-1", Some("ticket-one"), "ticket-one-intake", "intake"),
            Ok(TicketClaimResult::Claimed)
        ));

        let error = store
            .claim_ticket("T-1", Some("ticket-one"), "ticket-two-intake", "intake")
            .unwrap_err();
        assert!(matches!(error, PanelRegistryError::TicketAlreadyClaimed(_)));
        let claim = store.claim_for_ticket("T-1").unwrap().unwrap();
        assert_eq!(claim.pod_name, "ticket-one-intake");
        assert_eq!(claim.ticket_slug.as_deref(), Some("ticket-one"));
    }

    #[test]
    fn intake_session_relation_is_not_one_to_one_with_tickets() {
        let temp = TempDir::new().unwrap();
        let store = PanelRegistryStore::from_root(temp.path().join("registry"));

        store
            .record_session(
                "ticket-intake-preticket",
                "intake",
                RoleSessionOrigin::PreTicketIntake,
                None,
                [],
            )
            .unwrap();
        store
            .record_session(
                "ticket-intake-shared",
                "intake",
                RoleSessionOrigin::RoleLaunch,
                None,
                [
                    RelatedTicketRef {
                        id: "T-1".to_string(),
                        slug: Some("one".to_string()),
                    },
                    RelatedTicketRef {
                        id: "T-2".to_string(),
                        slug: Some("two".to_string()),
                    },
                ],
            )
            .unwrap();

        let snapshot = store.snapshot().unwrap();
        let preticket = snapshot
            .sessions
            .iter()
            .find(|session| session.pod_name == "ticket-intake-preticket")
            .unwrap();
        let shared = snapshot
            .sessions
            .iter()
            .find(|session| session.pod_name == "ticket-intake-shared")
            .unwrap();

        assert!(preticket.related_tickets.is_empty());
        assert_eq!(shared.role, "intake");
        assert_eq!(shared.origin, RoleSessionOrigin::RoleLaunch);
        assert!(!shared.created_at.is_empty());
        assert!(!shared.updated_at.is_empty());
        assert_eq!(
            shared.related_tickets,
            vec![
                RelatedTicketRef {
                    id: "T-1".to_string(),
                    slug: Some("one".to_string()),
                },
                RelatedTicketRef {
                    id: "T-2".to_string(),
                    slug: Some("two".to_string()),
                },
            ]
        );
    }
}
