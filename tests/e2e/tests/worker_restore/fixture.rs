use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use protocol::{SessionMessageRole, SessionSnapshot, SessionSnapshotEntryData};
use serde::Deserialize;
use serde_json::{Value, json};
use worker_runtime::test_support::StoppedWorkerTemplate;
use yoi_e2e::{FixtureSource, HarnessError, Result};

pub const PROFILE: &str = "project:fixture-chat";
const ASSETS: [&str; 3] = [
    "expected.json",
    "worker-template.json",
    "workspace-config.dcdl",
];
const MAX_ASSET_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub source_revision: String,
    pub runtime_schema_version: u32,
    pub session_schema_version: u32,
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub session_id: String,
    pub segment_id: String,
    pub model: String,
    pub state: String,
    pub acquisition: String,
    pub source_file_sha256: BTreeMap<String, String>,
    pub files: BTreeMap<String, Asset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    source_revision: String,
    model: String,
    worker_id: String,
    session_id: String,
    rows: Vec<Value>,
}

pub struct Fixture {
    pub root: PathBuf,
    pub source: FixtureSource,
    pub manifest: Manifest,
    expected: Expected,
}

pub fn failure(message: impl Into<String>) -> HarnessError {
    HarnessError::Protocol(message.into())
}
pub fn support<T, E: std::fmt::Display>(result: std::result::Result<T, E>) -> Result<T> {
    result.map_err(|error| failure(error.to_string()))
}

impl Fixture {
    pub fn load() -> Result<Self> {
        // Candidate override is explicit and read-only. Missing assets are failures,
        // never skips, and no test generates or repairs a fixture.
        let root = std::env::var_os("YOI_E2E_RESTORE_FIXTURE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/worker-codex-v1")
            });
        let manifest: Manifest =
            serde_json::from_slice(&bounded_read(&root.join("manifest.json"))?)?;
        if manifest.format_version != 1
            || manifest.state != "stopped"
            || manifest.runtime_schema_version == 0
            || manifest.session_schema_version == 0
            || manifest.acquisition.is_empty()
            || manifest.source_file_sha256.is_empty()
            || manifest.source_revision.len() != 40
            || !manifest
                .source_revision
                .bytes()
                .all(|c| c.is_ascii_hexdigit())
        {
            return Err(failure("unsupported/incomplete fixture manifest"));
        }
        let names: BTreeSet<_> = manifest.files.keys().map(String::as_str).collect();
        if names != BTreeSet::from(ASSETS) {
            return Err(failure("unexpected fixture asset set"));
        }
        for name in ASSETS {
            bounded_read(&root.join(name))?;
        }
        let source = FixtureSource::capture(&root)?;
        if source.hashes().len() != 4 {
            return Err(failure("fixture has unmanifested files"));
        }
        for (name, asset) in &manifest.files {
            if asset.bytes != fs::metadata(root.join(name))?.len()
                || source.hashes().get(Path::new(name)) != Some(&format!("sha256:{}", asset.sha256))
            {
                return Err(failure(format!("fixture asset hash/size mismatch: {name}")));
            }
        }
        let expected: Expected =
            serde_json::from_slice(&bounded_read(&root.join("expected.json"))?)?;
        if expected.source_revision != manifest.source_revision
            || expected.model != manifest.model
            || expected.worker_id != manifest.worker_id
            || expected.session_id != manifest.session_id
            || expected.rows.is_empty()
        {
            return Err(failure(
                "expected conversation identity differs from manifest",
            ));
        }
        verify_builtin_revision(&manifest.source_revision)?;
        let fixture = Self {
            root,
            source,
            manifest,
            expected,
        };
        fixture.template(&fixture.root)?;
        Ok(fixture)
    }

    pub fn template(&self, copy: &Path) -> Result<StoppedWorkerTemplate> {
        let bytes = bounded_read(&copy.join("worker-template.json"))?;
        let template: StoppedWorkerTemplate = serde_json::from_slice(&bytes)?;
        let shape: Value = serde_json::from_slice(&bytes)?;
        let session_manifest_bytes: Vec<u8> =
            serde_json::from_value(shape["session_manifest"].clone())?;
        let segment_bytes: Vec<u8> = serde_json::from_value(shape["segment_jsonl"].clone())?;
        for (name, bytes) in [
            (
                "session/session.json".to_owned(),
                session_manifest_bytes.as_slice(),
            ),
            (
                format!("session/segments/{}.jsonl", self.manifest.segment_id),
                segment_bytes.as_slice(),
            ),
        ] {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if self.manifest.source_file_sha256.get(&name) != Some(&digest) {
                return Err(failure(
                    "exported Session bytes differ from acquisition hash evidence",
                ));
            }
        }
        let session_manifest: Value = serde_json::from_slice(&session_manifest_bytes)?;
        let m = &self.manifest;
        if template.workspace_id() != m.workspace_id
            || template.runtime_id() != m.runtime_id
            || template.worker_id().to_string() != m.worker_id
            || template.session_id().to_string() != m.session_id
            || template.segment_id().to_string() != m.segment_id
            || shape["runtime_schema_version"] != m.runtime_schema_version
            || session_manifest["schema_version"] != m.session_schema_version
        {
            return Err(failure("template schema/identity differs from manifest"));
        }
        Ok(template)
    }

    pub fn verify_conversation(&self, snapshot: &SessionSnapshot) -> Result<()> {
        let mut rows = Vec::new();
        for entry in &snapshot.entries {
            let (role, parts) = match &entry.data {
                SessionSnapshotEntryData::UserInput { segments } => {
                    ("user", serde_json::to_value(segments)?)
                }
                SessionSnapshotEntryData::Message {
                    role: SessionMessageRole::Assistant,
                    content,
                } => ("assistant", serde_json::to_value(content)?),
                // Display-only accounting is not a conversation row. Its exact
                // IDs, timestamp and values are still checked in the full WS snapshot.
                SessionSnapshotEntryData::RunStats { .. } => continue,
                _ => {
                    return Err(failure(
                        "unexpected non-conversation entry in full Session snapshot",
                    ));
                }
            };
            rows.push(json!({"entry_id":entry.entry_id,"timestamp":entry.timestamp,"role":role,"parts":parts}));
        }
        if rows != self.expected.rows {
            return Err(failure(
                "full public conversation rows differ from expected IDs/time/roles/text/order",
            ));
        }
        Ok(())
    }
}

fn bounded_read(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        failure(format!(
            "required fixture asset {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() > MAX_ASSET_BYTES {
        return Err(failure("fixture asset is not a bounded regular file"));
    }
    Ok(fs::read(path)?)
}

fn verify_builtin_revision(revision: &str) -> Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    // Resolve the immutable commit before diff; bad revisions must not be treated
    // as an empty diff. All resources are checked, including transitive imports.
    let commit = Command::new("git")
        .args(["cat-file", "-t", revision])
        .current_dir(workspace)
        .output()?;
    if !commit.status.success() || commit.stdout != b"commit\n" {
        return Err(failure(
            "fixture source revision is unavailable for builtin verification",
        ));
    }
    let status = Command::new("git")
        .args([
            "diff",
            "--quiet",
            revision,
            "--",
            "resources",
            "crates/builtin-source",
        ])
        .current_dir(workspace)
        .status()?;
    if !status.success() {
        return Err(failure(
            "builtin/config resources differ from fixture source revision; provide a new version or frozen references",
        ));
    }
    Ok(())
}
