//! Real Rust PDK -> Component -> Wasmtime -> OwnerHost adapter regression.
//! The deterministic owner here tests the adapter/ABI, not DB durability or
//! production authorization; real owner/storage races belong to workspace-server.
use async_trait::async_trait;
use manifest::plugin::{self, PluginPackageLimits, PluginSourceKind};
use server_api::*;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;
use yoi_plugin_runtime::{
    host::{CancellationToken, Grants, InvocationContext},
    owner::{OwnerHost, PluginHostOwner},
    *,
};

fn component() -> &'static [u8] {
    static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
    COMPONENT.get_or_init(|| {
        // Fixture builds write only this explicitly isolated output directory.
        let target =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/t795-contracts/guest");
        let output = Command::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "--offline",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
                "--manifest-path",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ticket-guest/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let core =
            fs::read(target.join("wasm32-unknown-unknown/release/ticket_test_guest.wasm")).unwrap();
        let bytes = wit_component::ComponentEncoder::default()
            .module(&core)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap();
        fs::write(
            target.parent().unwrap().join("ticket-tool.component.wasm"),
            &bytes,
        )
        .unwrap();
        bytes
    })
}
fn artifact(manifest: &str) -> plugin::CanonicalPluginArtifact {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/t795-contracts");
    fs::create_dir_all(&target).unwrap();
    let dir = tempfile::tempdir_in(target).unwrap();
    fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    fs::write(dir.path().join("plugin.component.wasm"), component()).unwrap();
    let package = plugin::read_plugin_directory(
        dir.path(),
        PluginSourceKind::Explicit,
        &PluginPackageLimits::default(),
    )
    .unwrap();
    plugin::pack_plugin_package(&package, &PluginPackageLimits::default()).unwrap()
}
struct State {
    cell: PluginDataCell,
    receipts: BTreeMap<String, (PluginHostRequest, PluginDataReceipt)>,
    calls: Vec<PluginHostRequest>,
}
struct Owner {
    state: Mutex<State>,
    revoke_after_resolve: bool,
}
impl Owner {
    fn new(revoke_after_resolve: bool) -> Self {
        Self {
            state: Mutex::new(State {
                cell: PluginDataCell::NeverWritten,
                receipts: BTreeMap::new(),
                calls: vec![],
            }),
            revoke_after_resolve,
        }
    }
}
fn rejected(code: PluginHostErrorCode) -> PluginHostError {
    PluginHostError {
        code,
        effect: PluginEffectObservation::NotDispatched,
        diagnostic_id: "isolated-test".into(),
    }
}
#[async_trait]
impl PluginHostOwner for Owner {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> Result<PluginHostResponse, PluginHostError> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(request.clone());
        Ok(match request.clone() {
            PluginHostRequest::Begin { .. } => PluginHostResponse::Invocation {
                invocation_id: "logical-invocation".into(),
                expires_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64
                    + 60_000,
            },
            PluginHostRequest::Authorize { .. } => PluginHostResponse::Authorized,
            PluginHostRequest::ResolveTicket { ticket_ref, .. } => {
                assert_eq!(ticket_ref, "T-795");
                PluginHostResponse::TicketResolved {
                    ticket_id: "canonical-ticket".into(),
                }
            }
            PluginHostRequest::ReadTicket { ticket_id, .. } => {
                assert_eq!(ticket_id, "canonical-ticket");
                if self.revoke_after_resolve {
                    return Err(rejected(PluginHostErrorCode::Denied));
                }
                PluginHostResponse::TicketRead {
                    snapshot: PluginTicketSnapshot {
                        title: "unchanged title".into(),
                        case_state: PluginTicketCaseState::Open,
                        content_digest: "unchanged digest".into(),
                        body: "bounded body".into(),
                        truncated: false,
                    },
                }
            }
            PluginHostRequest::DataGet { .. } => PluginHostResponse::DataCell {
                cell: s.cell.clone(),
            },
            PluginHostRequest::DataList { .. } => PluginHostResponse::DataListed {
                page: PluginDataPage {
                    entries: vec![PluginDataEntry {
                        key: "main".into(),
                        cell: s.cell.clone(),
                    }],
                    next_cursor: None,
                },
            },
            PluginHostRequest::DataLookupMutation { operation_key, .. } => {
                PluginHostResponse::DataMutationLookup {
                    outcome: s
                        .receipts
                        .get(&operation_key)
                        .map(|(_, r)| PluginDataMutationLookup::Committed(r.clone()))
                        .unwrap_or(PluginDataMutationLookup::NotRecorded),
                }
            }
            PluginHostRequest::DataMutate {
                operation_key,
                mutation,
                ..
            } => {
                if let Some((original, receipt)) = s.receipts.get(&operation_key) {
                    if *original != request {
                        return Err(rejected(PluginHostErrorCode::Conflict));
                    }
                    return Ok(PluginHostResponse::DataMutated {
                        receipt: receipt.clone(),
                    });
                }
                let observed = match &s.cell {
                    PluginDataCell::NeverWritten => PluginDataExpectation::NeverWritten,
                    PluginDataCell::Present { last_write, .. } => {
                        PluginDataExpectation::LastWrite(last_write.clone())
                    }
                    PluginDataCell::Deleted { last_delete } => {
                        PluginDataExpectation::LastWrite(last_delete.clone())
                    }
                };
                let id = format!("mutation-{operation_key}");
                s.cell = match mutation {
                    PluginDataMutation::Put {
                        expected,
                        value_json,
                    } => {
                        if expected != observed {
                            return Err(rejected(PluginHostErrorCode::Conflict));
                        }
                        PluginDataCell::Present {
                            value_json,
                            last_write: id.clone(),
                        }
                    }
                    PluginDataMutation::Delete {
                        expected_last_write,
                    } => {
                        if PluginDataExpectation::LastWrite(expected_last_write) != observed {
                            return Err(rejected(PluginHostErrorCode::Conflict));
                        }
                        PluginDataCell::Deleted {
                            last_delete: id.clone(),
                        }
                    }
                };
                let receipt = PluginDataReceipt {
                    mutation_id: id,
                    result_digest: "result-digest".into(),
                };
                s.receipts.insert(operation_key, (request, receipt.clone()));
                PluginHostResponse::DataMutated { receipt }
            }
            _ => return Err(rejected(PluginHostErrorCode::Unsupported)),
        })
    }
}
fn context(compiled: &CompiledTool) -> InvocationContext {
    let namespace: (String, String, String) = ("notes".into(), "note".into(), "1".into());
    InvocationContext {
        workspace: "ws".into(),
        installation: "installation".into(),
        plan: "plan".into(),
        subject: "subject".into(),
        invocation_id: "not-authority".into(),
        artifact: compiled.identity().clone(),
        tool_name: "ticket_test".into(),
        grants: Grants {
            ticket_read: true,
            ticket_data_read: [namespace.clone()].into(),
            ticket_data_write: [namespace].into(),
            ..Default::default()
        },
        deadline: Instant::now() + Duration::from_secs(60),
        cancellation: CancellationToken::new(),
    }
}
async fn run(
    ex: &Executor,
    compiled: &CompiledTool,
    owner: Arc<Owner>,
    mode: &str,
    granted: bool,
) -> Result<InvocationResult, ExecutionError> {
    let mut ctx = context(compiled);
    if !granted {
        ctx.grants.ticket_data_write.clear();
    }
    let host = Arc::new(
        OwnerHost::begin(
            owner,
            ctx,
            PluginConsumer::ToolCall {
                tool_call_id: "call".into(),
                tool_name: "ticket_test".into(),
            },
        )
        .await
        .unwrap(),
    );
    ex.invoke(
        compiled,
        host.context().clone(),
        host,
        "ticket_test",
        &format!("\"{mode}\""),
        "{}",
    )
    .await
}
#[tokio::test]
async fn actual_component_resolves_reads_mutates_lists_and_reconciles_receipts() {
    let artifact = artifact(include_str!("ticket-guest/plugin.toml"));
    let ex = Executor::new(Limits::default()).unwrap();
    let compiled = ex
        .compile(
            &artifact.bytes,
            &artifact.package_digest,
            &artifact.artifact_digest,
            artifact.artifact_size,
        )
        .unwrap();
    let owner = Arc::new(Owner::new(false));
    let result = run(&ex, &compiled, owner.clone(), "all", true)
        .await
        .unwrap();
    assert_eq!(result.output.summary, "unchanged title");
    let s = owner.state.lock().unwrap();
    assert_eq!(s.receipts.len(), 3); // put/delete/recreate, not a fourth replay
    assert!(
        matches!(&s.cell, PluginDataCell::Present { last_write, .. } if last_write == "mutation-recreate")
    );
    assert_eq!(
        s.calls
            .iter()
            .filter(|r| matches!(r, PluginHostRequest::DataMutate { .. }))
            .count(),
        4
    );
}
#[tokio::test]
async fn actual_component_denies_old_grants_foreign_resources_revocation_and_bad_schema() {
    let artifact = artifact(include_str!("ticket-guest/plugin.toml"));
    let ex = Executor::new(Limits::default()).unwrap();
    let compiled = ex
        .compile(
            &artifact.bytes,
            &artifact.package_digest,
            &artifact.artifact_digest,
            artifact.artifact_size,
        )
        .unwrap();
    for (mode, revoked, granted) in [
        ("read", true, true),
        ("foreign", false, true),
        ("stale", false, true),
        ("other-schema", false, true),
        ("read", false, false),
        ("bad-schema", false, true),
    ] {
        let owner = Arc::new(Owner::new(revoked));
        assert!(
            run(&ex, &compiled, owner.clone(), mode, granted)
                .await
                .is_err(),
            "{mode}"
        );
        assert!(
            !owner
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|r| matches!(r, PluginHostRequest::DataMutate { .. })),
            "rejection precedes write"
        );
    }
}
